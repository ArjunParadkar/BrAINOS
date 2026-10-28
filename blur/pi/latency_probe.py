"""
latency_probe.py -- measure round-trip command latency to the control server.

Sends N commands over the WebSocket and times each ack (the server echoes the
client timestamp). Reports p50/p95/max. On localhost this measures the
server's own command path; across Wi-Fi to the Pi it measures what the brief
cares about (< 100 ms command latency).

  blur/pi/.venv/bin/python blur/pi/latency_probe.py --url ws://127.0.0.1:8777/ -n 200
"""
import argparse
import asyncio
import json
import time

import websockets


async def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--url", default="ws://127.0.0.1:8777/")
    ap.add_argument("-n", type=int, default=200)
    args = ap.parse_args()
    rtts = []
    async with websockets.connect(args.url) as ws:
        # drain any immediate telemetry/video pushes, then probe
        for i in range(args.n):
            t0 = time.perf_counter()
            await ws.send(json.dumps({"type": "skill", "name": "stand", "t_client": t0}))
            while True:
                m = json.loads(await ws.recv())
                if m.get("type") == "ack":
                    rtts.append((time.perf_counter() - t0) * 1e3)
                    break
            await asyncio.sleep(0.005)
    rtts.sort()
    p = lambda q: rtts[min(len(rtts) - 1, int(q * len(rtts)))]
    print(f"command round-trip over {len(rtts)} cmds: "
          f"p50 {p(0.5):.2f} ms  p95 {p(0.95):.2f} ms  max {rtts[-1]:.2f} ms  "
          f"(target < 100 ms over Wi-Fi)")


if __name__ == "__main__":
    asyncio.run(main())
