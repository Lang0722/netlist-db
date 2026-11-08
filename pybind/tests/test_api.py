"""The Python API, on the golden cases."""

import json
import pathlib

import pytest

import netlist_db

ROOT = pathlib.Path(__file__).resolve().parents[2]
CASES = ROOT / "golden" / "cases"


def golden(case: str) -> netlist_db.Netlist:
    return netlist_db.load(CASES / case / "deck.sp")


def expected(case: str) -> dict:
    return json.loads((CASES / case / "expected.json").read_text())


def test_load_and_summary():
    s = golden("hier_counts").summary()
    want = expected("hier_counts")["totals"]
    assert s["root"] == "top-level"
    assert s["parhier"] == "global"
    assert s["expanded_instances"] == want["subckt_instances"]
    assert s["expanded_devices"]["resistor"] == want["resistors"]
    assert s["expanded_devices"]["capacitor"] == want["capacitors"]
    assert s["expanded_devices"]["mosfet"] == want["mosfets"]
    assert s["errors"] == 0
    # a pathlib path and a str path load the same deck
    assert netlist_db.load(str(CASES / "hier_counts" / "deck.sp")).summary() == s


def test_counts_match_rust():
    net = golden("hier_counts")
    rows = net.list_subckts(limit=500)
    copies = {r["name"]: r["copies"] for r in rows["items"]}
    for name, n in expected("hier_counts")["extra"]["marker_counts"].items():
        assert copies[name] == n, name
    by_copies = net.list_subckts(sort="copies")["items"]
    assert by_copies[0]["name"] == "leaf"
    info = net.subckt_info("leaf")
    assert info["copies"] == 17
    assert net.instances_of("blk")["total"] == 2
    t = net.subckt_text("leaf")
    assert t["first_line"] == t["loc"]["line"] == 5
    assert t["lines"]["items"] == [
        ".subckt leaf a b", "rmk_leaf a b 1k", "mn a a b b nch w=1u l=1u", ".ends",
    ]
    assert t["caveats"] == []


def test_resolve_param_chain():
    net = golden("parhier_global")
    p = net.resolve_param("x1", "whowins")
    assert p["value"] == 4.0
    assert p["chain"][-1]["kind"] == "GlobalParam"
    ep = net.resolve_element("x1.r1")
    r = next(x for x in ep["params"] if x["name"] == "r")
    assert r["value"] == 4.0
    assert net.element_info("x1.r1")["kind"] == "resistor"


def test_full_node_name():
    net = golden("hier_counts")
    i = net.net_info("", "x1.xm1.n1")
    assert (i["scope"], i["net"], i["def"]) == ("x1.xm1", "n1", "mid")
    assert "full node name" in i["caveats"][0]
    t = net.trace_net("", "x1.xm1.xl1.a", direction="up", depth=3)
    assert t["scope"] == "x1.xm1.xl1" and t["hops"]["total"] == 3

    # a net name with a `.`, and nets that HSPICE joins (golden `node_dotted_*`)
    i = golden("node_dotted_in_subckt").net_info("", "x1.n.1")
    assert (i["scope"], i["net"]) == ("x1", "n.1")
    j = golden("node_dotted_collision").net_info("x1", "n")
    assert "HSPICE joins" in j["caveats"][0]


def test_rcl_reading_and_protected_subckt():
    e = golden("rcl_param_tc").element_info("r1")
    assert e["model"] is None
    assert [(p["name"], p["text"]) for p in e["params"]] == [("r", "rval"), ("tc1", "0.004")]
    s = golden("prot_subckt").summary()
    assert s["errors"] == 0


def test_error_is_dict():
    net = golden("hier_counts")
    e = net.subckt_info("lef")
    assert set(e) == {"error"}
    assert e["error"]["kind"] == "not_found"
    assert "leaf" in e["error"]["suggestions"]
    assert "lef" in e["error"]["message"]
    for bad in (
        net.list_subckts(sort="size"),
        net.net_info("", "n1", view="flat"),
        net.trace_net("", "n1", direction="sideways"),
        net.diagnostics(level="fatal"),
        net.diagnostics(limit=-1),
        net.trace_net("", "n1", depth=-1),
        net.diagnostics(cursor="x"),
    ):
        assert bad["error"]["kind"] == "bad_argument", bad
    with pytest.raises(OSError):
        netlist_db.load(CASES / "no_such_case" / "deck.sp")
    with pytest.raises(ValueError):
        netlist_db.load(CASES / "hier_counts" / "deck.sp", title_line="maybe")
    with pytest.raises(ValueError):
        netlist_db.load(CASES / "hier_counts" / "deck.sp", top="nosuch")


def test_net_info_paging(tmp_path):
    lines = ["* fanout", "v1 hub 0 1"] + [f"r{k} hub n{k} 1k" for k in range(120)]
    deck = tmp_path / "fanout.sp"
    deck.write_text("\n".join(lines) + "\n")
    net = netlist_db.load(deck)
    p1 = net.net_info("", "hub")
    assert p1["total"] == 121
    assert len(p1["connections"]["items"]) == 50
    seen = [c["element"] for c in p1["connections"]["items"]]
    cursor = p1["connections"]["next_cursor"]
    while cursor is not None:
        p = net.net_info("", "hub", limit=50, cursor=cursor)
        seen += [c["element"] for c in p["connections"]["items"]]
        cursor = p["connections"]["next_cursor"]
    assert seen == ["v1"] + [f"r{k}" for k in range(120)]
    t = net.trace_net("", "hub", direction="both", depth=2)
    assert t["hops"]["total"] == 0


