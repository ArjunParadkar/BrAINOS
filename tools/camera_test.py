#!/usr/bin/env python3
"""
camera_test.py -- live proof of the camera limb through a REAL booted body
and the REAL daemon: an image at the lens jack -> local motion tier -> VS
afferent -> the core absorbs it as reflex-grade experience; and a gated
vision.look -> AX -> the deliberate tier -> a real description -> AR ->
a StateNode the entity speaks from.

This is the camera's equivalent of voice_test.py, and it deliberately
tests the EMPTY case first. The honesty rule for this limb is not that it
describes well -- it is that when there is nothing at the lens, or the
rich tier can't answer, the entity says so instead of inventing a scene.
A camera that confabulates is worse than no camera, so the no-frame path
is a first-class pass criterion rather than an edge case.

Frames are generated locally (PIL) -- no real camera is ever opened, by
this test or by the daemon. The jack is the boundary.

Pass criteria:
  1. with NO frame present, vision.look answers honestly ("nothing at my
     lens") and never describes anything;
  2. a frame arriving at the jack produces a VS afferent (local tier) and
     the core prints "saw:" -- reflex-grade, with NO escalation;
  3. a SECOND, visually different frame is detected as movement;
  4. a near-identical frame is NOT reported as movement (cheap by default:
     a still view must cost nothing);
  5. vision.look with a frame present returns a real, non-empty digest;
     when ANTHROPIC_API_KEY is set this is a genuine model description,
     and the test asserts it actually mentions what was drawn;
  6. once the lens goes dark, the last frame is still described -- but
     dated, never as the present moment.

Usage: tools/camera_test.py          (needs GPU for the daemon's mind)
"""
import glob
import os
import re
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, HERE)
from qmp_client import QMP  # noqa: E402

BODY = "A"
CAM_IN = f"{ROOT}/workspace/vm_video_{BODY}/cam_in"
DLOG = f"{ROOT}/workspace/logs/body_daemon_{BODY}.log"
CONSOLE = f"{ROOT}/workspace/logs/console_{BODY}.log"
QMP_SOCK = f"/tmp/brainos_qmp_{BODY}.sock"

results = []


def check(name, ok, detail=""):
    results.append((name, ok))
    print(f"  [{'PASS' if ok else 'FAIL'}] {name}"
          + (f"  -- {detail}" if detail and not ok else ""))


def cleanup():
    subprocess.run(["pkill", "-f", "[b]ody_daemon.py"], stderr=subprocess.DEVNULL)
    subprocess.run(["pkill", "-f", "[B]rAInOS -- Body"], stderr=subprocess.DEVNULL)
    time.sleep(2)


def draw(path, kind):
    """Make a frame. Deliberately simple, high-contrast shapes so a real
    vision model has something unambiguous to name."""
    from PIL import Image, ImageDraw
    im = Image.new("RGB", (320, 240), (245, 245, 245))
    d = ImageDraw.Draw(im)
    if kind == "circle":
        d.ellipse([90, 60, 230, 200], fill=(200, 30, 30))
    elif kind == "circle_nudged":     # visually near-identical to 'circle'
        d.ellipse([92, 62, 232, 202], fill=(200, 30, 30))
    elif kind == "bars":              # very different: should read as motion
        for i in range(0, 320, 40):
            d.rectangle([i, 0, i + 20, 240], fill=(20, 20, 160))
    im.save(path)


def wait_log(pattern, timeout, path=DLOG, t0=None):
    rx = re.compile(pattern)
    t0 = t0 or time.time()
    while time.time() - t0 < timeout:
        if os.path.exists(path):
            for line in open(path, errors="replace"):
                m = rx.search(line)
                if m:
                    return m
        time.sleep(0.5)
    return None


def say_to_core(text):
    """Speak to the entity the way the mic would, without needing audio:
    the daemon's own HB path is what a transcript takes, so we drive the
    core through QMP typing instead -- same utterance handling."""
    q = QMP(QMP_SOCK)
    q.type_text(text)
    q.press_enter()
    q.close()


