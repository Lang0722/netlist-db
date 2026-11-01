import os
from typing import Iterable, Optional, Tuple

class Netlist:
    def summary(self) -> dict: ...
    def diagnostics(
        self, level: Optional[str] = None, limit: int = 50, cursor: Optional[str] = None
    ) -> dict: ...
    def list_subckts(
        self,
        filter: Optional[str] = None,
        sort: str = "name",
        limit: int = 50,
        cursor: Optional[str] = None,
    ) -> dict: ...
    def subckt_info(self, name: str, limit: int = 50, cursor: Optional[str] = None) -> dict: ...
    def instances_of(self, name: str, limit: int = 50, cursor: Optional[str] = None) -> dict: ...
    def element_info(self, path: str) -> dict: ...
    def resolve_param(self, scope_path: str, name: str) -> dict: ...
    def resolve_element(self, path: str) -> dict: ...
    def net_info(
        self,
        scope_path: str,
        net: str,
        view: str = "logical",
        limit: int = 50,
        cursor: Optional[str] = None,
    ) -> dict: ...
    def trace_net(
        self,
        scope_path: str,
        net: str,
        direction: str = "down",
        depth: int = 3,
        limit: int = 50,
        cursor: Optional[str] = None,
    ) -> dict: ...

def load(
    path: str | os.PathLike,
    top: Optional[str] = None,
    lib_override: Optional[Iterable[Tuple[str | os.PathLike, str, str]]] = None,
    title_line: str = "auto",
) -> Netlist: ...
