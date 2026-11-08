# netlist-mcp

An MCP server that answers small questions about HSPICE netlists and DSPF
parasitic files. A real netlist is too large for the context of an LLM. The
server parses the deck once and gives short, paged answers in JSON.

The server has no query logic. Each tool calls one method of the Python
library `netlist_db` (the Rust crate `netlist-query`).

## Install

From the repository root:

```sh
python3 -m venv .venv
.venv/bin/pip install maturin pytest "mcp>=2,<3"
(cd pybind && ../.venv/bin/maturin develop --release)
.venv/bin/pip install -e mcp_server
```

The package does not depend on `netlist_db`: the PyPI package of that name
is the upstream parser, which has no `load`. Build the binding of this
repository with `maturin` (above). The server stops at start with a
message when it finds the PyPI package.

## Run

The server uses stdio:

```sh
.venv/bin/python -m netlist_mcp
```

To add it to Claude Code:

```sh
claude mcp add netlist-db -- /path/to/netlist-db/.venv/bin/python -m netlist_mcp
```

## Tools

| Tool | Answer |
|---|---|
| `load_netlist(path, top, lib_override, title_line, reload)` | A handle (`n1`, `n2`, …) and the summary |
| `unload_netlist(handle)` | Frees the netlist |
| `summary(handle)` | Files, definitions, root, expanded totals, `parhier`, diagnostic counts, DSPF files |
| `diagnostics(handle, level, limit, cursor)` | Load and hierarchy diagnostics |
| `list_subckts(handle, filter, sort, limit, cursor)` | Definitions with ports, devices, `X` lines and copies |
| `subckt_info(handle, name, limit, cursor)` | One definition and a page of its elements |
| `subckt_text(handle, name, limit, cursor)` | The original text of one definition, `.subckt` to `.ends`, as a page of lines |
| `instances_of(handle, name, limit, cursor)` | The `X` lines that call a definition |
| `element_info(handle, path)` | One element by its instance path |
| `resolve_param(handle, scope_path, name)` | A parameter value in a scope, with its chain |
| `resolve_element(handle, path)` | The numeric parameters of one element |
| `resolve_model(handle, path, names, limit, cursor)` | The `.model` card of one element (a MOSFET's bin) and the card's parameters, evaluated where the card is defined |
| `net_info(handle, scope_path, net, view, limit, cursor)` | The pins on a net, and its DSPF parasitics |
| `trace_net(handle, scope_path, net, direction, depth, limit, cursor)` | Where a net goes through ports |

Rules for all tools:

- Give absolute file paths. A relative path is read from the directory
  where the server runs. The server does not resolve symlinks: HSPICE and
  the library read `.include` files from the directory of the path as
  given.
- An instance path is the lowercase instance names, joined by `.`
  (`x1.xinv.m3`). `""` is the top level.
- `net_info` and `trace_net` also take an HSPICE full node name as `net`
  (`x1.xm1.n1`, with `scope_path` `""`). The answer names the instance in
  `scope`, and a caveat says how the name was read.
- A list comes in pages. Give `next_cursor` back as `cursor`. `limit` is at
  most 500.
- An error is a JSON object with one key, `error`, that holds `kind`,
  `message` and `suggestions`. No tool raises. The kind `internal` is a
  bug in the server or the library.
- An empty string for an optional argument (`top`, `level`, `filter`,
  `cursor`) means "not given".
- Read `caveats` in each answer. They tell what makes the answer less
  certain.
- The same file with the same options (`top` and section names in any
  letter case) gives the same handle. Use `reload: true` to read changed
  files again.

## Tests

```sh
.venv/bin/pytest mcp_server/tests -q
```
