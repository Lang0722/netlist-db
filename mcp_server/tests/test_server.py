"""The MCP tools, called in process."""

import asyncio
import json
import pathlib

import netlist_db
from netlist_mcp import server

ROOT = pathlib.Path(__file__).resolve().parents[2]
CASES = ROOT / "golden" / "cases"


def call(tool: str, /, **args) -> dict:
    result = asyncio.run(server.mcp.call_tool(tool, args))
    assert not result.is_error, result
    return json.loads(result.content[0].text)


def load(case: str, **args) -> dict:
    return call("load_netlist", path=str(CASES / case / "deck.sp"), **args)


def test_load_then_list():
    r = load("hier_counts")
    h = r["handle"]
    assert h.startswith("n")
    assert r["summary"]["expanded_instances"] == 24
    rows = call("list_subckts", handle=h, sort="copies")
    assert [x["name"] for x in rows["items"]][:1] == ["leaf"]
    assert call("subckt_info", handle=h, name="leaf")["copies"] == 17
    t = call("subckt_text", handle=h, name="mid", limit=2)
    assert t["first_line"] == 9
    assert t["lines"]["items"] == [".subckt mid a b", "rmk_mid a b 2k"]
    assert t["lines"]["total"] == 10
    t2 = call("subckt_text", handle=h, name="mid", cursor=t["lines"]["next_cursor"])
    assert t2["first_line"] == 11 and t2["lines"]["items"][-1] == ".ends"
    n = call("net_info", handle=h, scope_path="", net="x1.xm1.n1")
    assert (n["scope"], n["net"]) == ("x1.xm1", "n1")


def test_handle_reuse():
    a = load("hier_counts")
    # another spelling of the same file
    other = str(CASES / "hier_counts" / ".." / "hier_counts" / "deck.sp")
    b = call("load_netlist", path=other)
    assert (b["handle"], b["reused"]) == (a["handle"], True)
    # other options: another netlist
    c = load("hier_counts", title_line="yes")
    assert c["handle"] != a["handle"]
    # reload keeps the handle
    d = load("hier_counts", reload=True)
    assert (d["handle"], d["reused"]) == (a["handle"], False)


def test_unknown_handle_error():
    e = call("summary", handle="n999999")
    assert e["error"]["kind"] == "unknown_handle"
    h = load("parhier_local")["handle"]
    assert call("unload_netlist", handle=h) == {"unloaded": h}
    assert call("net_info", handle=h, scope_path="", net="1")["error"]["kind"] == "unknown_handle"
    assert call("unload_netlist", handle=h)["error"]["kind"] == "unknown_handle"
    # a load error and a query error are errors too, not exceptions
    assert call("load_netlist", path=str(CASES / "nope.sp"))["error"]["kind"] == "load_failed"
    h = load("hier_counts")["handle"]
    q = call("subckt_info", handle=h, name="lef")
    assert q["error"]["kind"] == "not_found" and "leaf" in q["error"]["suggestions"]
    bad = call("load_netlist", path=str(CASES / "hier_counts" / "deck.sp"), lib_override=[["a", "b"]])
    assert bad["error"]["kind"] == "bad_argument"


def test_resolve_tool_matches_library():
    h = load("parhier_global")["handle"]
    lib = netlist_db.load(CASES / "parhier_global" / "deck.sp")
    assert call("resolve_element", handle=h, path="x1.r1") == lib.resolve_element("x1.r1")
    assert call("resolve_param", handle=h, scope_path="x1", name="whowins") == lib.resolve_param(
        "x1", "whowins"
    )
    h = load("dspf_included")["handle"]
    lib = netlist_db.load(CASES / "dspf_included" / "deck.sp")
    for view in ("logical", "physical"):
        assert call("net_info", handle=h, scope_path="xi", net="A", view=view) == lib.net_info(
            "xi", "A", view=view
        )
    assert call("trace_net", handle=h, scope_path="", net="in", direction="both") == lib.trace_net(
        "", "in", direction="both"
    )


def test_tools_have_docs():
    tools = asyncio.run(server.mcp.list_tools())
    names = {t.name for t in tools}
    assert names == {
        "load_netlist", "unload_netlist", "summary", "diagnostics", "list_subckts",
        "subckt_info", "subckt_text", "instances_of", "element_info", "resolve_param",
        "resolve_element", "resolve_model", "net_info", "trace_net",
    }
    for t in tools:
        assert t.description and len(t.description) > 40, t.name
        if t.name != "load_netlist":
            assert "handle" in t.input_schema["properties"], t.name


