#!/usr/bin/env python3
"""
crash_test.py -- Stage 1.4: kill power mid-shutdown, repeatedly, and prove
the entity's memory survives every time.

The host-side suite (mind/tests/journal_crash.rs) already proves the
journal survives a torn write at EVERY byte boundary, exhaustively. What
it cannot prove is the integration: that the real core, through the real
UEFI FAT driver, actually writes valid journal records, alternates slots,
and rehydrates the newest valid self at next boot. This test does that
with a real power cut: SIGKILL to QEMU at varying delays after ESC
(release-body), which lands before, during, or after the persist write.
The key drive is attached cache=writethrough so a UEFI write that the
core completed is on the host image when the power dies -- the honest
equivalent of pulling the plug on a USB stick.

Invariant checked after every kill: the key image still yields a valid
committed self (newest journal slot, or the legacy file if no slot ever
committed), and it is either the previous self or the new one -- never
garbage, never nothing. Loss is bounded to "since the last commit", by
design; losing the PREVIOUS self is the failure this test exists to catch.

Usage: tools/crash_test.py [iterations]     (default 8)
Assumes ./build.sh has run (key image exists). Uses its own copy of the
key image -- the real brainos-key.img is never touched.
"""
import os
import re
import shutil
import signal
import socket
import struct
import subprocess
import sys
import threading
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, HERE)
from make_key import journal_open, newest_memory, read_fat16_file  # noqa: E402
from qmp_client import QMP  # noqa: E402

QLOCAL = f"{ROOT}/tools/qemu-local"
KEY_IMG = "/tmp/brainos_crash_test_key.img"
SOCK = "/tmp/brainos_crash_test.sock"
QMP_SOCK = "/tmp/brainos_crash_test_qmp.sock"
VARS = "/tmp/brainos_crash_test_vars.fd"
LOGDIR = f"{ROOT}/workspace/logs"


class TalkOnce(threading.Thread):
    """Minimal scripted daemon: handshake, say one thing, then idle.
    Sets `spoke` once the core has consolidated the utterance turn."""

    def __init__(self, codeword):
        super().__init__(daemon=True)
        self.codeword = codeword
        self.awake = threading.Event()
        self.spoke = threading.Event()
        self.err = None
        self._sock = None
        self._buf = b""

    def _send(self, line):
        self._sock.sendall(line + b"\n")

    def _handle(self, prompt):
        if prompt == "__hello__":
            self._send(b"MM!ready (crash test)")
            self._send(b"LM+mic|body/ears|sense.hearing|crash-test jack")
        elif prompt == "__wake__":
            self._send(b"MM!awake and listening.")
            self.awake.set()
            self._send(b"HB" + f"remember the codeword {self.codeword}".encode())
        elif self.codeword in prompt:
            self._send(b"MM!" + f"noted: {self.codeword}.".encode())
            self.spoke.set()
        else:
            # dream prompts, grounded turns, anything else: plain prose
            self._send(b"MM!noted.")

    def run(self):
        try:
            for _ in range(150):
                try:
                    self._sock = socket.socket(socket.AF_UNIX)
                    self._sock.connect(SOCK)
                    break
                except (FileNotFoundError, ConnectionRefusedError):
                    time.sleep(0.1)
            else:
                self.err = "could not connect to link socket"
                return
            self._sock.settimeout(90)
            while True:
                try:
                    data = self._sock.recv(4096)
                except (socket.timeout, OSError):
                    return
                if not data:
                    return
                self._buf += data
                while b"\n" in self._buf:
                    line, self._buf = self._buf.split(b"\n", 1)
                    line = line.strip()
                    if line.startswith(b"MM?"):
                        self._handle(line[3:].decode("utf-8", "ignore"))
        except Exception as e:  # noqa: BLE001
            self.err = repr(e)


