"""Gate E2 evaluation: the stable-gait test and the push test.

    # laptop size (well under a minute)
    $MOTOR_PY motor/gait/eval.py --policy tripod --policy int8:RUN/policy.bmoti8 \
        --seeds 4 --seconds 5 --pushes 3 --json out.json
    # pre-registered size (cluster)
    $MOTOR_PY motor/gait/eval.py --policy tripod --policy int8:RUN/policy.bmoti8 \
        --seeds 20 --seconds 10 --pushes 50 --json RUN/eval.json

Policies:
    int8:PATH        an int8 blob, run through int8_ref.py (the arithmetic the body runs)
    float:CKPT       a training checkpoint's float actor (diagnostic only)
    tripod[:SPEED]   the hand-tuned tripod gait (`miniblur_core::gait`), speed as the
                     gait's own 0.1..1.0 scalar (default 1.0, the fastest it runs);
                     joint targets replayed open loop at the gait's sample rate

The hand-tuned gait's poses come from `brainos motor gait-reference`
when the binary has that command, else from `tripod_python` below, a
line-for-line port of `miniblur_core::gait::pose_at` (the JSON records
which one was used; `--check-port` compares the two).

Stable gait (pre-registered): over N seeds of T seconds, mean forward
speed >= 0.06 m/s and at least 18/20 episodes never cross 20 deg tilt.
At a reduced N the tilt count is scaled (>= 90 %), and the verdict is
labelled with the size it was run at. Episodes are NOT cut at the tilt
limit here: each runs its full T and records whether and when the limit
was crossed. Forward speed = displacement along the heading the
episode started with, divided by T, averaged over all episodes
(including ones that crossed the limit).

Each seed draws the start yaw and small joint offsets (+-0.05 rad).
The model is the generated one, unrandomised, unless --dr.

Push test: episodes of --push-seconds (default 8 s); at a time uniform
in [2, push-seconds - 2] s a horizontal force is applied to the base for
10 ms in a uniformly random direction, sized so force * 10 ms is the
stated impulse. Survived = the tilt limit was never crossed in the
episode. Episodes that crossed the limit before the push are counted
separately. The push times, directions and start states are the same
for every policy (paired).

Timestep check (NOT pre-registered; reported next to the verdict, never
folded into it): --dt-check K reruns the stable-gait test with the same
50 Hz control on a K-times finer physics step. A gait whose speed
depends on the physics step is suspect as a simulator artifact; smoke
runs found exactly that (chattering, hopping gaits on 1-2 feet).

Contact check (NOT pre-registered either): --contact-tc T reruns the
stable-gait test with every geom's contact time constant set to T s
(MuJoCo solref[0]; the generated model keeps the default 0.02 s, under
which a 3.15 mm foot can sink more than its own radius). A gait that
only works on soft contact is suspect.
"""
import argparse, json, math, os, subprocess, sys, time
import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import env as E

HERE = os.path.dirname(os.path.abspath(__file__))
BIN = os.environ.get('BRAINOS_BIN', os.path.join(HERE, '..', '..', 'os', 'target', 'release', 'brainos'))
SPEED_GATE = 0.5 * E.TOP_SPEED_MPS          # 0.06 m/s
PUSH_LEVELS = (0.05, 0.1, 0.2, 0.4)
PUSH_MS = 10
JOINTS12 = [f'{leg}_{ax}' for leg in E.LEGS for ax in ('swing', 'lift')]


