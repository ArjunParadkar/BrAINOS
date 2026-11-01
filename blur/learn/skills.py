"""
skills.py -- the Gate 2 reflexes as RL tasks: righting, stand-up, balance.

Every skill is scored the same way, so that its reward cannot be earned
without its success:

  * the TRUE reward is paid only inside the skill's success predicate
    (HOLD_R per step) plus SUCCESS_BONUS when the predicate has held for
    the required time (2 s for stand-up and balance, 0.5 s for the
    righting hand-off); costs (torque, action rate) are bounded negatives;
  * everything else is POTENTIAL-BASED shaping, F = gamma*Phi(s') - Phi(s),
    with Phi = imitation of the hand-designed keyframe reference (per-joint
    tolerance kernel, so it has a gradient far from the pose) plus a body
    term. Shaping of this form telescopes: over any episode it sums to
    gamma^T Phi(s_T) - Phi(s_0), bounded by max Phi, so no loitering policy
    can farm it, and it leaves the optimal policy unchanged (Ng, Harada &
    Russell 1999). getup_v2's failure -- 69% of the maximum reward from a
    crouch with joints on their stops -- is impossible here by construction.

Curriculum: every skill has `level` in [0, 1]. Level 0 starts near success
(reference-state initialisation late in the reference, small tilts); level
1 is the full start distribution. ppo.py raises the level only on the
DETERMINISTIC policy's success rate.

The policy's command is the elapsed skill time, min(t / 6 s, 1): the
supervisor can supply it without knowing the reference.
"""
import json, math, os
import numpy as np
import mujoco
import env as E
from env import NU, POLICY_JOINTS, QSTAND, QMIN, QMAX, TAU, stand_kp
from tasks import quat_from_rpy, GetUp
from ref.keyframes import Reference, STERNAL, STAND, settle
from ref.stances import STANCES

# the balance reflex's ready stance: wide + low survived 20/72 gate-range pushes
# under plain PD vs 13/72 for the stand pose, at the same holding torque
# (0.37 vs 0.32 x driver; ref/stances.py); chest at 0.243 m, above the gate's 0.2
READY = STANCES['wide+low']

HERE = os.path.dirname(os.path.abspath(__file__))
PJ = np.asarray(POLICY_JOINTS)
GAMMA = 0.99
HOLD_R = 0.5
SUCCESS_BONUS = 20.0
PHI_W = 2.0
COST_MAX = 0.2           # per-step costs are clipped to this, so they stay bounded
TOL = 0.5                # rad: the imitation kernel's per-joint tolerance
CMD_T = 6.0


def imitation(q, ref):
    e = (q[PJ] - ref[PJ]) / TOL
    return float(np.mean(np.exp(-e * e)))


def body_rates(env):
    return float(np.linalg.norm(env.d.qvel[3:6]))


def standing_frame(env, nominal=QSTAND):
    """one frame of the standing success predicate: upright, at height, near the
    stand pose, joints and body still"""
    up = env.up(); z = float(env.d.qpos[2])
    err = np.abs((env.q() - nominal)[PJ])
    return (up > 0.94 and z > 0.22 and float(err.max()) < 0.35
            and float(np.abs(env.dq()[PJ]).max()) < 1.5 and body_rates(env) < 0.5)


def sternal_frame(env):
    """one frame of the sternal predicate: belly down, low, legs near the tuck, still"""
    up = env.up(); z = float(env.d.qpos[2])
    err = np.abs((env.q() - STERNAL)[PJ])
    return (up > 0.90 and z < 0.21 and float(err.max()) < 0.5
            and float(np.abs(env.dq()[PJ]).max()) < 1.5 and body_rates(env) < 0.5)


def resting_class(env):
    R = env.chest_R(); up = R[2, 2]; roll = math.degrees(math.atan2(R[2, 1], R[2, 2]))
    return 'belly' if up > 0.7 else 'back' if up < -0.7 else ('right' if roll > 0 else 'left')


def place(env, q, rpy, clearance=0.01):
    """put the body in pose q at attitude rpy, lowest point `clearance` above the floor"""
    d, m = env.d, env.m
    mujoco.mj_resetDataKeyframe(m, d, env.key)
    for i in range(NU):
        if env.ten[i] < 0:
            d.qpos[env.qadr[i]] = q[i]
    d.qpos[3:7] = quat_from_rpy(*rpy)
    d.qpos[2] = 1.0
    mujoco.mj_forward(m, d)
    low = min(d.geom_xpos[g][2] - m.geom_rbound[g] * 0.6 for g in range(m.ngeom) if m.geom_bodyid[g] > 0)
    d.qpos[2] = 1.0 - low + clearance
    d.qvel[:] = 0
    mujoco.mj_forward(m, d)


