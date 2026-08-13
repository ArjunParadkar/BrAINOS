#!/usr/bin/env python3
"""
voice_test.py -- live proof of the full voice loop through a real booted
body: mic (a wav at the ear jack) -> Whisper STT -> HB -> core -> mind ->
SP -> Kokoro TTS -> a wav at the speaker jack. No keyboard, no cloud brain
required (Qwen alone can answer); this exercises the audio path itself.

The mic utterance is synthesized with piper on CPU so it doesn't contend
with the daemon's GPU (whisper + kokoro + the 8B mind). The wav is dropped
into mic_in ONLY AFTER the daemon prints it's watching the ear jack --
files present before that are folded into its "already seen" set and
ignored, so early drops would silently no-op.

Pass criteria:
  1. mic -> whisper: the daemon logs "[ears] heard ...: <transcript>" and
     the transcript resembles what we spoke (not an empty/garbage hallucination);
  2. voice out: a NEW wav appears in speaker_out after the utterance (the
     core's spoken reply, synthesized by Kokoro) -- beyond the wake greeting.

Usage: tools/voice_test.py
"""
import glob
import os
import re
import shutil
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, HERE)
from qmp_client import QMP  # noqa: E402

BODY = "A"
AUDIO_DIR = f"{ROOT}/workspace/vm_audio_{BODY}"
MIC_IN = f"{AUDIO_DIR}/mic_in"
SPK = f"{AUDIO_DIR}/speaker_out"
DLOG = f"{ROOT}/workspace/logs/body_daemon_{BODY}.log"
QMP_SOCK = f"/tmp/brainos_qmp_{BODY}.sock"
PIPER = f"{ROOT}/tools/piper/piper/piper"
PIPER_MODEL = f"{ROOT}/tools/piper/en_US-ryan-high.onnx"

PHRASE = "hello there. can you hear me clearly? please tell me your name."
HEARD_RE = re.compile(r"\[ears\] heard \([\d.]+s\): (.+)")


def cleanup():
    subprocess.run(["pkill", "-f", "[b]ody_daemon.py"], stderr=subprocess.DEVNULL)
    subprocess.run(["pkill", "-f", "[B]rAInOS -- Body"], stderr=subprocess.DEVNULL)
    time.sleep(2)


def synth(text, out):
    env = dict(os.environ)
    env["LD_LIBRARY_PATH"] = os.path.dirname(PIPER) + ":" + env.get("LD_LIBRARY_PATH", "")
    subprocess.run([PIPER, "--model", PIPER_MODEL, "--output_file", out],
                   input=text.encode(), env=env,
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=True)


def wait_for_log(pattern, timeout, t0=None):
    rx = re.compile(pattern) if isinstance(pattern, str) else pattern
    t0 = t0 or time.time()
    while time.time() - t0 < timeout:
        if os.path.exists(DLOG):
            for line in open(DLOG, errors="replace"):
                m = rx.search(line)
                if m:
                    return m
        time.sleep(1)
    return None


def main():
    cleanup()
    os.makedirs(MIC_IN, exist_ok=True)
    os.makedirs(SPK, exist_ok=True)
    # start from a clean slate so wav counts are unambiguous
    for f in glob.glob(f"{MIC_IN}/*.wav") + glob.glob(f"{SPK}/*.wav"):
        os.remove(f)
    try:
        os.remove(DLOG)
    except FileNotFoundError:
        pass

    print("[voice-test] synthesizing mic utterance (piper, CPU)...")
    mic_wav = "/tmp/voice_test_mic.wav"
    synth(PHRASE, mic_wav)

    print("[voice-test] booting Body A (daemon: whisper + Kokoro + Qwen)...")
    proc = subprocess.Popen(["bash", f"{ROOT}/tools/run_instance.sh", BODY, "--headless"],
                            cwd=ROOT)
    t0 = time.time()

    results = []

    def check(name, ok, detail=""):
        results.append((name, ok))
        print(f"  [{'PASS' if ok else 'FAIL'}] {name}" + (f"  -- {detail}" if detail and not ok else ""))

    ready = wait_for_log(r"\[mind\] ready", 240, t0)
    listening = wait_for_log(r"\[ears\] listening on the ear jack", 120)
    if not (ready and listening):
        check("daemon booted (mind + ears ready)", False,
              f"ready={bool(ready)} listening={bool(listening)}")
        proc.terminate()
        cleanup()
        return _report(results)
    check("daemon booted (mind + ears ready)", True)

    # let the wake greeting settle, then note the speaker wavs that exist so
    # far (the greeting itself makes one) before we speak.
    time.sleep(3)
    spk_before = set(glob.glob(f"{SPK}/*.wav"))

    print("[voice-test] dropping the utterance onto the ear jack...")
    shutil.copyfile(mic_wav, f"{MIC_IN}/utt_0001.wav")

    heard = wait_for_log(HEARD_RE, 60)
    transcript = heard.group(1).strip() if heard else ""
    # a real transcription of our phrase should contain recognizable words,
    # not be empty or a stock whisper hallucination.
    words = {"hello", "hear", "name", "there", "clearly", "tell", "you"}
    hit = sum(1 for w in words if w in transcript.lower())
    check("mic -> whisper: utterance transcribed (HB)", heard is not None,
          "no [ears] heard line")
    check(f"transcript resembles what was spoken ({hit}/7 keywords)",
          heard is not None and hit >= 2, f"got: {transcript!r}")

    # a new speaker_out wav after we spoke == the core's reply, spoken by Kokoro
    new_wav = None
    t1 = time.time()
    while time.time() - t1 < 90:
        now = set(glob.glob(f"{SPK}/*.wav"))
        fresh = now - spk_before
        if fresh:
            new_wav = sorted(fresh)[0]
            break
        time.sleep(1)
    check("voice out: Kokoro produced a spoken reply wav", new_wav is not None,
          "no new speaker_out wav")
    if new_wav:
        try:
            import wave
            w = wave.open(new_wav)
            dur = w.getnframes() / w.getframerate()
            check("reply wav is real audio (>0.3s)", dur > 0.3, f"{dur:.2f}s")
            print(f"  [info] reply wav {os.path.basename(new_wav)}: {dur:.2f}s; "
                  f"heard: {transcript!r}")
        except Exception as e:  # noqa: BLE001
            check("reply wav is real audio (>0.3s)", False, repr(e))

    # release the body cleanly (ESC), then clean up
    try:
        q = QMP(QMP_SOCK)
        q.press_esc()
        q.close()
    except Exception:
        pass
    time.sleep(3)
    proc.terminate()
    cleanup()
    return _report(results)


def _report(results):
    print("\n=== voice-test RESULTS ===")
    ok = all(p for _, p in results)
    for name, passed in results:
        print(f"  [{'PASS' if passed else 'FAIL'}] {name}")
    print("\nVOICE: ALL PASS" if ok else "\nVOICE: FAILURES PRESENT")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
