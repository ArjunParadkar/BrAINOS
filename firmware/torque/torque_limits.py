#!/usr/bin/env python3
"""
torque_limits.py -- first-principles per-joint torque limits for BLUR.

Computes, from the motor KV, the 16:1 cycloid ratio, an assumed gearbox
efficiency and each driver's RMS/peak current rating, what every joint can
really deliver *continuously* (thermal, RMS current) and in a *peak* burst.
This is the calculation the brief (section 1, "Driver torque shortfall")
asks for; it supersedes the single `drv_peak_a`/`current_ok` pair in
descriptor.json with a continuous-vs-peak pair per joint, and lists the
skills the real limits rule out.

It reads blur/descriptor.json for the body (KV, gear, tau_max, the
mechanical ceiling, the joint list) and writes:
  * firmware/torque/torque_limits.json  -- machine-readable, to be merged
    into BLUR_final/mujoco/blur_meta.json when that folder arrives, and
    consumed by the training env as the current/torque cap.
  * stdout                              -- the human table + ruled-out skills.

Nothing here is a hardware measurement. The B-G431B-ESC1 continuous
current is UNVERIFIED (ST only publishes a forced-air peak of 40 A); the
4015 KV (~100) and its driver (SimpleFOC Mini / DRV8313) are assumptions
carried from physics_checks.py and the brief. Every assumption is tagged
in the output so load-day numbers can replace it.

Motor torque constant: kt_motor [N.m/A] = 60 / (2*pi*KV) = 9.5493 / KV.
Output torque:         tau_out = kt_motor * gear * eta * I.

Usage:
  python3 firmware/torque/torque_limits.py            # eta = 0.75 (brief)
  python3 firmware/torque/torque_limits.py --eta 0.85 # descriptor's value
"""
import argparse
import json
import math
import os

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(HERE))
DESC = os.path.join(REPO, "blur", "descriptor.json")
OUT = os.path.join(HERE, "torque_limits.json")

# --- driver current ratings (A). peak = short burst; cont = RMS, thermal. ---
# Sources are cited in firmware/torque/TORQUE.md; every "verified: false"
# figure must be confirmed on load day with a lever arm and a scale.
DRIVERS = {
    "B-G431B-ESC1": {
        # ST publishes 40 A as the *peak* output current, "tested with a
        # propeller for forced-air cooling" -- not a continuous rating.
        # No official continuous figure exists. We use a conservative
        # no-/low-airflow band; the real number is a load-day measurement.
        "i_peak_a": 40.0, "i_peak_verified": False,
        "i_cont_a": 6.0, "i_cont_verified": False,
        "note": "40 A = ST forced-air peak; 6 A continuous is a conservative "
                "placeholder, MEASURE with the 2806.5 + its own cooling.",
    },
    "SimpleFOC-Mini-DRV8313": {
        # TI DRV8313: 2.5 A peak, 1.75 A RMS continuous per output.
        "i_peak_a": 2.5, "i_peak_verified": True,
        "i_cont_a": 1.75, "i_cont_verified": True,
        "note": "TI DRV8313 datasheet: 2.5 A peak / 1.75 A RMS per half-bridge.",
    },
    "DRV8833": {
        # TI DRV8833: 2 A peak, 1.5 A RMS per H-bridge (can be paralleled).
        "i_peak_a": 2.0, "i_peak_verified": True,
        "i_cont_a": 1.5, "i_cont_verified": True,
        "note": "TI DRV8833 datasheet: 2 A peak / 1.5 A RMS per H-bridge.",
    },
}

# which driver each motor kind actually runs on
MOTOR_DRIVER = {
    "esc2806": "B-G431B-ESC1",
    "gimbal4015": "SimpleFOC-Mini-DRV8313",
    "gimbal2204": "SimpleFOC-Mini-DRV8313",
    "winch2204": "SimpleFOC-Mini-DRV8313",
    "n20": "DRV8833",
}