def boot_qemu(console_log):
    for p in (SOCK, QMP_SOCK):
        try:
            os.remove(p)
        except FileNotFoundError:
            pass
    shutil.copyfile(f"{QLOCAL}/usr/share/edk2/x64/OVMF_VARS.4m.fd", VARS)
    env = dict(os.environ)
    env["LD_LIBRARY_PATH"] = f"{QLOCAL}/usr/lib"
    env["QEMU_MODULE_DIR"] = f"{QLOCAL}/usr/lib/qemu"
    cmd = [
        f"{QLOCAL}/usr/bin/qemu-system-x86_64",
        "-enable-kvm", "-m", "1G", "-cpu", "host",
        "-L", f"{QLOCAL}/usr/share/qemu",
        "-drive", f"if=pflash,format=raw,readonly=on,"
                  f"file={QLOCAL}/usr/share/edk2/x64/OVMF_CODE.4m.fd",
        "-drive", f"if=pflash,format=raw,file={VARS}",
        # writethrough: a write the core completed is ON THE IMAGE when
        # the power dies -- the honest model of a USB stick
        "-drive", f"format=raw,file={KEY_IMG},cache=writethrough",
        "-chardev", f"socket,id=mm,path={SOCK},server=on,wait=off",
        "-device", "isa-serial,chardev=mm,index=2",
        "-serial", f"file:{console_log}",
        "-qmp", f"unix:{QMP_SOCK},server,nowait",
        "-device", "qemu-xhci", "-device", "usb-tablet",
        "-nic", "none", "-display", "none",
    ]
    return subprocess.Popen(cmd, env=env,
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def committed_self():
    """The self the entity would wake as, from the image on disk."""
    return newest_memory(KEY_IMG, 2048, "EPI", "EPISODES.LOG")


def slot_state():
    out = []
    for s in ("A", "B"):
        raw = read_fat16_file(KEY_IMG, 2048, "BRAIN", f"EPI_{s}.JNL")
        rec = journal_open(raw) if raw else None
        out.append(f"{s}:gen{rec[0]}" if rec else f"{s}:{'torn' if raw else 'empty'}")
    return " ".join(out)


def one_session(tag, kill_after_esc, attempts=2):
    """Boot, converse once, ESC. kill_after_esc=None -> clean shutdown;
    a float -> SIGKILL that many seconds after ESC. Returns codeword.
    A boot that never reaches conversation (rare flaky firmware start) is
    retried once -- it dies before any write, so it cannot affect state."""
    for attempt in range(attempts):
        try:
            return _one_session(tag, kill_after_esc)
        except RuntimeError as e:
            if attempt + 1 == attempts:
                raise
            print(f"  (boot never reached conversation, retrying: {e})")


def _one_session(tag, kill_after_esc):
    codeword = f"{tag}{int(time.time() * 1000) % 100000}"
    log = f"{LOGDIR}/crash_test_{tag}.log"
    daemon = TalkOnce(codeword)
    qemu = boot_qemu(log)
    daemon.start()
    try:
        if not daemon.awake.wait(60) or not daemon.spoke.wait(30):
            raise RuntimeError(f"session never reached conversation ({daemon.err})")
        time.sleep(1.0)  # let consolidation land before releasing the body
        q = QMP(QMP_SOCK)
        q.hmp("sendkey esc")
        q.close()
        if kill_after_esc is None:
            qemu.wait(timeout=60)  # clean: core shuts the machine down
        else:
            time.sleep(kill_after_esc)
            qemu.kill()            # the power cut
            qemu.wait(timeout=10)
    finally:
        if qemu.poll() is None:
            qemu.kill()
            qemu.wait(timeout=10)
    return codeword


def main():
    iterations = int(sys.argv[1]) if len(sys.argv) > 1 else 8
    os.makedirs(LOGDIR, exist_ok=True)
    if not os.path.exists(f"{ROOT}/brainos-key.img"):
        sys.exit("no brainos-key.img -- run ./build.sh first")
    shutil.copyfile(f"{ROOT}/brainos-key.img", KEY_IMG)

    failures = []

    def check(name, ok, detail=""):
        print(f"  [{'PASS' if ok else 'FAIL'}] {name}" + (f"  ({detail})" if detail else ""))
        if not ok:
            failures.append(name)

    # ---- seed: one clean cycle proves the journaled persist end to end
    print("[crash-test] seed session (clean release)...")
    seed_word = one_session("seed", None)
    seeded = committed_self()
    check("clean release commits a valid journal record",
          seeded is not None and b"episode|" in seeded, slot_state())
    check("the session's own episode is in the committed self",
          seeded is not None and seed_word.encode() in seeded)

    # ---- the power cuts: sweep the window from before to after persist
    # measured: persist lands >0.5s after ESC (dream round-trip first), so
    # sweep from clearly-before through the write window to clearly-after
    delays = [0.0, 0.7, 0.9, 1.1, 1.3, 1.6, 2.0, 2.5, 3.2, 4.5]
    prev_self = seeded
    survived_all = True
    outcomes = []
    for i in range(iterations):
        delay = delays[i % len(delays)]
        word = one_session(f"kill{i}", delay)
        now = committed_self()
        if now is None:
            survived_all = False
            print(f"  [FAIL] iteration {i} (kill @ {delay}s): NO committed self "
                  f"survived -- {slot_state()}")
            failures.append(f"iteration {i}")
            prev_self = now
            continue
        if word.encode() in now:
            outcome = "committed-new"
        elif now == prev_self:
            outcome = "kept-previous"
        else:
            # a valid self that is neither previous nor new = corruption
            survived_all = False
            outcome = "CORRUPT"
            failures.append(f"iteration {i} corrupt")
        outcomes.append(outcome)
        print(f"  kill @ {delay:>4}s after ESC -> {outcome:<14} {slot_state()}")
        prev_self = now
    check("every power cut left a valid committed self", survived_all,
          f"{outcomes.count('committed-new')} committed, "
          f"{outcomes.count('kept-previous')} kept previous")

    # ---- final: the survivor actually rehydrates in a real boot
    print("[crash-test] final session (verify rehydration)...")
    final_word = one_session("final", None)
    final_self = committed_self()
    check("final clean session commits on top of the survivors",
          final_self is not None and final_word.encode() in final_self,
          slot_state())
    log = open(f"{LOGDIR}/crash_test_final.log", "rb").read().decode("utf-8", "ignore")
    m = re.search(r"(\d+) memories carried over|inherited", log)
    check("final boot rehydrated inherited memories",
          (b"episode|" in (final_self or b"")) and
          ("carried" in log or "inherited" in log or "memories" in log))

    print()
    if failures:
        print(f"[crash-test] FAILED: {failures}")
        sys.exit(1)
    print("[crash-test] ALL CHECKS PASSED -- memory survives the power cord")


if __name__ == "__main__":
    main()
