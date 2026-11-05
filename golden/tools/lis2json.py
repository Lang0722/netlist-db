#!/usr/bin/env python3
"""Turn an HSPICE .lis listing into golden expected.json data.

Usage:
  lis2json.py <hspice.lis> <case> [-o expected.json]
  lis2json.py --selftest

Output (JSON):
  {"case": <case>,
   "resistors": [{"path": "x1.r1", "value": 3.0}, ...],   # first run only
   "totals": {"resistors": N, "capacitors": N, "mosfets": N,
              "subckt_instances": N},   # all null when the job aborted
   "extra": {...}}

What is read from the listing:
- The ``.op`` resistor listing. HSPICE prints it as column blocks: a ``subckt``
  row (instance path, blank for top-level elements), an ``element`` row
  (``N:name``, N = circuit number), an ``r value`` row and more rows. Values
  use HSPICE engineering suffixes (a f p n u m k x g) or exponent form
  (``1.000e+12``).
- The ``circuit name directory`` (circuit number -> instance path, definition,
  multiplier). It gives the full path of each element and the number of
  subckt instances. Each row must be an X line (its name and its
  definition) of the definition of an earlier circuit, read from the deck
  files (circuit_names()); a row that is not is an error. The multiplier has an engineering suffix (``1.00k``,
  ``500.00m``). A path or definition that is too long for its column stays
  take 1, 2 or 3 lines. A line in the directory that fits none of these
  forms is an error. So are a repeated circuit number and a gap in 0..N.
- The ``Circuit Statistics`` totals (``# resistors = N`` ...). When the job
  aborted (``extra.rejected``), every total is null: HSPICE did not read the
  whole circuit, so its statistics are not counts of the deck.

HSPICE shortens names in the ``.op`` listing (golden case ``long_names``):
- The ``subckt`` row shows at most 10 characters of the instance path
  (``x1.xm.xd.xc`` -> ``x1.xm.xd.x``). The full path comes from the circuit
  name directory; the row must equal it or be its 10-character prefix.
- The ``element`` row shows at most 8 characters of the element name
  (``rverylongtoplevel`` -> ``0:rverylon``). An 8-character name is mapped
  back to the one element name of the deck files (every file next to the
  listing except ``hspice.lis`` and ``expected.json``; line 1 of ``deck.sp`` is
  the title) that starts with it and belongs to the instance's definition
  (top level for circuit 0). Lines of files other than ``deck.sp`` that are
  outside any ``.subckt`` can belong to any definition (``.include`` inside a
  ``.subckt``). No match, or more than one, is an error.

``path`` = lowercase instance path + "." + element name, without the ``N:``
index, both at full length; top-level elements are just the name. ``value``
is the ``r value``
exactly as HSPICE prints it. When the element's instance has a multiplier
other than 1 in the circuit name directory (``m=`` on an ``X`` line, inherited
down the hierarchy), HSPICE prints the effective value R/m and the entry gets
``"m": <multiplier>``; the element's own resistance is then ``value * m``.

Listings of the ``.option sim_dspf`` flow ("Advanced Analog Library" format)
have no ``.op`` element listing and no circuit name directory: ``resistors``
is empty, ``totals`` come from the ``Netlist Statistics`` table and
``subckt_instances`` is null.

Automatic ``extra`` keys (filled from the listing, overwritten on each run):
- ``alter_runs``: resistor values of the 2nd, 3rd ... run (``.alter``).
- ``after_end_runs``: resistor values of the runs after those, which come from
  a deck after the first ``.end`` (golden case ``after_end_second_deck``: one
  more job). The listing of such a job looks like an ``.alter`` run, so the
  number of ``.alter`` lines of the first deck in ``deck.sp`` decides which
  runs are ``.alter`` runs. Without ``deck.sp``, every later run is an
  ``.alter`` run.
- ``rejected``: the first ``**error**`` text when the job aborted.
- ``merged_listing``: present when the listing shows fewer resistors than
  ``# resistors`` (HSPICE merges identical subckt instances that are in
  parallel; per-instance values are then not in the listing).
- ``unnamed_circuit_resistors``: the resistors (first run) of the circuits
  with no path. Two instances can get one path (instance names can hold a
  ``.``); HSPICE gives the path to the instance it creates first, and a
  later one has no path (golden cases ``hier_dotted_collision*``). The
  directory prints the paths of the other circuits in order and ``.`` for
  the last rows, so when a circuit comes after such a later instance, the
  paths from that instance on are shifted by one row (golden
  ``hier_dotted_collision_below``, ``hier_dotted_collision_shift``), and so
  are the subckt rows of the ``.op`` listing. Each directory row is checked
  against the X lines of the deck files (see circuit_names()): a shifted
  directory is an error, unless the hand-written ``collided_circuits``
  lists the later instances; then the paths follow it. These resistors are
  not in ``resistors``; each entry has the circuit number, the definition,
  the element name and the value (and ``m``).
- ``shared_path_resistors``: the resistors (first run) whose path is the
  path of another resistor of the listing (an element name can hold a
  ``.``: golden case ``elem_dotted_name_collision``). The ``.op`` listing
  does not show which of them a ``.print`` of that path finds, so they are
  not in ``resistors``; each entry has the path, the circuit number, the
  definition, the element name and the value (and ``m``). In ``alter_runs``
  and ``after_end_runs`` such a path is left out.
- ``dspf_summary``: the ``Parasitic parsing summary`` of a ``sim_dspf`` run.
With ``-o``, every other key of an existing file's ``extra`` is kept (those are
written by hand).

Hand-written ``extra`` keys with a check: ``skip_value_check`` lists resistor
paths whose listed value is not the deck value (temperature-adjusted, golden
case ``res_tc_positional``); value tests skip them. Each path must be in
``resistors``. ``collided_circuits`` lists the circuit numbers of the later
instances with the path of an earlier one, when the directory is shifted
(read from the ``.print`` results of the case); the conversion uses it, and
the directory check must pass with it.

``--selftest`` checks the parser on built-in samples and checks every stored
case (a fresh conversion of ``hspice.lis`` must equal ``expected.json``). An
error in one case gives a ``FAIL`` line for that case, and the other cases are
still checked. The exit status is 1 when there is a failure.
"""

import argparse
import json
import re
import sys
import tempfile
from pathlib import Path

SCALE = {
    "a": 1e-18,
    "f": 1e-15,
    "p": 1e-12,
    "n": 1e-9,
    "u": 1e-6,
    "m": 1e-3,
    "k": 1e3,
    "x": 1e6,
    "g": 1e9,
    "t": 1e12,
}
VALUE_RE = re.compile(r"([+-]?(?:\d+\.?\d*|\.\d+)(?:e[+-]?\d+)?)([afpnumkxgt]?)")
AUTO_EXTRA = (
    "alter_runs",
    "after_end_runs",
    "rejected",
    "merged_listing",
    "dspf_summary",
    "unnamed_circuit_resistors",
    "shared_path_resistors",
)
TOTAL_KEYS = ("resistors", "capacitors", "mosfets", "subckt_instances")
NAME_WIDTH = 8  # element names in the .op listing are cut to this length
SUBCKT_WIDTH = 10  # subckt row tokens in the .op listing are cut to this length
NOT_DECK = ("hspice.lis", "expected.json")


def parse_value(tok):
    """Parse one HSPICE listing number such as '1.0000m', '1.000e+12', '0.'."""
    m = VALUE_RE.fullmatch(tok.strip().lower())
    if not m:
        raise ValueError(f"not an HSPICE listing number: {tok!r}")
    return float(m.group(1)) * SCALE.get(m.group(2), 1.0)


def tokens_with_pos(line):
    return [(m.start(), m.group(0)) for m in re.finditer(r"\S+", line)]


