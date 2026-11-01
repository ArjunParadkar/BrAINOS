"""
dayone.py -- the day-one skills that skills.py does not have yet: sit, lie,
walk and small hops (Track 4e.1). Same scoring as skills.py, so the reward
can't be earned without the success:

  * TRUE reward only inside the skill's success predicate (HOLD_R per step)
    plus SUCCESS_BONUS once the predicate has held long enough;
  * everything else is potential-based shaping toward a reference (the rig's
    own poses and motions from moves.npz, or a reference designed here);
  * success is a physical test on the final state / the whole episode,
    judged on the deterministic policy by eval.py, never on the reward.

  sit   stand -> the rig's `sit` pose (chest pitched ~40 deg nose-up, hind legs
        folded), held still 2 s.
  lie   stand -> the rig's `lay` pose (belly down, chest <= 0.19 m), held 2 s.
  walk  walk forward at the commanded speed (0.08-0.20 m/s; the rig's walk is
        0.11 m/s) for 8 s: never fallen, mean forward speed over the last 4 s
        within [0.5, 1.5] x command, lateral drift < 0.3 m, heading change
        < 30 deg. Reference: the rig's walk cycle, looped.
  hop   small hops in place: >= 2 flights (all four paws off the ground for
        >= 30 ms each; level < 0.5 asks for 1), never fallen (chest up > 0.8),
        then standing still for 1 s by the end of the 6 s episode. Reference:
        crouch-extend cycles designed here (the rig's `hop` lifts the base
        34 cm, far beyond 'small'; the v36 jump analysis found the motors
        short for big jumps, so a hop may simply not be reachable -- that is
        a result, not a bug).
"""
import math
import numpy as np
import mujoco
import env as E
from env import NU, QSTAND, QMIN, QMAX
from skills import Skill, standing_frame, body_rates, PJ, HOLD_R, SUCCESS_BONUS
from ref.keyframes import Reference, SIT, LAY, STAND
from ref.stances import lower

CMD_T = 6.0


def _pitch(env):
    R = env.chest_R()
    return math.degrees(math.asin(max(-1.0, min(1.0, -R[2, 0]))))


def _still(env):
    return float(np.abs(env.dq()[PJ]).max()) < 1.5 and body_rates(env) < 0.5


def _stand_start(env, rng, level):
    """the stand keyframe, settled 0.3 s, with level-scaled joint noise and a random heading"""
    mujoco.mj_resetDataKeyframe(env.m, env.d, env.key)
    yaw = rng.uniform(-math.pi, math.pi)
    env.d.qpos[3:7] = [math.cos(yaw / 2), 0, 0, math.sin(yaw / 2)]
    mujoco.mj_forward(env.m, env.d)
    q = np.clip(QSTAND + rng.normal(0, 0.08 * level, NU), QMIN, QMAX)
    env.apply_targets(q, 300)


class _PoseSkill(Skill):
    """stand -> a target pose, held still for 2 s"""
    target = STAND
    horizon = 300                 # 6 s
    blend = 1.5

    def __init__(self, level=1.0):
        super().__init__(level)
        self.ref = Reference([(0, STAND), (0.2, STAND), (0.2 + self.blend, self.target)], self.name)

    def reference(self, env):
        return self.ref(env.t)

    def fallen(self, env):
        return env.up() < 0.5

    def start(self, env):
        _stand_start(env, env.rng, self.level)


class Sit(_PoseSkill):
    name = 'sit'
    target = SIT
    PITCH = -40.5                 # the rig's sit chest pitch (deg; negative = nose up)

    def body(self, env):
        return math.exp(-((_pitch(env) - self.PITCH) / 15.0) ** 2) * (1.0 if env.up() > 0.5 else 0.0)

    def frame_ok(self, env):
        err = np.abs((env.q() - SIT)[PJ])
        return float(err.max()) < 0.35 and abs(_pitch(env) - self.PITCH) < 12.0 and _still(env)


class Lie(_PoseSkill):
    name = 'lie'
    target = LAY

    def body(self, env):
        z = float(env.d.qpos[2])
        return max(0.0, env.up()) * min(1.0, max(0.0, (0.26 - z) / (0.26 - 0.15)))

    def frame_ok(self, env):
        err = np.abs((env.q() - LAY)[PJ])
        return float(err.max()) < 0.35 and env.up() > 0.94 and float(env.d.qpos[2]) < 0.19 and _still(env)


# ---------------------------------------------------------------- walk
_RIG = {}


def _rig_walk():
    if not _RIG:
        z = np.load(E.DESC['mjcf_dir'] + '/moves.npz', allow_pickle=True)
        jn = list(z['joint_names']); wq = z['walk__q']; dt = float(z['walk__dt'])
        idx = [jn.index(a['target']) if a['target'] in jn else -1 for a in E.A]
        Q = np.array([[f[j] if j >= 0 else QSTAND[i] for i, j in enumerate(idx)] for f in wq])
        _RIG.update(Q=np.clip(Q, QMIN, QMAX), dt=dt, T=len(wq) * dt)
    return _RIG


