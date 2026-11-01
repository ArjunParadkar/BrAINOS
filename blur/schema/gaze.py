#!/usr/bin/env python3
"""
gaze.py -- B4: head stabilisation / gaze lock. Hand-tuned PD vs the reflex
grown from the body schema, in the same seeded episodes.

Scenarios (the test scene is the babbling rig's: same landmarks, head camera):
  push  : standing in the ready stance (balance policy if trained, else PD),
          10-30 N x 100 ms pushes in random directions, 4 s per episode
  walk  : the rig's walk gait (moves.npz) replayed on a compliant body
          harness -- BLUR has no learned locomotion policy and the rig's
          open-loop walk falls (rig report: fell, 1.2 m path error); the
          harness (a soft weld from the chest to a carriage moving at the
          gait's speed) reproduces gait-induced body pitch/roll/yaw without a
          balance controller. A stand-in, labelled as such.

Arms: 'none' (neck at the stand pose), 'pd' (hand-tuned on the Mahony
attitude), 'grown' (sees only the orientation module's latent and the neck
encoders; built by grown.py from the babbling log alone).

Metrics (ground truth, never given to a controller): head pitch RMS and
roll RMS in the world (roll is uncontrollable: the neck has no roll joint),
gaze drift RMS = angle between the head's forward axis and its direction at
t = 0.
"""
import os, sys, math, json, argparse
os.environ.setdefault('MUJOCO_GL', 'egl'); os.environ.setdefault('OMP_NUM_THREADS', '1')
HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(os.path.dirname(HERE), 'learn')); sys.path.insert(0, HERE)
import numpy as np
import mujoco
import env as E
from babble import build_rig, NoTask, GIMBAL_Z

NECK = [E.NAMES.index(n) for n in ('neck_lift', 'neck_pan', 'neck_tilt')]
EP_S = 4.0


def head_angles(d, head):
    R = d.xmat[head].reshape(3, 3)
    fwd = R[:, 0]
    pitch = math.asin(max(-1.0, min(1.0, fwd[2])))
    left = R[:, 1]
    roll = math.asin(max(-1.0, min(1.0, left[2])))
    return pitch, roll, fwd.copy()


class Scenario:
    def __init__(self, kind, seed, balance=None):
        self.kind, self.seed = kind, seed
        m = build_rig()
        self.m = m
        self.e = e = E.BlurEnv(NoTask(), seed=seed, randomize=True, model=m)
        e.randomize_body(); kp, kd = E.stand_kp(); e.set_gains(kp * e.strength, kd)
        d = e.d
        self.gid = m.body_mocapid[mujoco.mj_name2id(m, mujoco.mjtObj.mjOBJ_BODY, 'gimbal')]
        self.weld = mujoco.mj_name2id(m, mujoco.mjtObj.mjOBJ_EQUALITY, 'gimbal_weld')
        self.head = mujoco.mj_name2id(m, mujoco.mjtObj.mjOBJ_BODY, 'head')
        mujoco.mj_resetDataKeyframe(m, d, e.key)
        rng = np.random.default_rng(seed)
        self.rng = rng
        if kind == 'push':
            d.eq_active[self.weld] = 0                     # free standing: the gimbal weld is off
            import skills as S
            self.pose = S.READY.copy()
            self.balance = balance
            f = rng.uniform(*self.push_range(balance)); a = rng.uniform(0, 2 * math.pi); t0 = rng.uniform(0.5, 1.0)
            e.push = (e.chest, np.array([f * math.cos(a), f * math.sin(a), 0.0]), t0, t0 + 0.1)
        else:
            z = np.load(os.path.join(E.DESC['mjcf_dir'], 'moves.npz'), allow_pickle=True)
            jn = list(z['joint_names']); self.walk_q = z['walk__q']; self.walk_dt = float(z['walk__dt'])
            self.walk_idx = [jn.index(a['target']) if a['target'] in jn else -1 for a in E.A]
            base = z['walk__base']
            self.walk_v = (base[-1, :2] - base[0, :2]) / (len(base) * self.walk_dt)
            d.mocap_pos[self.gid] = [0, 0, 0.27]; d.mocap_quat[self.gid] = [1, 0, 0, 0]
            m.eq_solref[self.weld] = [0.15, 0.6]           # a compliant harness, not a jig
            d.qpos[2] = 0.27
            self.pose = E.QSTAND.copy()
            e.push = None
        mujoco.mj_forward(m, d)
        e.track_attitude = False; e.t = 0.0
        e.apply_targets(self.pose, 800)
        e.track_attitude = True; e.att = None; e.t = 0.0; e.prev_action = np.zeros(E.NA)

    @staticmethod
    def push_range(balance):
        # a PD-held stance survives only small pushes (ref/stances.py); with the
        # balance reflex the gate's lower range is used
        return (10.0, 30.0) if balance is not None else (5.0, 15.0)

    def body_targets(self):
        e = self.e
        if self.kind == 'walk':
            k = int(e.t / self.walk_dt) % len(self.walk_q)
            q = self.pose.copy()
            for i, j in enumerate(self.walk_idx):
                if j >= 0 and i not in NECK:
                    q[i] = self.walk_q[k][j]
            self.e.d.mocap_pos[self.gid][:2] = self.walk_v * e.t
            return q
        if self.balance is not None:
            import skills as S
            stub = type('S', (), {'nominal': lambda s, env: S.READY, 'command': lambda s, env: np.zeros(1)})()
            e.task = stub
            a = self.balance(e.observe(np.zeros(1)))
            e.prev_action = np.clip(a, -1, 1)
            return E.targets_from_action(S.READY, a, 'range')
        return self.pose.copy()


