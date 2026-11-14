"""Gate E2 environment: the generated Mini-Blur hexapod, and nothing else.

The body is `motor/models/hexapod_servo.xml`, the MJCF that
`brainos motor self-model hexapod --servo` writes from the body's own
description. Every mass and the servo torque in it are ASSUMED numbers
(listed in the file's header comment); a gait learned here is a gait for
a body with those assumed numbers.

What the policy sees (36 floats, physical units, all things a real body
could sense; no world position, no heading, no clock):

    0:3    gravity direction in the body frame (unit vector; IMU)
    3:6    base angular velocity in the body frame, rad/s (gyro)
    6:16   positions of the 10 driven joints, rad (servo feedback)
    16:26  velocities of the 10 driven joints, rad/s
    26:36  the previous action, in [-1, 1]

`OBS_RANGE` is a fixed, physical full-scale per input. The network sees
`clip(obs / OBS_RANGE, -1, 1)`; the int8 runner's input scale is
`127 / OBS_RANGE`, so the float policy and the int8 policy clip the
same way.

What it does: 10 numbers in [-1, 1]. Joint target = stand pose +
action * ACTION_SCALE rad, clipped to the joint range (+-70 deg). The
position servos in the model track the target. Control is 50 Hz, the
physics 1 kHz (20 steps per action).

Servo rate. The self-model assumes each driven joint's servo moves at
most 600 deg/s (`bodyd::selfmodel::HEX_ASSUMED_SERVO_DPS`; the MJCF
header lists it as "rate of X assumed 600.00"), but the MJCF emitter
does not put any rate limit into the model: its position actuators will
chase a target as fast as their torque allows. Without a limit, a
smoke run found a 22 Hz chatter that hopped the body forward at
0.13-0.15 m/s using about 3,000 deg/s of joint speed. So this
environment enforces the stated rate itself: the setpoint handed to each
position servo slews toward the commanded target by at most rate * 1 ms
per physics step. The rate is read from the model file's header, so it
stays the self-model's number. The hand-tuned gait goes through the same
limiter (its fastest joint motion is about 150 deg/s, so the limit
does not bind for it).

Reward ("from the self-model alone"): it reads only base velocity,
tilt, joint limits and actuator effort. There is no reference gait, no
phase clock and no foot-contact term.

    + W_VEL   * clip(v_fwd, -TOP, TOP) / TOP      forward base velocity, capped at TOP_SPEED_MPS
    - W_LAT   * min((v_lat / TOP)^2, 1)            sideways base velocity (bounded)
    - W_YAW   * min(yaw_rate^2, 1)                 base turning rate, rad/s (bounded)
    - W_VZ    * min((v_z / TOP)^2, 1)              vertical base velocity (bounded)
    - W_RP    * min(|roll/pitch rate|^2, 1)        base rocking rate, rad/s (bounded)
    - W_TILT  * (tilt / TILT_LIMIT)^2              tilt
    + W_ALIVE                                      every step spent inside the tilt limit
    - W_LIMIT * sum(max(0, |q|/q_max - 0.9))       joints near their limits
    - W_EFFORT* mean((tau / tau_max)^2)            actuator effort

v_fwd / v_lat / v_z are the base's displacement over the control step,
resolved along / across its own heading and vertically. The v_z and
rocking terms were added after smoke runs found hopping, chattering
gaits (see the README); they are still base-velocity terms. The episode ends when the tilt
crosses TILT_LIMIT (20 deg).

Domain randomisation of the assumed numbers (training only; evaluation
uses the model as generated unless asked):

    every link mass (and its inertia)   x U(1 - DR_MASS, 1 + DR_MASS)    each link independently
    friction of every geom              x U(1 - DR_FRIC, 1 + DR_FRIC)    one factor per episode
    servo gain kp (and kv with it)      x U(1 - DR_KP,   1 + DR_KP)      one factor per servo
"""
import hashlib, math, os, re
import mujoco
import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
MODEL_PATH = os.path.normpath(os.path.join(HERE, '..', 'models', 'hexapod_servo.xml'))

