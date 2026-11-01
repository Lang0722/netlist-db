"""The MCP server `netlist-db` (stdio).

Each tool calls one method of `netlist_db.Netlist` and returns its answer as
compact JSON text. No query logic is here. A loaded netlist has a handle
(`n1`, `n2`, ...); loading the same file with the same options again gives
the same handle. Errors are JSON objects `{"error": {"kind", "message",
"suggestions"}}`; no tool raises (a bug is the kind `internal`). An empty
string is "not given" for an optional argument.
"""

from __future__ import annotations

import json
import os
import threading
from typing import Any, Optional

from mcp.server.mcpserver import MCPServer

import netlist_db

if not hasattr(netlist_db, "load"):
    # the PyPI package `netlist_db` is the upstream parser, not this fork
    raise ImportError(
        "the installed `netlist_db` has no `load`: it is the upstream PyPI "
        "package. Build the binding of this repository: "
        "`cd pybind && maturin develop --release` (see mcp_server/README.md)"
    )

INSTRUCTIONS = """\
Answers small questions about HSPICE netlists (and DSPF parasitic files) \
that are too large to read. First call `load_netlist` with the path of the \
top deck: it returns a `handle` and a summary. Pass the handle to the other \
tools. Give absolute file paths (a relative path is read from the \
directory where the server runs). Instance paths are lowercase instance \
names joined by `.` (`x1.xinv.m3`); `""` is the top level. Lists come in pages: pass \
`next_cursor` back as `cursor` (`limit` is at most 500). An error is a JSON \
object with one key, `error` (`kind`, `message`, `suggestions`). Read \
`caveats` in each answer: they say what makes it less certain."""

mcp = MCPServer("netlist-db", instructions=INSTRUCTIONS)

_lock = threading.Lock()
_nets: dict[str, netlist_db.Netlist] = {}
_handle_of: dict[tuple, str] = {}
_key_of: dict[str, tuple] = {}
_count = 0


def _dump(value: Any) -> str:
    return json.dumps(value, separators=(",", ":"), ensure_ascii=False)


def _error(kind: str, message: str, suggestions: Optional[list[str]] = None) -> str:
    return _dump(
        {"error": {"kind": kind, "message": message, "suggestions": suggestions or []}}
    )


def _opt(s: Optional[str]) -> Optional[str]:
    """`None` for an empty string: clients often send `""` for an optional
    argument."""
    return s if s else None


def _get(handle: str) -> netlist_db.Netlist | str:
    """The netlist of `handle`, or the error text."""
    with _lock:
        net = _nets.get(handle)
        known = sorted(_nets)
    if net is None:
        return _error(
            "unknown_handle",
            f"no loaded netlist has the handle `{handle}`: call `load_netlist` first",
            known,
        )
    return net


@mcp.tool()
def load_netlist(
    path: str,
    top: Optional[str] = None,
    lib_override: Optional[list[list[str]]] = None,
    title_line: str = "auto",
    reload: bool = False,
) -> str:
    """Load an HSPICE deck (or a DSPF file) and return `{"handle", "summary", "reused"}`.

    `path`: the top file, an absolute path. `top`: the subckt to use as the root of a deck that
    has only definitions (`outer>inner` names a nested one). `lib_override`:
    `[file, from_section, to_section]` triples that switch `.lib` sections
    (another corner). `title_line`: `auto` (HSPICE: line 1 is a title),
    `yes` or `no`. The same file with the same options gives the same handle
    (`reused` true); `reload` true reads the files again under that handle.
    """
    try:
        return _load(path, _opt(top), lib_override, title_line, reload)
    except Exception as e:  # a bug: an error, not a broken tool call
        return _error("internal", f"{type(e).__name__}: {e}")


def _load(
    path: str,
    top: Optional[str],
    lib_override: Optional[list[list[str]]],
    title_line: str,
    reload: bool,
) -> str:
    global _count

    # The path as given, made absolute, not resolved through symlinks: the
    # library reads `.include` files from the directory of the path as
    # given, and matches a `lib_override` file by its name, as HSPICE does
    def absolute(p: str) -> str:
        return os.path.abspath(os.path.expanduser(p))

    overrides: list[tuple[str, str, str]] = []
    for o in lib_override or []:
        if len(o) != 3:
            return _error(
                "bad_argument",
                f"a lib_override item is [file, from_section, to_section], not {o!r}",
            )
        overrides.append((absolute(o[0]), o[1], o[2]))
    try:
        full = absolute(path)
    except ValueError as e:  # a NUL in the path
        return _error("bad_argument", f"bad path {path!r}: {e}")
    title_line = (title_line or "").strip().lower() or "auto"
    if title_line not in ("auto", "yes", "no"):
        return _error(
            "bad_argument", f"title_line `{title_line}` is not one of `auto`, `yes`, `no`"
        )
    # names and sections in any letter case are the same
    key = (
        full,
        top.lower() if top else None,
        tuple((f, a.lower(), b.lower()) for f, a, b in overrides),
        title_line,
    )
    with _lock:
        handle = _handle_of.get(key)
        net = _nets.get(handle) if handle else None
    if net is not None and not reload:
        return _dump({"handle": handle, "summary": net.summary(), "reused": True})
    try:
        net = netlist_db.load(full, top, overrides or None, title_line)
    except (OSError, ValueError) as e:
        return _error("load_failed", str(e))
    with _lock:
        handle = _handle_of.get(key)
        if handle is None:
            _count += 1
            handle = f"n{_count}"
            _handle_of[key] = handle
            _key_of[handle] = key
        _nets[handle] = net
    return _dump({"handle": handle, "summary": net.summary(), "reused": False})