def run_episode(kind, seed, arm, ctrl=None, balance=None):
    sc = Scenario(kind, seed, balance)
    e, d = sc.e, sc.e.d
    if ctrl is not None:
        ctrl.reset(sc)
    p0, r0, f0 = head_angles(d, sc.head)
    P, Rl, G = [], [], []
    while e.t < EP_S:
        q = sc.body_targets()
        if arm == 'none':
            q[NECK] = E.QSTAND[NECK]
        else:
            q[NECK] = ctrl.neck(sc)
        q = np.clip(q, E.QMIN, E.QMAX)
        old = d.ctrl[e.act].copy(); lat = min(e.latency_ms, 19)
        e.apply_targets(old, lat); e.apply_targets(q, 20 - lat)
        if ctrl is not None:
            ctrl.observe(sc)
        p, r, f = head_angles(d, sc.head)
        P.append(p - p0); Rl.append(r - r0); G.append(math.acos(max(-1.0, min(1.0, float(f @ f0)))))
    P, Rl, G = map(np.asarray, (P, Rl, G))
    return dict(pitch_rms=float(np.sqrt(np.mean(P ** 2))), roll_rms=float(np.sqrt(np.mean(Rl ** 2))),
                gaze_rms=float(np.sqrt(np.mean(G ** 2))), fell=bool(e.up() < 0.5))


class PD:
    """hand-tuned: head pitch error = body pitch from the Mahony attitude, sent
    to neck_tilt (and a share to neck_lift); gaze yaw = integrated gyro yaw
    rate (the IMU has no heading), sent to neck_pan; both leak to neutral"""
    def __init__(self, k_pitch, k_yaw, lift_share=0.3, leak_s=2.0):
        self.kp, self.ky, self.ls, self.leak = k_pitch, k_yaw, lift_share, leak_s

    def reset(self, sc):
        self.yaw = 0.0

    def observe(self, sc):
        w = np.array(sc.e.att.last_w) if sc.e.att is not None else np.zeros(3)
        g = np.array(sc.e.att.gravity_body()) if sc.e.att is not None else np.array([0, 0, -1.0])
        self.pitch = math.atan2(-g[0], -g[2])
        self.yaw += 0.02 * (w[2] - self.yaw / self.leak)

    def neck(self, sc):
        n = E.QSTAND[NECK].copy()
        if not hasattr(self, 'pitch'):
            return n
        n[2] += self.kp * (1 - self.ls) * self.pitch
        n[0] += self.kp * self.ls * self.pitch
        n[1] += self.ky * self.yaw
        return n


def tune_pd(kind, seeds):
    best = None
    for kp in (-1.5, -1.0, -0.5, 0.5, 1.0, 1.5):
        for ky in (-1.0, -0.5, 0.5, 1.0):
            r = [run_episode(kind, s, 'pd', PD(kp, ky)) for s in seeds]
            score = np.mean([x['pitch_rms'] + x['gaze_rms'] for x in r])
            if best is None or score < best[0]:
                best = (score, kp, ky)
    return best


def main():
    p = argparse.ArgumentParser()
    p.add_argument('--kind', default='push'); p.add_argument('--episodes', type=int, default=20)
    p.add_argument('--tune-episodes', type=int, default=6); p.add_argument('--grown', default=None)
    p.add_argument('--balance', default=None); p.add_argument('--out', default=None)
    a = p.parse_args()
    bal = None
    if a.balance:
        from eval import load_policy
        bal = load_policy(a.balance)[1]
    tune_seeds = list(range(9000, 9000 + a.tune_episodes)); test_seeds = list(range(100, 100 + a.episodes))
    score, kp, ky = tune_pd(a.kind, tune_seeds)
    print(f'PD tuned on {len(tune_seeds)} calibration episodes: k_pitch {kp}, k_yaw {ky}', flush=True)
    arms = {'none': None, 'pd': PD(kp, ky)}
    if a.grown:
        import grown
        arms['grown'] = grown.Grown(a.grown)
    res = {}
    for name, c in arms.items():
        r = [run_episode(a.kind, s, name, c, bal) for s in test_seeds]
        res[name] = {k: float(np.mean([x[k] for x in r])) for k in ('pitch_rms', 'roll_rms', 'gaze_rms')}
        res[name]['fell'] = int(sum(x['fell'] for x in r))
        res[name]['per_episode'] = r
        print(name, {k: round(v, 4) if isinstance(v, float) else v for k, v in res[name].items() if k != 'per_episode'}, flush=True)
    if a.out:
        json.dump(dict(kind=a.kind, pd_gains=[kp, ky], results=res), open(a.out, 'w'), indent=1)


if __name__ == '__main__':
    main()
