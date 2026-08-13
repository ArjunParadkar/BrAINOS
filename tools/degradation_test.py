#!/usr/bin/env python3
"""
degradation_test.py -- Stage 1.3: every dependency failure fails SAFE and
HONEST, against the real core in a real VM.

Scenarios (each an isolated boot on a throwaway copy of the key image):

  1. cloud brain unreachable MID-SESSION -- the daemon answers the wake
     handshake then goes silent. The entity must fall to the Domain-2
     reflex reply, say so honestly ("(reflex)"), and keep running.
  2. world disk absent -- a proposed fs.list has no limb to run through.
     KIRA must deny at authz and the entity must refuse honestly,
     never pretending it has a disk.
  3. corrupted key material -- SEED.HEX tampered so it derives a
     different identity. The core must refuse to wake AS ANYONE and shut
     the body down: no identity, no entity.
  4. key medium removed mid-session (drive_del = pulling the stick) --
     the notebook write must fail honestly ("the key would not take the
     ink"), and the release-time persist must degrade honestly
     ("the firmware would not let me write"), never claim success.

Usage: tools/degradation_test.py [--skip-slow]
  --skip-slow skips scenario 1 (it waits out the 60s link timeout).
"""
import os
import re
import shutil
import socket
import subprocess
import sys
import threading
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, HERE)
from qmp_client import QMP  # noqa: E402

QLOCAL = f"{ROOT}/tools/qemu-local"
KEY_IMG = "/tmp/brainos_degradation_key.img"
SOCK = "/tmp/brainos_degradation.sock"
QMP_SOCK = "/tmp/brainos_degradation_qmp.sock"
VARS = "/tmp/brainos_degradation_vars.fd"
LOGDIR = f"{ROOT}/workspace/logs"

failures = []


def check(name, ok, detail=""):
    print(f"  [{'PASS' if ok else 'FAIL'}] {name}" + (f"  ({detail})" if detail else ""))
    if not ok:
        failures.append(name)


class Daemon(threading.Thread):
    """Scripted body daemon with per-scenario behavior.
    mode 'mute_after_wake': answers the handshake, then silence forever.
    mode 'script': answers handshake, sends `heard`, answers the resulting
    think-request with `proposal` (a '~~verb|target' suffix or plain
    prose), then answers everything else with prose."""

    def __init__(self, mode, heard=None, proposal=None):
        super().__init__(daemon=True)
        self.mode = mode
        self.heard = heard
        self.proposal = proposal
        self.awake = threading.Event()
        self.proposed = threading.Event()
        self.err = None
        self._sock = None
        self._buf = b""

    def _send(self, line):
        self._sock.sendall(line + b"\n")

    def _handle(self, prompt):
        if prompt == "__hello__":
            self._send(b"MM!ready (degradation test)")
            self._send(b"LM+mic|body/ears|sense.hearing|test jack")
        elif prompt == "__wake__":
            self._send(b"MM!awake.")
            self.awake.set()
            if self.mode == "script" and self.heard:
                self._send(b"HB" + self.heard.encode())
        elif self.mode == "mute_after_wake":
            pass  # the cloud brain has gone dark: never answer again
        elif self.mode == "script" and not self.proposed.is_set():
            self.proposed.set()
            self._send(b"MM!" + (self.proposal or "mm-hmm.").encode())
        else:
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
                self.err = "could not connect"
                return
            self._sock.settimeout(150)
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