def main():
    cleanup()
    os.makedirs(CAM_IN, exist_ok=True)
    for f in glob.glob(f"{CAM_IN}/*"):
        os.remove(f)
    for p in (DLOG, CONSOLE):
        try:
            os.remove(p)
        except FileNotFoundError:
            pass

    have_key = bool(os.environ.get("ANTHROPIC_API_KEY"))
    print(f"[camera-test] booting Body A with the lens jack open "
          f"(deliberate tier: {'available' if have_key else 'ABSENT'})...")
    # No ears/voice: this test types at the core through QMP, so whisper
    # and kokoro are pure VRAM cost. On this 8GB box loading them
    # alongside the 8B mind leaves too little headroom and the mind OOMs
    # mid-turn -- which is a real constraint of the hardware, not of the
    # camera, and is reported as such rather than worked around silently.
    env = dict(os.environ, BRAINOS_CAMERA="on", BRAINOS_NO_VOICE="1")
    proc = subprocess.Popen(
        ["bash", f"{ROOT}/tools/run_instance.sh", BODY, "--headless"],
        cwd=ROOT, env=env)
    t0 = time.time()

    ready = wait_log(r"\[mind\] ready", 300, t0=t0)
    watching = wait_log(r"\[eyes\] watching the lens jack", 180)
    check("daemon booted with the camera jack open",
          bool(ready and watching),
          f"ready={bool(ready)} watching={bool(watching)}")
    if not (ready and watching):
        proc.terminate()
        cleanup()
        return report()

    # the lens must be acquired into the body map via the §8 offer
    check("camera acquired into the body map (§8 offer)",
          wait_log(r"incorporate\s+body/eyes|body/eyes", 60, path=CONSOLE)
          is not None, "no body/eyes in the console body map")

    # ---- 1. THE EMPTY CASE FIRST: nothing at the lens ----
    print("[camera-test] asking it to look with NO frame present...")
    # Phrased long enough to reach the deliberate pathway on purpose:
    # the fast local model is documented as unreliable at emitting an
    # exact verb cold, and what is under test here is the LIMB, not
    # Qwen's instruction-following.
    say_to_core("please look through your camera now and describe to me "
                "in detail whatever you can currently see in front of it")
    empty = wait_log(r"\[limb\] vision\.look -> (ok|err): (.+)", 90)
    empty_digest = empty.group(2).strip() if empty else ""
    check("vision.look answered with an empty jack", empty is not None)
    check("empty lens is reported honestly, nothing described",
          "nothing at my lens" in empty_digest.lower(),
          f"got: {empty_digest!r}")

    # ---- 2. a frame arrives: local tier -> VS -> core absorbs it ----
    print("[camera-test] dropping the first frame onto the lens jack...")
    draw(f"{CAM_IN}/frame_0001.png", "circle")
    first = wait_log(r"\[eyes\] (something is in front of my lens)", 60)
    check("local tier noticed the first frame (presence)", first is not None)
    check("core absorbed the sight as reflex-grade experience",
          wait_log(r"saw:", 60, path=CONSOLE) is not None,
          "no 'saw:' line on the console")
    # reflex-grade means NOT a deliberate thought: the sight itself must
    # not have driven an escalation
    dlog = open(DLOG, errors="replace").read()
    seg = dlog.split("something is in front of my lens", 1)[-1][:400]
    check("the sight did NOT wake deliberate cognition (cheap by default)",
          "deliberate pathway served by" not in seg,
          "an escalation followed the frame")

    # ---- 3. a very different frame reads as movement ----
    print("[camera-test] dropping a visually different frame...")
    draw(f"{CAM_IN}/frame_0002.png", "bars")
    moved = wait_log(r"\[eyes\] movement at my lens \((\d+)% of the view changed\)", 60)
    check("local tier detected movement between frames", moved is not None)

    # ---- 4. a near-identical frame must NOT cry wolf ----
    print("[camera-test] dropping a near-identical frame (should stay quiet)...")
    draw(f"{CAM_IN}/frame_0003.png", "bars")
    time.sleep(6)
    still = re.search(r"\[eyes\] frame frame_0003\.png: still",
                      open(DLOG, errors="replace").read())
    check("a still view costs nothing (no false motion report)",
          still is not None,
          "frame_0003 did not log as still")

    # ---- 5. look again, now that there IS something to see ----
    print("[camera-test] asking it to look WITH a frame present...")
    before = open(DLOG, errors="replace").read()
    say_to_core("please look through your camera again now and describe "
                "to me in detail whatever is in front of it at this moment")
    t1 = time.time()
    seen = None
    while time.time() - t1 < 120:
        now = open(DLOG, errors="replace").read()
        fresh = now[len(before):]
        m = re.search(r"\[limb\] vision\.look -> (ok|err): (.+)", fresh)
        if m:
            seen = m
            break
        time.sleep(1)
    digest = seen.group(2).strip() if seen else ""
    check("vision.look returned a result with a frame present", seen is not None,
          "no second vision.look in the log")
    check("the result is a real, non-empty digest",
          len(digest) > 10 and "nothing at my lens" not in digest.lower(),
          f"got: {digest!r}")
    if have_key:
        # a genuine description of blue vertical bars on white
        low = digest.lower()
        hit = any(w in low for w in
                  ("blue", "stripe", "bar", "vertical", "line", "band"))
        check("deliberate tier genuinely described the frame",
              hit, f"no visual content words in: {digest!r}")
        check("deliberate vision was served by a real model",
              "[eyes] vision served by" in open(DLOG, errors="replace").read(),
              "no vision-served line in the daemon log")
    else:
        check("without the deliberate tier it says so plainly, no invention",
              "needs more than my reflexes" in digest.lower(),
              f"got: {digest!r}")
        print("  [info] no ANTHROPIC_API_KEY: rich-tier description not exercised")

    # ---- 6. a lens that went dark must not pass old light off as now ----
    # The jack keeps the last frame forever, so without this the entity
    # would describe a view from an hour ago as what is in front of it at
    # this moment -- the same confabulation the empty case guards against,
    # just harder to notice. Backdating the frame is the only way to test
    # it without waiting out the threshold for real.
    print("[camera-test] backdating the frame: the lens has gone dark...")
    old = time.time() - 15 * 60
    os.utime(f"{CAM_IN}/frame_0003.png", (old, old))
    before = open(DLOG, errors="replace").read()
    say_to_core("please take another careful look through your camera and "
                "tell me in detail what is in front of it right now")
    t2 = time.time()
    aged = None
    while time.time() - t2 < 120:
        fresh = open(DLOG, errors="replace").read()[len(before):]
        m = re.search(r"\[limb\] vision\.look -> (ok|err): (.+)", fresh)
        if m:
            aged = m
            break
        time.sleep(1)
    aged_digest = aged.group(2).strip() if aged else ""
    check("vision.look answered on a stale frame", aged is not None)
    check("a stale view is dated, never passed off as the present moment",
          "nothing new has reached my lens" in aged_digest.lower()
          and "15 minutes ago" in aged_digest.lower(),
          f"got: {aged_digest!r}")

    print(f"  [info] empty-jack digest: {empty_digest!r}")
    print(f"  [info] with-frame digest: {digest!r}")
    print(f"  [info] stale-frame digest: {aged_digest!r}")

    try:
        q = QMP(QMP_SOCK)
        q.press_esc()
        q.close()
    except Exception:  # noqa: BLE001
        pass
    time.sleep(3)
    proc.terminate()
    cleanup()
    return report()


def report():
    print("\n=== camera-test RESULTS ===")
    ok = all(p for _, p in results)
    for name, passed in results:
        print(f"  [{'PASS' if passed else 'FAIL'}] {name}")
    print("\nCAMERA: ALL PASS" if ok else "\nCAMERA: FAILURES PRESENT")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
