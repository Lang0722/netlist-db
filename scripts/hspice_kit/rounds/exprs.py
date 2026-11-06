#!/usr/bin/env python3
"""Expression compare. usage: exprs.py write KIT FILE [PER_DECK] | cmp KIT FILE
FILE has one expression per line. Each deck holds `.param xK='expr'`, a `.meas` per
expression, and it is read with `.ms0` columns."""
import glob, math, os, re, shutil, subprocess, sys
S = os.environ['NDB_ROUNDS']
EVAL = S + '/target/release/examples/zz_eval'
HEAD = ".option ingold=2 numdgt=10 measdgt=10 post=0 nomod\n.model n nmos level=54\n.param a=2 b=3 c=-1.5 d=0 e1=1e-3 u1=1u\nM1 d g 0 0 n l=1u w=1u\nvd d 0 1\nvg g 0 1\n.dc vd 1 1 1\n"

def read(path):
    return [x.rstrip('\n') for x in open(path) if x.strip()]

def write(kit, exprs, per):
    root = os.path.join(S, kit)
    if os.path.exists(root):
        shutil.rmtree(root)
    os.makedirs(root + '/decks')
    for start in range(0, len(exprs), per):
        chunk = exprs[start:start + per]
        name = f'e{start:05d}'
        with open(f'{root}/decks/{name}.sp', 'w') as f:
            f.write(f'* {name}\n' + HEAD)
            for k, e in enumerate(chunk):
                f.write(f".param x{k}='{e}'\n.meas dc m{k} param='x{k}'\n")
            f.write('.end\n')
    print(len(exprs), 'expressions in', len(glob.glob(root + '/decks/*.sp')), 'decks')

def ms0(path):
    """column name -> value text"""
    if not os.path.exists(path):
        return None
    rows = [x.split() for x in open(path, errors='replace').read().splitlines()]
    rows = [r for r in rows if r and not r[0].startswith('$') and not r[0].startswith('.TITLE')]
    # names first, and then the values: the same count
    names, vals, in_names = [], [], True
    for r in rows:
        if in_names:
            names += r
            if names and names[-1].lower() == 'alter#':
                in_names = False
        else:
            vals += r
    return dict(zip([n.lower() for n in names], vals))

def main():
    kit, path = sys.argv[2], sys.argv[3]
    exprs = read(path)
    per = int(sys.argv[4]) if len(sys.argv) > 4 else 40
    if sys.argv[1] == 'write':
        write(kit, exprs, per)
        return
    root = os.path.join(S, kit)
    same = differ = hstop = nerr = bstop = none = 0
    rows = []
    for deck in sorted(glob.glob(root + '/decks/*.sp')):
        name = os.path.basename(deck)[:-3]
        start = int(name[1:])
        text = open(deck).read()
        chunk = re.findall(r"(?m)^\.param x\d+='(.*)'$", text)
        cols = ms0(f'{root}/results/{name}.ms0')
        lis = f'{root}/results/{name}.lis'
        lines = open(lis, errors='replace').read().splitlines() if os.path.exists(lis) else []
        errs = [x.strip() for x in lines if re.search(r'\*\*error\*\*|aborted', x)]
        out = subprocess.run([EVAL, os.path.basename(deck)], cwd=root + '/decks', capture_output=True, text=True, timeout=60).stdout
        nd = {}
        for x in out.splitlines():
            k, _, v = x.partition(' ')
            nd[k.lower()] = v
        for k, e in enumerate(chunk):
            nv = nd.get(f'x{k}')
            hv = None if cols is None else cols.get(f'm{k}')
            if cols is None or hv is None:
                if nv is None or nv.startswith('error'):
                    bstop += 1
                else:
                    hstop += 1
                    rows.append(('HSTOP', e, errs[0][:70] if errs else '?', nv))
                continue
            if hv.lower() == 'failed':
                hv_num = None
            else:
                try:
                    hv_num = float(hv)
                except ValueError:
                    hv_num = None
            # (the print of `.meas`: `*.` from 1e100 on, `0.` below 1e-28)
            if hv == '*.' and nv is not None and ('not a finite' in nv or (not nv.startswith('error') and abs(float(nv)) >= 1e100)):
                same += 1
                continue
            if hv == '0.' and nv is not None and not nv.startswith('error') and abs(float(nv)) < 1.0000001e-28:
                same += 1
                continue
            if nv is None or nv.startswith('error'):
                nerr += 1
                rows.append(('NERR', e, hv, nv))
                continue
            nv_num = float(nv)
            if hv_num is None:
                rows.append(('DIFF', e, hv, nv))
                differ += 1
                continue
            big = max(abs(hv_num), abs(nv_num))
            if big == 0 or abs(hv_num - nv_num) <= 2e-9 * big or (math.isinf(nv_num) and abs(hv_num) > 1e300):
                same += 1
            else:
                differ += 1
                rows.append(('DIFF', e, hv, nv))
    for r in rows:
        print(f'{r[0]:6} {r[1]:48} H={r[2]:<30} N={r[3]}')
    print(f'same {same}  differ {differ}  HSPICE stops only {hstop}  netlist-db error only {nerr}  both stop {bstop}')

main()
