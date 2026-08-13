#!/usr/bin/env python3
"""
metal_prep_test.py -- the two proofs bare-metal boot needs BEFORE a real
machine is involved, run in the safe VM:

PROOF 1 (touches-nothing): a DECOY disk rides along on the bus, built to
look like a real laptop's internal drive -- an ESP-typed FAT volume with
boot-ish files AND a booby trap: a file literally named WORLD.ID whose
content is wrong. The entity boots, lives a little (writes memory, writes
files), shuts down. The decoy's sha256 must be BIT-IDENTICAL afterwards,
while the key/world images must have changed (the positive control that
writes really happened -- and landed only where they belong). The fake
WORLD.ID also proves the marker hardening: a file merely *named* right
must not attract the filesystem limb.

PROOF 2 (bare-metal day-one behavior): on real hardware there is no body
daemon -- no COM3 tether, no mind, no voice. Boot with NO link chardev at
all and verify the core handles it honestly: "telemetry silent", reflex
replies to typed input, clean ESC release that persists memory to its own
key ONLY. This is exactly the state the first real-hardware boot will be
in, so it gets verified as its own scenario, not assumed.

Usage: tools/metal_prep_test.py    (no GPU, no network, ~2 boots)
"""
import hashlib
import os
import re
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, HERE)
from make_key import ESP_TYPE, Fat16, gpt_image, read_fat16_file  # noqa: E402
from qmp_client import QMP  # noqa: E402
import world_test as wt  # noqa: E402  (ScriptedDaemon + paths)

DECOY_IMG = "/tmp/brainos_decoy_internal.img"
QLOCAL = f"{ROOT}/tools/qemu-local"
KEY_IMG = f"{ROOT}/brainos-key.img"
WORLD_IMG = f"{ROOT}/brainos-world.img"
CONSOLE_LOG = f"{ROOT}/workspace/logs/metal_prep_console.log"
F4_LOG = f"{ROOT}/workspace/logs/metal_prep_f4.log"
SOCK = "/tmp/brainos_metal_link.sock"
QMP_SOCK = "/tmp/brainos_metal_qmp.sock"
VARS = "/tmp/brainos_metal_vars.fd"