# from the self-model (os/bodyd/src/hexapod.rs and the MJCF)
TOP_SPEED_MPS = 0.12
TILT_LIMIT_DEG = 20.0
LEGS = ['L1', 'R1', 'L2', 'R2', 'L3', 'R3']

CONTROL_HZ = 50
PHYS_PER_CTRL = 20          # 1 kHz physics; checked against the model's timestep
ACTION_SCALE = 0.6          # rad of target offset at |action| = 1 (the range is +-1.2217)
N_OBS, N_ACT = 36, 10

W_VEL, W_LAT, W_YAW, W_TILT = 1.0, 0.1, 0.1, 1.0
W_ALIVE, W_LIMIT, W_EFFORT = 0.2, 0.5, 0.02
W_VZ, W_RP = 0.2, 0.1

DR_MASS, DR_FRIC, DR_KP = 0.2, 0.3, 0.2


def model_sha256(path=MODEL_PATH):
    with open(path, 'rb') as f:
        return hashlib.sha256(f.read()).hexdigest()


def assumed_rates_dps(path=MODEL_PATH):
    """The servo rates the self-model writes into the MJCF header, deg/s, by joint."""
    with open(path) as f:
        head = f.read().split('-->', 1)[0]
    return {j: float(v) for j, v in re.findall(r'rate of (\S+) assumed ([0-9.]+)', head)}


def load_model(path=MODEL_PATH):
    m = mujoco.MjModel.from_xml_path(path)
    assert abs(m.opt.timestep * CONTROL_HZ * PHYS_PER_CTRL - 1.0) < 1e-9, 'timestep is not 1 ms'
    assert m.nu == N_ACT, m.nu
    return m


class Layout:
    """Indices into one model, by name, so nothing below hard-codes a number."""

    def __init__(self, m):
        A = mujoco.mjtObj.mjOBJ_ACTUATOR
        self.act_names = [mujoco.mj_id2name(m, A, i) for i in range(m.nu)]
        self.act_joint = [int(m.actuator_trnid[i, 0]) for i in range(m.nu)]
        self.qadr = np.array([m.jnt_qposadr[j] for j in self.act_joint])
        self.vadr = np.array([m.jnt_dofadr[j] for j in self.act_joint])
        self.ctrl_lo = m.actuator_ctrlrange[:, 0].copy()
        self.ctrl_hi = m.actuator_ctrlrange[:, 1].copy()
        self.q_max = np.array([m.jnt_range[j, 1] for j in self.act_joint])
        self.tau_max = m.actuator_forcerange[:, 1].copy()
        self.base = mujoco.mj_name2id(m, mujoco.mjtObj.mjOBJ_BODY, 'body')
        # follower joints (equality: q_follower = c1 * q_leader)
        self.follow = []
        for k in range(m.neq):
            if m.eq_type[k] == mujoco.mjtEq.mjEQ_JOINT:
                self.follow.append((m.jnt_qposadr[m.eq_obj1id[k]], m.jnt_qposadr[m.eq_obj2id[k]],
                                    float(m.eq_data[k, 1])))


def rate_per_ms(lay, path=MODEL_PATH):
    """Largest setpoint change per 1 ms physics step, rad, per actuator."""
    rates = assumed_rates_dps(path)
    missing = [n for n in lay.act_names if n not in rates]
    if missing:
        raise SystemExit(f'{path}: no servo rate for {missing}; regenerate the model')
    return np.radians([rates[n] for n in lay.act_names]) * 1e-3


def _obs_range(lay):
    r = np.empty(N_OBS, np.float32)
    r[0:3] = 1.0                 # unit gravity vector
    r[3:6] = 8.0                 # rad/s
    r[6:16] = lay.q_max          # joint range, rad
    r[16:26] = 12.0              # rad/s: the assumed servo rate is 600 deg/s = 10.5 rad/s
    r[26:36] = 1.0
    return r


OBS_RANGE = _obs_range(Layout(load_model()))


def tilt_deg(d, base):
    """Angle between the body's up axis and the world's up axis."""
    zz = float(np.clip(d.xmat[base][8], -1.0, 1.0))
    return math.degrees(math.acos(zz))