def boot_qemu(console_log, key_img=KEY_IMG):
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
        "-drive", f"id=brainkey,format=raw,file={key_img},cache=writethrough",
        "-chardev", f"socket,id=mm,path={SOCK},server=on,wait=off",
        "-device", "isa-serial,chardev=mm,index=2",
        "-serial", f"file:{console_log}",
        "-qmp", f"unix:{QMP_SOCK},server,nowait",
        "-device", "qemu-xhci", "-device", "usb-tablet",
        "-nic", "none", "-display", "none",
    ]
    return subprocess.Popen(cmd, env=env,
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def end_session(qemu, clean=True):
    try:
        if clean and qemu.poll() is None:
            q = QMP(QMP_SOCK)
            q.hmp("sendkey esc")
            q.close()
            qemu.wait(timeout=60)
    except Exception:  # noqa: BLE001
        pass
    finally:
        if qemu.poll() is None:
            qemu.kill()
            qemu.wait(timeout=10)


def read_log(path):
    time.sleep(1.0)
    try:
        return open(path, "rb").read().decode("utf-8", "ignore")
    except FileNotFoundError:
        return ""


def scenario_link_dead():
    print("[degradation] 1. cloud brain goes dark mid-session (waits out 60s timeout)...")
    log = f"{LOGDIR}/degradation_linkdead.log"
    d = Daemon("mute_after_wake")
    qemu = boot_qemu(log)
    d.start()
    ok_boot = d.awake.wait(90)
    if ok_boot:
        # speak to the entity while its mind is unreachable
        d._send(b"HBare you still there?")
        # the core's 60s link timeout starts only after the wake sequence
        # drains its pending queue, so the reflex legitimately lands ~75s
        # after the HB; 150s also rides out slow boots without flaking
        deadline = time.time() + 150
        text = ""
        while time.time() < deadline:
            text = read_log(log)
            if "(reflex)" in text:
                break
            time.sleep(3)
    end_session(qemu, clean=False)
    text = read_log(log)
    check("boot reached conversation", ok_boot)
    check("reflex reply while the mind is dark", "(reflex)" in text)
    # NB: no bare "reflex" here — the boot banner's "reflex load: minimal"
    # would satisfy it even when no reflex reply ever happened
    check("the reflex is honest about being a reflex",
          re.search(r"reflexes only right now|cognition will catch up|"
                    r"link is quiet|hold that thought|mind is back online",
                    text) is not None)


def scenario_no_world_disk():
    print("[degradation] 2. world disk absent, fs.list proposed anyway...")
    log = f"{LOGDIR}/degradation_noworld.log"
    d = Daemon("script", heard="list my files please",
               proposal="let me look~~fs.list|")
    qemu = boot_qemu(log)
    d.start()
    d.awake.wait(90)
    d.proposed.wait(30)
    time.sleep(8)  # give KIRA + the spoken refusal time to land
    end_session(qemu)
    text = read_log(log)
    check("KIRA denied fs.list at authz", "no limb advertises 'fs.list'" in text)
    check("entity refused honestly, no pretend disk",
          "i can't do that" in text and "i won't pretend" in text)
    check("no fake listing was spoken", "is empty" not in text and "the top of my disk" not in text)


def scenario_corrupt_identity():
    print("[degradation] 3. tampered SEED.HEX (derives a different identity)...")
    log = f"{LOGDIR}/degradation_badseed.log"
    img = "/tmp/brainos_degradation_badseed.img"
    shutil.copyfile(f"{ROOT}/brainos-key.img", img)
    # the seed rides the ESP as ASCII hex; flip one hex digit in place
    import json
    seed_hex = json.load(open(f"{ROOT}/key/brain_key.json"))["private_seed"]
    raw = open(img, "rb").read()
    pos = raw.find(seed_hex.encode())
    assert pos > 0, "seed hex not found in image"
    flipped = ("0" if seed_hex[0] != "0" else "1") + seed_hex[1:]
    patched = raw[:pos] + flipped.encode() + raw[pos + len(seed_hex):]
    open(img, "wb").write(patched)

    qemu = boot_qemu(log, key_img=img)
    try:
        qemu.wait(timeout=90)  # the core must shut the body down itself
        shutdown_ok = True
    except subprocess.TimeoutExpired:
        shutdown_ok = False
    end_session(qemu, clean=False)
    text = read_log(log)
    check("core refused to wake as anyone",
          "no identity" in text and "seed does not derive" in text)
    check("body shut down, not left running as an impostor", shutdown_ok)
    check("no domain bring-up happened", "DOMAIN 3" not in text)
    os.remove(img)


def scenario_key_removed():
    print("[degradation] 4. key medium removed mid-session...")
    log = f"{LOGDIR}/degradation_keypull.log"
    d = Daemon("script", heard="write a note that the sky is clear",
               proposal="noting that~~notes.write|the sky is clear tonight")
    qemu = boot_qemu(log)
    d.start()
    d.awake.wait(90)
    # let the first notebook write SUCCEED (proves the baseline)...
    d.proposed.wait(30)
    time.sleep(8)
    # ...then pull the stick
    q = QMP(QMP_SOCK)
    q.hmp("drive_del brainkey")
    # a second note now has nowhere to land
    d.proposed.clear()
    d._send(b"HBwrite another note please")
    time.sleep(2)
    d._send(b"MM!writing~~notes.write|a second note after the pull")
    time.sleep(10)
    q.hmp("sendkey esc")  # release: the persist must also degrade honestly
    q.close()
    try:
        qemu.wait(timeout=60)
    except subprocess.TimeoutExpired:
        pass
    end_session(qemu, clean=False)
    text = read_log(log)
    check("first notebook write succeeded (baseline)",
          "acting through limb 'notes.write'" in text
          and "limb returned . understood as a state node" in text)
    check("write after removal failed honestly",
          "the key would not take the ink" in text
          or "limb failed" in text)
    check("release-time persist degraded honestly",
          "the firmware would not let me write" in text)
    check("no false success after removal",
          "wrote it in the notebook: a second note" not in text)


def main():
    skip_slow = "--skip-slow" in sys.argv
    os.makedirs(LOGDIR, exist_ok=True)
    if not os.path.exists(f"{ROOT}/brainos-key.img"):
        sys.exit("no brainos-key.img -- run ./build.sh first")
    shutil.copyfile(f"{ROOT}/brainos-key.img", KEY_IMG)

    if skip_slow:
        print("[degradation] (--skip-slow: scenario 1 skipped)")
    else:
        scenario_link_dead()
    scenario_no_world_disk()
    scenario_corrupt_identity()
    scenario_key_removed()

    print()
    if failures:
        print(f"[degradation] FAILED: {failures}")
        sys.exit(1)
    print("[degradation] ALL CHECKS PASSED -- every failure fails safe and honest")


if __name__ == "__main__":
    main()