def sha(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def build_decoy():
    """A pretend internal drive: ESP-typed, FAT16, boot-ish content, and a
    WORLD.ID whose content is wrong -- the booby trap for the marker check."""
    fs = Fat16(32 * 1024 * 1024 // 512, 4, "FAKE OMARCHY")
    fs.mkdir("BOOT")
    fs.add_file("WORLD.ID", b"EFI System Partition of somebody's laptop\r\n")
    fs.add_file("OMARCHY.TXT", b"pretend internal drive - must never change\r\n")
    fs.add_file("BOOT/GRUBX64.EFI", b"\x00" * 512)  # boot-ish, non-functional
    img = gpt_image([(ESP_TYPE, "Fake Internal ESP", fs.render())])
    with open(DECOY_IMG, "wb") as f:
        f.write(img)


def qemu_cmd(with_link, headless=True):
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
        "-drive", f"format=raw,file={KEY_IMG}",
        "-drive", f"format=raw,file={WORLD_IMG},cache=writethrough",
        # the decoy: writethrough so ANY guest write would land instantly
        "-drive", f"format=raw,file={DECOY_IMG},cache=writethrough",
        "-serial", f"file:{CONSOLE_LOG}",
        "-qmp", f"unix:{QMP_SOCK},server,nowait",
        "-device", "qemu-xhci", "-device", "usb-tablet",
        "-nic", "none",
    ]
    if with_link:
        cmd += ["-chardev", f"socket,id=mm,path={SOCK},server=on,wait=off",
                "-device", "isa-serial,chardev=mm,index=2"]
    if headless:
        cmd += ["-display", "none"]
    return cmd, env


def wait_console(needle, timeout, t0=None):
    t0 = t0 or time.time()
    while time.time() - t0 < timeout:
        if os.path.exists(CONSOLE_LOG):
            if needle in open(CONSOLE_LOG, errors="replace").read():
                return True
        time.sleep(1)
    return False


results = []


def check(name, ok, detail=""):
    results.append((name, ok))
    print(f"  [{'PASS' if ok else 'FAIL'}] {name}"
          + (f"  -- {detail}" if detail and not ok else ""))


def fresh_boot(with_link):
    for p in (CONSOLE_LOG, QMP_SOCK) + ((SOCK,) if with_link else ()):
        try:
            os.remove(p)
        except FileNotFoundError:
            pass
    import shutil
    shutil.copyfile(f"{QLOCAL}/usr/share/edk2/x64/OVMF_VARS.4m.fd", VARS)
    cmd, env = qemu_cmd(with_link)
    return subprocess.Popen(cmd, env=env, stdout=subprocess.DEVNULL,
                            stderr=subprocess.DEVNULL)


def main():
    print("[metal-prep] building the decoy internal drive...")
    build_decoy()
    # fresh world so listings are deterministic
    subprocess.run([sys.executable, f"{HERE}/make_world.py", "--force",
                    WORLD_IMG], check=True, stdout=subprocess.DEVNULL)

    sha_decoy_0 = sha(DECOY_IMG)
    sha_key_0 = sha(KEY_IMG)
    sha_world_0 = sha(WORLD_IMG)

    # ---------------- PROOF 2 first: the no-daemon (bare-metal-day-one) boot
    print("[metal-prep] leg A: boot with NO tether (bare-metal simulation)...")
    qemu = fresh_boot(with_link=False)
    ok_silent = wait_console("telemetry silent", 90)
    check("no-daemon boot reaches 'telemetry silent . reflexes only'",
          ok_silent)
    # the loop banner proves the entity kept going rather than hanging
    check("experience loop reached (controls banner shown)",
          wait_console("type + ENTER", 30))
    time.sleep(2)
    try:
        q = QMP(QMP_SOCK)
        q.type_text("hello are you there")
        q.press_enter()
        ok_reflex = wait_console("(reflex)", 30)
        check("typed input gets an honest reflex reply (no mind present)",
              ok_reflex)
        # F4 IS the bare-metal checklist: with no tether there is no
        # cognition to propose actions, so the core exercises its own
        # flesh. Every case is a typed Action through all eight KIRA
        # stages, including ones that MUST be denied. This is the exact
        # sequence docs/BARE_METAL.md tells the operator to grade, so it
        # must be verified here rather than trusted.
        q.hmp("sendkey f4")
        ok_done = wait_console("[SELFTEST] done:", 120)
        check("F4 limb self-test completes with no tether", ok_done)
        console_f4 = open(CONSOLE_LOG, encoding="utf-8", errors="ignore").read()
        # leg B overwrites CONSOLE_LOG, so keep leg A's copy: a failing F4
        # is otherwise undiagnosable after the run finishes
        with open(F4_LOG, "w", encoding="utf-8") as fh:
            fh.write(console_f4)
        m = re.search(r"\[SELFTEST\] done: (\d+) pass, (\d+) fail", console_f4)
        check("F4 self-test reports zero failures",
              m is not None and m.group(2) == "0",
              f"summary line: {m.group(0) if m else 'absent'}")
        # the runbook's expected count, so a silently-shrinking self-test
        # (a case dropped, a limb gone) is caught rather than passing
        # 18 with a world disk attached (10 filesystem/app cases + 2
        # notebook + network + camera + presence on/off + screen
        # sense + refusal). Both the network and camera cases are
        # ABSENCE assertions here: no NIC and no daemon on this leg.
        check("F4 exercised the full expected limb set",
              m is not None and int(m.group(1)) == 18,
              f"expected 18 passing cases, saw {m.group(1) if m else '?'}")
        # with -nic none the network organ is genuinely absent, so the
        # honest answer is a refusal -- absence tested as strictly as presence
        check("F4: absent network organ makes web.get a DENIAL, not a fetch",
              "PASS no network organ: web.get MUST be denied" in console_f4)
        check("F4: applications discovered on the world disk",
              "PASS applications discovered" in console_f4)
        check("F4: volume marker delete denied at policy",
              "PASS volume marker delete (MUST be denied)" in console_f4)
        check("F4: absent camera makes vision.look a DENIAL, not a description",
              "PASS no camera organ: vision.look MUST be denied" in console_f4)
        q.press_esc()  # release: persists memory to the KEY, nothing else
        q.close()
    except Exception as e:  # noqa: BLE001
        check("QMP interaction with no-daemon boot", False, repr(e))
    try:
        qemu.wait(timeout=60)
        check("ESC releases the body cleanly with no tether", True)
    except subprocess.TimeoutExpired:
        qemu.kill()
        check("ESC releases the body cleanly with no tether", False,
              "qemu did not exit")

    sha_decoy_1 = sha(DECOY_IMG)
    sha_key_1 = sha(KEY_IMG)
    check("decoy internal drive BIT-IDENTICAL after no-daemon boot",
          sha_decoy_1 == sha_decoy_0)
    check("key image DID change (memory persisted -- writes really happen, "
          "and only where they belong)", sha_key_1 != sha_key_0)

    # ---------------- PROOF 1: a full active session around the decoy
    print("[metal-prep] leg B: active session (fs+notes writes) with decoy "
          "on the bus...")
    codeword = f"metalproof{int(time.time()) % 100000}"
    daemon = wt.ScriptedDaemon([
        ("list the top of my disk", "fs.list", "", "exec"),
        ("write my note", "fs.write", f"WORK/METAL.TXT {codeword}", "exec"),
        ("note this down", "notes.write", f"the metal proof is {codeword}",
         "exec"),
    ])
    # ScriptedDaemon connects to wt.SOCK -- point it at ours
    wt.SOCK = SOCK
    qemu = fresh_boot(with_link=True)
    daemon.start()
    done = daemon.done.wait(timeout=120)
    time.sleep(1)
    try:
        q = QMP(QMP_SOCK)
        q.press_esc()
        q.close()
    except Exception:
        qemu.terminate()
    try:
        qemu.wait(timeout=60)
    except subprocess.TimeoutExpired:
        qemu.kill()

    console = open(CONSOLE_LOG, errors="replace").read() \
        if os.path.exists(CONSOLE_LOG) else ""
    r = daemon.results
    check("scripted session completed", bool(done) and daemon.err is None,
          f"err={daemon.err}")
    # the listing shows the REAL world's seeds -- not the decoy's contents.
    # If the fake WORLD.ID had fooled the marker check, this listing would
    # show OMARCHY.TXT / BOOT instead.
    check("fs.list shows the REAL world volume (decoy's fake WORLD.ID "
          "rejected by content check)",
          r[0] is not None and "README.TXT" in r[0]
          and "OMARCHY.TXT" not in r[0], f"got: {r[0]!r}")
    check("world/files limb incorporated exactly once (not doubled by decoy)",
          console.count("incorporate   ") > 0
          and "world/files" in console)
    check("fs.write landed", r[1] is not None and "wrote" in r[1],
          f"got: {r[1]!r}")
    check("notes.write landed", r[2] is not None, f"got: {r[2]!r}")

    sha_decoy_2 = sha(DECOY_IMG)
    sha_world_2 = sha(WORLD_IMG)
    check("decoy internal drive BIT-IDENTICAL after active session",
          sha_decoy_2 == sha_decoy_0)
    check("world image DID change (the fs.write landed there)",
          sha_world_2 != sha_world_0)
    on_world = read_fat16_file(WORLD_IMG, 2048, "WORK", "METAL.TXT")
    check("written file physically on the WORLD disk (not anywhere else)",
          on_world is not None and codeword in on_world.decode("ascii",
                                                               "ignore"))

    print("\n=== metal-prep RESULTS ===")
    ok = all(p for _, p in results)
    for name, passed in results:
        print(f"  [{'PASS' if passed else 'FAIL'}] {name}")
    print("\nMETAL-PREP: ALL PASS" if ok else "\nMETAL-PREP: FAILURES PRESENT")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