class HexEnv:
    """One hexapod. `reset(rng)` then `step(action)`.

    `dr=True` randomises the assumed numbers at every reset. `push`
    (eval only) is (physics step, steps, force xyz in N) applied to the
    base: impulse = force * steps * 1 ms.
    """

    def __init__(self, dr=False, max_steps=500, model_path=MODEL_PATH, init_noise=0.05, rate_limit=True,
                 phys_div=1, contact_tc=None):
        self.m = load_model(model_path)
        # phys_div > 1 runs the same 50 Hz control on a finer physics step
        # (1 ms / phys_div): a numerical check, not the model as generated
        self.k = int(phys_div)
        self.m.opt.timestep = self.m.opt.timestep / self.k
        # contact_tc sets every geom's contact time constant (MuJoCo solref[0],
        # 0.02 s by default, which the generated model keeps): a stiffness
        # check, not the model as generated
        if contact_tc is not None:
            self.m.geom_solref[:, 0] = contact_tc
        self.d = mujoco.MjData(self.m)
        self.lay = Layout(self.m)
        self.slew = (rate_per_ms(self.lay, model_path) / self.k if rate_limit
                     else np.full(N_ACT, np.inf))
        self.setpoint = np.zeros(N_ACT)
        self.dr = dr
        self.max_steps = max_steps
        self.init_noise = init_noise
        self.nom_mass = self.m.body_mass.copy()
        self.nom_inertia = self.m.body_inertia.copy()
        self.nom_fric = self.m.geom_friction.copy()
        self.nom_gain = self.m.actuator_gainprm.copy()
        self.nom_bias = self.m.actuator_biasprm.copy()
        self.total_mass = float(self.m.body_mass.sum())
        self.push = None
        self.prev_action = np.zeros(N_ACT)
        self.t = 0

    # ---------------------------------------------------------------- reset
    def randomise(self, rng):
        m = self.m
        m.body_mass[:] = self.nom_mass
        m.body_inertia[:] = self.nom_inertia
        m.geom_friction[:] = self.nom_fric
        m.actuator_gainprm[:] = self.nom_gain
        m.actuator_biasprm[:] = self.nom_bias
        if not self.dr:
            return
        s = rng.uniform(1 - DR_MASS, 1 + DR_MASS, size=m.nbody)
        s[0] = 1.0                                    # world
        m.body_mass[:] = self.nom_mass * s
        m.body_inertia[:] = self.nom_inertia * s[:, None]
        m.geom_friction[:, 0] = self.nom_fric[:, 0] * rng.uniform(1 - DR_FRIC, 1 + DR_FRIC)
        k = rng.uniform(1 - DR_KP, 1 + DR_KP, size=m.nu)
        m.actuator_gainprm[:, 0] = self.nom_gain[:, 0] * k
        m.actuator_biasprm[:, 1] = self.nom_bias[:, 1] * k     # -kp
        m.actuator_biasprm[:, 2] = self.nom_bias[:, 2] * k     # -kv
        self.total_mass = float(m.body_mass.sum())

    def reset(self, rng, yaw=None):
        m, d, lay = self.m, self.d, self.lay
        self.randomise(rng)
        mujoco.mj_resetData(m, d)
        q0 = rng.uniform(-self.init_noise, self.init_noise, size=N_ACT)
        d.qpos[lay.qadr] = q0
        for a, b, c in lay.follow:
            d.qpos[a] = c * d.qpos[b]
        yaw = rng.uniform(-math.pi, math.pi) if yaw is None else yaw
        d.qpos[3:7] = [math.cos(yaw / 2), 0.0, 0.0, math.sin(yaw / 2)]
        d.ctrl[:] = q0
        self.setpoint = q0.copy()
        mujoco.mj_setConst(m, d)
        # settle onto the feet before the policy acts (0.2 s, not counted)
        mujoco.mj_step(m, d, nstep=200 * self.k)
        self.prev_action = np.zeros(N_ACT)
        self.t = 0
        self.phys = 0
        self.push = None
        self.start_xy = d.xpos[lay.base][:2].copy()
        self.start_heading = self._heading()
        return self.obs()

    # ---------------------------------------------------------------- sense
    def _heading(self):
        R = self.d.xmat[self.lay.base]
        return math.atan2(R[3], R[0])          # yaw of the body's x axis

    def obs(self):
        d, lay = self.d, self.lay
        R = d.xmat[lay.base].reshape(3, 3)
        o = np.empty(N_OBS, np.float32)
        o[0:3] = -R[2, :]                       # R^T @ (0, 0, -1)
        o[3:6] = d.qvel[3:6]                    # free joint: body-frame angular velocity
        o[6:16] = d.qpos[lay.qadr]
        o[16:26] = d.qvel[lay.vadr]
        o[26:36] = self.prev_action
        return o

    def tilt(self):
        return tilt_deg(self.d, self.lay.base)

    def forward_progress(self):
        """Displacement along the heading the episode started with, m."""
        dxy = self.d.xpos[self.lay.base][:2] - self.start_xy
        h = self.start_heading
        return float(dxy[0] * math.cos(h) + dxy[1] * math.sin(h))

    # ---------------------------------------------------------------- act
    def targets(self, action):
        a = np.clip(action, -1.0, 1.0)
        return np.clip(a * ACTION_SCALE, self.lay.ctrl_lo, self.lay.ctrl_hi)

    def _advance(self, ctrl):
        """One control period. `ctrl` is a target array, or a function of
        the time in whole ms since reset returning one. Each physics step
        the servo setpoint moves toward the target by at most the servo
        rate. `push` is (start ms, length ms, force xyz N)."""
        m, d = self.m, self.d
        per_ms = callable(ctrl)
        at, n, f = self.push if self.push is not None else (-1, 0, 0.0)
        lo, hi, slew, sp = self.lay.ctrl_lo, self.lay.ctrl_hi, self.slew, self.setpoint
        target = None if per_ms else np.clip(ctrl, lo, hi)
        for _ in range(PHYS_PER_CTRL * self.k):
            ms = self.phys // self.k
            if per_ms:
                target = np.clip(ctrl(ms), lo, hi)
            sp += np.clip(target - sp, -slew, slew)
            d.ctrl[:] = sp
            if at >= 0:
                d.xfrc_applied[self.lay.base, :3] = f if at <= ms < at + n else 0.0
            mujoco.mj_step(m, d)
            self.phys += 1
        if at >= 0:
            d.xfrc_applied[self.lay.base, :3] = 0.0

    def step_targets(self, ctrl):
        """Advance one control period with raw joint targets (rad), or a
        function of the physics step giving them; used to replay the
        hand-tuned gait. Returns tilt in degrees."""
        self._advance(ctrl if callable(ctrl) else np.clip(ctrl, self.lay.ctrl_lo, self.lay.ctrl_hi))
        self.t += 1
        return self.tilt()

    def step(self, action):
        d, lay = self.d, self.lay
        action = np.clip(np.asarray(action, dtype=np.float64), -1.0, 1.0)
        xyz0 = d.xpos[lay.base].copy()
        h = self._heading()
        self._advance(self.targets(action))
        self.t += 1
        v3 = (d.xpos[lay.base] - xyz0) * CONTROL_HZ
        v = v3[:2]
        v_fwd = v[0] * math.cos(h) + v[1] * math.sin(h)
        v_lat = -v[0] * math.sin(h) + v[1] * math.cos(h)
        tilt = self.tilt()
        q = d.qpos[lay.qadr]
        tau = d.actuator_force / lay.tau_max
        terms = dict(
            vel=W_VEL * float(np.clip(v_fwd, -TOP_SPEED_MPS, TOP_SPEED_MPS)) / TOP_SPEED_MPS,
            lat=-W_LAT * min((v_lat / TOP_SPEED_MPS) ** 2, 1.0),
            yaw=-W_YAW * min(float(d.qvel[5]) ** 2, 1.0),
            vz=-W_VZ * min((float(v3[2]) / TOP_SPEED_MPS) ** 2, 1.0),
            rock=-W_RP * min(float(d.qvel[3] ** 2 + d.qvel[4] ** 2), 1.0),
            tilt=-W_TILT * (tilt / TILT_LIMIT_DEG) ** 2,
            alive=W_ALIVE,
            limit=-W_LIMIT * float(np.maximum(np.abs(q) / lay.q_max - 0.9, 0.0).sum()),
            effort=-W_EFFORT * float(np.mean(tau ** 2)),
        )
        terminated = tilt > TILT_LIMIT_DEG or not np.isfinite(d.qpos).all()
        if terminated:
            terms['alive'] = 0.0
        self.prev_action = action
        truncated = (not terminated) and self.t >= self.max_steps
        info = dict(v_fwd=v_fwd, tilt=tilt, terms=terms)
        return self.obs(), sum(terms.values()), terminated, truncated, info


