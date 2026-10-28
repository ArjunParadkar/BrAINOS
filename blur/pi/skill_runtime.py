"""
skill_runtime.py -- the Pi-side skill runtime inside the BLUR BrAInOS
instance.

Loads the trained policies, sequences skills with smooth transitions, and
gates every request through KIRA (`kira_gate.py`, the Pi-side mirror of the
Rust engine). It runs at 50 Hz on the Pi CPU; `timing_selfcheck()` measures
the per-step policy budget so the brief's "run at 50 Hz, check the timing"
claim is a number, not a hope.

Policies export to ONNX (`blur/learn/export.py`). onnxruntime is optional: if
it is not installed, or a policy is missing, the runtime falls back to a
scripted keyframe transition toward the skill's reference pose -- which is
exactly what the stand/sit/lie/stand-up skills need and is enough to drive the
SIL twin end to end. Locomotion skills (walk/hop) need the trained policy and
the 7x4015 spine/abduction motors; KIRA denies them until those are fitted.
"""
from __future__ import annotations

import time
from dataclasses import dataclass, field

import numpy as np

from kira_gate import BodyState, Decision, KiraGate, Posture

# posture -> target joint vector is filled from the descriptor / moves.npz by
# the caller via set_keyframes(); these are the skills the runtime sequences.
TRANSITIONS = {
    "stand": Posture.STAND,
    "sit": Posture.SIT,
    "lie": Posture.LIE,
    "lie_down": Posture.LIE,
    "standup": Posture.STAND,
    "righting": Posture.LIE,   # righting ends sternal (belly down)
}


@dataclass
class Skill:
    name: str
    target_posture: Posture
    q_target: np.ndarray            # desired joint vector (rad)
    settle_s: float = 1.0           # hold time to count as done
    policy: object = None           # onnxruntime session or None


class SkillRuntime:
    def __init__(self, njoints: int, gate: KiraGate | None = None, rate_hz: float = 50.0):
        self.n = njoints
        self.gate = gate or KiraGate()
        self.dt = 1.0 / rate_hz
        self.state = BodyState()
        self.q = np.zeros(njoints)          # current joint estimate
        self.q_cmd = np.zeros(njoints)      # commanded joint target
        self.kp = np.full(njoints, 12.0)
        self.kd = np.full(njoints, 0.3)
        self.skills: dict[str, Skill] = {}
        self.active: Skill | None = None
        self._t_in_skill = 0.0
        self.last_decision: Decision | None = None

    def add_skill(self, skill: Skill):
        self.skills[skill.name] = skill

    def set_posture_pose(self, name: str, posture: Posture, q_target, settle_s=1.0, policy=None):
        self.add_skill(Skill(name, posture, np.asarray(q_target, float), settle_s, policy))

    # --- request path: gate -> schedule ---
    def request(self, skill: str) -> Decision:
        d = self.gate.check(skill, self.state)
        self.last_decision = d
        if not d.granted():
            return d
        if skill in ("stop", "freeze"):
            self.active = None
            self.kp[:] = 0.3            # go soft/damped
            return d
        sk = self.skills.get(skill)
        if sk is None:
            # a granted skill with no loaded pose (e.g. a policy skill): no-op target
            return d
        self.active = sk
        self._t_in_skill = 0.0
        return d

    # --- 50 Hz step: produce joint setpoints ---
    def step(self, q_meas=None) -> np.ndarray:
        if q_meas is not None:
            self.q = np.asarray(q_meas, float)
        if self.active is not None:
            sk = self.active
            if sk.policy is not None:
                obs = self._obs()
                self.q_cmd = self._run_policy(sk.policy, obs)
            else:
                # critically-damped-ish approach to the target pose
                alpha = min(1.0, self.dt / 0.25)
                self.q_cmd = self.q_cmd + alpha * (sk.q_target - self.q_cmd)
            self._t_in_skill += self.dt
            if np.max(np.abs(self.q - sk.q_target)) < 0.08:
                if self._t_in_skill >= sk.settle_s:
                    self.state.posture = sk.target_posture
                    self.active = None
        return self.q_cmd

    def _obs(self) -> np.ndarray:
        return np.concatenate([self.q, np.zeros(self.n), [self.state.upright]])

    def _run_policy(self, sess, obs) -> np.ndarray:
        inp = {sess.get_inputs()[0].name: obs.astype(np.float32)[None]}
        out = sess.run(None, inp)[0][0]
        return np.clip(out, -3.14, 3.14)

    # --- timing self-check: is a 50 Hz policy step affordable on the Pi CPU? ---
    def timing_selfcheck(self, obs_dim=81, hidden=256, act=26, iters=2000) -> dict:
        """Time a representative MLP forward (obs->256->256->act, tanh) in
        numpy as a proxy for the ONNX policy, to size the 20 ms budget."""
        rng = np.random.default_rng(0)
        W1 = rng.standard_normal((obs_dim, hidden)).astype(np.float32)
        W2 = rng.standard_normal((hidden, hidden)).astype(np.float32)
        W3 = rng.standard_normal((hidden, act)).astype(np.float32)
        x = rng.standard_normal((1, obs_dim)).astype(np.float32)
        # warmup
        for _ in range(50):
            np.tanh(np.tanh(x @ W1) @ W2) @ W3
        t0 = time.perf_counter()
        for _ in range(iters):
            np.tanh(np.tanh(x @ W1) @ W2) @ W3
        dt = (time.perf_counter() - t0) / iters
        return {"per_step_us": dt * 1e6, "budget_us": self.dt * 1e6,
                "headroom_x": (self.dt) / dt}
