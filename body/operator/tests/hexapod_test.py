#!/usr/bin/env python3
"""Phase 0 on the entity side, end to end (Execution Plan §4.7) — against the
simulated Mini-Blur, and later against the real board with the same script.

  H1  a hexapod joins: schema parsed, region class legs, HexapodPolicy bound;
      a wrong-key hexapod is refused and tamper-logged
  H2  "on miniblur, stand up"                 → gait.stand runs on the body
  H3  "on miniblur, walk forward 3 steps"     → gait.walk runs; digest says so
  H4  refusals at NAMED stages, on a motor:
        40 steps            → DENY at validate  (envelope)
        a tippy body        → DENY at simulate  (forward model)
        a body gone silent  → DENY at policy    (stale link)
  H5  calibration is memory, not the device: record a joint, restart the
      operator AND the body, the calibration comes back from the state graph
      and is pushed down to the body
  D1  laptop: a note enters the ONE memory
  D2  hexapod: "do the greeting dance" — the deliberate tier reads the note
      and walks it (two bodies, one memory)               [needs ANTHROPIC_API_KEY]
  D3  the same utterance replays as a signed skill, every step re-gated
  H6  the body leaves: its verbs and its policy are gone

Run:  python3 body/operator/tests/hexapod_test.py
      (operator built with cargo build --release; sim built with
       cargo build --release -p miniblur-sim in miniblur/)
"""
import os
import signal
import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import entity_test as et  # noqa: E402  (the harness: operator, face, ok/skip, summary)

REPO = et.REPO
SIM = REPO / "miniblur" / "target" / "release" / "miniblur-sim"
API = et.API


def start_sim(name, pub, *extra):
    d = et.SCRATCH / f"sim-{name}"
    d.mkdir(parents=True, exist_ok=True)
    log = open(d / "log", "w")
    args = [str(SIM), "serve", "--seat", et.REACH_ADDR, "--name", name, "--pub", pub, "--dir", str(d), *extra]
    p = subprocess.Popen(args, stderr=log, stdout=log)
    return p, d


def deny_stage(msgs, verb):
    """The KIRA stage named in the first denial of `verb`, or None."""
    for a in et.acts(msgs, verb):
        if a.get("status") == "deny":
            d = a.get("digest", "")
            for st in ("authn", "authz", "validate", "simulate", "policy"):
                if f"denied at {st}" in d:
                    return st, d
            return "?", d
    return None, ""


