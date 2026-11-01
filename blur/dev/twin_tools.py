"""Helpers for prototyping reflexes and skills directly on the twin's node
model (nerves.Twin), bypassing the CAN link. Prototype only: what ships
runs in the Rust Autonomic over the nerve link."""
import argparse, math, os, sys
import numpy as np, mujoco
HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(os.path.dirname(HERE), 'sim'))
import nerves  # noqa

def twin(seed=1, jig=False, scene='flat', faults=(), record=None):
    a = argparse.Namespace(sock='x', scene=scene, jig=jig, jig_height=0.3, fault=list(faults), seed=seed, unit=36,
                           latency=[0.1, 0.8], record=record)
    t = nerves.Twin(a)
    # calibrated by fiat (Gate 1 shows the real procedure gets within 0.027 rad)
    for n in t.nodes:
        n.offset = 0.0
        n.offset = n.q_phys(t.d) - n.q_node(t.d)
        n.committed = True
        n.cur_limit = n.peak
    return t

def stand_targets():
    return {a['name']: a['q_stand'] for a in nerves.DESC['actuators']}

def hold(t, targets, kp_scale=1.0, kd_ratio=0.04, ms=1, tau_ff=None):
    for _ in range(ms):
        for n in t.nodes:
            if n.name not in targets: continue
            deliv = min(n.tau_max, (n.kt or 1e9) * n.peak)
            kp = kp_scale * (6.0 if not n.tendon else 3000.0 / 160.0) * deliv
            ff = 0.0 if tau_ff is None else tau_ff.get(n.name, 0.0)
            n.sp = dict(slot=0, flags=1, q=targets[n.name], tau=ff, kp=kp, kd=kd_ratio * kp); n.sp_t = t.t_ms
        t.tick(False, [])

def tilt_deg(t):
    q = t.d.qpos[3:7]
    R = np.zeros(9); mujoco.mju_quat2Mat(R, q); R = R.reshape(3, 3)
    return math.degrees(math.acos(max(-1, min(1, R[2, 2]))))

def chest_z(t): return float(t.d.qpos[2])
