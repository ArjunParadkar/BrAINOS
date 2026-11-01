#!/usr/bin/env python3
"""
eval.py -- evaluate an exported policy deterministically, in parallel.

  python eval.py runs/getup_v1/policy.json --episodes 200 [--stress] [--nominal] [--film out.png]

Success is the task's own physical test (tasks.success_*), never the
reward. --stress pins every randomised quantity to the edges of its range
(the brief's "sim-to-real stress" setting); --nominal turns randomisation
off. Prints success by start kind and writes a JSON summary next to the
policy.
"""
import argparse, json, math, multiprocessing as mp, os, sys
os.environ.setdefault('OMP_NUM_THREADS', '1')
import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)


def load_policy(path):
    P = json.load(open(path))
    Ws = [(np.asarray(l['w'], np.float32), np.asarray(l['b'], np.float32)) for l in P['layers']]
    mean = np.asarray(P['obs_mean'], np.float32); std = np.asarray(P['obs_std'], np.float32)

    def act(o):
        x = np.clip((o - mean) / std, -P['obs_clip'], P['obs_clip'])
        for k, (w, b) in enumerate(Ws):
            x = w @ x + b
            if k + 1 < len(Ws):
                x = np.where(x > 0, x, np.exp(x) - 1)
        return np.clip(x, -1, 1)
    return P, act


def stress(env):
    """randomisation pinned to the edges of the training ranges"""
    r = env.rng
    m, n = env.m, env.nom
    m.body_mass[:] = n['mass'] * r.choice([0.85, 1.15], len(n['mass']))
    m.body_ipos[:] = n['ipos']; m.body_ipos[env.chest] += r.choice([-0.01, 0.01], 3)
    mu = r.choice([0.4, 1.2])
    m.geom_friction[:] = n['friction']; m.geom_friction[:, 0] = np.where(n['friction'][:, 0] > 0, mu, 0)
    m.tendon_stiffness[:] = n['ten_k'] * r.choice([0.85, 1.15], len(n['ten_k']))
    m.jnt_stiffness[:] = n['jnt_k'] * r.choice([0.85, 1.15], len(n['jnt_k']))
    env.strength = r.choice([0.8, 1.2], len(env.strength))
    env.latency_ms = int(r.choice([2, 15]))
    env.enc_sd = 0.004
    a = math.radians(1.0) * r.choice([-1, 1], 2)
    cr, sr, cp, sp = math.cos(a[0]), math.sin(a[0]), math.cos(a[1]), math.sin(a[1])
    env.imu_R = np.array([[cp, 0, sp], [0, 1, 0], [-sp, 0, cp]]) @ np.array([[1, 0, 0], [0, cr, -sr], [0, sr, cr]])
    env.gyro_bias = r.choice([-0.02, 0.02], 3)


def run(args):
    path, task_name, task_kw, seed, n, mode = args
    import env as E, tasks as T, ppo
    if path.startswith('ZERO:'):
        # the reference baseline: zero residual = the skill's reference played open loop
        P = dict(task=path[5:]); act = lambda o: np.zeros(E.NA)
    else:
        P, act = load_policy(path)
    e = E.BlurEnv(ppo.registry()[task_name](**task_kw), seed=seed, randomize=(mode != 'nominal'))
    if mode == 'stress':
        # BLUR_DR=wide: every quantity at an edge of the Track 4e.1 ranges,
        # uneven + softest ground always; v1: the old edges
        e.randomize_body = (lambda: e.randomize_wide(edges=True)) if E.DR != 'v1' else (lambda: stress(e))
    out = []
    for k in range(n):
        o = e.reset()
        peak_tau = 0.0
        while True:
            o, r, d, info = e.step(act(o))
            peak_tau = max(peak_tau, float(np.max(np.abs(e.d.actuator_force[e.act]) / E.TAU)))
            if d:
                break
        succ = T.success_standing(e) if task_name == 'getup' else e.task.success(e)
        out.append(dict(kind=getattr(e.task, 'kind', ''), success=bool(succ), up=e.up(), z=float(e.d.qpos[2]), peak_tau=peak_tau))
    return out


def main():
    p = argparse.ArgumentParser()
    p.add_argument('policy')
    p.add_argument('--task', default=None)
    p.add_argument('--task-kw', default='{}')
    p.add_argument('--episodes', type=int, default=200)
    p.add_argument('--procs', type=int, default=16)
    p.add_argument('--stress', action='store_true')
    p.add_argument('--nominal', action='store_true')
    p.add_argument('--out', default=None, help='summary JSON path (default: next to the policy)')
    a = p.parse_args()
    P = dict(task=a.policy[5:]) if a.policy.startswith('ZERO:') else json.load(open(a.policy))
    task = a.task or P['task']
    mode = 'stress' if a.stress else 'nominal' if a.nominal else 'random'
    per = max(1, a.episodes // a.procs)
    with mp.get_context('fork').Pool(a.procs) as pool:
        res = sum(pool.map(run, [(a.policy, task, json.loads(a.task_kw), 10_000 + i, per, mode) for i in range(a.procs)]), [])
    kinds = sorted({r['kind'] for r in res})
    import env as E
    summ = dict(policy=a.policy, task=task, mode=mode, dr=E.DR, deterministic=True, episodes=len(res),
                success=float(np.mean([r['success'] for r in res])),
                by_kind={k: dict(n=sum(r['kind'] == k for r in res),
                                 success=float(np.mean([r['success'] for r in res if r['kind'] == k]))) for k in kinds},
                peak_torque_frac_p95=float(np.percentile([r['peak_tau'] for r in res], 95)))
    print(json.dumps(summ, indent=1))
    json.dump(summ, open(a.out or a.policy.replace('.json', f'.eval_{mode}.json'), 'w'), indent=1)


if __name__ == '__main__':
    main()
