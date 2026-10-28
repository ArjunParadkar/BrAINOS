"""Unit tests for the Pi skill runtime + KIRA gate, plus the 50 Hz policy
timing self-check. Runs in the pi venv (numpy only). No sim, no hardware."""
import sys, os
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import numpy as np
from kira_gate import KiraGate, BodyState, Posture
from skill_runtime import SkillRuntime

fails = 0
def ok(c, m):
    global fails
    print(("  ok " if c else "  FAIL ") + m)
    if not c: fails += 1

def test_gate():
    print("KIRA gate:")
    g = KiraGate(allow_spine_abd=False)
    ok(g.check("stop", BodyState(estopped=True)).granted(), "stop is always allowed, even e-stopped")
    ok(not g.check("stand", BodyState(estopped=True)).granted(), "stand denied while e-stopped")
    ok(g.check("stand", BodyState(estopped=True)).code == "estopped", "...with code estopped")
    ok(not g.check("walk", BodyState()).granted(), "walk denied: no spine/abd motors fitted")
    ok(g.check("walk", BodyState()).code == "no_spine_abd", "...with code no_spine_abd")
    ok(not g.check("walk", BodyState(posture=Posture.ON_BACK)).granted(), "walk denied on back")
    ok(g.check("righting", BodyState(posture=Posture.ON_BACK, upright=-0.5)).granted(), "righting allowed on back")
    ok(not g.check("stand", BodyState(battery_frac=0.1)).granted(), "stand denied on low battery")
    ok(not g.check("sit", BodyState(posture=Posture.ON_BACK, upright=-0.3)).granted(), "sit denied on back")
    g2 = KiraGate(allow_spine_abd=True)
    ok(g2.check("walk", BodyState()).granted(), "walk allowed once spine/abd enabled")

def test_sequencer():
    print("skill sequencer:")
    rt = SkillRuntime(njoints=4)
    tgt = np.array([0.5, -0.3, 0.2, 0.0])
    rt.set_posture_pose("sit", Posture.SIT, tgt, settle_s=0.2)
    d = rt.request("sit")
    ok(d.granted(), "sit granted from stand")
    q = np.zeros(4)
    for _ in range(200):
        cmd = rt.step(q)
        q = q + 0.5 * (cmd - q)        # a lagging body that follows the command
    ok(rt.state.posture is Posture.SIT, "posture reaches SIT after the transition settles")
    ok(np.max(np.abs(q - tgt)) < 0.1, "body converges to the sit pose")

def test_timing():
    print("50 Hz policy timing self-check:")
    rt = SkillRuntime(njoints=26)
    t = rt.timing_selfcheck()
    print(f"  policy forward {t['per_step_us']:.1f} us vs {t['budget_us']:.0f} us budget "
          f"({t['headroom_x']:.0f}x headroom)")
    ok(t["per_step_us"] < t["budget_us"], "a policy step fits the 20 ms (50 Hz) budget")

if __name__ == "__main__":
    test_gate(); test_sequencer(); test_timing()
    print(f"\n{'ALL PASS' if not fails else str(fails)+' FAILED'}")
    sys.exit(1 if fails else 0)