def scan_elements(lines, top):
    """(name, scope) of each element line; scope = innermost .subckt or `top`."""
    stack = []
    out = []
    for line in lines:
        s = line.strip().lower()
        if not s or s[0] in "*$+":
            continue
        words = s.split()
        if words[0] in (".subckt", ".macro"):
            stack.append(words[1] if len(words) > 1 else "")
        elif words[0] in (".ends", ".eom"):
            if stack:
                stack.pop()
        elif not words[0].startswith("."):
            out.append((words[0], stack[-1] if stack else top))
    return out


def strip_dollar(s):
    """`s` without its `$` comment (a `$` outside quotes)."""
    quote = None
    for i, c in enumerate(s):
        if quote:
            if c == quote:
                quote = None
        elif c in "'\"":
            quote = c
        elif c == "$":
            return s[:i]
    return s


def statements(lines):
    """The statements of `lines`, lowercase: each line that is not a comment
    or blank, with the text of its `+` lines (comment and blank lines
    between them are passed over), without `$` comments."""
    out = []
    for line in lines:
        s = line.strip().lower()
        if not s or s[0] in "*$":
            continue
        if s[0] == "+":
            if out:
                out[-1] += " " + strip_dollar(s[1:])
            continue
        out.append(strip_dollar(s))
    return out


def scan_x_lines(lines, top):
    """(name, callee, scope) of each X line; scope as in scan_elements.

    The callee is the last word before the first `key=value` (spaces around
    `=` allowed); the name is the first word.
    """
    stack = []
    out = []
    for s in statements(lines):
        words = re.sub(r"\s*=\s*", "=", s).split()
        if not words:
            continue
        if words[0] in (".subckt", ".macro"):
            stack.append(words[1] if len(words) > 1 else "")
        elif words[0] in (".ends", ".eom"):
            if stack:
                stack.pop()
        elif words[0].startswith("x"):
            positional = []
            for w in words:
                if "=" in w:
                    break
                positional.append(w)
            if len(positional) >= 2:
                out.append((positional[0], positional[-1], stack[-1] if stack else top))
    return out


def deck_files(case_dir):
    """(file, lines, scope of its top level) of each deck file of `case_dir`:
    line 1 of deck.sp is the title; the top level of deck.sp is "" and the
    top level of the other files is None (any definition), because an
    .include can sit inside a .subckt."""
    for f in sorted(Path(case_dir).iterdir()):
        if not f.is_file() or f.name in NOT_DECK or f.name.startswith("."):
            continue
        lines = f.read_text(errors="replace").splitlines()
        if f.name == "deck.sp":
            yield f, lines[1:], ""
        else:
            yield f, lines, None


def deck_elements(case_dir):
    """(name, scope) of every element line in the deck files of `case_dir`.

    scope is the lowercase name of the innermost enclosing .subckt, "" for the
    top level of deck.sp, and None (any definition) for the top level of the
    other files, because an .include can sit inside a .subckt.
    """
    out = []
    for _, lines, top in deck_files(case_dir):
        out.extend(scan_elements(lines, top))
    return out


def deck_x_lines(case_dir):
    """scan_x_lines() of every deck file of `case_dir` (scopes as in
    deck_elements)."""
    out = []
    for _, lines, top in deck_files(case_dir):
        out.extend(scan_x_lines(lines, top))
    return out


def deck_alter_count(case_dir):
    """Number of `.alter` lines of the first deck in `case_dir`/deck.sp, or
    None when there is no deck.sp.

    The first deck is line 2 up to the first `.end` line (line 1 is the
    title). HSPICE runs it once plus once per `.alter` (golden case `alter`).
    A complete deck after `.end` runs as a separate job (golden case
    `after_end_second_deck`), and its listing looks the same as the listing of
    an `.alter` run, so this count tells them apart. `.alter` lines in other
    files are not counted.
    """
    deck = Path(case_dir) / "deck.sp"
    if not deck.is_file():
        return None
    n = 0
    for line in deck.read_text(errors="replace").splitlines()[1:]:
        words = line.strip().lower().split()
        if not words:
            continue
        if words[0] == ".end":
            break
        if words[0] == ".alter":
            n += 1
    return n


def full_element_name(name, definition, elements):
    """Undo the 8-character cut of an element name (see the module doc)."""
    if len(name) != NAME_WIDTH:
        return name
    if elements is None:
        raise ValueError(f"element name {name!r} can be shortened; the deck files are needed")
    found = sorted({n for n, scope in elements if n.startswith(name) and scope in (None, definition)})
    where = f"definition {definition!r}" if definition else "the top level"
    if not found:
        raise ValueError(f"no element of {where} in the deck files starts with {name!r}")
    if len(found) > 1:
        raise ValueError(
            f"shortened element name {name!r} of {where} is ambiguous: {found}; "
            f"rename the elements so that their first {NAME_WIDTH} characters differ"
        )
    return found[0]


DIR_HEADER = ["number", "circuitname", "definition", "multiplier"]


def parse_directory(lines):
    """Circuit number -> (instance path, definition, multiplier).

    0 = main circuit ("", "", 1.0). Empty when the listing has no directory
    (a deck without subckt instances). A circuit that the directory names
    ``.`` (a later instance with the path of an earlier one) has the path
    None.

    An entry has 4 tokens: number, path (ends with "."), definition,
    multiplier. A token that is too long for its column stays on its line and
    the next token goes onto the next line, so an entry has 1, 2 or 3 lines
    (golden case ``long_defs_mult``). The multiplier has an engineering
    suffix (``1.00k``, ``500.00m``, ``1.00x``). The directory ends at the
    first blank line or ``*`` line. Any other line that is not part of an
    entry is an error.

    HSPICE numbers the circuits 0..N, each once. A repeated number or a gap
    is an error.
    """
    directory = {}
    start = next((i for i, l in enumerate(lines) if "circuit number to circuit name directory" in l), None)
    if start is None:
        return directory

    def add(num, value, line):
        if num in directory:
            raise ValueError(f"circuit number {num} appears twice in the circuit name directory: {line!r}")
        directory[num] = value

    entry = []  # tokens of an entry that is not complete yet
    entry_line = ""  # first line of that entry
    for line in lines[start + 1 :]:
        s = line.strip()
        toks = s.split()
        if entry:
            if not toks or len(entry) + len(toks) > 4:
                raise ValueError(f"directory entry {entry[0]}: bad continuation line {line!r}")
            entry += toks
        elif not s or s.startswith("*"):
            break
        elif toks == DIR_HEADER:
            continue
        elif toks == ["0", "main", "circuit"]:
            add(0, ("", "", 1.0), line)
            continue
        elif 2 <= len(toks) <= 4 and toks[0].isdigit() and toks[1].endswith("."):
            entry, entry_line = toks, line
        else:
            raise ValueError(f"unrecognized line in the circuit name directory: {line!r}")
        if len(entry) == 4:
            num, path, definition, mult = entry
            try:
                mult = parse_value(mult)
            except ValueError as e:
                raise ValueError(f"directory entry {num}: bad multiplier ({e})") from None
            name = None if path == "." else path.lower().rstrip(".")
            add(int(num), (name, definition.lower(), mult), entry_line)
            entry = []
    if entry:
        raise ValueError(f"directory entry {entry[0]} is not complete: {entry}")
    if directory:
        top = max(directory)
        missing = [k for k in range(top + 1) if k not in directory]
        if missing:
            shown = ", ".join(map(str, missing[:10])) + (", ..." if len(missing) > 10 else "")
            raise ValueError(
                f"circuit name directory: no entry for circuit number(s) {shown} (want each number 0..{top} once)"
            )
    return directory