def kt_motor(kv):
    """N.m per amp, motor side, from KV (rpm/V)."""
    return 60.0 / (2.0 * math.pi * kv)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--eta", type=float, default=0.75,
                    help="gearbox efficiency (brief: 0.75; descriptor: 0.85)")
    args = ap.parse_args()
    eta = args.eta

    d = json.load(open(DESC))
    motors = d["motors"]
    rows = []
    for a in d["actuators"]:
        mk = a["motor"]
        mm = motors[mk]
        drv = DRIVERS[MOTOR_DRIVER[mk]]
        tendon = a["tendon"]
        kv = mm["kv"]
        gear = a["gear"]
        if kv is None:
            # brushed N20 gripper: no KV, torque set mechanically by the
            # lead screw; report the mechanical ceiling and skip the
            # electrical model.
            tau_mech = a["tau_max"]
            rows.append({
                "name": a["name"], "motor": mk, "driver": MOTOR_DRIVER[mk],
                "unit": a["unit"], "kv": None, "gear": gear, "eta": eta,
                "kt_out_Nm_per_A": None, "tau_mech_design": tau_mech,
                "i_cont_A": drv["i_cont_a"], "i_peak_A": drv["i_peak_a"],
                "i_for_tau_mech_A": None,
                "tau_cont_deliverable": tau_mech, "tau_peak_deliverable": tau_mech,
                "limited_by_peak": "mechanical", "limited_by_cont": "mechanical",
                "current_reaches_design": True, "tendon": tendon,
            })
            continue
        ktm = kt_motor(kv)
        kt_out = ktm * gear * eta           # N.m/A at the gearbox output
        tau_mech = a["tau_max"]             # mechanical/design ceiling (N.m or drum N.m)

        # electrical ceilings from the driver currents
        tau_peak_elec = kt_out * drv["i_peak_a"]
        tau_cont_elec = kt_out * drv["i_cont_a"]

        # a winch's "tau_max" is in drum-torque units with a huge kt_out
        # (lead screw); its cable force is handled elsewhere. Report the
        # electrical cap but flag it as force-domain.
        if tendon:
            deliver_peak = min(tau_mech, tau_peak_elec)
            deliver_cont = min(tau_mech, tau_cont_elec)
        else:
            deliver_peak = min(tau_mech, tau_peak_elec)
            deliver_cont = min(tau_mech, tau_cont_elec)

        i_for_mech = tau_mech / kt_out if kt_out > 0 else float("nan")
        rows.append({
            "name": a["name"], "motor": mk, "driver": MOTOR_DRIVER[mk],
            "unit": a["unit"], "kv": kv, "gear": gear, "eta": eta,
            "kt_out_Nm_per_A": round(kt_out, 5),
            "tau_mech_design": tau_mech,
            "i_cont_A": drv["i_cont_a"], "i_peak_A": drv["i_peak_a"],
            "i_for_tau_mech_A": round(i_for_mech, 3),
            "tau_cont_deliverable": round(deliver_cont, 3),
            "tau_peak_deliverable": round(deliver_peak, 3),
            "limited_by_peak": "driver" if tau_peak_elec < tau_mech else "mechanical",
            "limited_by_cont": "driver" if tau_cont_elec < tau_mech else "mechanical",
            "current_reaches_design": i_for_mech <= drv["i_peak_a"] + 1e-6,
            "tendon": tendon,
        })

    out = {
        "generator": "firmware/torque/torque_limits.py",
        "eta_gearbox": eta,
        "kt_formula": "kt_out = (9.5493/KV) * gear * eta  [N.m/A]",
        "drivers": DRIVERS,
        "motor_driver": MOTOR_DRIVER,
        "joints": rows,
        "note": "peak = short burst at driver i_peak; cont = sustainable at "
                "driver RMS (thermal). For the 2806.5 legs the continuous cap "
                "is thermal, not current-ceiling: use the node I2t estimator. "
                "Merge into BLUR_final/mujoco/blur_meta.json when available.",
    }
    json.dump(out, open(OUT, "w"), indent=1)

    # --- human report ---
    w = max(len(r["name"]) for r in rows)
    print(f"\nBLUR per-joint torque limits  (eta={eta}, kt=9.5493/KV*gear*eta)\n")
    print(f"{'joint':<{w}}  {'motor':<11} {'driver':<22} "
          f"{'kt_out':>7} {'design':>7} {'cont':>6} {'peak':>6}  limited-by(cont/peak)")
    for r in rows:
        kt = f"{r['kt_out_Nm_per_A']:>7.4f}" if r["kt_out_Nm_per_A"] is not None else f"{'--':>7}"
        print(f"{r['name']:<{w}}  {r['motor']:<11} {r['driver']:<22} "
              f"{kt} {r['tau_mech_design']:>7.2f} "
              f"{r['tau_cont_deliverable']:>6.2f} {r['tau_peak_deliverable']:>6.2f}  "
              f"{r['limited_by_cont']}/{r['limited_by_peak']}")

    # --- skills ruled out, from the 4015 spine/abduction limits ---
    s1 = next(r for r in rows if r["name"] == "spine_s1")
    abd = next(r for r in rows if r["name"] == "FR_abd")
    print("\n--- what the real limits allow (4015 spine/abduction on DRV8313) ---")
    print(f"spine S1 : cont {s1['tau_cont_deliverable']} / peak {s1['tau_peak_deliverable']} N.m")
    print(f"abduction: cont {abd['tau_cont_deliverable']} / peak {abd['tau_peak_deliverable']} N.m")
    print("gallop/bound need spine S1 6.6-11 N.m p95 (RESULTS_v36.md): RULED OUT "
          f"({s1['tau_peak_deliverable']:.2f} peak, 2-3x short).")
    print("table-jump (750 mm) uses abduction+spine push: AT RISK, measure margin.")
    print("walk needs spine ~2.12 N.m p95: feasible at peak, but near the "
          f"continuous cap ({s1['tau_cont_deliverable']:.2f}); keep the training torque penalty.")
    print("sit/lie/stand-up/righting/balance are leg-pitch/knee driven "
          "(2806.5, 4.0 N.m peak OK): feasible.")
    print(f"\nwrote {OUT}")


if __name__ == "__main__":
    main()