def test_symlinked_deck_reads_includes_beside_the_link(tmp_path):
    # the library reads `.include` from the directory of the path as given
    # (as HSPICE does), so the server must not resolve symlinks
    (tmp_path / "real").mkdir()
    (tmp_path / "link").mkdir()
    (tmp_path / "real" / "deck.sp").write_text(
        "* t\n.include 'sub.sp'\nr1 1 0 rsub\nv1 1 0 1\n.op\n.end\n"
    )
    (tmp_path / "link" / "sub.sp").write_text(".param rsub=7k\n")
    (tmp_path / "link" / "deck.sp").symlink_to(tmp_path / "real" / "deck.sp")
    h = call("load_netlist", path=str(tmp_path / "link" / "deck.sp"))["handle"]
    assert call("resolve_param", handle=h, scope_path="", name="rsub")["value"] == 7000.0


def test_letter_case_and_empty_strings():
    a = load("hier_counts", top="Leaf")
    assert load("hier_counts", top="leaf")["handle"] == a["handle"]
    # "" is "not given"
    b = load("hier_counts", top="")
    assert b["summary"]["expanded_instances"] == 24
    h = b["handle"]
    assert "items" in call("diagnostics", handle=h, level="", cursor="")
    assert "items" in call("list_subckts", handle=h, filter="", cursor="")
    # an empty choice is its default, not an error
    assert call("list_subckts", handle=h, sort="") == call("list_subckts", handle=h, sort="name")
    assert call("net_info", handle=h, scope_path="", net="1", view="")["net"] == "1"
    assert call("trace_net", handle=h, scope_path="", net="1", direction="")["hops"]["total"] > 0
    # an empty or odd-case title_line is `auto`: the same handle
    assert load("hier_counts", title_line="")["handle"] == load("hier_counts")["handle"]
    assert load("hier_counts", title_line=" AUTO ")["handle"] == load("hier_counts")["handle"]
    bad = load("hier_counts", title_line="maybe")
    assert bad["error"]["kind"] == "bad_argument", bad


def test_odd_arguments_give_answers_or_errors():
    h = load("hier_counts")["handle"]
    assert len(call("list_subckts", handle=h, limit=10**20)["items"]) == 4
    assert call("trace_net", handle=h, scope_path="", net="1", depth=2**64)["hops"]["total"] == 24
    assert call("trace_net", handle=h, scope_path="", net="1", depth=-1)["error"]["kind"] == "bad_argument"
    assert call("load_netlist", path="a\x00b")["error"]["kind"] in ("bad_argument", "load_failed")


def test_resolve_model(tmp_path):
    (tmp_path / "pdk.lib").write_text(
        "* pdk\n.lib tt\n.param dv=0.01\n.lib 'pdk.lib' core\n.endl tt\n.lib core\n"
        ".subckt mac d g s b l=100n w=1u\n"
        ".model n.1 nmos level=54 lmin=0 lmax=1u wmin=0 wmax=1 vth0='0.3+dv+1e-8/l'\n"
        ".model n.2 nmos level=54 lmin=1u lmax=1 wmin=0 wmax=1 vth0='0.5+dv'\n"
        "M0 d g s b n l=l w=w\n.ends mac\n.endl core\n"
    )
    (tmp_path / "top.sp").write_text(
        "* t\n.lib 'pdk.lib' tt\nX1 d g 0 0 mac l=0.1u\nX2 d g 0 0 mac l=2u\nV1 d 0 1\n.end\n"
    )
    h = call("load_netlist", path=str(tmp_path / "top.sp"))["handle"]
    m = call("resolve_model", handle=h, path="x1.m0", names=["vth0"])
    assert (m["card"]["name"], m["card"]["scope"]) == ("n.1", "x1")
    assert abs(m["params"]["items"][0]["value"] - 0.41) < 1e-12
    m2 = call("resolve_model", handle=h, path="x2.m0", names=["vth0"], cursor="")
    assert m2["card"]["name"] == "n.2"
    assert abs(m2["params"]["items"][0]["value"] - 0.51) < 1e-12
    # an empty list of names is all the items
    assert call("resolve_model", handle=h, path="x2.m0", names=[])["params"]["total"] == 6
    assert call("resolve_model", handle=h, path="x1")["error"]["kind"] == "bad_argument"