# ------------------------------------------------------------ hand-tuned gait
def tripod_python(speed=1.0, seconds=10.0, amp=18.0, lift=28.0, cycle_ms=1200, spc=48):
    """Port of miniblur_core::gait (GaitParams::default, Dir::Fwd, Plan::sample_ms).
    Returns (sample_ms, [{joint: deg}, ...]) covering `seconds`."""
    speed = min(max(speed, 0.1), 1.0)
    sample_ms = int(np.float32(cycle_ms) / np.float32(speed) / np.float32(spc))
    tripod_a = {'L1', 'R2', 'L3'}
    out = []
    n = int(math.ceil(seconds * 1000 / sample_ms))
    for k in range(n):
        t = (k % spc) / spc
        pose = {}
        for leg in E.LEGS:
            if leg in tripod_a:
                u, swinging = (t * 2, True) if t < 0.5 else ((t - 0.5) * 2, False)
            else:
                u, swinging = (t * 2, False) if t < 0.5 else ((t - 0.5) * 2, True)
            sense = -1.0 if leg.startswith('L') else 1.0      # forward_sign; side_sense = 1 for Fwd
            if swinging:
                pose[f'{leg}_swing'] = sense * amp * -math.cos(u * math.pi)
                pose[f'{leg}_lift'] = lift * math.sin(u * math.pi)
            else:
                pose[f'{leg}_swing'] = sense * amp * (1.0 - 2.0 * u)
                pose[f'{leg}_lift'] = 0.0
        out.append(pose)
    return sample_ms, out


def tripod_rust(speed=1.0, seconds=10.0):
    """From `brainos motor gait-reference SPEED_MPS SECONDS`; None if unavailable."""
    if not os.path.exists(BIN):
        return None
    mps = speed * E.TOP_SPEED_MPS
    r = subprocess.run([BIN, 'motor', 'gait-reference', repr(mps), repr(seconds)],
                       capture_output=True, text=True)
    if r.returncode != 0 or not r.stdout.strip():
        return None
    rows = [json.loads(l) for l in r.stdout.splitlines() if l.strip()]
    sample_ms = rows[1]['t_ms'] - rows[0]['t_ms'] if len(rows) > 1 else 1
    return sample_ms, [{j: row[j] for j in JOINTS12} for row in rows]


def tripod_poses(speed, seconds, source='auto'):
    got = tripod_rust(speed, seconds) if source in ('auto', 'rust') else None
    if got is not None:
        return got + ('rust',)
    if source == 'rust':
        raise SystemExit(f'{BIN} has no `motor gait-reference`')
    return tripod_python(speed, seconds) + ('python',)


