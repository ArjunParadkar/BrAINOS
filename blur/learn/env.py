"""
env.py -- BLUR as a reinforcement-learning environment, on CPU MuJoCo.

The physics is the v36 MJCF (all of its contacts, springs and tendons) at a
1 ms timestep; the controller is the node firmware's impedance law, run by
MuJoCo's native actuators (kp, kd, force clamp = what the driver delivers),
with the speed derate applied to the force clamp every 5 ms. A policy acts
at 50 Hz: its action is a joint-target offset around the skill's nominal
pose, held through a randomised command latency. It observes what the
robot can sense -- encoders (noisy), the BMI270 (bias, noise, mounting
tilt), its own last action -- never the foot touch sensors or the true
body pose.

Domain randomisation (the brief's list): link masses +-15 %, chest CoM
+-1 cm, ground friction 0.4-1.2, motor strength +-20 %, spring rates
+-15 %, command latency 2-15 ms, encoder noise, IMU bias and noise, pushes.

Validated afterwards, not here: the 0.5 ms model behind the real nerve
formats (blur/sim/nerves.py).
"""
import json, math, os, sys
import numpy as np
import mujoco

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, ROOT)
import blurcfg  # noqa: E402  (per-machine MJCF location, sha-checked)
DESC = blurcfg.load_descriptor()
A = DESC['actuators']
NAMES = [a['name'] for a in A]
NU = len(A)


def _deliverable(a):
    kt = a['kt_out']
    return a['tau_max'] if kt != kt else min(a['tau_max'], kt * a['drv_peak_a'])


TAU = np.array([_deliverable(a) for a in A])
W0 = np.array([a['w0'] for a in A])
QMIN = np.array([a['q_min'] for a in A]); QMAX = np.array([a['q_max'] for a in A])
QSTAND = np.array([a['q_stand'] for a in A])
TENDON = np.array([a['tendon'] for a in A])

# the joints a locomotion/recovery policy drives; the neck, jaw, tail and
# gripper hold a posture (the head/tail-level skills are separate primitives)
POLICY_JOINTS = [i for i, n in enumerate(NAMES)
                 if n[:2] in ('FR', 'FL', 'RR', 'RL') or n.startswith('spine')]
NA = len(POLICY_JOINTS)                                  # 19

import attitude

# Domain-randomisation profile, chosen per process by BLUR_DR. 'v1' (the
# default, and what every laptop run so far used) is the list in the module
# docstring. 'wide' is Track 4e.1: link masses +-20 %, ground friction
# 0.3-1.3, motor strength +-25 %, command latency 2-19 ms (a 20 ms latency
# would be a whole policy step; the step caps it at 19), encoder noise up to
# 0.006 rad, IMU bias x2, soft ground (contact time constant 0.02-0.06 s
# instead of MuJoCo's 0.02) and, on half the episodes, uneven ground: a
# height field of smooth bumps up to 1.5 cm replaces the flat floor.
DR = os.environ.get('BLUR_DR', 'v1')
DR_RANGES = {
    'v1': dict(mass=0.15, mu=(0.4, 1.2), strength=0.20, lat=(2, 15), enc=(0.0005, 0.004), gyro=0.01,
               soft=None, uneven=0.0),
    'wide': dict(mass=0.20, mu=(0.3, 1.3), strength=0.25, lat=(2, 19), enc=(0.0005, 0.006), gyro=0.02,
                 soft=(0.02, 0.06), uneven=0.5, bump=0.015),
}
TERRAIN_N = 121                  # 6 m x 6 m height field, 5 cm cells


def build_scene(scene='flat', terrain=False):
    """the scene MJCF, plus (terrain=True) a height-field floor `terrain`
    overlaid on the plane, off until an episode switches it on. The files on
    disk are untouched (MjSpec, in memory)."""
    path = os.path.join(DESC['mjcf_dir'], f'scene_{scene}.xml')
    if not terrain:
        return mujoco.MjModel.from_xml_path(path)
    spec = mujoco.MjSpec.from_file(path)
    floor = next(g for g in spec.geoms if g.name == 'floor')
    hf = spec.add_hfield(name='terrain')
    hf.size = [3.0, 3.0, DR_RANGES['wide']['bump'], 0.05]
    hf.nrow = TERRAIN_N; hf.ncol = TERRAIN_N
    hf.userdata = [0.0] * (TERRAIN_N * TERRAIN_N)
    spec.worldbody.add_geom(name='terrain', type=mujoco.mjtGeom.mjGEOM_HFIELD, hfieldname='terrain',
                            contype=0, conaffinity=1, friction=list(floor.friction))   # on at compile (MuJoCo skips geoms compiled off), toggled per episode
    return spec.compile()