# -------------------------------------------------------------------- vectors
class VecEnv:
    """N environments stepped one after another in this process."""

    def __init__(self, n, seed, dr=True, max_steps=500, contact_tc=None):
        self.envs = [HexEnv(dr=dr, max_steps=max_steps, contact_tc=contact_tc) for _ in range(n)]
        self.rngs = [np.random.default_rng([seed, i]) for i in range(n)]
        self.n = n

    def reset(self):
        return np.stack([e.reset(r) for e, r in zip(self.envs, self.rngs)])

    def step(self, actions):
        """Auto-resets finished envs. Returns obs (after reset), reward,
        terminated, truncated, and the final obs of truncated envs (for
        bootstrapping), plus finished-episode stats."""
        obs = np.empty((self.n, N_OBS), np.float32)
        rew = np.empty(self.n)
        term = np.zeros(self.n, bool)
        trunc = np.zeros(self.n, bool)
        final = np.zeros((self.n, N_OBS), np.float32)
        done_stats = []
        for i, (e, a) in enumerate(zip(self.envs, actions)):
            o, r, te, tr, info = e.step(a)
            rew[i], term[i], trunc[i] = r, te, tr
            if te or tr:
                final[i] = o
                done_stats.append((e.t, e.forward_progress(), te))
                o = e.reset(self.rngs[i])
            obs[i] = o
        return obs, rew, term, trunc, final, done_stats

    def cpu_seconds(self):
        return 0.0          # counted by the parent's own process time

    def close(self):
        pass


