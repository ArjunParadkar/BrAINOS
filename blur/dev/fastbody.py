"""
fastbody.py -- the twin's physics without the nerve link, for search and training.

Same MJCF, same actuator limits as nerves.py's node model (torque cap =
min(MJCF forcerange, driver peak x kt), the w0/2-knee speed derate), the
same impedance law tau = kp(q* - q) - kd*dq + tau_ff, but vectorised in
numpy and run at the physics rate with no CAN, no quantisation, no
latency. Anything found here is re-validated through blur/sim/nerves.py
and the Rust Autonomic before it is believed.
"""
import json, math, os, sys
import numpy as np
import mujoco

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.dirname(HERE))
import blurcfg  # noqa: E402  (per-machine MJCF location, sha-checked)
DESC = blurcfg.load_descriptor()
A = DESC['actuators']
NAMES = [a['name'] for a in A]
IDX = {n: i for i, n in enumerate(NAMES)}
NU = len(A)


def deliverable(a):
    kt = a['kt_out']
    if kt != kt:
        return a['tau_max']
    return min(a['tau_max'], kt * a['drv_peak_a'])


TAU = np.array([deliverable(a) for a in A])
W0 = np.array([a['w0'] for a in A])
QMIN = np.array([a['q_min'] for a in A]); QMAX = np.array([a['q_max'] for a in A])
QSTAND = np.array([a['q_stand'] for a in A])
TENDON = np.array([a['tendon'] for a in A])
LEG = np.array([n[:2] in ('FR', 'FL', 'RR', 'RL') for n in NAMES])


class Body:
    def __init__(self, scene='flat', mjcf_dir=None):
        d = mjcf_dir or DESC['mjcf_dir']
        self.m = mujoco.MjModel.from_xml_path(os.path.join(d, f'scene_{scene}.xml'))
        m = self.m
        for i in range(m.nu):
            m.actuator_gainprm[i, :] = 0; m.actuator_gainprm[i, 0] = 1.0
            m.actuator_biasprm[i, :] = 0
        self.d = mujoco.MjData(m)
        self.act = np.array([mujoco.mj_name2id(m, mujoco.mjtObj.mjOBJ_ACTUATOR, n) for n in NAMES])
        self.qadr = np.zeros(NU, int); self.dadr = np.zeros(NU, int); self.ten = np.full(NU, -1)
        for i, a in enumerate(A):
            if a['tendon']:
                self.ten[i] = mujoco.mj_name2id(m, mujoco.mjtObj.mjOBJ_TENDON, a['target'])
            else:
                j = mujoco.mj_name2id(m, mujoco.mjtObj.mjOBJ_JOINT, a['target'])
                self.qadr[i], self.dadr[i] = m.jnt_qposadr[j], m.jnt_dofadr[j]
        self.key = mujoco.mj_name2id(m, mujoco.mjtObj.mjOBJ_KEY, 'stand')
        self.chest = mujoco.mj_name2id(m, mujoco.mjtObj.mjOBJ_BODY, 'chest')
        self.feet = [mujoco.mj_name2id(m, mujoco.mjtObj.mjOBJ_SITE, f'{l}_foot') for l in ('FR', 'FL', 'RR', 'RL')]
        self.substeps = max(1, round(0.001 / m.opt.timestep))
        self.reset()

    def reset(self, qpos=None):
        mujoco.mj_resetDataKeyframe(self.m, self.d, self.key)
        if qpos is not None:
            self.d.qpos[:] = qpos
        mujoco.mj_forward(self.m, self.d)
        self.t = 0.0

    def q(self):
        d = self.d
        out = d.qpos[self.qadr].copy()
        t = self.ten >= 0
        out[t] = d.ten_length[self.ten[t]]
        return out

    def dq(self):
        d = self.d
        out = d.qvel[self.dadr].copy()
        t = self.ten >= 0
        out[t] = d.ten_velocity[self.ten[t]]
        return out

    def step_ms(self, q_tgt, kp, kd, tau_ff=0.0, push=None):
        """1 ms of impedance control at the physics rate."""
        for _ in range(self.substeps):
            q, dq = self.q(), self.dq()
            tau = kp * (q_tgt - q) - kd * dq + tau_ff
            w = np.abs(dq)
            avail = np.where(w <= 0.5 * W0, TAU, np.where(w >= W0, 0.0, TAU * (W0 - w) / (0.5 * W0)))
            tau = np.clip(tau, -avail, avail)
            self.d.ctrl[self.act] = tau
            if push is not None:
                self.d.xfrc_applied[self.chest, :3] = push
            mujoco.mj_step(self.m, self.d)
        if push is not None:
            self.d.xfrc_applied[self.chest, :] = 0
        self.t += 0.001
        return tau

    def up(self):
        """z component of the chest's up axis (1 = upright, -1 = on its back)"""
        return float(self.d.xmat[self.chest][8])

    def tilt_deg(self):
        return math.degrees(math.acos(max(-1.0, min(1.0, self.up()))))

    def chest_z(self):
        return float(self.d.qpos[2])

    def roll_pitch(self):
        R = self.d.xmat[self.chest].reshape(3, 3)
        return math.atan2(R[2, 1], R[2, 2]), math.asin(-max(-1.0, min(1.0, R[2, 0])))


