#!/usr/bin/env python3
"""One row per deck: the M lines, HSPICE's and netlist-db's L and W. usage: rows.py KITDIR [BINARY]"""
import glob, os, re, subprocess, sys
S = os.environ['NDB_ROUNDS']
sys.path.insert(0, S + '/hard')
import cmp_list
kit = sys.argv[1]
ms = S + '/target/release/examples/model_select'
for a_ in sys.argv[2:]:
    if not a_.startswith('--'):
        ms = a_
near = lambda a, b: a is not None and b is not None and abs(a - b) <= 6e-4 * max(abs(a), abs(b))
bad = 0
for p in sorted(glob.glob(kit + '/decks/*.sp')):
    n = os.path.basename(p)[:-3]
    text = open(p, errors='replace').read()
    first = [x for x in text.splitlines() if re.match(r'(?i)m\w*\s', x)]
    tail = re.sub(r'^\S+\s+', '', first[-1] if first else '?')[:34]
    path = os.path.join(kit, 'results', n + '.lis')
    lines = open(path, errors='replace').read().splitlines() if os.path.exists(path) else []
    errs = cmp_list.errors(lines)
    tab = cmp_list.table(lines) if not errs else {}
    r = subprocess.run([ms, os.path.basename(p)], cwd=kit + '/decks', capture_output=True, text=True)
    rows = [x.split('\t') for x in r.stdout.splitlines()[1:]]
    err = 'netlist has errors' in r.stderr
    if not lines:
        print(f'{n:14} {tail:35} no listing')
        continue
    if errs:
        h = 'STOP'
    else:
        h = ' '.join(f"{v.get('model','?').split(':')[-1]} L={v.get('l drawn')} W={v.get('w drawn')}" for v in tab.values())
    if err:
        nd = 'ERR'
    else:
        nd = ' '.join(f'{x[5] if len(x) > 5 else "?"} L={x[2]} W={x[3]}' for x in rows)
    same = 'ok'
    if errs:
        same = 'ok' if err or any(x[5:] and x[5].startswith('error') for x in rows) else 'H-STOPS'
    elif err:
        same = 'N-ERR'
    else:
        byname = {x[0].lower(): x for x in rows}
        for path_, v in tab.items():
            x = byname.get(path_)
            if not x or x[5:] and x[5].startswith('error'):
                same = 'DIFF'
                continue
            for k, c in (('l drawn', 2), ('w drawn', 3)):
                d = cmp_list.num(v.get(k))
                if d and not near(d, cmp_list.num(x[c])):
                    same = 'DIFF'
    bad += same != 'ok'
    if '--all' in sys.argv or same != 'ok':
        print(f'{n:14} {tail:35} {same:8} H: {h[:48]:48} N: {nd[:44]}')
print('not equal:', bad)
