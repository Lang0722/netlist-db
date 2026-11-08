"""Query HSPICE netlists (and DSPF files) without reading them into an LLM.

`load(path)` parses the deck once. Each method of `Netlist` answers one small
question as a `dict`. A query error is a `dict` with one key, `error`
(`kind`, `message`, `suggestions`); the kind `internal` is a bug in the
library. Only `load` raises: `OSError` when the top file cannot be read,
`ValueError` for a bad option, `RuntimeError` for a bug. Lists come in pages:
`limit` (at most 500) and `cursor` (the `next_cursor` of the page before).
"""

from __future__ import annotations

import json
import os
from typing import Any, Iterable, Optional, Sequence, Tuple

from . import netlist_db as _native

__all__ = ["Netlist", "load"]

_I64_MAX = (1 << 63) - 1


def _i64(x: Any) -> Any:
    """An `int` limited to the range of the native module (i64): a larger
    `limit` is the largest page, a larger `depth` has no bound. Other
    types go on as they are (the native module rejects them)."""
    if isinstance(x, int) and not isinstance(x, bool):
        return max(-_I64_MAX - 1, min(x, _I64_MAX))
    return x


class Netlist:
    """A loaded netlist. Load one with `load`."""

    __slots__ = ("_n",)

    def __init__(self, native: Any) -> None:
        self._n = native

    def summary(self) -> dict:
        return json.loads(self._n.summary())

    def diagnostics(
        self, level: Optional[str] = None, limit: int = 50, cursor: Optional[str] = None
    ) -> dict:
        return json.loads(self._n.diagnostics(level, _i64(limit), cursor))

    def list_subckts(
        self,
        filter: Optional[str] = None,
        sort: str = "name",
        limit: int = 50,
        cursor: Optional[str] = None,
    ) -> dict:
        return json.loads(self._n.list_subckts(filter, sort, _i64(limit), cursor))

    def subckt_info(self, name: str, limit: int = 50, cursor: Optional[str] = None) -> dict:
        return json.loads(self._n.subckt_info(name, _i64(limit), cursor))

    def subckt_text(self, name: str, limit: int = 200, cursor: Optional[str] = None) -> dict:
        return json.loads(self._n.subckt_text(name, _i64(limit), cursor))

    def instances_of(self, name: str, limit: int = 50, cursor: Optional[str] = None) -> dict:
        return json.loads(self._n.instances_of(name, _i64(limit), cursor))

    def element_info(self, path: str) -> dict:
        return json.loads(self._n.element_info(path))

    def resolve_param(self, scope_path: str, name: str) -> dict:
        return json.loads(self._n.resolve_param(scope_path, name))

    def resolve_element(self, path: str) -> dict:
        return json.loads(self._n.resolve_element(path))

    def resolve_model(
        self,
        path: str,
        names: Optional[Iterable[str]] = None,
        limit: int = 50,
        cursor: Optional[str] = None,
    ) -> dict:
        """The `.model` card that the element at `path` uses (for a MOSFET,
        the bin it gets), and the card's parameters evaluated where the card
        is defined. `names`: only these parameters (all when omitted)."""
        if isinstance(names, str):
            names = [names]
        listed = None if names is None else [str(n) for n in names]
        return json.loads(self._n.resolve_model(path, listed, _i64(limit), cursor))

    def net_info(
        self,
        scope_path: str,
        net: str,
        view: str = "logical",
        limit: int = 50,
        cursor: Optional[str] = None,
    ) -> dict:
        return json.loads(self._n.net_info(scope_path, net, view, _i64(limit), cursor))

    def trace_net(
        self,
        scope_path: str,
        net: str,
        direction: str = "down",
        depth: int = 3,
        limit: int = 50,
        cursor: Optional[str] = None,
    ) -> dict:
        return json.loads(
            self._n.trace_net(
                scope_path, net, direction, _i64(depth), _i64(limit), cursor
            )
        )


def load(
    path: str | os.PathLike,
    top: Optional[str] = None,
    lib_override: Optional[Iterable[Tuple[str | os.PathLike, str, str]]] = None,
    title_line: str = "auto",
) -> Netlist:
    """Load the deck `path`.

    `top`: the definition to use as the root of a bare netlist (`a>b` names a
    nested one). `lib_override`: `(file, from_section, to_section)` triples
    that switch `.lib` sections (another corner). `title_line`: `auto`, `yes`
    or `no` (how line 1 of the top file is read).
    """
    overrides: Optional[Sequence[Tuple[str, str, str]]] = None
    if lib_override is not None:
        overrides = [(os.fspath(f), a, b) for f, a, b in lib_override]
    return Netlist(_native.load(os.fspath(path), top, overrides, title_line))
