#!/usr/bin/env python3
"""
continuity_test.py -- automated proof that the same BrAIn Key entity
survives a body swap: boot Body A, teach it a unique fact (written to the
on-key notebook), cleanly release it, boot Body B off the SAME key image,
and confirm it wakes with more moments than before and can recall the
fact. Then reverse direction. Repeats for N round trips.

This never touches real hardware and never runs both bodies at once (the
launcher's flock on the key image would refuse that anyway). Verification
reads ONLY the body_daemon log -- deterministic lines the core itself
speaks (the wake line's moment-count, and the grounded prompt's "the real
result was: ..." line, which carries the raw notebook digest verbatim
before any LLM paraphrasing) -- so pass/fail never depends on how the
model happens to phrase a reply.

Usage: tools/continuity_test.py [--cycles N]
"""
import argparse
import os
import re
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, HERE)
from qmp_client import QMP  # noqa: E402

WAKE_RE = re.compile(r"i remember (\d+) moments from before")
RESULT_RE = re.compile(r"the real result was: ([^.]*)\.")


class Body:
    def __init__(self, label):
        self.label = label
        self.daemon_log = f"{ROOT}/workspace/logs/body_daemon_{label}.log"
        self.qmp_sock = f"/tmp/brainos_qmp_{label}.sock"
        self.proc = None

    def boot(self, timeout=180):
        try:
            os.remove(self.daemon_log)
        except FileNotFoundError:
            pass
        self.proc = subprocess.Popen(
            ["bash", f"{ROOT}/tools/run_instance.sh", self.label, "--headless"],
            cwd=ROOT,
        )
        t0 = time.time()
        self._wait_for(r"\[mind\] ready", timeout=timeout, t0=t0)
        moments = self._wait_for(WAKE_RE, timeout=timeout, t0=t0)
        elapsed = time.time() - t0
        n = int(moments.group(1)) if moments else 0
        print(f"[test] {self.label} woke: {n} moments ({elapsed:.1f}s boot)")
        return n

    def _wait_for(self, pattern, timeout, t0):
        rx = re.compile(pattern) if isinstance(pattern, str) else pattern
        while time.time() - t0 < timeout:
            if self.proc.poll() is not None:
                raise RuntimeError(f"{self.label}: qemu/launcher exited early "
                                    f"(code {self.proc.returncode}), see {self.daemon_log}")
            if os.path.exists(self.daemon_log):
                text = open(self.daemon_log, errors="replace").read()
                m = rx.search(text)
                if m:
                    return m
            time.sleep(1)
        return None

    def qmp(self, timeout=15):
        last_err = None
        t0 = time.time()
        while time.time() - t0 < timeout:
            try:
                return QMP(self.qmp_sock)
            except (FileNotFoundError, ConnectionError, OSError) as e:
                last_err = e
                time.sleep(1)
        raise RuntimeError(f"{self.label}: could not reach QMP socket: {last_err}")

    def say(self, text):
        q = self.qmp()
        q.type_text(text)
        q.press_enter()
        q.close()

    def wait_result_contains(self, needle, timeout=60):
        t0 = time.time()
        while time.time() - t0 < timeout:
            if os.path.exists(self.daemon_log):
                text = open(self.daemon_log, errors="replace").read()
                for m in RESULT_RE.finditer(text):
                    if needle in m.group(1):
                        return True
            time.sleep(1)
        return False

    def release(self, timeout=60):
        q = self.qmp()
        q.press_esc()
        q.close()
        t0 = time.time()
        while time.time() - t0 < timeout:
            if self.proc.poll() is not None:
                print(f"[test] {self.label} released cleanly ({time.time()-t0:.1f}s)")
                return True
            time.sleep(1)
        print(f"[test] {self.label} did NOT exit within {timeout}s of ESC", file=sys.stderr)
        return False


def cleanup():
    # bracket trick ([b]ody_daemon.py / [B]rAInOS) so the pattern doesn't
    # match this very pkill invocation's own argv (pkill -f self-match).
    subprocess.run(["pkill", "-f", "[b]ody_daemon.py"], stderr=subprocess.DEVNULL)
    subprocess.run(["pkill", "-f", "[B]rAInOS -- Body"], stderr=subprocess.DEVNULL)
    time.sleep(2)


def run_cycle(n, results):
    codeword = f"falcon{n:03d}{int(time.time()) % 100000}"
    fact = f"write down the launch code is {codeword}"

    a = Body("A")
    n_before = a.boot()
    a.say(fact)
    ok_write = a.wait_result_contains(codeword, timeout=60)
    results.append((f"cycle{n}: A learns fact", ok_write))
    a.release()

    b = Body("B")
    n_mid = b.boot()
    results.append((f"cycle{n}: B inherited >= A's count ({n_mid} >= {n_before})",
                     n_mid >= n_before))
    b.say(f"what do your notes say about the launch code")
    ok_recall_b = b.wait_result_contains(codeword, timeout=60)
    results.append((f"cycle{n}: B recalls fact from A", ok_recall_b))
    b.release()

    a2 = Body("A")
    n_final = a2.boot()
    results.append((f"cycle{n}: A rebooted with >= B's count ({n_final} >= {n_mid})",
                     n_final >= n_mid))
    a2.say("what do your notes say about the launch code")
    ok_recall_a = a2.wait_result_contains(codeword, timeout=60)
    results.append((f"cycle{n}: A still recalls fact after B's session", ok_recall_a))
    a2.release()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--cycles", type=int, default=2)
    args = ap.parse_args()

    cleanup()
    results = []
    try:
        for n in range(1, args.cycles + 1):
            print(f"\n=== cycle {n}/{args.cycles} ===")
            run_cycle(n, results)
    finally:
        cleanup()

    print("\n=== RESULTS ===")
    ok = True
    for name, passed in results:
        print(f"  [{'PASS' if passed else 'FAIL'}] {name}")
        ok = ok and passed
    print("\nALL PASS" if ok else "\nFAILURES PRESENT")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
