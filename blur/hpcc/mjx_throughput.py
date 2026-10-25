#!/usr/bin/env python3
"""
mjx_throughput.py -- MJX physics steps per second for BLUR at N parallel envs.

Same model and actuators as mjx_parity.py (training model, stand gains,
1 ms step). Each env starts from the stand keyframe with a small random
velocity kick and tracks stand targets plus a per-env 2 Hz sinusoid on
every joint (+-0.15 rad), so feet load and unload. One timed call runs K
physics steps for all N envs inside a single jit (lax.scan, vmap); the
first call (compile) is reported separately.

  env-steps/s = N x K / wall        (physics steps at 1 ms)
  policy-steps/s = env-steps/s / 20 (a 50 Hz policy)

  bpy mjx_throughput.py --n 1024,4096,8192 --solver default,fast --out results/4c
"""
import argparse, json, os, sys, time

HERE = os.path.dirname(os.path.abspath(__file__))
BLUR = os.path.dirname(HERE)
sys.path[:0] = [os.path.join(BLUR, 'learn'), os.path.join(BLUR, 'dev'), BLUR, HERE]

ap = argparse.ArgumentParser()
ap.add_argument('--n', default='1024,4096,8192')
ap.add_argument('--solver', default='fast,fast_feet,tol6,default')   # see SOLVERS
ap.add_argument('--impl', default='jax')
ap.add_argument('--k', type=int, default=200)
ap.add_argument('--reps', type=int, default=3)
ap.add_argument('--out', default='results/4c')
ap.add_argument('--tag', default='')
args, _ = ap.parse_known_args()
sys.argv = [sys.argv[0], '--variant', 'cpu']           # mjx_parity parses its own args at import

import numpy as np
import jax, jax.numpy as jp
import mujoco
from mujoco import mjx
import mjx_parity as P

SOLVERS = {'default': dict(), 'fast': dict(iters=5, ls=8), 'tol6': dict(tol=1e-6),
           'fast_feet': dict(iters=5, ls=8, feetonly=True)}


def bench(n, solver):
    m, act = P.build(0.001, **SOLVERS[solver])
    d0 = P.initial(m, 'walk')
    mx = mjx.put_model(m, impl=args.impl)
    dx = mjx.put_data(m, d0, impl=args.impl)
    stand = jp.asarray(np.clip(P.E.QSTAND, P.E.QMIN, P.E.QMAX))
    act_j = jp.asarray(act)
    rng = jax.random.PRNGKey(0)
    k1, k2 = jax.random.split(rng)
    qv = jax.random.normal(k1, (n, m.nv)) * 0.05
    phase = jax.random.uniform(k2, (n, 1)) * 2 * jp.pi
    batch = jax.vmap(lambda v: dx.replace(qvel=dx.qvel + v))(qv)

    def one(d, ph, t):
        u = jp.zeros(m.nu).at[act_j].set(stand + 0.15 * jp.sin(2 * jp.pi * 2.0 * t + ph))
        return mjx.step(mx, d.replace(ctrl=u))

    vstep = jax.vmap(one, in_axes=(0, 0, None))

    def run(b):
        def body(c, _):
            b, t = c
            return (vstep(b, phase[:, 0], t), t + 0.001), None
        (b, _), _ = jax.lax.scan(body, (b, 0.0), None, length=args.k)
        return b

    f = jax.jit(run)
    t0 = time.time(); out = f(batch); jax.block_until_ready(out.qpos); tc = time.time() - t0
    # one more untimed call: the output pytree can differ from the input in weak
    # types, which recompiles once (seen at n=1024 on the A100, job 1367639)
    t0 = time.time(); out = f(out); jax.block_until_ready(out.qpos); tc2 = time.time() - t0
    walls = []
    reps = args.reps if tc2 < 60 else 1               # the slow settings: one timed call is enough
    for _ in range(reps):
        t0 = time.time(); out = f(out); jax.block_until_ready(out.qpos); walls.append(time.time() - t0)
    w = min(walls)
    ok = bool(jp.isfinite(out.qpos).all())
    z = float(jp.mean(out.qpos[:, 2]))
    r = dict(n=n, solver=solver, impl=args.impl, k=args.k, first_call_s=tc, second_call_s=tc2, wall_s=w,
             env_steps_per_s=n * args.k / w, policy_steps_per_s=n * args.k / w / 20, finite=ok, mean_base_z=z,
             ncon_candidates=int(dx._impl.contact.dist.shape[0]) if args.impl == 'jax' else None)
    print(json.dumps(r), flush=True)
    return r


def main():
    os.makedirs(args.out, exist_ok=True)
    dev = jax.devices()[0]
    res = dict(device=str(dev), device_kind=getattr(dev, 'device_kind', ''), jax=jax.__version__,
               mujoco=mujoco.__version__, runs=[])
    print(res, flush=True)
    kind = res['device_kind'].replace(' ', '_') or 'dev'
    path = os.path.join(args.out, f'throughput_{kind}{args.tag}.json')
    for s in args.solver.split(','):
        for n in [int(x) for x in args.n.split(',')]:
            try:
                res['runs'].append(bench(n, s))
            except Exception as e:
                res['runs'].append(dict(n=n, solver=s, error=repr(e)[:1500])); print('ERROR', n, s, repr(e)[:400], flush=True)
            json.dump(res, open(path, 'w'), indent=1)          # after every run: a timeout keeps what finished
    print('wrote', path)


if __name__ == '__main__':
    main()