def _worker(conn, n, seed, dr, max_steps, offset, contact_tc):
    import time
    v = VecEnv(n, seed, dr, max_steps, contact_tc)
    v.rngs = [np.random.default_rng([seed, offset + i]) for i in range(n)]
    while True:
        cmd, arg = conn.recv()
        if cmd == 'reset':
            conn.send(v.reset())
        elif cmd == 'step':
            conn.send(v.step(arg))
        elif cmd == 'cpu':
            conn.send(time.process_time())
        else:
            conn.close()
            return


class ProcVecEnv:
    """The same N environments split across worker processes. Only used
    when train.py is given --workers > 1 (the cluster)."""

    def __init__(self, n, seed, dr=True, max_steps=500, workers=2, contact_tc=None):
        import multiprocessing as mp
        ctx = mp.get_context('spawn')
        sizes = [n // workers + (1 if k < n % workers else 0) for k in range(workers)]
        self.sizes = [s for s in sizes if s > 0]
        self.conns, self.procs, off = [], [], 0
        for s in self.sizes:
            a, b = ctx.Pipe()
            p = ctx.Process(target=_worker, args=(b, s, seed, dr, max_steps, off, contact_tc), daemon=True)
            p.start()
            self.conns.append(a)
            self.procs.append(p)
            off += s
        self.n = n

    def reset(self):
        for c in self.conns:
            c.send(('reset', None))
        return np.concatenate([c.recv() for c in self.conns])

    def step(self, actions):
        off = 0
        for c, s in zip(self.conns, self.sizes):
            c.send(('step', actions[off:off + s]))
            off += s
        parts = [c.recv() for c in self.conns]
        stats = [x for p in parts for x in p[5]]
        return tuple(np.concatenate([p[k] for p in parts]) for k in range(5)) + (stats,)

    def cpu_seconds(self):
        for c in self.conns:
            c.send(('cpu', None))
        return float(sum(c.recv() for c in self.conns))

    def close(self):
        for c in self.conns:
            c.send(('close', None))
        for p in self.procs:
            p.join(timeout=5)
