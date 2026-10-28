"""
control_server.py -- the laptop-control WebSocket server on the Pi.

Serves a small web page and a WebSocket. The page (web/index.html) sends
velocity commands, skill buttons, head pan/tilt, a big software e-stop, and
reads live telemetry + a mock eye-camera video; it also supports a gamepad
via the browser Gamepad API. Every command is gated through KIRA
(kira_gate), then handed to the skill runtime. Each command is acked with the
client's timestamp echoed back, so the page and the latency probe can measure
round-trip command latency.

Standalone it drives a kinematic MockBody (no mujoco) so the page and the
latency test run anywhere. On the robot the same server talks to the Teensy
(or, in SIL, the twin) instead of MockBody; the command/telemetry schema is
unchanged.

Run (pi venv):  blur/pi/.venv/bin/python blur/pi/control_server.py --port 8777
"""
from __future__ import annotations

import argparse
import asyncio
import json
import os
import struct
import time

import numpy as np
import websockets

from kira_gate import BodyState, KiraGate, Posture
from skill_runtime import SkillRuntime

HERE = os.path.dirname(os.path.abspath(__file__))
PAGE = open(os.path.join(HERE, "web", "index.html"), "rb").read()


def bmp(w, h, frame):
    """A tiny 24-bit BMP with a moving bar + scanlines -- a real mock video
    frame the browser can render as data:image/bmp (no PIL/opencv needed)."""
    row_pad = (-w * 3) % 4
    rows = []
    bar = int((frame * 3) % w)
    for y in range(h):
        px = bytearray()
        for x in range(w):
            r = (x * 255) // w
            g = (y * 255) // h
            b = 40 + (120 if abs(x - bar) < 3 else 0) + ((y % 8) * 4)
            px += bytes((min(255, b), g, r))     # BMP is BGR
        px += b"\x00" * row_pad
        rows.append(bytes(px))
    pixels = b"".join(reversed(rows))            # BMP is bottom-up
    size = 54 + len(pixels)
    hdr = b"BM" + struct.pack("<IHHI", size, 0, 0, 54)
    dib = struct.pack("<IiiHHIIiiII", 40, w, h, 1, 24, 0, len(pixels), 2835, 2835, 0, 0)
    return hdr + dib + pixels


class MockBody:
    """A kinematic stand-in: integrates velocity, warms joints under torque,
    drains the battery, reports an upright IMU. The real body replaces this."""
    def __init__(self, n=26):
        self.n = n
        self.pos = np.zeros(3)      # x, y, yaw
        self.temps = np.full(n, 30.0)
        self.torques = np.zeros(n)
        self.battery = 1.0
        self.upright = 1.0
        self.vx = self.vy = self.wz = 0.0

    def set_velocity(self, vx, vy, wz):
        self.vx, self.vy, self.wz = vx, vy, wz

    def step(self, dt, q_cmd, q, estopped):
        self.pos[2] += self.wz * dt
        c, s = np.cos(self.pos[2]), np.sin(self.pos[2])
        self.pos[0] += (c * self.vx - s * self.vy) * dt
        self.pos[1] += (s * self.vx + c * self.vy) * dt
        self.torques = 2.0 * (q_cmd - q)
        load = np.abs(self.torques)
        self.temps += dt * (0.5 * load - 0.05 * (self.temps - 30.0))
        self.battery = max(0.0, self.battery - dt * (0.0008 + 0.0005 * float(load.mean())))


async def telemetry_loop(state):
    body, rt = state["body"], state["rt"]
    q = np.zeros(body.n)
    last = time.perf_counter()
    while True:
        now = time.perf_counter(); dt = now - last; last = now
        cmd = rt.step(q)
        q = q + min(1.0, dt / 0.2) * (cmd - q)
        body.step(dt, cmd, q, rt.state.estopped)
        rt.state.battery_frac = body.battery
        rt.state.upright = body.upright
        state["frame"] += 1
        state["telemetry"] = {
            "type": "telemetry", "t": now,
            "posture": rt.state.posture.value,
            "estopped": rt.state.estopped,
            "battery": round(body.battery, 3),
            "pose": [round(float(x), 3) for x in body.pos],
            "temps_c": [round(float(x), 1) for x in body.temps[:6]],
            "torques": [round(float(x), 2) for x in body.torques[:6]],
            "upright": round(float(body.upright), 3),
            "active": rt.active.name if rt.active else None,
        }
        await asyncio.sleep(1 / 30)               # 30 Hz telemetry


async def handler(ws, state):
    # push video at 10 Hz + telemetry as it updates
    async def pusher():
        while True:
            t = state.get("telemetry")
            if t:
                await ws.send(json.dumps(t))
            if state["frame"] % 3 == 0:
                import base64
                img = base64.b64encode(bmp(96, 64, state["frame"])).decode()
                await ws.send(json.dumps({"type": "video", "bmp": img}))
            await asyncio.sleep(1 / 30)
    push = asyncio.create_task(pusher())
    try:
        async for msg in ws:
            m = json.loads(msg)
            kind = m.get("type")
            reply = {"type": "ack", "t_client": m.get("t_client"), "t_server": time.perf_counter()}
            rt, body = state["rt"], state["body"]
            if kind == "ping":
                pass
            elif kind == "estop":
                rt.state.estopped = bool(m.get("on", True))
                reply["estopped"] = rt.state.estopped
            elif kind == "skill":
                d = rt.request(m.get("name", ""))
                reply.update(verdict=d.verdict.value, code=d.code, reason=d.reason)
            elif kind == "velocity":
                d = rt.request("walk")           # velocity implies locomotion -> gated
                if d.granted():
                    body.set_velocity(float(m.get("vx", 0)), float(m.get("vy", 0)), float(m.get("wz", 0)))
                reply.update(verdict=d.verdict.value, code=d.code, reason=d.reason)
            elif kind == "head":
                reply.update(ok=True)            # head pan/tilt is always allowed
            await ws.send(json.dumps(reply))
    finally:
        push.cancel()


async def process_request(connection, request):
    # serve the page on a plain GET; let the WS upgrade through
    if request.headers.get("Upgrade", "").lower() == "websocket":
        return None
    return connection.respond(200, PAGE.decode("latin1"))


async def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=8777)
    ap.add_argument("--host", default="127.0.0.1")
    ap.add_argument("--allow-locomotion", action="store_true",
                    help="enable walk/hop in KIRA (only once the 7x4015 are fitted)")
    args = ap.parse_args()

    rt = SkillRuntime(njoints=26, gate=KiraGate(allow_spine_abd=args.allow_locomotion))
    for name, pose in [("stand", Posture.STAND), ("sit", Posture.SIT),
                       ("lie", Posture.LIE), ("standup", Posture.STAND)]:
        rt.set_posture_pose(name, pose, np.zeros(26), settle_s=0.3)
    state = {"rt": rt, "body": MockBody(26), "telemetry": None, "frame": 0}
    asyncio.create_task(telemetry_loop(state))
    async with websockets.serve(lambda ws: handler(ws, state), args.host, args.port,
                                process_request=process_request):
        print(f"BLUR control server on http://{args.host}:{args.port}  (ws same port)", flush=True)
        await asyncio.Future()


if __name__ == "__main__":
    asyncio.run(main())