class Skill:
    name = 'skill'
    command_dim = 1
    # residual actions: target = nominal + 0.6 rad x a, with nominal the
    # skill's reference at its own clock; and the command filter (targets
    # move alpha of the way each 20 ms). With range actions (+-1 = a joint's
    # whole travel) and i.i.d. exploration no noisy rollout ever met the
    # stillness predicate (0/8 even at sigma 0.05), so PPO learned only from
    # failure and the deterministic policy decayed from 0.46-0.71 success
    # to 0 (runs/*_v1, the first attempt).
    action_mode = 'offset'
    action_filter = 0.1
    init_std = 0.1
    horizon = 400                 # 8 s at 50 Hz
    hold_need = 100               # 2 s
    terminate_on_success = True

    def __init__(self, level=1.0):
        self.level = level

    # the interface env.py / ppo.py use
    def gains(self, env):
        return stand_kp()

    def nominal(self, env):
        return self.reference(env)

    def command(self, env):
        return np.array([min(env.t / CMD_T, 1.0)])

    # per-skill pieces
    def reference(self, env):
        raise NotImplementedError

    def body(self, env):
        raise NotImplementedError

    def frame_ok(self, env):
        raise NotImplementedError

    def phi(self, env):
        return PHI_W * (0.6 * imitation(env.q(), self.reference(env)) + 0.4 * self.body(env))

    def start(self, env):
        raise NotImplementedError

    def reset(self, env):
        env.push = None
        self.start(env)
        env.t = 0.0
        self.hold = 0
        self.succeeded = False
        self.fell = False
        self.phi_prev = None

    def fallen(self, env):
        return False

    def reward(self, env, action):
        if self.phi_prev is None:
            self.phi_prev = self.phi(env)
        phi = self.phi(env)
        shaping = GAMMA * phi - self.phi_prev
        self.phi_prev = phi
        ok = self.frame_ok(env)
        self.hold = self.hold + 1 if ok else 0
        r = HOLD_R if ok else 0.0
        tau = env.d.actuator_force[env.act][PJ] / TAU[PJ]
        cost = 0.002 * float(tau @ tau) / len(PJ) + 0.002 * float(np.sum((action - env.prev_action) ** 2))
        r += shaping - min(cost, COST_MAX)
        done = False
        if self.hold >= self.hold_need and not self.succeeded:
            self.succeeded = True
            r += SUCCESS_BONUS
            done = self.terminate_on_success
        if self.fallen(env):
            self.fell = True
            done = True
        steps = int(round(env.t / 0.02))
        if steps >= self.horizon:
            done = True
        return r, done, dict(success=self.succeeded, hold=self.hold)

    def success(self, env):
        return self.succeeded


# ---------------------------------------------------------------- stand-up
STANDUP_REF = Reference([(0, STERNAL), (0.3, STERNAL), (2.3, STAND)], 'standup')


class StandUp(Skill):
    """sternal -> stand. Reference: tuck held, then all four legs push up
    together (the order the sim found most stable open loop: 10/10, chest
    never below 0.99 up; hind-first 10/10 at 0.91; front-first 9/10)."""
    name = 'standup'
    horizon = 300                 # 6 s

    def reference(self, env):
        return STANDUP_REF(self.t0 + env.t)

    def body(self, env):
        h = min(1.0, max(0.0, (float(env.d.qpos[2]) - 0.08) / (0.268 - 0.08)))
        return max(0.0, env.up()) * h

    def frame_ok(self, env):
        return standing_frame(env)

    def start(self, env):
        L, r = self.level, env.rng
        # level 0..0.5: reference-state initialisation, from late in the
        # reference back to its start; 0.5..1: perturbed sternal starts and
        # real belly-down falls
        if L <= 0.5 or r.random() < 0.3:
            lo = max(0.0, 0.85 - 1.7 * min(L, 0.5))
            phase = r.uniform(lo, 1.0)
            self.t0 = phase * STANDUP_REF.T
            q = STANDUP_REF(self.t0) + r.normal(0, 0.05, NU)
            place(env, np.clip(q, QMIN, QMAX), (r.normal(0, 0.03), r.normal(0, 0.03), r.uniform(-math.pi, math.pi)))
            env.apply_targets(np.clip(q, QMIN, QMAX), 300)
        else:
            self.t0 = 0.0
            k = (L - 0.5) * 2
            q = np.clip(STERNAL + r.normal(0, 0.6 * k, NU), QMIN, QMAX)
            rpy = (r.normal(0, math.radians(25) * k), r.normal(0, math.radians(15) * k), r.uniform(-math.pi, math.pi))
            settle(env, q, rpy, ms=1200, z=0.30)