KP_SCALE = 6.0            # kp = KP_SCALE x deliverable torque per rad
KD_RATIO = 0.04
ACTION_SCALE = 0.6        # rad per unit action
CONTROL_DT = 0.02


def targets_from_action(nominal, action, mode):
    """'offset': nominal + ACTION_SCALE * a (fine skills around a pose);
    'range': a in [-1, 1] spans the joint's whole range piecewise through the
    nominal (-1 -> q_min, 0 -> nominal, +1 -> q_max) -- what recovery needs.
    os/blur/src/supervisor.rs `targets_from_action` is the same function."""
    t = np.array(nominal, float).copy()
    pj = np.asarray(POLICY_JOINTS)
    a = np.clip(np.asarray(action, float), -1.0, 1.0)
    if mode == 'range':
        lo, hi, n = QMIN[pj], QMAX[pj], t[pj]
        t[pj] = np.where(a >= 0, n + a * (hi - n), n + a * (n - lo))
    else:
        t[pj] = t[pj] + ACTION_SCALE * a
    return np.clip(t, QMIN, QMAX)


def stand_kp():
    kp = KP_SCALE * TAU.copy()
    kp[TENDON] = 100.0
    kd = KD_RATIO * kp
    kd[TENDON] = 5.0
    return kp, kd