def circuit_names(directory, xlines, collided=None):
    """Circuit number -> full instance path, from parse_directory() checked
    against `xlines` (deck_x_lines()); None for a circuit that has the path
    of an earlier one (a "collided" circuit, which has no path).

    HSPICE creates the instances breadth-first and lists a later instance
    with the path of an earlier one as circuit ``.``. The directory prints
    the paths of the other circuits in order, then ``.`` for as many rows as
    there are collided circuits (golden cases ``hier_dotted_collision*``):
    when a circuit comes after a collided one, the paths from the collided
    one on are shifted by one row (``hier_dotted_collision_below``,
    ``hier_dotted_collision_shift``). `collided` lists the collided circuits
    (the hand-written ``extra.collided_circuits``, from the ``.print``
    results of the case); by default they are the rows named ``.``.

    Each named circuit must be an X line, calling its definition, of the
    definition of an earlier circuit whose path is the circuit's path up to
    a ``.`` (the main circuit: the whole path). Each collided circuit must
    be such an X line whose path is the path of an earlier circuit. A row
    that fits neither is an error: a shifted directory with no (or a wrong)
    ``collided_circuits`` gives one. The check reads the X lines of every
    definition with a name (the directory prints no nesting) and takes the
    top-level lines of the files other than deck.sp as lines of any
    definition, so it can pass rows that are not right; it never makes a
    path up.
    """
    nums = sorted(k for k in directory if k != 0)
    dots = [k for k in nums if directory[k][0] is None]
    if dots != nums[len(nums) - len(dots) :]:
        raise ValueError(f"circuit name directory: a circuit named '.' comes before a named circuit (rows {dots})")
    if collided is None:
        collided = dots
    else:
        collided = sorted(collided)
        if len(collided) != len(dots) or len(set(collided)) != len(collided) or not set(collided) <= set(nums):
            raise ValueError(
                f"collided_circuits {collided}: want {len(dots)} distinct circuit numbers of the directory "
                f"(it names {len(dots)} circuit(s) '.')"
            )
    named = [k for k in nums if k not in collided]
    printed = [directory[k][0] for k in nums if directory[k][0] is not None]
    paths = dict(zip(named, printed))

    calls = {}  # definition -> {(X line name, callee)}; None: any definition
    for x, callee, scope in xlines:
        calls.setdefault(scope, set()).add((x, callee))

    def lines_of(definition):
        return calls.get(definition, set()) | calls.get(None, set())

    defs = {0: ""}
    possible = {0: {""}}  # a circuit -> the paths it can have
    having = {"": [0]}  # a path -> the circuits that can have it
    names = {0: ""}
    for k in nums:
        d = directory[k][1]
        if k in paths:
            p = paths[k]
            splits = [("", p)] + [(p[:i], p[i + 1 :]) for i, c in enumerate(p) if c == "."]
            if not any((x, d) in lines_of(defs[j]) for q, x in splits for j in having.get(q, [])):
                shift = (
                    "; with two instances of one path, the directory shifts the paths of the later rows: "
                    "give the later instances in extra.collided_circuits"
                )
                raise ValueError(
                    f"circuit name directory: circuit {k} `{p}.` ({d}) is no X line calling `{d}` in the "
                    f"definition of an earlier circuit{shift}"
                )
            names[k] = p
            possible[k] = {p}
            having.setdefault(p, []).append(k)
        else:
            found = set()
            for j in list(possible):
                for x, callee in lines_of(defs[j]):
                    if callee != d:
                        continue
                    for q in possible[j]:
                        n = x if q == "" else f"{q}.{x}"
                        if n in having:
                            found.add(n)
            if not found:
                raise ValueError(
                    f"circuit name directory: circuit {k} ({d}) is taken as a later instance with the path "
                    f"of an earlier circuit, but no X line of an earlier circuit gives it such a path"
                )
            names[k] = None
            possible[k] = found
            for n in found:
                having.setdefault(n, []).append(k)
        defs[k] = d
    return names


def split_runs(lines):
    """Line ranges of each run's operating point section (one per .alter run
    and one per job of a deck after `.end`)."""
    starts = [i for i, l in enumerate(lines) if "operating point information" in l]
    runs = []
    for k, s in enumerate(starts):
        end = starts[k + 1] if k + 1 < len(starts) else len(lines)
        for j in range(s, end):
            if "job concluded" in lines[j] or "job aborted" in lines[j]:
                end = j
                break
        runs.append(lines[s:end])
    return runs


def parse_resistor_blocks(run_lines, directory, elements, unnamed=None, names=None):
    """Resistors of one run as a list of (path, value, multiplier, circuit
    number, definition, element name).

    `elements` is deck_elements() of the case; it restores element names that
    the listing cut to 8 characters. `names` is circuit_names() (default:
    the paths that the directory prints). The resistors of a circuit with
    no path (named ``.``, or collided) are appended to `unnamed` as (circuit
    number, definition, element name, value, multiplier), or dropped when
    `unnamed` is None. The subckt row of the listing is checked against the
    path that the directory prints (also when the directory is shifted).
    """
    out = []
    i = 0
    n = len(run_lines)
    while i < n and not run_lines[i].strip().startswith("**** resistors"):
        i += 1
    if i == n:
        return out
    i += 1
    block = []

    def flush(block):
        rows = {}
        for line in block:
            label = line.strip().split()[0] if line.strip() else ""
            if label == "subckt":
                rows["subckt"] = line
            elif label == "element":
                rows["element"] = line
            elif line.strip().startswith("r value"):
                rows["value"] = line
        if "element" not in rows:
            return
        elems = tokens_with_pos(rows["element"])[1:]
        values = rows["value"].split()[2:] if "value" in rows else []
        if len(values) != len(elems):
            raise ValueError(
                f"column mismatch in resistor block: {len(elems)} element(s) in {rows['element'].strip()!r}, "
                f"{len(values)} value(s) in {rows.get('value', '').strip()!r}"
            )
        # Subckt-row tokens are left-aligned near the element token start (the
        # element token's circuit number is right-aligned, so it can start one
        # or two characters earlier). Assign each to the nearest column.
        sub = [""] * len(elems)
        if "subckt" in rows:
            for pos, tok in tokens_with_pos(rows["subckt"])[1:]:
                col = min(range(len(elems)), key=lambda c: abs(elems[c][0] - pos))
                sub[col] = tok.lower()
        for (pos, etok), vtok, stok in zip(elems, values, sub):
            m = re.fullmatch(r"(\d+):(\S+)", etok)
            if not m:
                raise ValueError(f"bad element token {etok!r}")
            num, name = int(m.group(1)), m.group(2).lower()
            if num == 0:
                inst, definition, mult = "", "", 1.0
            elif num in directory:
                inst, definition, mult = directory[num]
            else:
                raise ValueError(f"circuit number {num} of {etok} is not in the circuit name directory")
            # The subckt row is the directory path, cut to SUBCKT_WIDTH characters
            # (blank for a circuit named ".").
            if inst is None:
                if stok not in ("", "."):
                    raise ValueError(f"subckt row {stok!r} above {etok}, whose circuit the directory names '.'")
            elif stok and stok != inst and not (len(stok) == SUBCKT_WIDTH and inst.startswith(stok)):
                raise ValueError(f"subckt row {stok!r} does not match directory path {inst!r} for {etok}")
            try:
                name = full_element_name(name, definition, elements)
            except ValueError as e:
                raise ValueError(f"{etok}: {e}") from None
            if names is not None and num != 0:
                inst = names[num]
            if inst is None:
                if unnamed is not None:
                    unnamed.append((num, definition, name, parse_value(vtok), mult))
                continue
            path = f"{inst}.{name}" if inst else name
            out.append((path, parse_value(vtok), mult, num, definition, name))

    while i < n:
        line = run_lines[i]
        s = line.strip()
        if s.startswith("****") or s.startswith("total"):
            break
        if s == "":
            if block:
                flush(block)
                block = []
        else:
            block.append(line)
        i += 1
    if block:
        flush(block)
    return out