@mcp.tool()
def unload_netlist(handle: str) -> str:
    """Free the memory of a loaded netlist. Its handle is no longer valid."""
    with _lock:
        net = _nets.pop(handle, None)
        key = _key_of.pop(handle, None)
        if key is not None:
            _handle_of.pop(key, None)
        known = sorted(_nets)
    if net is None:
        return _error("unknown_handle", f"no loaded netlist has the handle `{handle}`", known)
    return _dump({"unloaded": handle})


def _call(handle: str, method: str, *args: Any) -> str:
    net = _get(handle)
    if isinstance(net, str):
        return net
    try:
        return _dump(getattr(net, method)(*args))
    except Exception as e:  # a bug: an error, not a broken tool call
        return _error("internal", f"{type(e).__name__}: {e}")


@mcp.tool()
def summary(handle: str) -> str:
    """Overview: files, subckt definitions, the root, expanded device and instance totals (with a note when they are not exact), `parhier`, diagnostic counts, DSPF files."""
    return _call(handle, "summary")


@mcp.tool()
def diagnostics(
    handle: str, level: Optional[str] = None, limit: int = 50, cursor: Optional[str] = None
) -> str:
    """Load and hierarchy diagnostics (`level`: `error`, `warning` or `info`; all when omitted), paged. An `error` diagnostic is usually a deck that HSPICE rejects."""
    return _call(handle, "diagnostics", _opt(level), limit, _opt(cursor))


@mcp.tool()
def list_subckts(
    handle: str,
    filter: Optional[str] = None,
    sort: str = "name",
    limit: int = 50,
    cursor: Optional[str] = None,
) -> str:
    """The subckt definitions with ports, devices, `X` lines and `copies` (instances in the expanded hierarchy), paged. `filter`: a part of the name. `sort`: `name`, `copies` or `devices`."""
    return _call(handle, "list_subckts", _opt(filter), _opt(sort) or "name", limit, _opt(cursor))


@mcp.tool()
def subckt_info(handle: str, name: str, limit: int = 50, cursor: Optional[str] = None) -> str:
    """One subckt: ports, header defaults, `.param` lines, devices by type, the subckts it calls, copies, and a page of its element lines. `outer>inner` names a nested subckt."""
    return _call(handle, "subckt_info", name, limit, _opt(cursor))


@mcp.tool()
def subckt_text(handle: str, name: str, limit: int = 200, cursor: Optional[str] = None) -> str:
    """The original text of one subckt, from its `.subckt` line to its `.ends` line, as a page of lines. `first_line` is the file line number of the first line of the page. The text of `.include` files is not shown. `outer>inner` names a nested subckt."""
    return _call(handle, "subckt_text", name, limit, _opt(cursor))


@mcp.tool()
def instances_of(handle: str, name: str, limit: int = 50, cursor: Optional[str] = None) -> str:
    """The `X` lines that call subckt `name`, with the definition that holds each and its copies, paged."""
    return _call(handle, "instances_of", name, limit, _opt(cursor))


@mcp.tool()
def element_info(handle: str, path: str) -> str:
    """One element or instance by its instance path (`x1.xinv.m3`): type, nodes, model or callee, the parameters as written, and where it is."""
    return _call(handle, "element_info", path)


@mcp.tool()
def resolve_param(handle: str, scope_path: str, name: str) -> str:
    """The value of parameter `name` as seen in the instance `scope_path` (`""`: the top level), under `.option parhier`, with the chain of definitions that gave it."""
    return _call(handle, "resolve_param", scope_path, name)


@mcp.tool()
def resolve_element(handle: str, path: str) -> str:
    """The numeric parameters of one element (R value, W, L, m, ...) by its instance path, each with its chain of definitions."""
    return _call(handle, "resolve_element", path)


@mcp.tool()
def net_info(
    handle: str,
    scope_path: str,
    net: str,
    view: str = "logical",
    limit: int = 50,
    cursor: Optional[str] = None,
) -> str:
    """The pins on net `net` in the instance `scope_path` (`""`: the top level), with their roles (MOSFET `d g s b`, `X` lines by port name), paged; DSPF parasitics of the net. `view`: `logical` (a DSPF net as one net, without parasitic R/C) or `physical` (the nodes as written). `net` can be an HSPICE full node name such as `x1.xm1.n1` with `scope_path` `""`: the answer gives the instance in `scope`."""
    return _call(handle, "net_info", scope_path, net, _opt(view) or "logical", limit, _opt(cursor))


@mcp.tool()
def trace_net(
    handle: str,
    scope_path: str,
    net: str,
    direction: str = "down",
    depth: int = 3,
    limit: int = 50,
    cursor: Optional[str] = None,
) -> str:
    """Where net `net` of the instance `scope_path` goes through ports: `down` into instances, `up` to the parent, or `both`, up to `depth` steps, nearest first, paged. Ground and `.global` nets are not followed (they connect by name). `net` can be an HSPICE full node name such as `x1.xm1.n1` with `scope_path` `""`."""
    return _call(
        handle, "trace_net", scope_path, net, _opt(direction) or "down", depth, limit, _opt(cursor)
    )


def main() -> None:
    mcp.run("stdio")
