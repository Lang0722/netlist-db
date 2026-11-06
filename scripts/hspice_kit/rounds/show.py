#!/usr/bin/env python3
"""One line per deck: what HSPICE did. usage: show.py RESULTS [PREFIX...]"""
import glob, os, re, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from cmp_list import table, errors
res = sys.argv[1]
prefixes = tuple(sys.argv[2:])
for path in sorted(glob.glob(os.path.join(res, "*.lis"))):
    name = os.path.basename(path)[:-4]
    if prefixes and not name.startswith(prefixes):
        continue
    lines = open(path, errors="replace").read().splitlines()
    errs = errors(lines)
    t = table(lines)
    count = None
    for x in lines:
        m = re.search(r"# mosfets\s*=\s*(\d+)", x)
        if m:
            count = m.group(1)
            break
    body = " ".join(f"{p}[{r.get('model','?').split(':',1)[-1]} L={r.get('l drawn')} W={r.get('w drawn')}" + (f" nf={r.get('nf')}" if r.get('nf') not in (None, '1.0000', '1.000e+00') else "") + "]" for p, r in t.items())
    e = next((x for x in errs if "job aborted" not in x), errs[0] if errs else "")
    e = re.sub(r"^\*+\s*error\s*\*+\s*", "", e, flags=re.I)
    print(f"{name:30} " + (f"STOP: {e[:112]}" if errs else f"#{count} {body[:150]}"))
