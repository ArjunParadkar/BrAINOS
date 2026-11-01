#!/usr/bin/env python3
"""
keyframes.py -- hand-designed keyframe references for the two get-up skills.

A reference is a list of (seconds, pose) keys. Between keys the joint
targets follow a minimum-jerk blend; the keys themselves are chosen so the
body passes through statically sensible postures (the rig's `lay`, `sit`
and `stand`, and tucks designed here), never a straight lay->stand joint
interpolation, which flips the body (Gate 1 finding).

    righting : side/back -> sternal (belly down, legs tucked)
    standup  : sternal -> stand

`play(ref, start, ...)` runs a reference open loop on the RL environment's
physics (same actuators, gains, derate, latency) and reports where the body
ends up; the references are judged by that before any RL sees them.
"""
import os, sys, math
HERE = os.path.dirname(os.path.abspath(__file__)); sys.path.insert(0, os.path.dirname(HERE))
import numpy as np
import env as E

N = E.NAMES
IX = {n: i for i, n in enumerate(N)}
RIG = os.path.join(E.DESC['mjcf_dir'], 'moves.npz')


def _rig(move):
    z = np.load(RIG, allow_pickle=True)
    jn = list(z['joint_names'])
    q = E.QSTAND.copy()
    for i, a in enumerate(E.A):
        if a['target'] in jn:
            q[i] = z[move + '__q'][-1][jn.index(a['target'])]
    return q


def pose(base, **kv):
    """a pose = base with named joints replaced; FX_/RX_/XX_ set both sides"""
    q = np.array(base, float).copy()
    for k, v in kv.items():
        if k.startswith('FX_') or k.startswith('RX_'):
            for s in 'RL':
                q[IX[k[0] + s + k[2:]]] = v
        elif k.startswith('XX_'):
            for l in ('FR', 'FL', 'RR', 'RL'):
                q[IX[l + k[2:]]] = v
        else:
            q[IX[k]] = v
    return np.clip(q, E.QMIN, E.QMAX)


STAND = E.QSTAND.copy()
SIT = _rig('sit')
LAY = _rig('lay')
# symmetric sternal tuck: the rig's lay legs, both sides alike, spine straight
STERNAL = pose(LAY, FX_abd=0.0, FX_pitch=0.72, FX_knee=-2.34, FX_dist=0.15,
               RX_abd=0.0, RX_pitch=-1.00, RX_knee=2.06, RX_dist=-2.97, spine_twist=0.0, spine_s1=0.0, spine_s2=0.0)


def min_jerk(s):
    s = min(max(s, 0.0), 1.0)
    return s * s * s * (10 - 15 * s + 6 * s * s)


class Reference:
    """keys: [(t_seconds, pose)], t increasing, first at 0"""

    def __init__(self, keys, name=''):
        self.keys = [(float(t), np.asarray(q, float)) for t, q in keys]
        self.name = name
        self.T = self.keys[-1][0]

    def __call__(self, t):
        ks = self.keys
        if t <= 0:
            return ks[0][1].copy()
        for (t0, q0), (t1, q1) in zip(ks, ks[1:]):
            if t <= t1:
                return q0 + (q1 - q0) * min_jerk((t - t0) / max(t1 - t0, 1e-9))
        return ks[-1][1].copy()

    def dq(self, t, h=0.01):
        return (self(t + h) - self(t - h)) / (2 * h)


def settle(env, q, rpy, ms=1500, z=0.30):
    """drop the body from z at attitude rpy holding pose q and let it come to rest"""
    import mujoco
    from tasks import quat_from_rpy
    d = env.d
    env.push = None
    env.track_attitude = False
    mujoco.mj_resetDataKeyframe(env.m, d, env.key)
    for i in range(E.NU):
        if env.ten[i] < 0:
            d.qpos[env.qadr[i]] = q[i]
    d.qpos[2] = z
    d.qpos[3:7] = quat_from_rpy(*rpy)
    d.qvel[:] = 0
    mujoco.mj_forward(env.m, d)
    env.apply_targets(q, ms)
    env.t = 0.0


def success_hold(env, hold):
    """the success predicate, one frame of it; the caller counts 2 s of consecutive frames"""
    up = env.up(); z = float(env.d.qpos[2])
    err = (env.q() - E.QSTAND)[E.POLICY_JOINTS]
    w = np.linalg.norm(env.d.qvel[3:6])
    return up > 0.94 and z > 0.22 and float(np.max(np.abs(err))) < 0.35 and float(np.abs(env.dq()[E.POLICY_JOINTS]).max()) < 1.5 and w < 0.5


def sternal_ok(env):
    """belly down, chest low and level-ish, legs near the tuck"""
    up = env.up()
    return up > 0.85 and float(env.d.qpos[2]) < 0.22


def play(ref, env, start_q, start_rpy, after=2.5, film=None):
    """run ref open loop from a settled start; returns a trace summary"""
    settle(env, start_q, start_rpy)
    tr = []
    held = 0; first_ok = None
    t = 0.0
    T = ref.T + after
    frames = []
    while t < T:
        env.apply_targets(ref(t), 20)
        t += 0.02
        ok = success_hold(env, None)
        held = held + 1 if ok else 0
        if held >= 100 and first_ok is None:
            first_ok = t
        tr.append((t, env.up(), float(env.d.qpos[2])))
        if film is not None:
            film(env)
    return dict(success=first_ok is not None, t_success=first_ok, up_min=min(x[1] for x in tr),
                final_up=tr[-1][1], final_z=tr[-1][2])