class BlurEnv:
    """One environment. `task` supplies reset states, the nominal pose,
    commands and the reward."""

    def __init__(self, task, seed=0, scene='flat', timestep=0.001, randomize=True, model=None):
        self.task = task
        self.rng = np.random.default_rng(seed)
        self.randomize = randomize
        # `model`: a model built in memory from the same MJCF (e.g. with a test
        # rig added by MjSpec); the files on disk are never edited
        self.dr = DR_RANGES[DR]
        self.m = model if model is not None else build_scene(scene, terrain=self.dr['uneven'] > 0)
        m = self.m
        self.floor = mujoco.mj_name2id(m, mujoco.mjtObj.mjOBJ_GEOM, 'floor')
        self.terrain = mujoco.mj_name2id(m, mujoco.mjtObj.mjOBJ_GEOM, 'terrain')       # -1 without terrain
        self.uneven = False
        m.opt.timestep = timestep
        self.nsub = int(round(0.001 / timestep))
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
        self.imu = mujoco.mj_name2id(m, mujoco.mjtObj.mjOBJ_SITE, 'imu')
        self.sgyr = m.sensor_adr[mujoco.mj_name2id(m, mujoco.mjtObj.mjOBJ_SENSOR, 'imu_gyro')]
        self.feet = [mujoco.mj_name2id(m, mujoco.mjtObj.mjOBJ_SITE, f'{l}_foot') for l in ('FR', 'FL', 'RR', 'RL')]
        # nominal values, kept so randomisation is always relative to them
        self.nom = dict(mass=m.body_mass.copy(), ipos=m.body_ipos.copy(), friction=m.geom_friction.copy(),
                        ten_k=m.tendon_stiffness.copy(), jnt_k=m.jnt_stiffness.copy(), solref=m.geom_solref.copy())
        self.kp0, self.kd0 = stand_kp()
        self.obs_dim = None
        self.prev_action = np.zeros(NA)
        self.t = 0.0
        self.sacc = m.sensor_adr[mujoco.mj_name2id(m, mujoco.mjtObj.mjOBJ_SENSOR, 'imu_acc')]
        self.att = None

    # ------------------------------------------------------------ sensing
    def q(self):
        out = self.d.qpos[self.qadr].copy()
        t = self.ten >= 0
        out[t] = self.d.ten_length[self.ten[t]]
        return out

    def dq(self):
        out = self.d.qvel[self.dadr].copy()
        t = self.ten >= 0
        out[t] = self.d.ten_velocity[self.ten[t]]
        return out

    def chest_R(self):
        return self.d.xmat[self.chest].reshape(3, 3)

    def up(self):
        return float(self.d.xmat[self.chest][8])

    def gravity_imu(self):
        """unit gravity direction in the (mis-mounted) IMU frame: what the
        Autonomic's attitude filter hands the policy"""
        g = self.chest_R().T @ np.array([0.0, 0.0, -1.0])
        return self.imu_R @ g

    def gyro_imu(self):
        return self.imu_R @ self.d.sensordata[self.sgyr:self.sgyr + 3] + self.gyro_bias

    def imu_sample(self):
        """what the BMI270 reads: specific force and rate, mounting tilt, bias, noise"""
        acc = self.imu_R @ self.d.sensordata[self.sacc:self.sacc + 3] + self.rng.normal(0, 0.05, 3)
        gyr = self.imu_R @ self.d.sensordata[self.sgyr:self.sgyr + 3] + self.gyro_bias + self.rng.normal(0, 0.005, 3)
        return acc, gyr

    def att_update(self):
        acc, gyr = self.imu_sample()
        if self.att is None:
            # calibration knows the mount to within a fraction of a degree
            rr, pp = self.mount_true
            self.att = attitude.Mahony(rr + self.rng.normal(0, 0.003), pp + self.rng.normal(0, 0.003),
                                       self.gyro_bias + self.rng.normal(0, 0.002, 3))
            self.att.align(acc)
        else:
            self.att.update(acc, gyr, 0.001)

    def observe(self, command):
        q = self.q() + self.rng.normal(0, self.enc_sd, NU)
        dq = self.dq() + self.rng.normal(0, 0.05, NU)
        if self.att is None:
            self.att_update()
        g = np.array(self.att.gravity_body())
        w = np.array(self.att.last_w)
        nominal = self.task.nominal(self)
        o = np.concatenate([g, 0.25 * w,
                            (q - nominal)[POLICY_JOINTS], 0.05 * dq[POLICY_JOINTS],
                            self.prev_action, np.asarray(command, float)])
        return o.astype(np.float32)

    # ------------------------------------------------------------ randomisation
    def set_ground(self, uneven, timeconst=None, heights=None):
        """flat plane or the bump field; timeconst softens the ground contact"""
        m = self.m
        m.geom_solref[:] = self.nom['solref']
        if timeconst is not None:
            for g in (self.floor, self.terrain):
                if g >= 0:
                    m.geom_solref[g] = [timeconst, 1.0]
        self.uneven = bool(uneven and self.terrain >= 0)
        if self.terrain >= 0:
            m.geom_conaffinity[self.floor] = 0 if self.uneven else 1
            m.geom_conaffinity[self.terrain] = 1 if self.uneven else 0
            if self.uneven:
                m.hfield_data[:] = heights.ravel()

    def bumps(self):
        """smooth random bumps: a 13 x 13 grid of uniform heights, bilinear up to 121 x 121"""
        c = self.rng.uniform(0, 1, (13, 13))
        x = np.linspace(0, 12, TERRAIN_N)
        i = np.minimum(x.astype(int), 11); f = x - i
        rows = c[i] * (1 - f)[:, None] + c[i + 1] * f[:, None]
        return rows[:, i] * (1 - f)[None, :] + rows[:, i + 1] * f[None, :]

    def randomize_body(self):
        m, n, r = self.m, self.nom, self.rng
        if self.randomize and DR != 'v1':
            return self.randomize_wide()
        if self.terrain >= 0:
            self.set_ground(False)
        if not self.randomize:
            m.body_mass[:] = n['mass']; m.body_ipos[:] = n['ipos']; m.geom_friction[:] = n['friction']
            m.tendon_stiffness[:] = n['ten_k']; m.jnt_stiffness[:] = n['jnt_k']
            self.strength = np.ones(NU); self.latency_ms = 5; self.enc_sd = 0.002
            self.imu_R = np.eye(3); self.gyro_bias = np.zeros(3); self.mount_true = (0.0, 0.0)
            return
        m.body_mass[:] = n['mass'] * r.uniform(0.85, 1.15, len(n['mass']))
        m.body_ipos[:] = n['ipos']
        m.body_ipos[self.chest] += r.uniform(-0.01, 0.01, 3)
        mu = r.uniform(0.4, 1.2)
        m.geom_friction[:] = n['friction']; m.geom_friction[:, 0] = np.where(n['friction'][:, 0] > 0, mu, 0)
        m.tendon_stiffness[:] = n['ten_k'] * r.uniform(0.85, 1.15, len(n['ten_k']))
        m.jnt_stiffness[:] = n['jnt_k'] * r.uniform(0.85, 1.15, len(n['jnt_k']))
        self.strength = r.uniform(0.8, 1.2, NU)
        self.latency_ms = int(r.integers(2, 16))
        self.enc_sd = r.uniform(0.0005, 0.004)
        rr, pp = r.normal(0, math.radians(1.0), 2)
        cr, sr, cp, sp = math.cos(rr), math.sin(rr), math.cos(pp), math.sin(pp)
        self.imu_R = np.array([[cp, 0, sp], [0, 1, 0], [-sp, 0, cp]]) @ np.array([[1, 0, 0], [0, cr, -sr], [0, sr, cr]])
        self.mount_true = (rr, pp)
        self.gyro_bias = r.normal(0, 0.01, 3)

    def randomize_wide(self, edges=False):
        """the 'wide' profile (Track 4e.1). edges=True pins every quantity to
        the end of its range (the stress evaluation)"""
        m, n, r, R = self.m, self.nom, self.rng, self.dr
        U = (lambda lo, hi, size=None: r.choice([lo, hi], size)) if edges else r.uniform
        k = R['mass']
        m.body_mass[:] = n['mass'] * U(1 - k, 1 + k, len(n['mass']))
        m.body_ipos[:] = n['ipos']
        m.body_ipos[self.chest] += U(-0.01, 0.01, 3)
        mu = U(*R['mu'])
        m.geom_friction[:] = n['friction']; m.geom_friction[:, 0] = np.where(n['friction'][:, 0] > 0, mu, 0)
        m.tendon_stiffness[:] = n['ten_k'] * U(0.85, 1.15, len(n['ten_k']))
        m.jnt_stiffness[:] = n['jnt_k'] * U(0.85, 1.15, len(n['jnt_k']))
        k = R['strength']
        self.strength = U(1 - k, 1 + k, NU)
        lo, hi = R['lat']
        self.latency_ms = int(r.choice([lo, hi])) if edges else int(r.integers(lo, hi + 1))
        self.enc_sd = R['enc'][1] if edges else r.uniform(*R['enc'])
        rr, pp = (math.radians(1.0) * r.choice([-1, 1], 2)) if edges else r.normal(0, math.radians(1.0), 2)
        cr, sr, cp, sp = math.cos(rr), math.sin(rr), math.cos(pp), math.sin(pp)
        self.imu_R = np.array([[cp, 0, sp], [0, 1, 0], [-sp, 0, cp]]) @ np.array([[1, 0, 0], [0, cr, -sr], [0, sr, cr]])
        self.mount_true = (rr, pp)
        self.gyro_bias = r.choice([-1, 1], 3) * R['gyro'] if edges else r.normal(0, R['gyro'], 3)
        tc = R['soft'][1] if edges else r.uniform(*R['soft'])
        uneven = edges or r.random() < R['uneven']
        self.set_ground(uneven, tc, self.bumps() if uneven else None)

    def set_gains(self, kp, kd):
        m = self.m
        for i in range(NU):
            a = self.act[i]
            m.actuator_gainprm[a, :] = 0; m.actuator_gainprm[a, 0] = kp[i]
            m.actuator_biasprm[a, :] = 0; m.actuator_biasprm[a, 1] = -kp[i]; m.actuator_biasprm[a, 2] = -kd[i]
            m.actuator_forcelimited[a] = 1
        self.kp, self.kd = kp, kd

    def derate(self):
        """the torque/speed line, as a force clamp refreshed every 5 ms"""
        w = np.abs(self.dq())
        avail = np.where(w <= 0.5 * W0, TAU, np.where(w >= W0, 0.0, TAU * (W0 - w) / (0.5 * W0)))
        avail = avail * self.strength
        self.m.actuator_forcerange[self.act, 0] = -avail
        self.m.actuator_forcerange[self.act, 1] = avail

    # ------------------------------------------------------------ episode
    def reset(self):
        self.randomize_body()
        mujoco.mj_resetDataKeyframe(self.m, self.d, self.key)
        kp, kd = self.task.gains(self)
        self.set_gains(kp * (self.strength if self.randomize else 1.0), kd)
        self.prev_action = np.zeros(NA)
        self.t = 0.0
        self.push = None
        self.att = None
        self.track_attitude = False           # not while the reset settles the body
        self.task.reset(self)
        mujoco.mj_forward(self.m, self.d)
        self.track_attitude = True
        self.att = None                       # the filter starts when the policy does, aligned to what it reads
        self.d.ctrl[self.act] = self.task.nominal(self)
        return self.observe(self.task.command(self))

    def apply_targets(self, targets, ms):
        for k in range(ms):
            if k % 5 == 0:
                self.derate()
            if self.push is not None:
                body, f, t0, t1 = self.push
                self.d.xfrc_applied[body, :3] = f if t0 <= self.t < t1 else 0.0
            self.d.ctrl[self.act] = targets
            mujoco.mj_step(self.m, self.d, nstep=self.nsub)
            self.t += 0.001
            if self.track_attitude:
                self.att_update()

    def step(self, action):
        action = np.clip(np.asarray(action, float), -1.0, 1.0)
        nominal = self.task.nominal(self)
        old = self.d.ctrl[self.act].copy()
        new = targets_from_action(nominal, action, getattr(self.task, 'action_mode', 'offset'))
        # the command filter (supervisor.rs runs the same): targets move a
        # fraction alpha of the way to the policy's each 20 ms step
        alpha = getattr(self.task, 'action_filter', 1.0)
        if alpha < 1.0:
            new = old + alpha * (new - old)
        lat = min(self.latency_ms, 19)                # 20 ms = the whole step
        self.apply_targets(old, lat)                 # the command is still in flight
        self.apply_targets(new, 20 - lat)
        r, done, info = self.task.reward(self, action)
        self.prev_action = action
        if not np.all(np.isfinite(self.d.qpos)):
            return self.observe(self.task.command(self)) * 0, -10.0, True, dict(nan=True)
        return self.observe(self.task.command(self)), r, done, info
