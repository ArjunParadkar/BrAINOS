"""
tasks.py -- what each skill is asked to do: where it starts, what pose it
acts around, what it is told, and how it is scored.

Every task's reward is written so the score means something physical
(upright fraction, height, pose error, torque) and the evaluation in
eval.py uses its own success test, never the reward.
"""
import math
import numpy as np
import mujoco
from env import NA, NU, NAMES, POLICY_JOINTS, QSTAND, QMIN, QMAX, TAU, stand_kp

IDX = {n: i for i, n in enumerate(NAMES)}
STAND_Z = 0.268


def quat_from_rpy(r, p, y):
    cr, sr, cp, sp, cy, sy = math.cos(r / 2), math.sin(r / 2), math.cos(p / 2), math.sin(p / 2), math.cos(y / 2), math.sin(y / 2)
    return np.array([cr * cp * cy + sr * sp * sy, sr * cp * cy - cr * sp * sy,
                     cr * sp * cy + sr * cp * sy, cr * cp * sy - sr * sp * cy])


class GetUp:
    """Reflex training target: from any fallen pose, or a push while
    standing, end standing at the stand pose. One policy covers 'stays up',
    'falls', 'self-rights' and 'stands back up'."""
    name = 'getup'
    command_dim = 0
    horizon = 300                       # 6 s at 50 Hz
    action_mode = 'range'               # recovery needs the joints' whole range

    def __init__(self, p_push=0.4, push_n=(10.0, 60.0), severity=1.0):
        self.p_push = p_push
        self.push_n = push_n
        self.severity = severity        # 0..1: how far from upright a fallen start may be (curriculum)

    def gains(self, env):
        return stand_kp()

    def nominal(self, env):
        return QSTAND

    def command(self, env):
        return np.zeros(0)

    def reset(self, env):
        r = env.rng
        d, m = env.d, env.m
        self.kind = 'push' if r.random() < self.p_push else 'fallen'
        if self.kind == 'push':
            mujoco.mj_forward(m, d)
            a = r.uniform(0, 2 * math.pi)
            f = r.uniform(*self.push_n)
            t0 = r.uniform(0.2, 1.0)
            env.push = (env.chest, np.array([f * math.cos(a), f * math.sin(a), 0.0]), t0, t0 + 0.1)
            env.t = 0.0
            return
        # fallen: a random attitude, a random joint configuration, dropped and settled
        sev = self.severity
        roll = r.uniform(-math.pi, math.pi) * sev
        pitch = (r.uniform(-0.6 * math.pi, 0.6 * math.pi) if r.random() < 0.3 else r.uniform(-0.3, 0.3)) * sev
        d.qpos[3:7] = quat_from_rpy(roll, pitch, r.uniform(-math.pi, math.pi))
        d.qpos[2] = 0.35
        pose = QSTAND + r.uniform(-0.6, 0.6, NU) * (np.arange(NU) < 99)
        pose = np.clip(pose, QMIN, QMAX)
        for i in range(NU):
            if env.ten[i] < 0:
                d.qpos[env.qadr[i]] = pose[i]
        mujoco.mj_forward(m, d)
        env.apply_targets(pose, 800)                 # fall and come to rest
        env.t = 0.0

    def reward(self, env, action):
        d = env.d
        up = env.up()
        z = float(d.qpos[2])
        q = env.q()
        err = q[POLICY_JOINTS] - QSTAND[POLICY_JOINTS]
        pose = math.exp(-float(err @ err) / 1.0)
        dq = env.dq()[POLICY_JOINTS]
        tau = d.actuator_force[env.act][POLICY_JOINTS] / TAU[POLICY_JOINTS]
        h = min(1.0, max(0.0, (z - 0.06) / (STAND_Z - 0.06)))
        upright = max(0.0, up)
        r = (1.0 * (up + 1) / 2
             + 1.5 * h * upright
             + 1.0 * pose * upright * h
             - 0.0002 * float(dq @ dq)
             - 0.05 * float(tau @ tau) / len(tau)
             - 0.01 * float(np.sum((action - env.prev_action) ** 2)))
        done = env.t >= self.horizon * 0.02 - 1e-9
        return r, done, dict(up=up, z=z, pose=pose)


def success_standing(env):
    """the eval's test, not the reward: upright, at height, near the stand pose, still"""
    up = env.up()
    z = float(env.d.qpos[2])
    err = (env.q() - QSTAND)[POLICY_JOINTS]
    return up > 0.94 and z > 0.22 and float(np.max(np.abs(err))) < 0.35 and float(np.abs(env.dq()[POLICY_JOINTS]).max()) < 1.5


TASKS = {'getup': GetUp}