def parse_totals(lines):
    """'# resistors = N' style statistics; None when the listing has none."""
    start = next((i for i, l in enumerate(lines) if "Circuit Statistics" in l), None)
    if start is None:
        return None
    totals = {}
    for line in lines[start + 1 : start + 12]:
        for m in re.finditer(r"#\s*([A-Za-z_][A-Za-z_ ]*?)\s*=\s*(\d+)", line):
            totals[m.group(1).strip().lower()] = int(m.group(2))
    return totals


def parse_netlist_statistics(lines):
    """The 'Netlist Statistics' Type/Count table of the sim_dspf flow."""
    start = next((i for i, l in enumerate(lines) if l.strip() == "Netlist Statistics"), None)
    if start is None:
        return None
    counts = {}
    for line in lines[start + 2 :]:
        m = re.match(r"\s*(\w+)\s+(\d+)\s*$", line)
        if not m:
            break
        counts[m.group(1).lower()] = int(m.group(2))
    return counts


def parse_dspf_summary(lines):
    """The 'Parasitic parsing summary' block of a sim_dspf run."""
    start = next((i for i, l in enumerate(lines) if l.strip() == "Parasitic parsing summary"), None)
    if start is None:
        return None
    keys = {
        ("nets", "parsed"): "nets_parsed",
        ("nets", "expanded"): "nets_expanded",
        ("nets", "error net"): "error_nets",
        ("resistor(s)", "parsed"): "resistors_parsed",
        ("resistor(s)", "written"): "resistors_written",
        ("capacitor(s)", "parsed"): "capacitors_parsed",
        ("capacitor(s)", "written"): "capacitors_written",
        ("mosfet(s)", "parsed"): "mosfets_parsed",
        ("mosfet(s)", "written"): "mosfets_written",
    }
    out = {}
    group = None
    for line in lines[start + 1 : start + 16]:
        if line.strip().startswith("New net(s)"):
            out["new_nets"] = int(line.split()[-1])
            break
        m = re.match(r"(\S+)?\s+(parsed|expanded|error net|written)\s+(\d+)\s*$", line)
        if m:
            if m.group(1):
                group = m.group(1).lower()
            key = keys.get((group, m.group(2)))
            if key:
                out[key] = int(m.group(3))
    return out


def first_error(lines):
    for i, line in enumerate(lines):
        if "**error**" in line:
            text = [line.strip()]
            for nxt in lines[i + 1 : i + 4]:
                if not nxt.strip() or "*****" in nxt:
                    break
                text.append(nxt.strip())
            return " ".join(text)
    return None


def convert(text, case, elements=None, alters=None, xlines=None, collided=None):
    """Listing text -> expected.json data; `elements` = deck_elements(case dir),
    `alters` = deck_alter_count(case dir), `xlines` = deck_x_lines(case dir),
    `collided` = the hand-written extra.collided_circuits. With `alters` None,
    every run after the first one is an `.alter` run. With `xlines` None (no
    deck files), the directory is not checked (circuit_names) and
    `collided` must be None."""
    lines = text.replace("\r\n", "\n").replace("\r", "\n").split("\n")
    directory = parse_directory(lines)
    if xlines is not None:
        names = circuit_names(directory, xlines, collided) if directory else None
    elif collided is not None:
        raise ValueError("collided_circuits needs the deck files (xlines)")
    else:
        names = None
    runs = split_runs(lines)
    stats = parse_totals(lines)
    netstats = parse_netlist_statistics(lines)
    rejected = any("job aborted" in l for l in lines)
    if rejected:  # the statistics of an aborted read-in count nothing reliable
        totals = dict.fromkeys(TOTAL_KEYS)
    elif stats is not None or netstats is None:
        stats = stats or {}
        totals = {
            "resistors": stats.get("resistors", 0),
            "capacitors": stats.get("capacitors", 0),
            "mosfets": stats.get("mosfets", 0),
            "subckt_instances": len([k for k in directory if k != 0]),
        }
    else:  # sim_dspf flow: no circuit statistics, no directory
        totals = {
            "resistors": netstats.get("resistor", 0),
            "capacitors": netstats.get("capacitor", 0),
            "mosfets": netstats.get("nmos", 0) + netstats.get("pmos", 0),
            "subckt_instances": None,
        }
    resistors = []
    extra = {}
    unnamed = []
    shared = []
    if runs:
        first = parse_resistor_blocks(runs[0], directory, elements, unnamed, names)
        counts = {}
        for r in first:
            counts[r[0]] = counts.get(r[0], 0) + 1
        for p, v, mult, num, d, n in first:
            m = {"m": mult} if mult != 1.0 else {}
            if counts[p] > 1:
                shared.append({"path": p, "circuit": num, "definition": d, "element": n, "value": v, **m})
            else:
                resistors.append({"path": p, "value": v, **m})
        if shared:
            extra["shared_path_resistors"] = shared
        if unnamed:
            extra["unnamed_circuit_resistors"] = [
                {"circuit": num, "definition": d, "element": n, "value": v, **({"m": m} if m != 1.0 else {})}
                for num, d, n, v, m in unnamed
            ]
        first_job = len(runs) if alters is None else 1 + alters
        later, after_end = [], []
        for k, run in enumerate(runs[1:], start=2):
            rs = parse_resistor_blocks(run, directory, elements, None, names)
            seen = {}
            for r in rs:
                seen[r[0]] = seen.get(r[0], 0) + 1
            # a path of two resistors names neither (as in the first run)
            entry = {"run": k, "resistors": {r[0]: r[1] for r in rs if seen[r[0]] == 1}}
            (later if k <= first_job else after_end).append(entry)
        if later:
            extra["alter_runs"] = later
        if after_end:
            extra["after_end_runs"] = after_end
    dspf = parse_dspf_summary(lines)
    if dspf is not None:
        extra["dspf_summary"] = dspf
    if rejected:
        extra["rejected"] = first_error(lines) or "job aborted"
    elif runs and len(resistors) + len(unnamed) + len(shared) != totals["resistors"]:
        shown = len(resistors) + len(unnamed) + len(shared)
        extra["merged_listing"] = f"listing shows {shown} resistors, statistics count {totals['resistors']}"
    return {"case": case, "resistors": resistors, "totals": totals, "extra": extra}


def merge_extra(new, old_path):
    if old_path.exists():
        old = json.loads(old_path.read_text())
        for k, v in old.get("extra", {}).items():
            if k not in AUTO_EXTRA:
                new["extra"][k] = v
    return new


def dump(obj):
    return json.dumps(obj, indent=1) + "\n"


# --------------------------------------------------------------------------
# self test


def _filler(lo, hi):
    """Directory entries lo..hi-1 of instances without resistors.

    They fill the gaps of SAMPLE's circuit numbers (a gap is an error) and keep
    its 2- and 3-digit circuit numbers.
    """
    return "".join(f"{n:>9} {f'xf{n}.':<32}{'c':<19}1.00\n" for n in range(lo, hi))


