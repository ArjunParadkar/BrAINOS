"""stances.py -- candidate standing poses for the balance reflex: is a wider
or lower stance free? Each is held by plain PD (no policy) against the gate's
push range; reports survival, holding torque, and chest height."""
import os, sys, math
HERE = os.path.dirname(os.path.abspath(__file__)); sys.path.insert(0, os.path.dirname(HERE))
import numpy as np, mujoco
import env as E, tasks as T
from ref.keyframes import pose

QS = E.QSTAND


def lower(q, f):
    """fold every leg's knee by fraction f of its range toward shortening, pitch compensating half"""
    q = q.copy()
    for leg in ('FR', 'FL', 'RR', 'RL'):
        k = E.NAMES.index(leg + '_knee'); p = E.NAMES.index(leg + '_pitch')
        folded = E.QMAX[k] if QS[k] > 0 else E.QMIN[k]
        dk = f * (folded - QS[k]); q[k] += dk; q[p] -= 0.5 * dk
    return np.clip(q, E.QMIN, E.QMAX)


STANCES = {
    'stand': QS,
    'wide': pose(QS, FR_abd=0.15, FL_abd=-0.15, RR_abd=0.30, RL_abd=-0.30),
    'low': lower(QS, 0.15),
    'wide+low': lower(pose(QS, FR_abd=0.15, FL_abd=-0.15, RR_abd=0.30, RL_abd=-0.30), 0.15),
}


def trial(q, f, ang, seed=0):
    e = E.BlurEnv(T.GetUp(), seed=seed, randomize=False); m, d = e.m, e.d
    e.set_gains(*E.stand_kp()); e.randomize_body()
    mujoco.mj_resetDataKeyframe(m, d, e.key); mujoco.mj_forward(m, d)
    e.push = None; e.track_attitude = False; e.t = 0.0
    e.apply_targets(q, 1500)
    hold = float(np.max(np.abs(d.actuator_force[e.act][E.POLICY_JOINTS]) / E.TAU[E.POLICY_JOINTS]))
    z0 = float(d.qpos[2])
    e.t = 0.0; e.push = (e.chest, np.array([f * math.cos(ang), f * math.sin(ang), 0.0]), 0.1, 0.2)
    e.apply_targets(q, 3000)
    return e.up() > 0.9 and d.qpos[2] > 0.18, hold, z0


if __name__ == '__main__':
    forces = [10, 20, 30, 40, 50, 60]
    for name, q in STANCES.items():
        res = {f: 0 for f in forces}; holds = []; z0 = 0
        for f in forces:
            for k in range(12):
                ok, h, z0 = trial(q, f, 2 * math.pi * k / 12)
                res[f] += ok; holds.append(h)
        print(f'{name:9s} chest z {z0:.3f}  peak hold torque {max(holds):.2f} x driver  survived per force (of 12): ' +
              ' '.join(f'{f}N:{res[f]}' for f in forces) + f'  total {sum(res.values())}/72', flush=True)