class Tripod:
    def __init__(self, speed=1.0, seconds=10.0, source='auto'):
        self.sample_ms, poses, self.source = tripod_poses(speed, seconds + 1.0, source)
        self.name = f'tripod:{speed:g}'
        self.speed = speed
        self.table = None
        self.poses = poses

    def _ctrl_table(self, env):
        # the 10 driven joints, by actuator name; followers are moved by the model
        return np.array([[math.radians(p[n]) for n in env.lay.act_names] for p in self.poses])

    def run_step(self, env, obs):
        if self.table is None:
            self.table = self._ctrl_table(env)
        tab, sm = self.table, self.sample_ms
        return env.step_targets(lambda ms: tab[min(ms // sm, len(tab) - 1)]), None


class Learned:
    def __init__(self, name, fn):
        self.name, self.fn = name, fn

    def run_step(self, env, obs):
        o, _, _, _, info = env.step(self.fn(obs))
        return info['tilt'], o


def make_policy(spec, seconds, source='auto'):
    kind, _, arg = spec.partition(':')
    if kind == 'tripod':
        return Tripod(float(arg) if arg else 1.0, seconds, source)
    if kind == 'int8':
        import int8_ref
        return Learned(spec, int8_ref.policy_fn(arg))
    if kind == 'float':
        import torch
        from ppo import Agent, float_policy
        ck = torch.load(arg, map_location='cpu', weights_only=False)
        ag = Agent(hidden=tuple(ck['config']['hidden']))
        ag.load_state_dict(ck['agent'])
        return Learned(spec, float_policy(ag.actor))
    raise SystemExit(f'unknown policy {spec}')


# ------------------------------------------------------------ episodes
def episode(pol, seed, seconds, dr=False, push=None, phys_div=1, contact_tc=None):
    """One episode of `seconds`. push: (time_s, impulse_Ns, direction_rad) or
    None. contact_tc None = the model as generated."""
    rng = np.random.default_rng(seed)
    env = E.HexEnv(dr=dr, max_steps=10 ** 9, phys_div=phys_div, contact_tc=contact_tc)
    obs = env.reset(rng)
    if push is not None:
        t, J, ang = push
        f = J / (PUSH_MS * 1e-3)
        env.push = (int(round(t * 1000)), PUSH_MS, np.array([f * math.cos(ang), f * math.sin(ang), 0.0]))  # ms
    n = int(round(seconds * E.CONTROL_HZ))
    max_tilt, t_cross = 0.0, None
    for k in range(n):
        tilt, o = pol.run_step(env, obs)
        obs = o if o is not None else obs
        max_tilt = max(max_tilt, tilt)
        if t_cross is None and tilt > E.TILT_LIMIT_DEG:
            t_cross = (k + 1) / E.CONTROL_HZ
    prog = env.forward_progress()
    return dict(seed=seed, progress_m=prog, speed_mps=prog / seconds, max_tilt_deg=max_tilt,
                crossed=t_cross is not None, t_cross_s=t_cross)


def stable_gait(pol, seeds, seconds, dr=False, phys_div=1, contact_tc=None):
    eps = [episode(pol, s, seconds, dr, phys_div=phys_div, contact_tc=contact_tc) for s in seeds]
    n = len(eps)
    ok = sum(not e['crossed'] for e in eps)
    need = math.ceil(0.9 * n)
    mean = float(np.mean([e['speed_mps'] for e in eps]))
    return dict(n=n, seconds=seconds, mean_speed_mps=mean, upright=ok, upright_needed=need,
                speed_needed=SPEED_GATE, passes=bool(mean >= SPEED_GATE and ok >= need),
                full_size=(n >= 20 and seconds >= 10.0), phys_div=phys_div, contact_tc=contact_tc,
                episodes=eps)


def push_test(pol, levels, pushes, seconds, seed0, dr=False):
    out = []
    for li, J in enumerate(levels):
        res = []
        for k in range(pushes):
            r = np.random.default_rng([seed0, li, k])
            t = float(r.uniform(2.0, seconds - 2.0))
            ang = float(r.uniform(0.0, 2 * math.pi))
            seed = int(r.integers(2 ** 31))
            e = episode(pol, seed, seconds, dr, push=(t, J, ang))
            e.update(push_t=t, push_dir=ang, fell_before_push=bool(e['crossed'] and e['t_cross_s'] < t))
            res.append(e)
        surv = sum(not e['crossed'] for e in res)
        out.append(dict(impulse_Ns=J, pushes=pushes, survived=surv, rate=surv / pushes,
                        fell_before_push=sum(e['fell_before_push'] for e in res), episodes=res))
    return out


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('--policy', action='append', required=True)
    ap.add_argument('--seeds', type=int, default=20)
    ap.add_argument('--seed0', type=int, default=0, help='first stable-gait seed (training evals use 1000+)')
    ap.add_argument('--seconds', type=float, default=10.0)
    ap.add_argument('--pushes', type=int, default=50)
    ap.add_argument('--levels', default=','.join(map(str, PUSH_LEVELS)))
    ap.add_argument('--push-seconds', type=float, default=8.0)
    ap.add_argument('--skip-push', action='store_true')
    ap.add_argument('--contact-tc', type=float, default=0.005,
                    help='also rerun the stable test with contact time constant T s (0 = skip); not pre-registered')
    ap.add_argument('--dt-check', type=int, default=2,
                    help='also rerun the stable test at physics step 1 ms / K (0 = skip); not pre-registered')
    ap.add_argument('--dr', action='store_true', help='randomise the assumed numbers in evaluation too')
    ap.add_argument('--gait-source', default='auto', choices=['auto', 'rust', 'python'])
    ap.add_argument('--check-port', action='store_true',
                    help='compare the Rust gait-reference with the Python port and exit')
    ap.add_argument('--json', default=None)
    a = ap.parse_args()

    if a.check_port:
        r = tripod_rust(1.0, 3.0)
        if r is None:
            raise SystemExit(f'{BIN}: no `motor gait-reference`')
        for speed in (1.0, 0.5, 0.35):
            (sr, pr), (sp, pp) = tripod_rust(speed, 3.0), tripod_python(speed, 3.0)
            n = min(len(pr), len(pp))
            d = max(abs(pr[i][j] - pp[i][j]) for i in range(n) for j in JOINTS12)
            print(f'speed {speed}: sample_ms rust {sr} python {sp}, samples {len(pr)}/{len(pp)}, '
                  f'max |deg diff| {d:.2e}')
        return

    t0 = time.time()
    levels = [float(x) for x in a.levels.split(',') if x]
    seeds = list(range(a.seed0, a.seed0 + a.seeds))
    report = dict(model_sha256=E.model_sha256(), model=E.MODEL_PATH, dr=a.dr,
                  size=dict(seeds=a.seeds, seconds=a.seconds, pushes=a.pushes,
                            push_seconds=a.push_seconds, levels=levels),
                  policies={})
    for spec in a.policy:
        pol = make_policy(spec, max(a.seconds, a.push_seconds), a.gait_source)
        t1 = time.time()
        r = dict(stable=stable_gait(pol, seeds, a.seconds, a.dr))
        if a.dt_check > 1:
            if isinstance(pol, Tripod):
                pol.table = None
            r['dt_check'] = stable_gait(pol, seeds, a.seconds, a.dr, phys_div=a.dt_check)
        if a.contact_tc > 0:
            r['contact_check'] = stable_gait(pol, seeds, a.seconds, a.dr, contact_tc=a.contact_tc)
        if not a.skip_push and a.pushes > 0:
            r['push'] = push_test(pol, levels, a.pushes, a.push_seconds, 7_000 + a.seed0, a.dr)
        r['wall_s'] = time.time() - t1
        if isinstance(pol, Tripod):
            r['gait_source'] = pol.source
            r['sample_ms'] = pol.sample_ms
        report['policies'][spec] = r
    report['wall_s'] = time.time() - t0

    print(f"model {report['model_sha256'][:16]}  dr={a.dr}  size: {a.seeds} seeds x {a.seconds:g} s, "
          f"{a.pushes} pushes/level x {a.push_seconds:g} s")
    print(f"{'policy':34s} {'speed m/s':>10s} {'upright':>9s} {'stable':>8s}  " +
          ' '.join(f'{J:>7g}Ns' for J in levels) + ('   | dt/%d speed, upright' % a.dt_check if a.dt_check > 1 else '')
          + ('   | tc %g s speed, upright' % a.contact_tc if a.contact_tc > 0 else ''))
    for spec, r in report['policies'].items():
        s = r['stable']
        verdict = ('PASS' if s['passes'] else 'fail') + ('' if s['full_size'] else '*')
        cells = ' '.join(f"{p['survived']:>4d}/{p['pushes']:<4d}" for p in r.get('push', []))
        dtc = r.get('dt_check')
        tail = f"   | {dtc['mean_speed_mps']:.4f}, {dtc['upright']}/{dtc['n']}" if dtc else ''
        cc = r.get('contact_check')
        tail += f"   | {cc['mean_speed_mps']:.4f}, {cc['upright']}/{cc['n']}" if cc else ''
        print(f"{spec:34s} {s['mean_speed_mps']:10.4f} {s['upright']:>4d}/{s['n']:<4d} {verdict:>8s}  {cells}{tail}")
    print('* = not at the pre-registered size (20 seeds x 10 s); not a gate verdict' if
          not all(r['stable']['full_size'] for r in report['policies'].values()) else '')
    print(f"wall {report['wall_s']:.1f} s")
    if a.json:
        os.makedirs(os.path.dirname(os.path.abspath(a.json)), exist_ok=True)
        with open(a.json, 'w') as f:
            json.dump(report, f, indent=1)


if __name__ == '__main__':
    main()
