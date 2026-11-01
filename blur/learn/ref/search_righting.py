#!/usr/bin/env python3
"""
search_righting.py -- design the righting keyframe reference for one resting
class (left side down / right side down / on the back) by CMA-ES over a
small, structured keyframe family, scored open loop on the real physics.

The structure is hand-chosen, cat-style: two shaping keys (tuck, spine
twist, hind-leg swing, tail) then the sternal tuck. The search only tunes
the numbers. BLUR is not left/right symmetric (RL_abd reaches -0.55, RR_abd
-1.01; spine twist -0.36..+1.29), so each class gets its own reference.

    python search_righting.py left  out.json [--gens 25] [--pop 12]
"""
import os, sys, json, math, argparse, time
os.environ.setdefault('OMP_NUM_THREADS', '1')
HERE = os.path.dirname(os.path.abspath(__file__)); sys.path.insert(0, os.path.dirname(HERE)); sys.path.insert(0, HERE)
import numpy as np
import cma
import env as E, tasks as T
from keyframes import Reference, STERNAL, pose, IX, settle

KEYJ = ['FR_abd', 'FL_abd', 'RR_abd', 'RL_abd', 'FX_pitch', 'FX_knee', 'RX_pitch', 'RX_knee',
        'spine_twist', 'spine_s1', 'spine_s2', 'tail_winch_pitch', 'tail_winch_yaw']


def jrange(k):
    i = IX[k.replace('FX_', 'FR_').replace('RX_', 'RR_')]
    return E.QMIN[i], E.QMAX[i]


def key_from(x):
    kv = {}
    for k, v in zip(KEYJ, x):
        lo, hi = jrange(k)
        kv[k] = lo + (hi - lo) * (0.5 + 0.5 * math.tanh(v))
    return pose(STERNAL, **kv)


def ref_from(x):
    nk = len(KEYJ)
    k1, k2 = key_from(x[:nk]), key_from(x[nk:2 * nk])
    d = 0.3 + 1.2 * (0.5 + 0.5 * np.tanh(x[2 * nk:2 * nk + 3]))
    t1 = d[0]; t2 = t1 + d[1]; t3 = t2 + d[2]
    return Reference([(0, k1), (t1, k1), (t2, k2), (t3, STERNAL), (t3 + 0.8, STERNAL)], 'righting')


def classify(env):
    R = env.chest_R(); up = R[2, 2]; roll = math.degrees(math.atan2(R[2, 1], R[2, 2]))
    return 'belly' if up > 0.7 else 'back' if up < -0.7 else ('right' if roll > 0 else 'left')


def starts(cls, n, seed0=5000):
    """settled fallen states of one class, from the GetUp task's own fallen-pose generator"""
    out = []
    s = seed0
    while len(out) < n:
        e = E.BlurEnv(T.GetUp(p_push=0.0), seed=s); e.reset(); s += 1
        if classify(e) == cls:
            out.append(s - 1)
    return out


def rollout(x, seed, randomize=True):
    e = E.BlurEnv(T.GetUp(p_push=0.0), seed=seed, randomize=randomize)
    e.reset()                                         # the fallen, settled start
    ref = ref_from(x)
    t = 0.0; ok_run = 0; best_run = 0
    while t < ref.T + 1.0:
        e.apply_targets(ref(t), 20); t += 0.02
        ok = e.up() > 0.85
        ok_run = ok_run + 1 if ok else 0
        best_run = max(best_run, ok_run)
    torque = 0.0
    return e.up(), best_run * 0.02, e.up() > 0.85 and ok_run >= 25


def fitness(x, seeds):
    r = [rollout(x, s) for s in seeds]
    up = np.mean([a for a, _, _ in r]); succ = np.mean([c for _, _, c in r])
    return -(up + 2.0 * succ), succ


def main():
    p = argparse.ArgumentParser()
    p.add_argument('cls'); p.add_argument('out')
    p.add_argument('--gens', type=int, default=25); p.add_argument('--pop', type=int, default=12)
    p.add_argument('--n-starts', type=int, default=8)
    a = p.parse_args()
    seeds = starts(a.cls, a.n_starts)
    held = starts(a.cls, 20, seed0=90000)             # held-out starts for the final report
    nx = 2 * len(KEYJ) + 3
    es = cma.CMAEvolutionStrategy(np.zeros(nx), 0.8, {'popsize': a.pop, 'seed': 1, 'verbose': -9})
    best = (1e9, None, 0)
    log = open(a.out + '.log', 'a')
    for g in range(a.gens):
        X = es.ask()
        F = [fitness(x, seeds) for x in X]
        es.tell(X, [f for f, _ in F])
        i = int(np.argmin([f for f, _ in F]))
        if F[i][0] < best[0]:
            best = (F[i][0], np.array(X[i]), F[i][1])
        print(f'[{time.strftime("%H:%M:%S")}] {a.cls} gen {g} best {-best[0]:.3f} train-success {best[2]:.2f} gen-mean {-np.mean([f for f, _ in F]):.3f}', file=log, flush=True)
        json.dump(dict(cls=a.cls, x=best[1].tolist(), fit=-best[0], train_success=best[2], gen=g), open(a.out, 'w'))
    r = [rollout(best[1], s) for s in held]
    res = dict(cls=a.cls, x=best[1].tolist(), fit=-best[0], train_success=best[2],
               heldout_success=float(np.mean([c for _, _, c in r])), heldout_n=len(held))
    json.dump(res, open(a.out, 'w'))
    print(json.dumps({k: v for k, v in res.items() if k != 'x'}), file=log, flush=True)


if __name__ == '__main__':
    main()