def main():
    if not et.BIN.exists():
        sys.exit(f"build the operator first: cargo build --release ({et.BIN} missing)")
    if not SIM.exists():
        sys.exit(f"build the sim first: cargo build --release -p miniblur-sim ({SIM} missing)")
    et.SCRATCH.mkdir(parents=True, exist_ok=True)
    brain, pub = et.setup_brain()
    cfg = et.write_config(brain)
    print(f"scratch: {et.SCRATCH}\nentity pub: {pub[:8]}..{pub[-4:]}\napi key: {'yes' if API else 'NO (D2/D3 skipped)'}", flush=True)

    procs = []
    op = et.start_operator(cfg, "operator.log")
    procs.append(op)
    face = et.Face()
    face.drain(0.5)
    ok, skip, says, acts = et.ok, et.skip, et.says, et.acts

    try:
        # ---- H1 a hexapod joins
        print("\n[H1] a hexapod joins through §8", flush=True)
        sim, simdir = start_sim("miniblur", pub)
        procs.append(sim)
        joined = face.wait_for(lambda m: m.get("t") == "body" and m.get("name") == "miniblur" and m.get("joined"), 20)
        ok("H1a body 'miniblur' joined as legs", joined is not None and joined.get("class") == "legs",
           f"class={joined.get('class') if joined else '?'} hands={len(joined.get('hands', [])) if joined else 0} os={joined.get('os') if joined else '?'}")
        msgs, _, _ = face.utter("where are you", timeout=10)
        s = says(msgs)
        ok("H1b body.list shows the bound hexapod policy", "bound by" in s and "hexapod" in s and "uncalibrated" in s, s.replace("\n", " | ")[:200])

        evil, evildir = start_sim("evil", "00" * 32, "--once")
        procs.append(evil)
        tlog = evildir / "tamper.log"
        t_end = time.time() + 15
        while time.time() < t_end and not tlog.exists():
            time.sleep(0.3)
        ok("H1c wrong-key hexapod refused + tamper logged", tlog.exists(), (tlog.read_text().strip().splitlines()[-1] if tlog.exists() else "no tamper.log"))
        never = face.wait_for(lambda m: m.get("t") == "body" and m.get("name") == "evil", 2)
        ok("H1d 'evil' never incorporated", never is None)

        # ---- H2 stand
        print("\n[H2] reflex on the body", flush=True)
        msgs, _, total = face.utter("on miniblur, stand up", timeout=15)
        a = acts(msgs, "gait.stand")
        ok("H2 gait.stand ran on 'miniblur'", a and a[0].get("body") == "miniblur" and a[0].get("status") == "ok",
           f"act={a[0] if a else None} total {total:.1f} ms")

        # ---- H3 walk
        msgs, _, total = face.utter("on miniblur, walk forward 3 steps", timeout=15)
        a = acts(msgs, "gait.walk")
        ok("H3 gait.walk fwd 3 ran on 'miniblur'", a and a[0].get("status") == "ok" and "walked 3" in a[0].get("digest", ""),
           f"target={a[0].get('target') if a else None} digest={a[0].get('digest') if a else None} total {total:.1f} ms")

        # ---- H4 refusals at named stages
        print("\n[H4] KIRA refuses on a motor, at named stages", flush=True)
        msgs, _, _ = face.utter("on miniblur, walk forward 40 steps", timeout=15)
        st, d = deny_stage(msgs, "gait.walk")
        ok("H4a 40 steps → DENY at validate", st == "validate", d[:120])
        ok("H4a' nothing reached the body", not any(a.get("status") == "ok" for a in acts(msgs, "gait.walk")))

        tippy, _ = start_sim("tippy", pub, "--tippy")
        procs.append(tippy)
        face.wait_for(lambda m: m.get("t") == "body" and m.get("name") == "tippy" and m.get("joined"), 20)
        msgs, _, _ = face.utter("on tippy, walk forward 1 steps", timeout=15)
        st, d = deny_stage(msgs, "gait.walk")
        ok("H4b tippy body → DENY at simulate", st == "simulate", d[:140])
        tippy.send_signal(signal.SIGTERM)

        mute, _ = start_sim("mute", pub, "--silent-after", "1")
        procs.append(mute)
        face.wait_for(lambda m: m.get("t") == "body" and m.get("name") == "mute" and m.get("joined"), 20)
        msgs, _, _ = face.utter("on mute, stand up", timeout=15)
        a = acts(msgs, "gait.stand")
        ok("H4c-1 first command reached 'mute'", a and a[0].get("status") == "ok", f"{a[0] if a else None}")
        time.sleep(1.5)  # the body has gone dark; the seat's link clock goes stale
        msgs, _, _ = face.utter("on mute, walk forward 1 steps", timeout=15)
        st, d = deny_stage(msgs, "gait.walk")
        ok("H4c-2 silent body → DENY at policy (stale link)", st == "policy" and "stale" in d, d[:140])
        mute.kill()

        # ---- H5 calibration is memory
        print("\n[H5] calibration lives in the state graph, not on the body", flush=True)
        msgs, _, _ = face.utter("on miniblur, begin calibration", timeout=15)
        a = acts(msgs, "body.calibrate")
        ok("H5a calibration mode", a and a[0].get("status") == "ok", f"{a[0].get('digest') if a else None}")
        msgs, _, _ = face.utter("on miniblur, record servo 6 center 92.5 min 30 max 150", timeout=15)
        a = acts(msgs, "body.calibrate")
        ok("H5b joint 6 recorded", a and a[0].get("status") == "ok" and "ch6 center 92.5" in a[0].get("digest", ""), f"{a[0].get('digest') if a else None}")
        msgs, _, _ = face.utter("on miniblur, end calibration", timeout=15)
        ok("H5c calibration ended", any(x.get("status") == "ok" for x in acts(msgs, "body.calibrate")))

        # restart everything: operator saves the journal on SIGTERM; the body forgets on power-off
        op.send_signal(signal.SIGTERM)
        sim.send_signal(signal.SIGTERM)
        try:
            op.wait(timeout=10)
        except subprocess.TimeoutExpired:
            op.kill()
        sim.kill()
        time.sleep(0.5)
        op = et.start_operator(cfg, "operator2.log")
        procs.append(op)
        face = et.Face()
        face.drain(0.5)
        sim, simdir = start_sim("miniblur", pub)
        procs.append(sim)
        joined = face.wait_for(lambda m: m.get("t") == "body" and m.get("name") == "miniblur" and m.get("joined"), 20)
        ok("H5d body rejoined after both restarted", joined is not None)
        time.sleep(1.0)
        msgs, _, _ = face.utter("where are you", timeout=10)
        s = says(msgs)
        ok("H5e calibration came from memory", "calibration from memory" in s, s.replace("\n", " | ")[:200])
        simlog = (simdir / "log").read_text() if (simdir / "log").exists() else ""
        ok("H5f the remembered record was pushed down to the body", "ch6 center 92.5 min 30.0 max 150.0" in simlog,
           next((l for l in simlog.splitlines() if "ch6" in l), "no ch6 line in sim log")[:120])
        msgs, _, _ = face.utter("on miniblur, walk forward 1 steps", timeout=15)
        a = acts(msgs, "gait.walk")
        ok("H5g the body walks on the remembered calibration", a and a[0].get("status") == "ok", f"{a[0].get('digest') if a else None}")

        # ---- D1–D3 two bodies, one memory, and it compounds
        print("\n[D] two bodies, one memory", flush=True)
        msgs, _, _ = face.utter("write a note: the greeting dance is three steps forward, then two steps back", timeout=60)
        ok("D1 the note entered memory from the laptop body", any(x.get("status") == "ok" for x in acts(msgs, "notes.write")) or "noted" in says(msgs).lower(),
           says(msgs)[:100])
        if not API:
            skip("D2", "no ANTHROPIC_API_KEY")
            skip("D3", "no ANTHROPIC_API_KEY")
        else:
            msgs, first, total = face.utter("on miniblur, do the greeting dance", timeout=120)
            walks = [x for x in acts(msgs, "gait.walk") if x.get("body") == "miniblur"]
            targets = [x.get("target") for x in walks]
            ok("D2 the robot's action reflects the laptop's memory (fwd 3, back 2)",
               len(walks) >= 2 and any(t.startswith("fwd 3") for t in targets) and any(t.startswith("back 2") for t in targets) and all(x.get("status") == "ok" for x in walks),
               f"targets={targets} tiers={[x.get('tier') for x in walks]} total {total:.0f} ms")
            msgs2, _, total2 = face.utter("on miniblur, do the greeting dance", timeout=60)
            walks2 = [x for x in acts(msgs2, "gait.walk") if x.get("body") == "miniblur"]
            ok("D3 the dance replays as a signed skill, every step re-gated",
               len(walks2) >= 2 and all(x.get("tier") == "skill" and x.get("status") == "ok" for x in walks2),
               f"tiers={[x.get('tier') for x in walks2]} total {total2:.0f} ms (was {total:.0f})")

        # ---- H6 the body leaves
        print("\n[H6] the body leaves", flush=True)
        sim.send_signal(signal.SIGTERM)
        left = face.wait_for(lambda m: m.get("t") == "body" and m.get("name") == "miniblur" and not m.get("joined"), 15)
        ok("H6a face told 'miniblur' left", left is not None)
        msgs, _, _ = face.utter("where are you", timeout=10)
        ok("H6b body.list no longer lists it or its policy", "miniblur" not in says(msgs) and "bound by" not in says(msgs), says(msgs)[:80])
        msgs, _, _ = face.utter("on miniblur, stand up", timeout=30)
        ok("H6c nothing reaches it any more", not any(x.get("body") == "miniblur" and x.get("status") == "ok" for x in acts(msgs)))

    finally:
        for p in procs:
            try:
                p.send_signal(signal.SIGTERM)
            except Exception:
                pass
        time.sleep(0.5)
        for p in procs:
            try:
                p.kill()
            except Exception:
                pass

    print("\n=== SUMMARY ===")
    fails = [r for r in et.results if r[1] is False]
    skips = [r for r in et.results if r[1] is None]
    print(f"  {len([r for r in et.results if r[1]])} passed, {len(fails)} failed, {len(skips)} skipped")
    for n, _, d in fails:
        print(f"  FAIL {n}: {d}")
    print(f"  logs: {et.SCRATCH}")
    sys.exit(1 if fails else 0)


if __name__ == "__main__":
    main()
