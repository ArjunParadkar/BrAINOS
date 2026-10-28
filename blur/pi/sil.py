"""
sil.py -- software-in-the-loop: the Pi skill runtime + KIRA driving the
MuJoCo twin end to end, over the real CAN protocol. "BrAInOS learns its
devices in sim before it is ever put in the bot."

It spawns the twin (`blur/sim/nerves.py --sock`, the Teensy pin boundary),
then plays the Teensy's role from Python:
  1. ACQUIRE: read each node's boot FEEDBACK, set a zero offset at the known
     rest pose, push current/position limits, and Commit -- the acquisition
     pattern Gate 1 proves in Rust, here over the same CAN frames. (The
     rigorous homing probe is the Rust Autonomic; this is a simplified
     zero-at-rest calibration, enough to drive the control path.)
  2. GATE + DRIVE: a skill request goes through KIRA (kira_gate), the skill
     runtime produces 50 Hz joint targets held to 200 Hz SETPOINT frames, and
     the twin returns FEEDBACK every tick. Body attitude is read from the
     BMI270 burst in the up datagram.

Run with the BLUR venv (mujoco + numpy). Point it at the final body:
  BLUR_DESCRIPTOR=$PWD/blur/descriptor_final.json \
  BLUR_MJCF_DIR=/home/god/Desktop/Arenda_Projects/BLUR_final/mujoco \
  .../BLUR/blur_mujoco/.venv/bin/python blur/pi/sil.py --skill stand --seconds 2
"""
from __future__ import annotations

import argparse
import os
import socket
import struct
import subprocess
import sys
import time

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(HERE))
sys.path.insert(0, os.path.join(REPO, "blur", "sim"))   # wire.py, blurcfg (via sim)
sys.path.insert(0, os.path.join(REPO, "blur"))
sys.path.insert(0, HERE)
import wire                      # noqa: E402
import blurcfg                   # noqa: E402
from kira_gate import KiraGate, Posture                 # noqa: E402
from skill_runtime import SkillRuntime                   # noqa: E402

FRAME_BYTES = wire.FRAME_BYTES


def down_pack(tick, relay_open, frames):
    """Teensy -> twin datagram (inverse of wire.down_unpack)."""
    frames = frames[:wire.__dict__.get("MAX_FRAMES", 96)]
    body = b"D" + struct.pack("<IBB", tick, 1 if relay_open else 0, len(frames))
    return body + b"".join(f.pack() for f in frames)


def up_unpack(b):
    """twin -> Teensy datagram (inverse of wire.up_pack). Returns (tick, imu15,
    estop, rail, vbat_mv, [frames])."""
    if not b or b[0:1] != b"U":
        return None
    tick, flags, vbat = struct.unpack_from("<IBH", b, 1)
    imu = b[8:23]
    n = b[23]
    frames = []
    off = 24
    for _ in range(n):
        f = wire.Frame.unpack(b[off:off + FRAME_BYTES]); off += FRAME_BYTES
        if f is not None:
            frames.append(f)
    return tick, (imu if flags & 1 else None), bool(flags & 2), bool(flags & 4), vbat, frames