def test_dspf_and_views():
    net = golden("dspf_included")
    a = net.net_info("xi", "A")
    assert [c["pin_role"] for c in a["connections"]["items"]] == ["g", "g"]
    assert a["parasitics"]["r_count"] == 3
    phys = net.net_info("xi", "A", view="physical", limit=0)
    assert phys["total"] == 10
    assert net.summary()["dspf"][0]["mode"] == "included"
    up = net.trace_net("xi", "a", direction="up", depth=1)
    assert up["hops"]["items"][0]["to_net"] == "in"


def test_lib_override(tmp_path):
    (tmp_path / "models.lib").write_text(
        ".lib tt\n.param rv=1k\n.endl tt\n.lib ff\n.param rv=2k\n.endl ff\n"
    )
    (tmp_path / "main.sp").write_text(
        "* t\n.lib 'models.lib' tt\nv1 1 0 1\nr1 1 0 rv\n.end\n"
    )
    base = netlist_db.load(tmp_path / "main.sp")
    assert base.resolve_param("", "rv")["value"] == 1000.0
    ff = netlist_db.load(
        tmp_path / "main.sp", lib_override=[(tmp_path / "models.lib", "tt", "ff")]
    )
    assert ff.resolve_param("", "rv")["value"] == 2000.0


# A PDK library in the macro-model style (corners, a wrapper subckt with
# binned cards inside it), and a circuit that uses it
PDK = """* pdk
.lib tt
.param dvth_g=0
.lib 'pdk.lib' core
.endl tt
.lib ff
.param dvth_g=-0.03
.lib 'pdk.lib' core
.endl ff
.lib core
.subckt nch_mac d g s b l=100n w=1u nf=1
.param dvth_l='5e-9/l'
.model nch.1 nmos level=54 lmin=20n lmax=200n wmin=0.1u wmax=5u vth0='0.35+dvth_g+dvth_l' tox=1.2n
.model nch.2 nmos level=54 lmin=200n lmax=2u wmin=0.1u wmax=5u vth0='0.40+dvth_g' tox=1.2n
M0 d g s b nch l=l w=w nf=nf
.ends nch_mac
.endl core
"""

TOP = """* circuit
.lib 'pdk.lib' tt
X1 d g 0 0 nch_mac l=60n w=2u nf=2
X2 d g 0 0 nch_mac l=500n w=1u
V1 d 0 1
.end
"""


def test_resolve_model(tmp_path):
    (tmp_path / "pdk.lib").write_text(PDK)
    (tmp_path / "top.sp").write_text(TOP)
    net = netlist_db.load(tmp_path / "top.sp")
    m = net.resolve_model("x1.m0", names=["vth0", "tox"])
    assert (m["card"]["name"], m["card"]["scope"], m["card"]["def"]) == ("nch.1", "x1", "nch_mac")
    assert m["selection"]["by"] == "bin" and m["selection"]["ranges"][1]["value"] == 1e-6
    vth0, tox = m["params"]["items"]
    assert vth0["value"] == pytest.approx(0.35 + 5e-9 / 60e-9)
    assert vth0["chain"][0]["kind"] == "ModelParam"
    assert tox["value"] == pytest.approx(1.2e-9)
    # one name as a string; the other bin; another corner
    assert net.resolve_model("x2.m0", names="vth0")["params"]["items"][0]["value"] == pytest.approx(0.4)
    ff = netlist_db.load(tmp_path / "top.sp", lib_override=[(tmp_path / "pdk.lib", "tt", "ff")])
    got = ff.resolve_model("x1.m0", names=["vth0"])["params"]["items"][0]["value"]
    assert got == pytest.approx(0.32 + 5e-9 / 60e-9)
    # all the items, in pages
    first = net.resolve_model("x1.m0", limit=2)
    assert [p["name"] for p in first["params"]["items"]] == ["level", "lmin"]
    assert first["params"]["total"] == 7
    rest = net.resolve_model("x1.m0", cursor=first["params"]["next_cursor"])
    assert rest["params"]["items"][-1]["name"] == "tox"
    # errors are dicts
    assert net.resolve_model("x1")["error"]["kind"] == "bad_argument"
    assert net.resolve_model("x9.m0")["error"]["kind"] == "not_found"
    assert net.resolve_model("x1.m0", limit=-1)["error"]["kind"] == "bad_argument"
    # a golden deck: a top-level card with no bins
    exact = golden("hier_counts").resolve_model("x1.xl0.mn")
    assert exact["card"]["scope"] == "" and exact["selection"]["by"] == "exact"
    first = exact["params"]["items"][0]
    assert (first["name"], first["value"]) == ("level", 1.0)
