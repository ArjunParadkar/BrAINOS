#!/usr/bin/env python3
"""
summarize.py DIR -- one table for a campaign: every run ranked by its DETERMINISTIC
evaluation (policy mean action, 200 episodes, eval.py), nominal and stress side by
side, next to the skill's reference baseline (the reference played open loop).

  nominal  no randomisation, flat ground
  stress   every Track 4e.1 range pinned at an edge, uneven + soft ground

Training-curve numbers are not results and are not shown as success. A skill's
"best" is the run with the highest stress success (ties: nominal). Plain stdlib, so
it runs anywhere (laptop, login node).
"""
import csv, json, math, os, sys

D = sys.argv[1]
R = os.path.join(D, 'runs')


def wilson(p, n, z=1.96):
    if not n:
        return (float('nan'), float('nan'))
    c = (p + z * z / (2 * n)) / (1 + z * z / n)
    h = z * math.sqrt(p * (1 - p) / n + z * z / (4 * n * n)) / (1 + z * z / n)
    return max(0.0, c - h), min(1.0, c + h)


def load(p):
    try:
        return json.load(open(p))
    except Exception:
        return None


rows, refs = [], {}
manifest = [l.split() for l in open(os.path.join(D, 'manifest.txt')) if l.strip() and not l.startswith('#')] \
    if os.path.exists(os.path.join(D, 'manifest.txt')) else []
for parts in manifest:
    rid, task = parts[0], parts[1]
    d = os.path.join(R, rid)
    nom, st = load(os.path.join(d, 'eval_nominal.json')), load(os.path.join(d, 'eval_stress.json'))
    tr = load(os.path.join(d, 'TRAINED'))
    steps = act = ''
    pc = os.path.join(d, 'progress.csv')
    if os.path.exists(pc):
        rr = list(csv.reader(open(pc)))
        if len(rr) > 1:
            steps = int(rr[-1][2]); act = float(rr[-1][1]) / 3600
    status = ('done' if os.path.exists(os.path.join(d, 'DONE')) else 'evaluating' if tr or task.startswith('ZERO:')
              else 'training' if steps else 'queued')
    if os.path.exists(os.path.join(D, 'state', 'failed', rid)):
        status = 'FAILED'
    row = dict(id=rid, skill=task.replace('ZERO:', ''), ref=task.startswith('ZERO:'), seed=parts[2], status=status,
               nom=nom['success'] if nom else None, n_nom=nom['episodes'] if nom else 0,
               st=st['success'] if st else None, n_st=st['episodes'] if st else 0,
               tau95=st.get('peak_torque_frac_p95') if st else None, steps=steps, act=act)
    (refs.__setitem__(row['skill'], row) if row['ref'] else rows.append(row))


def f(p, n):
    if p is None:
        return '—'
    lo, hi = wilson(p, n)
    return f'{100 * p:.0f} % [{100 * lo:.0f}–{100 * hi:.0f}]'


key = lambda r: (-(r['nom'] if r['nom'] is not None else -1), -(r['st'] if r['st'] is not None else -1))
out = ['# Day-one campaign: deterministic evaluation',
       '',
       'Success = the skill\'s own physical test on the final state / whole episode, policy mean action,',
       'deterministic episodes (200 per evaluation in the campaign; 95 % Wilson interval). Nominal = no randomisation; stress = every Track 4e.1',
       'range at an edge, uneven + soft ground. "reference" = the skill\'s reference played open loop.',
       'Training curves are not results.', '',
       '| rank | run | skill | seed | nominal | stress | reference nominal / stress | peak torque p95 (× deliverable, stress) | steps | active h | status |',
       '|---|---|---|---|---|---|---|---|---|---|---|']
for k, r in enumerate(sorted(rows, key=key), 1):
    ref = refs.get(r['skill'])
    rs = f"{f(ref['nom'], ref['n_nom'])} / {f(ref['st'], ref['n_st'])}" if ref else '—'
    out.append(f"| {k} | {r['id']} | {r['skill']} | {r['seed']} | {f(r['nom'], r['n_nom'])} | {f(r['st'], r['n_st'])} | {rs} | "
               f"{'' if r['tau95'] is None else round(r['tau95'], 2)} | {r['steps']} | {'' if r['act'] == '' else round(r['act'], 2)} | {r['status']} |")
out += ['', '## Best per skill (by stress success; only evaluated runs)', '',
        '| skill | best run | nominal | stress | reference stress | seeds evaluated |', '|---|---|---|---|---|---|']
for s in sorted({r['skill'] for r in rows}):
    ev = [r for r in rows if r['skill'] == s and r['st'] is not None]
    if not ev:
        out.append(f'| {s} | — | — | — | — | 0 |'); continue
    b = max(ev, key=lambda r: (r['st'], r['nom'] or 0))
    ref = refs.get(s)
    out.append(f"| {s} | {b['id']} | {f(b['nom'], b['n_nom'])} | {f(b['st'], b['n_st'])} | "
               f"{f(ref['st'], ref['n_st']) if ref else '—'} | {len(ev)} |")
if refs:
    out += ['', '## References (the skill reference played open loop, zero residual)', '',
            '| skill | nominal | stress |', '|---|---|---|']
    for sk, r in sorted(refs.items()):
        out.append(f"| {sk} | {f(r['nom'], r['n_nom'])} | {f(r['st'], r['n_st'])} |")
u = os.path.join(D, 'state', 'usage.log')
if os.path.exists(u):
    s, e = {}, {}
    for l in open(u):
        p = l.split()
        if len(p) >= 4:
            (s if p[2] == 'start' else e)[p[0]] = int(p[3])
    h = sum(e.get(j, v) - v for j, v in s.items() if j in e) / 3600
    out += ['', f'Lane node-hours (finished lane jobs): {h:.1f}; lane jobs: {len(s)}.']
print('\n'.join(out))
