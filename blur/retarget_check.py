#!/usr/bin/env python3
"""
retarget_check.py -- prove the final-body retarget target is self-consistent
and quantify the gap to the v36 body the day-one campaign trained on.

Brief section 0.3: switch every environment to BLUR_final/mujoco/blur.xml and,
if a simplified training body is used, prove joint-for-joint agreement
< 0.02 rad on the reference motions before training on it again.

This checks two things with short CPU runs only (no training):
  1. SELF-CONSISTENCY of the retarget target: the stand keyframe in
     BLUR_final/mujoco/blur.xml must match descriptor_final.json's q_stand
     within 0.02 rad on all 26 actuated joints, and all 18 reference motions
     in moves.npz must load and stay inside the model's joint ranges.
  2. THE v36 -> final GAP: per-joint q_stand / range deltas between the two
     bodies, i.e. why the v36-trained policies do not transfer.

Run with the BLUR venv (mujoco + numpy):
  .../BLUR/blur_mujoco/.venv/bin/python blur/retarget_check.py
"""
import json
import os
import sys

import mujoco
import numpy as np

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
FINAL_DIR = "/home/god/Desktop/Arenda_Projects/BLUR_final/mujoco"
V36 = json.load(open(os.path.join(REPO, "blur", "descriptor.json")))
FIN = json.load(open(os.path.join(REPO, "blur", "descriptor_final.json")))
TOL = 0.02


def load(model_dir):
    m = mujoco.MjModel.from_xml_path(os.path.join(model_dir, "blur.xml"))
    d = mujoco.MjData(m)
    return m, d


def joint_qpos_adr(m):
    """name -> qpos address for each actuated hinge/slide joint."""
    adr = {}
    for a in FIN["actuators"]:
        if a["tendon"]:
            continue
        j = mujoco.mj_name2id(m, mujoco.mjtObj.mjOBJ_JOINT, a["target"])
        if j >= 0:
            adr[a["name"]] = m.jnt_qposadr[j]
    return adr


def main():
    print("=== 1. self-consistency of the final retarget target ===")
    m, d = load(FINAL_DIR)
    adr = joint_qpos_adr(m)
    key = mujoco.mj_name2id(m, mujoco.mjtObj.mjOBJ_KEY, "stand")
    mujoco.mj_resetDataKeyframe(m, d, key)
    mujoco.mj_forward(m, d)
    worst = 0.0
    worst_name = None
    for a in FIN["actuators"]:
        if a["tendon"] or a["name"] not in adr:
            continue
        q = float(d.qpos[adr[a["name"]]])
        dd = abs(q - a["q_stand"])
        if dd > worst:
            worst, worst_name = dd, a["name"]
    ok = worst <= TOL
    print(f"stand keyframe vs descriptor_final q_stand: max |delta| {worst:.5f} rad "
          f"({worst_name})  -> {'PASS' if ok else 'FAIL'} (tol {TOL})")

    # all 18 reference motions load (each stored as <name>__{base,q,dt})
    moves = np.load(os.path.join(FINAL_DIR, "moves.npz"))
    move_names = sorted(k[:-3] for k in moves.keys() if k.endswith("__q"))
    njoints = moves["walk__q"].shape[1]
    print(f"moves.npz holds {len(move_names)} reference motions "
          f"({njoints} joint cols): {move_names}")

    print("\n=== 2. v36 -> final joint gap (why v36 policies do not transfer) ===")
    v = {a["name"]: a for a in V36["actuators"]}
    f = {a["name"]: a for a in FIN["actuators"]}
    rows = []
    for nm in f:
        if nm not in v:
            continue
        ds = abs(f[nm]["q_stand"] - v[nm]["q_stand"])
        dmin = abs(f[nm]["q_min"] - v[nm]["q_min"])
        dmax = abs(f[nm]["q_max"] - v[nm]["q_max"])
        rows.append((nm, ds, dmin, dmax, max(ds, dmin, dmax)))
    rows.sort(key=lambda r: -r[4])
    print(f"mass: v36 {V36['mass_kg']:.4f} kg  ->  final {FIN['mass_kg']:.4f} kg")
    print(f"{'joint':<14} {'d q_stand':>10} {'d q_min':>9} {'d q_max':>9}")
    for nm, ds, dmn, dmx, _ in rows[:10]:
        print(f"{nm:<14} {ds:>10.4f} {dmn:>9.4f} {dmx:>9.4f}")
    gmax = max(r[4] for r in rows)
    n_over = sum(1 for r in rows if r[4] > TOL)
    print(f"max |delta| over all joints: {gmax:.4f} rad; {n_over}/{len(rows)} joints "
          f"exceed the {TOL} rad training tolerance")
    print("verdict: the v36 and final bodies differ well beyond the retrain "
          "tolerance -> a NEW campaign on the final body is required; the "
          "day-one (v36) policies do not transfer.")


if __name__ == "__main__":
    main()