def gains(kind='firm'):
    """Per-actuator kp, kd for a named stiffness, scaled by what each motor can deliver."""
    scale = dict(firm=6.0, soft=2.5, limp=0.0, stiff=10.0)[kind]
    kp = scale * TAU.copy()
    kp[TENDON] = scale * 3000.0 / 160.0 * TAU[TENDON] / 1.0 * 0.0 + (100.0 if kind != 'limp' else 0.0)
    kd = 0.04 * kp
    kd[TENDON] = 5.0 if kind != 'limp' else 0.0
    return kp, kd


def quat_axis_angle(axis, ang):
    axis = np.asarray(axis, float) / np.linalg.norm(axis)
    return np.r_[math.cos(ang / 2), math.sin(ang / 2) * axis]


def quat_mul(a, b):
    w1, x1, y1, z1 = a; w2, x2, y2, z2 = b
    return np.array([w1 * w2 - x1 * x2 - y1 * y2 - z1 * z2, w1 * x2 + x1 * w2 + y1 * z2 - z1 * y2,
                     w1 * y2 - x1 * z2 + y1 * w2 + z1 * x2, w1 * z2 + x1 * y2 - y1 * x2 + z1 * w2])


def rig_pose(name):
    """a static rig pose (stand/sit/lay/a/b/c/d) as actuator targets"""
    npz = np.load(os.path.join(DESC['mjcf_dir'], 'moves.npz'))
    jn = list(npz['joint_names']); q = npz[f'{name}__q'][-1]
    b = Body.__new__(Body)
    m = mujoco.MjModel.from_xml_path(os.path.join(DESC['mjcf_dir'], 'scene_flat.xml'))
    d = mujoco.MjData(m)
    for k, n in enumerate(jn):
        j = mujoco.mj_name2id(m, mujoco.mjtObj.mjOBJ_JOINT, n)
        d.qpos[m.jnt_qposadr[j]] = q[k]
    mujoco.mj_kinematics(m, d); mujoco.mj_tendon(m, d) if hasattr(mujoco, 'mj_tendon') else mujoco.mj_forward(m, d)
    out = np.zeros(NU)
    for i, a in enumerate(A):
        if a['tendon']:
            out[i] = d.ten_length[mujoco.mj_name2id(m, mujoco.mjtObj.mjOBJ_TENDON, a['target'])]
        else:
            out[i] = d.qpos[m.jnt_qposadr[mujoco.mj_name2id(m, mujoco.mjtObj.mjOBJ_JOINT, a['target'])]]
    return out