SAMPLE = (
    """\
 ****** circuit name directory
 circuit number to circuit name directory
   number circuitname                     definition         multiplier
        0 main circuit
        1 x1.                             blk                1.00
        2 x1.xm2.                         mid                2.00
"""
    + _filler(3, 10)
    + """\
       10 x1.xm2.xl1.                     leaf               2.00
       11 xtop_instance_name_very_long_123456789.
                                            wrap               1.00
       12 xtop_instance_name_very_long_123456789.xlonginstname_abcdef.
                                            xlongcellname      1.00
       13 x2.xa.xb.xc.                    cellx              1.00
"""
    + _filler(14, 101)
    + """\
      101 xk.                             c                  1.00k
      102 xh.                             c                500.00m
      103 xmeg.                           c                  1.00x
      104 xd.                             very_long_definition_name_abcdef
                                                                 1.00
      105 xdk.                            very_long_definition_name_abcdef
                                                                 1.00k
      106 xtop_instance_name_very_long_123456789.
                                            very_long_definition_name_abcdef
                                                               500.00m
      107 xtop_instance_name_very_long_12345678b.
                                            c                  2.00k

 ****** operating point information tnom=  25.000 temp=  25.000 ******
 **** resistors

 subckt                                                          x1
 element  0:r0       0:r1       0:r2       0:r3       0:r4       1:rs
  r value    1.0000     2.0000k    4.0000m   25.4000u    1.0000x    1.000e+12
  v drop     1.0000     1.0000     1.0000     1.0000     1.0000     1.0000

 subckt    x1.xm2      x1.xm2.xl1
 element   2:rmk_mid  10:rmk_leaf
  r value   333.3333     55.5556
  v drop      1.0000      1.0000

 subckt               xtop_insta  xtop_insta  x2.xa.xb.x
 element   0:rverylon 12:rverylon 12:rshort   13:rlongnam
  r value    11.0000      7.0000      9.0000      1.0000
  v drop      1.0000      1.0000      1.0000      1.0000

 subckt     xk           xh           xmeg         xd           xdk
 element  101:rc       102:rc       103:rc       104:rverylon 105:rverylon
  r value      1.0000       2.0000k      1.0000m      7.0000       7.0000m
  v drop     999.0010u    666.6667m    999.9990n      6.9513m      7.0000u

 subckt     xtop_insta   xtop_insta
 element  106:rverylon 107:rc
  r value     14.0000     500.0000m
  v drop      13.8067m    499.7501u


          ***** job concluded

  ******  Circuit Statistics  ******
  # nodes       =       2 # elements   =      20
  # resistors   =      19 # capacitors =       1 # inductors   =       0
  # jfets       =       0 # mosfets    =       3 # U elements  =       0
"""
)

# Deck text of SAMPLE for the element-name restore (only the lines it needs).
SAMPLE_DECK = """\
* title line: rtitle_line_element is not an element
r0 1 0 1
r1 1 0 2k
r2 1 0 4m
r3 1 0 1mil
r4 1 0 1meg
rverylongtoplevel 1 0 11
.subckt blk a b
rs a b 1e12
.ends
.subckt mid a b
rmk_mid a b 1k
.ends
.subckt leaf a b
rmk_leaf a b 1k
.ends
.subckt xlongcellname a b
rverylongelementname_abc a b 7
rshort a b 9
.ends
.SUBCKT cellx a b
* rlongname_comment is a comment
rlongname_cell a b
+ 1
.ENDS
.subckt c a b
rc a b 1k
.ends
.subckt very_long_definition_name_abcdef a b
rverylongname_x a b 7
.ends
"""

# An aborted job (golden case stat_direct): HSPICE still prints statistics,
# but they do not count a circuit that was read in full.
ABORT_SAMPLE = """\
  **error** (deck.sp:3) "agauss" function cannot be defined in expressions directly.

          ***** job aborted
1****** HSPICE -- P-2019.06-SP1-1 win64 (Sep 27 2019) ******
  ******  Circuit Statistics  ******
  # nodes       =       0 # elements   =       2
  # resistors   =       1 # capacitors =       0 # inductors   =       0
  # jfets       =       0 # mosfets    =       0 # U elements  =       0
"""

# The smallest listing of a concluded job, for the check_cases() test.
TINY_LIS = """\
 ****** operating point information tnom=  25.000 temp=  25.000 ******
 **** resistors

 element  0:r1
  r value    2.0000k
  v drop     1.0000

          ***** job concluded
  ******  Circuit Statistics  ******
  # resistors   =       1 # capacitors =       0 # inductors   =       0
  # jfets       =       0 # mosfets    =       0 # U elements  =       0
"""
TINY_DECK = "* tiny\nv1 1 0 1\nr1 1 0 2k\n.op\n.end\n"

# Two instances with one path (golden case hier_dotted_collision): the later
# one is circuit "." and its resistor has no path.
UNNAMED_LIS = """\
 ****** circuit name directory
 circuit number to circuit name directory
   number circuitname                     definition         multiplier
        0 main circuit
        1 x0.                             wrap               1.00 
        2 x0.xq.                          cell3              1.00 
        3 .                               cell               2.00 
  
 ****** operating point information tnom=  25.000 temp=  25.000 ******
 **** resistors

 subckt              x0         x0.xq      
 element  0:rt1      1:rw       2:rc       3:rc      
  r value    1.0000     4.0000     3.0000     1.0000 
  v drop   250.0000m  631.5789m  750.0000m  421.0526m


          ***** job concluded
  ******  Circuit Statistics  ******
  # resistors   =       4 # capacitors =       0 # inductors   =       0
  # jfets       =       0 # mosfets    =       0 # U elements  =       0
"""
# The deck of UNNAMED_LIS: the top-level `x0.xq` and wrap's `xq` under `x0`.
UNNAMED_DECK = """\
* unnamed
.subckt cell a
rc a 0 1
.ends
.subckt cell3 a
rc a 0 3
.ends
.subckt wrap a
rw a 0 4
xq a cell m=2
.ends
rt1 1 0 1
x0 1 wrap
x0.xq 1 cell3
"""

# A collision whose later instance is not the last circuit (golden case
# hier_dotted_collision_shift): the directory prints the names of the
# circuits from that instance on shifted by one, and "." for the last one.
SHIFT_DECK = """\
* shift
.subckt leafk a b
rk a b 5
.ends
.subckt leafp a b
rp a b 7
.ends
.subckt cella a b
rc a b 2
xk a b leafk
.ends
.subckt cellb a b
rc a b 3
.ends
.subckt mid a b
xb a b cella
.ends
.subckt wrap a b
xa.xb a b cellb
.ends
.subckt other a b
xp a b leafp $ a comment
ro a b 11
.ends
x0 1 t1 wrap
x0.xa 1
+ t2 mid
x9 1 t3 other m = 1
"""
SHIFT_LIS = """\
 ****** circuit name directory
 circuit number to circuit name directory
   number circuitname                     definition         multiplier
        0 main circuit
        1 x0.                             wrap               1.00
        2 x0.xa.                          mid                1.00
        3 x9.                             other              1.00
        4 x0.xa.xb.                       cellb              1.00
        5 x9.xp.                          cella              1.00
        6 x0.xa.xb.xk.                    leafp              1.00
        7 .                               leafk              1.00

 ****** operating point information tnom=  25.000 temp=  25.000 ******
 **** resistors

 subckt   x9         x0.xa.xb   x9.xp      x0.xa.xb.x
 element  3:ro       4:rc       5:rc       6:rp       7:rk
  r value   11.0000     3.0000     2.0000     7.0000     5.0000
  v drop   810.5263m  750.0000m  588.2353m  810.5263m  588.2353m


          ***** job concluded
  ******  Circuit Statistics  ******
  # resistors   =       5 # capacitors =       0 # inductors   =       0
  # jfets       =       0 # mosfets    =       0 # U elements  =       0
"""

# Two resistors with one full path (golden case elem_dotted_name_collision):
# the device `rb` of the instance `x0.ra`, and the device `ra.rb` of `x0`.
SHARED_DECK = """\
* shared
.subckt cell a b
rb a b 2
.ends
.subckt wrap a b
ra.rb a b 3
.ends
x0.ra 1 t1 cell
x0 1 t2 wrap
"""
SHARED_LIS = """\
 ****** circuit name directory
 circuit number to circuit name directory
   number circuitname                     definition         multiplier
        0 main circuit
        1 x0.ra.                          cell               1.00
        2 x0.                             wrap               1.00

 ****** operating point information tnom=  25.000 temp=  25.000 ******
 **** resistors

 subckt                         x0.ra      x0
 element  0:rt1      0:rt2      1:rb       2:ra.rb
  r value    1.0000     1.0000     2.0000     3.0000
  v drop   333.3333m  250.0000m  666.6667m  750.0000m


          ***** job concluded
  ******  Circuit Statistics  ******
  # resistors   =       4 # capacitors =       0 # inductors   =       0
  # jfets       =       0 # mosfets    =       0 # U elements  =       0
"""
CASES_DIR = Path(__file__).resolve().parent.parent / "cases"


