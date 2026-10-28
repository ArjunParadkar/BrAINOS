"""
kira_gate.py -- the Pi-side KIRA gate for physical skill requests.

Every skill request on the robot passes through KIRA. The AUTHORITATIVE
policy engine is the no_std Rust crate `brainos-kira` in `os/` (validate ->
simulate -> policy, with sealed evidence); this module is the Pi-side client
that mirrors KIRA's *physical-state* deny rules so the skill runtime can gate
locally at 50 Hz and produce the same verdict and the same spoken reason. It
is intentionally a faithful, testable mirror, not a second source of truth:
on the robot it defers to the Rust engine for anything beyond these rules and
records the Rust decision.

Deny rules (brief section 3): KIRA denies anything unsafe for the current
state -- on its back, low battery, e-stopped, tethered mode -- plus the
preconditions each skill needs (e.g. you cannot `walk` from on your back;
you must `right` then `stand` first).
"""
from __future__ import annotations

from dataclasses import dataclass, field
from enum import Enum


class Posture(str, Enum):
    STAND = "stand"
    SIT = "sit"
    LIE = "lie"          # sternal, belly down
    ON_BACK = "on_back"  # or on a side: cannot act until righted
    FALLEN = "fallen"


@dataclass
class BodyState:
    posture: Posture = Posture.STAND
    battery_frac: float = 1.0        # 0..1
    estopped: bool = False
    tethered: bool = False           # on the stand / legs off the ground
    committed: bool = True           # nodes calibrated + committed
    upright: float = 1.0             # cos(tilt): 1 upright, <0 on back
    low_battery_frac: float = 0.15


class Verdict(str, Enum):
    GRANT = "grant"
    DENY = "deny"


@dataclass
class Decision:
    verdict: Verdict
    code: str = ""
    reason: str = ""                 # one sentence, sayable
    def granted(self) -> bool:
        return self.verdict is Verdict.GRANT


# which postures a skill may start from
_PRECOND = {
    "stand":    {Posture.SIT, Posture.LIE, Posture.STAND},
    "sit":      {Posture.STAND, Posture.SIT},
    "lie":      {Posture.STAND, Posture.SIT, Posture.LIE},
    "standup":  {Posture.LIE, Posture.SIT, Posture.STAND},
    "righting": {Posture.ON_BACK, Posture.FALLEN, Posture.LIE},
    "balance":  {Posture.STAND},
    "walk":     {Posture.STAND},
    "hop":      {Posture.STAND},
    "come_here":{Posture.STAND},
    "follow":   {Posture.STAND},
    "look_at_me": {Posture.STAND, Posture.SIT, Posture.LIE},
    "stop":     set(Posture),        # stop is always allowed
    "lie_down": {Posture.STAND, Posture.SIT, Posture.LIE},
}
# skills that are locomotion / require the (missing) 4015 spine+abduction
_NEEDS_SPINE_ABD = {"walk", "hop", "come_here", "follow"}


@dataclass
class KiraGate:
    allow_spine_abd: bool = False    # False until the 7x4015 are installed
    def check(self, skill: str, s: BodyState) -> Decision:
        if skill in ("stop", "freeze"):
            return Decision(Verdict.GRANT, "", "Stopping.")
        if s.estopped:
            return Decision(Verdict.DENY, "estopped",
                            "I can't move while the emergency stop is engaged.")
        if not s.committed:
            return Decision(Verdict.DENY, "uncommitted",
                            "My joints aren't calibrated yet, so I won't move.")
        if s.battery_frac <= s.low_battery_frac:
            return Decision(Verdict.DENY, "low_battery",
                            "My battery is too low for that right now.")
        if s.upright < 0.0 and skill not in ("righting", "stop"):
            return Decision(Verdict.DENY, "on_back",
                            "I'm on my back; let me right myself first.")
        if s.tethered and skill in _NEEDS_SPINE_ABD:
            return Decision(Verdict.DENY, "tethered",
                            "I'm on the stand, so I'll hold off on walking.")
        if skill in _NEEDS_SPINE_ABD and not self.allow_spine_abd:
            return Decision(Verdict.DENY, "no_spine_abd",
                            "My spine and hip motors aren't fitted yet, so I can't walk.")
        allowed = _PRECOND.get(skill)
        if allowed is None:
            return Decision(Verdict.DENY, "unknown_skill",
                            "I don't know how to do that.")
        if s.posture not in allowed:
            return Decision(Verdict.DENY, "bad_precondition",
                            f"I can't do that from {s.posture.value}.")
        return Decision(Verdict.GRANT, "", "")