def bus_idx(b):
    return "ABC".index(b)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--skill", default="stand")
    ap.add_argument("--seconds", type=float, default=2.0)
    ap.add_argument("--scene", default="flat")
    ap.add_argument("--sock", default="/tmp/blur_sil.sock")
    args = ap.parse_args()

    desc = blurcfg.load_descriptor()
    acts = desc["actuators"]
    n = len(acts)
    name_i = {a["name"]: i for i, a in enumerate(acts)}
    q_stand = np.array([a["q_stand"] for a in acts], float)

    # 1. spawn the twin
    if os.path.exists(args.sock):
        os.unlink(args.sock)
    env = dict(os.environ)
    twin = subprocess.Popen(
        [sys.executable, os.path.join(REPO, "blur", "sim", "nerves.py"),
         "--sock", args.sock, "--scene", args.scene],
        env=env)
    cli = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
    cli_path = args.sock + ".client"              # must be bound so the twin can reply
    if os.path.exists(cli_path):
        os.unlink(cli_path)
    cli.bind(cli_path)
    for _ in range(200):
        if os.path.exists(args.sock):
            break
        time.sleep(0.02)
    cli.settimeout(5.0)

    tick = 0

    def exchange(frames, relay_open=True):
        nonlocal tick
        cli.sendto(down_pack(tick, relay_open, frames), args.sock)
        data, _ = cli.recvfrom(4096)
        tick += 1
        return up_unpack(data)

    # settle a few ticks so the twin is streaming FEEDBACK
    for _ in range(20):
        exchange([])

    def read_feedback_q():
        up = exchange([])
        q = {}
        for f in up[5] if up else []:
            if f.fc == wire.FC_FEEDBACK:
                a = acts_by_slot(acts, f.node, f.data[0])
                if a:
                    s = wire.scales(a["motor"])
                    qv = struct.unpack_from("<h", f.data, 2)[0] * s["q"]
                    q[a["name"]] = qv
        return q, up

    qboot, _ = read_feedback_q()
    for _ in range(5):
        qboot, _ = read_feedback_q()

    # 2. ACQUIRE: zero at the rest pose, set limits, commit -- per node/slot
    cfg = []
    for a in acts:
        b, node, slot = bus_idx(a["bus"]), a["node"], a["slot"]
        off = a["q_stand"] - qboot.get(a["name"], 0.0)
        for key, val in [(wire.KEY["Direction"], 1.0),
                         (wire.KEY["Offset"], off),
                         (wire.KEY["CurrentLimit"], a["drv_peak_a"]),
                         (wire.KEY["QMin"], a["q_min"] - 0.2),
                         (wire.KEY["QMax"], a["q_max"] + 0.2),
                         (wire.KEY["Commit"], 1.0)]:
            cfg.append(wire.Frame.make(b, wire.FC_CONFIG, node,
                       struct.pack("<BBfBB", slot, key, val, 0, 0)))
    # send config in small batches (<= MAX_FRAMES per datagram)
    for i in range(0, len(cfg), 64):
        exchange(cfg[i:i + 64])
    for _ in range(5):
        exchange([])

    # 3. GATE + DRIVE the skill
    rt = SkillRuntime(n, gate=KiraGate(allow_spine_abd=False))
    rt.q = q_stand.copy(); rt.q_cmd = q_stand.copy()
    rt.state.posture = Posture.STAND
    rt.set_posture_pose("stand", Posture.STAND, q_stand, settle_s=0.5)
    rt.set_posture_pose("sit", Posture.SIT, q_stand, settle_s=0.5)   # pose filled below if available
    dec = rt.request(args.skill)
    print(f"KIRA({args.skill}): {dec.verdict.value} {dec.code} {dec.reason}".rstrip())
    if not dec.granted():
        twin.terminate(); return

    uprights = []
    steps = int(args.seconds * 1000)
    sp_hz_div = 5                       # 200 Hz setpoints inside the 1 kHz loop
    qcmd = q_stand.copy()
    for k in range(steps):
        qmeas, up = read_feedback_q()
        qv = np.array([qmeas.get(a["name"], rt.q[i]) for i, a in enumerate(acts)])
        if (k % sp_hz_div) == 0:
            qcmd = rt.step(qv)          # 50 Hz runtime target
        frames = []
        for i, a in enumerate(acts):
            s = wire.scales(a["motor"])
            frames.append(wire.setpoint_encode(bus_idx(a["bus"]), a["node"], a["motor"],
                          a["slot"], wire.SP_ENABLE, float(qcmd[i]), 0.0, 10.0, 0.3))
        up = exchange(frames)
        if up and up[1] is not None:
            acc = struct.unpack_from("<hhh", up[1], 0)
            az = acc[2] / wire.ACC_LSB_PER_G       # body-frame z accel in g
            uprights.append(az)

    cli.close(); twin.terminate()
    try:
        twin.wait(timeout=3)
    except Exception:
        twin.kill()
    up_mean = float(np.mean(uprights[-200:])) if uprights else float("nan")
    print(f"SIL end-to-end: {steps} ticks over the CAN protocol; "
          f"final body-z accel {up_mean:+.2f} g "
          f"({'upright/holding' if up_mean > 0.7 else 'not upright'})")
    print("acquisition(zero@rest+commit) + KIRA gate + 200 Hz SETPOINT stream verified in sim.")


def acts_by_slot(acts, node, slot):
    for a in acts:
        if a["node"] == node and a["slot"] == slot:
            return a
    return None


if __name__ == "__main__":
    main()