def check_case(case_dir):
    """Failure lines of one stored case: a fresh conversion of its listing
    must equal its expected.json, and the totals must fit the job's end."""
    name = case_dir.name
    lis, exp = case_dir / "hspice.lis", case_dir / "expected.json"
    if not lis.exists() or not exp.exists():
        return [f"{name}: hspice.lis or expected.json missing"]
    text = lis.read_text()
    stored = json.loads(exp.read_text())
    collided = stored.get("extra", {}).get("collided_circuits")
    fresh = convert(
        text, name, deck_elements(case_dir), deck_alter_count(case_dir), deck_x_lines(case_dir), collided
    )
    out = []
    for key in ("case", "resistors", "totals"):
        if fresh[key] != stored.get(key):
            out.append(f"{name}: '{key}' differs from a fresh conversion")
    for key in AUTO_EXTRA:
        if fresh["extra"].get(key) != stored.get("extra", {}).get(key):
            out.append(f"{name}: extra.{key} differs from a fresh conversion")
    paths = {r["path"] for r in stored.get("resistors", [])}
    for p in stored.get("extra", {}).get("skip_value_check", []):
        if p not in paths:
            out.append(f"{name}: extra.skip_value_check path {p!r} is not in 'resistors'")
    totals = stored.get("totals") or {}
    if "rejected" in stored.get("extra", {}):
        if any(totals.get(k) is not None for k in TOTAL_KEYS):
            out.append(f"{name}: rejected case, but totals are not null: {totals}")
    else:
        if "job concluded" not in text:
            out.append(f"{name}: no 'job concluded'")
        if not all(isinstance(totals.get(k), int) for k in ("resistors", "capacitors", "mosfets")):
            out.append(f"{name}: device totals are not numbers: {totals}")
    return out


def check_cases(cases_dir):
    """(number of case dirs, failure lines) of every case dir in cases_dir.

    An exception in one case becomes a failure line of that case, and the
    other cases are still checked.
    """
    dirs = sorted(p for p in Path(cases_dir).iterdir() if p.is_dir())
    failures = []
    for d in dirs:
        try:
            failures += check_case(d)
        except Exception as e:  # report and go on with the next case
            failures.append(f"{d.name}: {type(e).__name__}: {e}")
    return len(dirs), failures