# ---------------------------------------------------------------- righting
def load_righting(path=None):
    """the searched righting references, one per resting class"""
    import ref.search_righting as S
    out = {}
    for c in ('left', 'right', 'back'):
        p = os.path.join(HERE, 'ref', 'designs', f'righting_{c}.json')
        if os.path.exists(p):
            out[c] = S.ref_from(np.array(json.load(open(p))['x']))
    return out


class Righting(Skill):
    """side/back -> sternal (belly down, legs tucked), held 0.5 s for the
    hand-off to stand-up. Reference: the cat-style keyframe family (tuck,
    spine twist, hind-leg swing, tail), its numbers searched per resting
    class on the physics (ref/search_righting.py)."""
    name = 'righting'
    horizon = 300                 # 6 s
    hold_need = 25                # 0.5 s, then stand-up takes over

    def __init__(self, level=1.0):
        super().__init__(level)
        self.refs = load_righting()

    def reference(self, env):
        ref = self.refs.get(self.cls)
        return ref(env.t) if ref is not None else STERNAL

    def body(self, env):
        return (env.up() + 1) / 2

    def frame_ok(self, env):
        return sternal_frame(env)

    def start(self, env):
        L, r = self.level, env.rng
        g = GetUp(p_push=0.0, severity=1.0)
        # level sets how far over the body may start: from ~30 deg to fully on its back
        for _ in range(20):
            roll_max = math.radians(30 + 150 * L)
            roll = r.uniform(-roll_max, roll_max)
            pitch = r.uniform(-0.3, 0.3) * (0.3 + 0.7 * L)
            q = np.clip(QSTAND + r.uniform(-0.6, 0.6, NU) * (0.3 + 0.7 * L), QMIN, QMAX)
            settle(env, q, (roll, pitch, r.uniform(-math.pi, math.pi)), ms=800, z=0.35)
            self.cls = resting_class(env)
            if self.cls != 'belly' or L < 0.3:
                break
        if self.cls == 'belly':
            self.cls = 'left' if r.random() < 0.5 else 'right'     # already over: the reference just tucks


# ---------------------------------------------------------------- balance
class Balance(Skill):
    """standing push recovery: random 10-60 N x 100 ms pushes; stay up, step
    if the push needs it, and be back standing still within the episode.
    Success = never fell AND the standing predicate held over the last 2 s."""
    name = 'balance'
    horizon = 300                 # 6 s: push at 0.3-1.5 s, then >= 4.5 s to settle
    terminate_on_success = False
    action_filter = 0.3           # faster than the get-up skills: stepping can't wait
    init_std = 0.05

    def __init__(self, level=1.0, stance=None):
        super().__init__(level)
        self.stance = READY if stance is None else np.asarray(stance, float)

    def nominal(self, env):
        return self.stance

    def command(self, env):
        # the balance reflex runs for as long as the body stands: no clock
        return np.zeros(1)

    def reference(self, env):
        return self.stance

    def body(self, env):
        v = float(np.linalg.norm(env.d.qvel[0:2]))
        h = min(1.0, max(0.0, (float(env.d.qpos[2]) - 0.08) / (0.268 - 0.08)))
        return max(0.0, env.up()) * h * math.exp(-v * v / 0.1)

    def frame_ok(self, env):
        return standing_frame(env, self.stance)

    def fallen(self, env):
        return env.up() < 0.5 or float(env.d.qpos[2]) < 0.12

    def start(self, env):
        L, r = self.level, env.rng
        mujoco.mj_resetDataKeyframe(env.m, env.d, env.key)
        mujoco.mj_forward(env.m, env.d)
        env.apply_targets(self.stance, 300)
        f_hi = 10.0 + 50.0 * L
        f = r.uniform(10.0, max(10.0, f_hi))
        a = r.uniform(0, 2 * math.pi)
        t0 = r.uniform(0.3, 1.5)
        env.push = (env.chest, np.array([f * math.cos(a), f * math.sin(a), 0.0]), t0, t0 + 0.1)
        self.push_f = f

    def reward(self, env, action):
        r, done, info = super().reward(env, action)
        # success for balance is judged at the end: no fall, and the last 2 s held
        if done:
            self.succeeded = (not self.fell) and self.hold >= self.hold_need
            info['success'] = self.succeeded
        return r, done, info


SKILLS = {'standup': StandUp, 'righting': Righting, 'balance': Balance}