class Walk(Skill):
    name = 'walk'
    command_dim = 3               # speed (m/s, x5), sin/cos of the reference phase
    horizon = 400                 # 8 s
    terminate_on_success = False
    action_filter = 0.3
    init_std = 0.05
    BLEND = 0.5

    def __init__(self, level=1.0):
        super().__init__(level)
        self.rig = _rig_walk()

    def reference(self, env):
        R = self.rig
        q = R['Q'][int(env.t / R['dt']) % len(R['Q'])]
        b = min(1.0, env.t / self.BLEND)
        return QSTAND + b * (q - QSTAND)

    def command(self, env):
        ph = 2 * math.pi * (env.t % self.rig['T']) / self.rig['T']
        return np.array([5.0 * self.v_cmd, math.sin(ph), math.cos(ph)])

    def _fwd_speed(self, env):
        v = env.d.qvel[0:2]
        return float(v @ self.heading0), float(v @ np.array([-self.heading0[1], self.heading0[0]]))

    def body(self, env):
        vf, vl = self._fwd_speed(env)
        h = min(1.0, max(0.0, (float(env.d.qpos[2]) - 0.08) / (0.24 - 0.08)))
        return max(0.0, env.up()) * h * math.exp(-((vf - self.v_cmd) / 0.1) ** 2)

    def frame_ok(self, env):
        vf, _ = self._fwd_speed(env)
        self.v_ema = 0.95 * self.v_ema + 0.05 * vf        # ~0.4 s window at 50 Hz
        return (env.up() > 0.85 and float(env.d.qpos[2]) > 0.18 and env.t > 1.0
                and 0.5 * self.v_cmd <= self.v_ema <= 1.5 * self.v_cmd)

    def fallen(self, env):
        return env.up() < 0.6 or float(env.d.qpos[2]) < 0.12

    def start(self, env):
        r = env.rng
        _stand_start(env, r, self.level)
        lo, hi = 0.08, 0.08 + 0.12 * self.level
        self.v_cmd = float(r.uniform(lo, max(lo, hi)))
        R = env.chest_R()
        h = np.array([R[0, 0], R[1, 0]]); self.heading0 = h / (np.linalg.norm(h) + 1e-9)
        self.xy0 = env.d.qpos[0:2].copy()
        self.yaw0 = math.atan2(h[1], h[0])
        self.v_ema = 0.0
        self.track = []

    def reward(self, env, action):
        r, done, info = super().reward(env, action)
        self.track.append((env.t, env.d.qpos[0:2].copy(), float(env.up())))
        if done:
            self.succeeded = self.judge(env)
            info['success'] = self.succeeded
        return r, done, info

    def judge(self, env):
        if self.fell or env.t < self.horizon * 0.02 - 1e-6:
            return False
        tr = self.track
        t_end = tr[-1][0]
        a = next(p for t, p, _ in tr if t >= t_end - 4.0)
        b = tr[-1][1]
        d = b - a
        vf = float(d @ self.heading0) / 4.0
        lat = float((b - self.xy0) @ np.array([-self.heading0[1], self.heading0[0]]))
        R = env.chest_R(); yaw = math.atan2(R[1, 0], R[0, 0])
        dyaw = abs((yaw - self.yaw0 + math.pi) % (2 * math.pi) - math.pi)
        return (0.5 * self.v_cmd <= vf <= 1.5 * self.v_cmd and abs(lat) < 0.3 and math.degrees(dyaw) < 30
                and min(u for _, _, u in tr) > 0.6)


# ---------------------------------------------------------------- hop
CROUCH = lower(QSTAND, 0.35)


def _hop_ref(n=3, period=0.8, t0=0.5):
    keys = [(0.0, STAND), (t0, STAND)]
    t = t0
    for _ in range(n):
        keys += [(t + 0.35 * period, CROUCH), (t + 0.5 * period, STAND), (t + period, STAND)]
        t += period
    return Reference(keys, 'hop')


class Hop(Skill):
    name = 'hop'
    horizon = 300                 # 6 s
    terminate_on_success = False
    action_filter = 1.0           # a hop is 100 ms of push: no command filter
    init_std = 0.1
    FLIGHT_MS = 30

    def __init__(self, level=1.0):
        super().__init__(level)
        self.ref = _hop_ref()

    def reference(self, env):
        return self.ref(env.t)

    def _touch(self, env):
        s = env.m.sensor_adr
        ids = [mujoco.mj_name2id(env.m, mujoco.mjtObj.mjOBJ_SENSOR, f'{l}_foot_touch') for l in ('FR', 'FL', 'RR', 'RL')]
        return [float(env.d.sensordata[s[i]]) for i in ids]

    def body(self, env):
        return max(0.0, env.up())

    def frame_ok(self, env):
        # the TRUE reward of a hop is a counted flight (see reward); the hold
        # at the end is the standing predicate
        return self.flights >= self.need and standing_frame(env)

    def fallen(self, env):
        return env.up() < 0.8 or float(env.d.qpos[2]) < 0.12

    def start(self, env):
        _stand_start(env, env.rng, 0.0)
        self.need = 1 if self.level < 0.5 else 2
        self.flights = 0; self.air_ms = 0
        self.hold_need = 50       # 1 s standing at the end

    def reward(self, env, action):
        # flight detection at the policy rate from the paw touch sensors over
        # the last 20 ms (sampled at the step end; 30 ms = two steps airborne)
        airborne = max(self._touch(env)) < 1e-6
        self.air_ms = self.air_ms + 20 if airborne else 0
        bonus = 0.0
        if self.air_ms == 40 and env.up() > 0.85 and self.flights < 3:
            self.flights += 1
            bonus = 5.0
        r, done, info = super().reward(env, action)
        r += bonus
        if done:
            self.succeeded = (not self.fell) and self.flights >= self.need and self.hold >= self.hold_need
            info['success'] = self.succeeded
        return r, done, info


DAYONE = {'sit': Sit, 'lie': Lie, 'walk': Walk, 'hop': Hop}