def selftest():
    failures = []

    def check(cond, msg):
        if not cond:
            failures.append(msg)

    def check_error(what, fn, *needles):
        """fn() must raise ValueError, and its text must hold every needle
        (the offending line or token of the listing)."""
        try:
            fn()
        except ValueError as e:
            for needle in needles:
                check(needle in str(e), f"{what}: error text {str(e)!r} lacks {needle!r}")
            return
        check(False, f"{what}: no error")

    def sample_edit(old, new):
        if old not in SAMPLE:
            raise AssertionError(f"SAMPLE text not found: {old!r}")
        return SAMPLE.replace(old, new)

    def sample_elements():
        return scan_elements(SAMPLE_DECK.splitlines()[1:], "")

    long_dir = "xtop_instance_name_very_long_123456789.xlonginstname_abcdef"
    long_def = "very_long_definition_name_abcdef"

    def t_parse_value():
        for tok, want in [
            ("1.0000m", 1e-3),
            ("1.0000x", 1e6),
            ("25.4000u", 25.4e-6),
            ("1.000e+12", 1e12),
            ("333.3333", 333.3333),
            ("0.", 0.0),
            ("-1.9688", -1.9688),
            ("232.6766a", 232.6766e-18),
            ("1.0000g", 1e9),
        ]:
            got = parse_value(tok)
            check(abs(got - want) <= 1e-12 * max(abs(want), 1e-30), f"parse_value({tok!r}) = {got}, want {want}")

    def t_scan_elements():
        elements = sample_elements()
        check(
            ("rlongname_cell", "cellx") in elements
            and ("rverylongtoplevel", "") in elements
            and not any(n.startswith(("rtitle", "rlongname_comment", "+")) for n, _ in elements),
            f"scan_elements: {elements}",
        )

    def t_directory():
        directory = parse_directory(SAMPLE.split("\n"))
        for num, want_entry in [
            (12, (long_dir, "xlongcellname", 1.0)),  # path / definition + multiplier
            (101, ("xk", "c", 1000.0)),  # 1.00k
            (102, ("xh", "c", 0.5)),  # 500.00m
            (103, ("xmeg", "c", 1e6)),  # 1.00x
            (104, ("xd", long_def, 1.0)),  # path + definition / multiplier
            (105, ("xdk", long_def, 1000.0)),
            (106, ("xtop_instance_name_very_long_123456789", long_def, 0.5)),  # 3 lines
            (107, ("xtop_instance_name_very_long_12345678b", "c", 2000.0)),
        ]:
            check(directory.get(num) == want_entry, f"parse_directory: entry {num} = {directory.get(num)}")
        check(sorted(directory) == list(range(108)), f"parse_directory: numbers {sorted(directory)}, want 0..107")

    def t_convert_sample():
        data = convert(SAMPLE, "sample", sample_elements())
        got = [(r["path"], r["value"]) for r in data["resistors"]]
        want = [
            ("r0", 1.0),
            ("r1", 2000.0),
            ("r2", 4e-3),
            ("r3", 25.4e-6),
            ("r4", 1e6),
            ("x1.rs", 1e12),
            ("x1.xm2.rmk_mid", 333.3333),
            ("x1.xm2.xl1.rmk_leaf", 55.5556),
            ("rverylongtoplevel", 11.0),
            (long_dir + ".rverylongelementname_abc", 7.0),
            (long_dir + ".rshort", 9.0),
            ("x2.xa.xb.xc.rlongname_cell", 1.0),
            ("xk.rc", 1.0),
            ("xh.rc", 2000.0),
            ("xmeg.rc", 1e-3),
            ("xd.rverylongname_x", 7.0),
            ("xdk.rverylongname_x", 7e-3),
            ("xtop_instance_name_very_long_123456789.rverylongname_x", 14.0),
            ("xtop_instance_name_very_long_12345678b.rc", 0.5),
        ]
        check(len(got) == len(want), f"sample: {len(got)} resistors, want {len(want)}")
        ms = {r["path"]: r.get("m") for r in data["resistors"]}
        want_ms = {
            "x1.xm2.rmk_mid": 2.0,
            "x1.rs": None,
            "xk.rc": 1000.0,
            "xh.rc": 0.5,
            "xmeg.rc": 1e6,
            "xd.rverylongname_x": None,
            "xdk.rverylongname_x": 1000.0,
            "xtop_instance_name_very_long_123456789.rverylongname_x": 0.5,
            "xtop_instance_name_very_long_12345678b.rc": 2000.0,
        }
        check(all(ms.get(p) == m for p, m in want_ms.items()), f"sample m fields: {ms}")
        for (gp, gv), (wp, wv) in zip(got, want):
            check(gp == wp and abs(gv - wv) <= 1e-9 * abs(wv), f"sample: got {gp}={gv}, want {wp}={wv}")
        check(
            data["totals"] == {"resistors": 19, "capacitors": 1, "mosfets": 3, "subckt_instances": 107},
            f"sample totals: {data['totals']}",
        )
        check(data["extra"] == {}, f"sample extra: {data['extra']}")
        # An element of another file outside any .subckt can belong to any definition.
        other = [e for e in sample_elements() if e[0] != "rlongname_cell"] + [("rlongname_cell", None)]
        check(
            "x2.xa.xb.xc.rlongname_cell" in [r["path"] for r in convert(SAMPLE, "sample", other)["resistors"]],
            "element of an included file (scope None)",
        )

    # Shortened names that cannot be restored, and listing rows that do not
    # fit, are errors, never silent. Each error names the offending token.
    def t_block_errors():
        elements = sample_elements()
        check_error(
            "ambiguous name",
            lambda: convert(SAMPLE, "sample", elements + [("rverylong_other", "")]),
            "ambiguous",
            "0:rverylon",
        )
        check_error(
            "unknown name",
            lambda: convert(SAMPLE, "sample", [e for e in elements if e[0] != "rlongname_cell"]),
            "no element",
            "13:rlongnam",
        )
        check_error("no deck files", lambda: convert(SAMPLE, "sample", None), "deck files are needed", "10:rmk_leaf")
        bad_row = sample_edit(" x2.xa.xb.x\n", " x2.xa.xb.y\n")
        check_error("bad subckt row", lambda: convert(bad_row, "sample", elements), "does not match", "x2.xa.xb.y")
        short_row = sample_edit("  r value   333.3333     55.5556\n", "  r value   333.3333\n")
        check_error(
            "column mismatch", lambda: convert(short_row, "sample", elements), "column mismatch", "10:rmk_leaf"
        )
        bad_tok = sample_edit("   2:rmk_mid  10:rmk_leaf", "   2:rmk_mid  10-rmk_leaf")
        check_error("bad element token", lambda: convert(bad_tok, "sample", elements), "10-rmk_leaf")

    # A directory line that fits no entry form is an error, never the end of
    # the directory (the entries after it would be lost). So are a repeated
    # circuit number and a gap in 0..N. Each error names the offending line.
    def t_directory_errors():
        elements = sample_elements()
        main = "        0 main circuit\n"
        entry_2 = "        2 x1.xm2.                         mid                2.00\n"
        entry_13 = "       13 x2.xa.xb.xc.                    cellx              1.00\n"
        entry_102 = "      102 xh.                             c                500.00m\n"
        mult_104 = "                                                                 1.00\n"
        edits = [
            (
                "unknown directory line",
                entry_102,
                entry_102 + " something else\n",
                ["unrecognized line", " something else"],
            ),
            ("missing multiplier line", mult_104, "", ["bad continuation line", "105 xdk."]),
            ("blank continuation line", mult_104, "\n", ["directory entry 104", "bad continuation line ''"]),
            ("bad multiplier", "500.00m\n", "500.00q\n", ["bad multiplier", "500.00q"]),
            (
                "repeated circuit number",
                entry_2,
                entry_2 + entry_2.replace("x1.xm2.", "x1.xm3."),
                ["circuit number 2 appears twice", "x1.xm3."],
            ),
            ("repeated main circuit", main, main + main, ["circuit number 0 appears twice", "0 main circuit"]),
            ("gap in the numbers", entry_13, "", ["no entry for circuit number(s) 13 ("]),
            ("no main circuit", main, "", ["no entry for circuit number(s) 0 ("]),
        ]
        for what, old, new, needles in edits:
            text = sample_edit(old, new)
            check_error(what, lambda: convert(text, "sample", elements), *needles)
        ends_in_entry = SAMPLE.split(mult_104)[0].rstrip("\n")
        check_error(
            "listing ends in an entry",
            lambda: convert(ends_in_entry, "sample", elements),
            "not complete",
            "'104'",
        )

    # A rejected (aborted) job has no totals: null, never 0 or the statistics
    # of the aborted read-in.
    def t_rejected():
        data = convert(ABORT_SAMPLE, "abort", [])
        nulls = {"resistors": None, "capacitors": None, "mosfets": None, "subckt_instances": None}
        check(data["totals"] == nulls, f"rejected totals: {data['totals']}")
        check(data["resistors"] == [], f"rejected resistors: {data['resistors']}")
        check(
            data["extra"].get("rejected", "").startswith('**error** (deck.sp:3) "agauss" function'),
            f"rejected text: {data['extra']}",
        )
        check("merged_listing" not in data["extra"], f"rejected extra: {data['extra']}")

    # A complete deck after `.end` runs as a separate job (golden case
    # after_end_second_deck). Its listing looks like an `.alter` run, so the
    # number of `.alter` lines of the first deck in deck.sp tells them apart.
    def t_after_end_runs():
        two = TINY_LIS + TINY_LIS.replace("0:r1", "0:r2").replace("2.0000k", "5.0000k")
        later = [{"run": 2, "resistors": {"r2": 5000.0}}]
        jobs = convert(two, "two", [], alters=0)
        check(
            "alter_runs" not in jobs["extra"] and jobs["extra"].get("after_end_runs") == later,
            f"after_end_runs, no .alter: {jobs['extra']}",
        )
        check(
            jobs["resistors"] == [{"path": "r1", "value": 2000.0}] and jobs["totals"]["resistors"] == 1,
            f"after_end_runs, first job: {jobs['resistors']} {jobs['totals']}",
        )
        alt = convert(two, "two", [], alters=1)
        check(
            alt["extra"].get("alter_runs") == later and "after_end_runs" not in alt["extra"],
            f"after_end_runs, one .alter: {alt['extra']}",
        )
        old = convert(two, "two", [])  # no deck count: every later run is an .alter run
        check(old["extra"].get("alter_runs") == later, f"after_end_runs, no count: {old['extra']}")
        with tempfile.TemporaryDirectory() as tmp:
            d = Path(tmp)
            check(deck_alter_count(d) is None, "deck_alter_count without deck.sp")
            (d / "deck.sp").write_text(
                ".alter on line 1 is the title\n.ALTER\n.endl\n.ends\n  .alter x\n.END\n.alter\n.end\n"
            )
            check(deck_alter_count(d) == 2, f"deck_alter_count = {deck_alter_count(d)}, want 2")

    # A circuit that the directory names "." (a later instance with the path
    # of an earlier one) has no path: its resistors are not filed as
    # top-level resistors, and they count for merged_listing.
    def t_unnamed_circuit():
        data = convert(UNNAMED_LIS, "unnamed", [])
        got = [(r["path"], r["value"]) for r in data["resistors"]]
        check(got == [("rt1", 1.0), ("x0.rw", 4.0), ("x0.xq.rc", 3.0)], f"unnamed circuit: resistors {got}")
        want = [{"circuit": 3, "definition": "cell", "element": "rc", "value": 1.0, "m": 2.0}]
        check(data["extra"] == {"unnamed_circuit_resistors": want}, f"unnamed circuit: extra {data['extra']}")
        check(data["totals"]["subckt_instances"] == 3, f"unnamed circuit: totals {data['totals']}")
        check(parse_directory(UNNAMED_LIS.split("\n"))[3] == (None, "cell", 2.0), "unnamed circuit: directory")
        bad = UNNAMED_LIS.replace("x0.xq      \n", "x0.xq      x0.xq.y\n")
        check_error("subckt row of circuit '.'", lambda: convert(bad, "unnamed", []), "x0.xq.y", "3:rc")
        # the same with the deck: the directory fits its X lines
        xl = scan_x_lines(UNNAMED_DECK.splitlines()[1:], "")
        checked = convert(UNNAMED_LIS, "unnamed", [], xlines=xl)
        check(checked == data, f"unnamed circuit, checked against the deck: {checked}")

    # Each directory row must be an X line of the definition of an earlier
    # circuit (its name after that circuit's path, and its definition): a
    # row that is not, as in a shifted directory, is an error, never a
    # wrong path. The hand-written `collided_circuits` names the later
    # instances of a shifted directory, and the paths follow it.
    def t_directory_check():
        xl = scan_x_lines(SHIFT_DECK.splitlines()[1:], "")
        for want in [("x0.xa", "mid", ""), ("xp", "leafp", "other"), ("x9", "other", ""), ("xk", "leafk", "cella")]:
            check(want in xl, f"scan_x_lines: {want} not in {xl}")
        check(len(xl) == 7, f"scan_x_lines: {len(xl)} X lines, want 7: {xl}")
        check_error(
            "shifted directory",
            lambda: convert(SHIFT_LIS, "shift", [], xlines=xl),
            "circuit 5",
            "x9.xp",
            "cella",
            "collided_circuits",
        )
        data = convert(SHIFT_LIS, "shift", [], xlines=xl, collided=[5])
        got = [(r["path"], r["value"]) for r in data["resistors"]]
        want = [("x9.ro", 11.0), ("x0.xa.xb.rc", 3.0), ("x9.xp.rp", 7.0), ("x0.xa.xb.xk.rk", 5.0)]
        check(got == want, f"shift: resistors {got}")
        unnamed = [{"circuit": 5, "definition": "cella", "element": "rc", "value": 2.0}]
        check(data["extra"] == {"unnamed_circuit_resistors": unnamed}, f"shift: extra {data['extra']}")
        check(data["totals"]["subckt_instances"] == 7, f"shift: totals {data['totals']}")
        check_error(
            "a collided circuit with no earlier path",
            lambda: convert(SHIFT_LIS, "shift", [], xlines=xl, collided=[4]),
            "circuit 4",
            "cellb",
        )
        check_error(
            "more collided circuits than '.' rows",
            lambda: convert(SHIFT_LIS, "shift", [], xlines=xl, collided=[5, 6]),
            "collided_circuits",
            "[5, 6]",
        )
        check_error(
            "collided circuits without the deck",
            lambda: convert(SHIFT_LIS, "shift", [], collided=[5]),
            "collided_circuits",
        )
        # the stored case: its directory is shifted, so it needs its key
        d = CASES_DIR / "hier_dotted_collision_shift"
        check_error(
            "stored shifted case without collided_circuits",
            lambda: convert((d / "hspice.lis").read_text(), d.name, deck_elements(d), 0, deck_x_lines(d)),
            "circuit 5",
            "collided_circuits",
        )
        wrong_def = UNNAMED_LIS.replace("x0.xq.                          cell3", "x0.xq.                          cellz")
        uxl = scan_x_lines(UNNAMED_DECK.splitlines()[1:], "")
        check_error(
            "a row with another definition", lambda: convert(wrong_def, "unnamed", [], xlines=uxl), "circuit 2", "cellz"
        )

    # Two resistors with one full path: the .op listing does not say which
    # one a .print of that path finds, so neither is in `resistors`.
    def t_shared_path():
        xl = scan_x_lines(SHARED_DECK.splitlines()[1:], "")
        data = convert(SHARED_LIS, "shared", [], xlines=xl)
        got = [(r["path"], r["value"]) for r in data["resistors"]]
        check(got == [("rt1", 1.0), ("rt2", 1.0)], f"shared path: resistors {got}")
        want = [
            {"path": "x0.ra.rb", "circuit": 1, "definition": "cell", "element": "rb", "value": 2.0},
            {"path": "x0.ra.rb", "circuit": 2, "definition": "wrap", "element": "ra.rb", "value": 3.0},
        ]
        check(data["extra"] == {"shared_path_resistors": want}, f"shared path: extra {data['extra']}")

    # check_cases() reports an error in one case as a FAIL line of that case
    # and goes on with the other cases.
    def t_check_cases():
        broken = (
            " circuit number to circuit name directory\n"
            "   number circuitname                     definition         multiplier\n"
            "        0 main circuit\n"
            " something else\n\n" + TINY_LIS
        )
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            cases = [("a_broken", broken), ("b_good", TINY_LIS), ("c_differs", TINY_LIS), ("e_bad_skip", TINY_LIS)]
            for name, lis in cases:
                d = root / name
                d.mkdir()
                (d / "deck.sp").write_text(TINY_DECK)
                (d / "hspice.lis").write_text(lis)
                data = convert(TINY_LIS, name, deck_elements(d))
                if name == "c_differs":
                    data["resistors"][0]["value"] = 3000.0
                if name == "b_good":
                    data["extra"]["skip_value_check"] = ["r1"]
                if name == "e_bad_skip":
                    data["extra"]["skip_value_check"] = ["r1", "r9"]
                (d / "expected.json").write_text(dump(data))
            (root / "d_missing").mkdir()
            n, fails = check_cases(root)
        check(n == 5, f"check_cases: {n} cases, want 5")
        names = ("a_broken", "b_good", "c_differs", "d_missing", "e_bad_skip")
        per_case = {name: [f for f in fails if f.startswith(name + ":")] for name in names}
        check(
            len(per_case["a_broken"]) == 1 and "something else" in per_case["a_broken"][0],
            f"check_cases: broken case: {fails}",
        )
        check(per_case["b_good"] == [], f"check_cases: good case: {fails}")
        check(
            len(per_case["c_differs"]) == 1 and "'resistors' differs" in per_case["c_differs"][0],
            f"check_cases: changed case: {fails}",
        )
        check(len(per_case["d_missing"]) == 1 and "missing" in per_case["d_missing"][0], f"check_cases: {fails}")
        check(
            len(per_case["e_bad_skip"]) == 1 and "skip_value_check path 'r9'" in per_case["e_bad_skip"][0],
            f"check_cases: skip_value_check path that is not a resistor: {fails}",
        )
        check(len(fails) == 4, f"check_cases: {fails}")

    ncases = 0

    def t_stored_cases():
        nonlocal ncases
        ncases, fails = check_cases(CASES_DIR)
        failures.extend(fails)

    for name, test in [
        ("parse_value", t_parse_value),
        ("scan_elements", t_scan_elements),
        ("parse_directory", t_directory),
        ("convert sample", t_convert_sample),
        ("block errors", t_block_errors),
        ("directory errors", t_directory_errors),
        ("rejected totals", t_rejected),
        ("after_end runs", t_after_end_runs),
        ("unnamed circuit", t_unnamed_circuit),
        ("directory check", t_directory_check),
        ("shared path", t_shared_path),
        ("check_cases", t_check_cases),
        ("stored cases", t_stored_cases),
    ]:
        try:
            test()
        except Exception as e:  # one broken test must not hide the others
            failures.append(f"{name}: {type(e).__name__}: {e}")

    for f in failures:
        print("FAIL:", f)
    print(f"selftest: {ncases} cases checked, {len(failures)} failure(s)")
    return 1 if failures else 0


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("lis", nargs="?")
    ap.add_argument("case", nargs="?")
    ap.add_argument("-o", "--output", help="write here, keeping hand-written 'extra' keys of an existing file")
    ap.add_argument("--selftest", action="store_true")
    args = ap.parse_args()
    if args.selftest:
        return selftest()
    if not args.lis or not args.case:
        ap.error("need <hspice.lis> <case> (or --selftest)")
    lis = Path(args.lis)
    case_dir = lis.resolve().parent
    # the hand-written collided_circuits of the output file (or of the
    # expected.json next to the listing)
    old = Path(args.output) if args.output else case_dir / "expected.json"
    collided = json.loads(old.read_text()).get("extra", {}).get("collided_circuits") if old.exists() else None
    data = convert(
        lis.read_text(),
        args.case,
        deck_elements(case_dir),
        deck_alter_count(case_dir),
        deck_x_lines(case_dir),
        collided,
    )
    if args.output:
        out = Path(args.output)
        out.write_text(dump(merge_extra(data, out)))
    else:
        sys.stdout.write(dump(data))
    return 0


if __name__ == "__main__":
    sys.exit(main())
