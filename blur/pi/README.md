# BLUR Pi stack

The high-level software that runs inside the BLUR BrAInOS instance on the
Raspberry Pi 5: the KIRA-gated skill runtime, the SIL bridge to the MuJoCo
twin, the laptop control server, the voice pipeline and the vision scaffold.

Nothing here has run on real hardware. The SIL results are sim; the voice
latency is simulated until API keys are set.

## Venvs

- `blur/pi/.venv` (this dir): the app venv -- `websockets numpy httpx`. Used by
  the control server, the latency probe and the tests.
- The twin (`blur/sim/nerves.py`) needs mujoco, which lives in the uv venv at
  `~/Desktop/Arenda_Projects/BLUR/blur_mujoco/.venv`. The SIL harness runs the
  whole loop in that venv (its numpy covers the Pi modules too).

## What runs and is tested now

```sh
PI=blur/pi/.venv/bin/python
TWINVENV=~/Desktop/Arenda_Projects/BLUR/blur_mujoco/.venv/bin/python

# 1. runtime + KIRA gate + 50 Hz policy timing
$PI blur/pi/test_pi.py                      # ALL PASS; policy step ~14 us

# 2. SIL end to end against the twin, on the FINAL body, over the CAN protocol
BLUR_DESCRIPTOR=$PWD/blur/descriptor_final.json \
BLUR_MJCF_DIR=~/Desktop/Arenda_Projects/BLUR_final/mujoco \
  $TWINVENV blur/pi/sil.py --skill stand --seconds 2
# -> acquisition(zero@rest+commit) + KIRA + 200 Hz SETPOINT; body holds stand

# 3. laptop control server + localhost latency
$PI blur/pi/control_server.py --port 8777 &  # open http://127.0.0.1:8777
$PI blur/pi/latency_probe.py -n 200          # p50 ~0.4 ms on localhost

# 4. voice pipeline (mocks until keys) + one-command latency test
$PI blur/pi/voice/latency_test.py            # sit granted, come_here KIRA-denied,
                                             # stop offline, barge-in, ~90 ms TTFA
```

## Pieces

| file | what | runs on |
|---|---|---|
| `kira_gate.py` | Pi-side mirror of the Rust KIRA physical-state deny rules | Pi |
| `skill_runtime.py` | policy loader (ONNX optional) + sequencer + 50 Hz step + timing check | Pi |
| `sil.py` | SIL: Teensy role in Python driving the twin over the CAN protocol | laptop/dev |
| `control_server.py` + `web/index.html` | WebSocket control: velocity, skills, head, telemetry, e-stop, mock video, gamepad | Pi serves, laptop views |
| `latency_probe.py` | command round-trip latency | laptop |
| `voice/pipeline.py` | STT -> claude-haiku-5-5 (tools) -> Cartesia Sonic TTS; VAD, barge-in, stop-word KWS, offline fallback, latency log | Pi |
| `voice/latency_test.py` | one-command voice latency + tool-routing test | Pi |
| `vision/vision.py` | Hailo person/face detector (real) + synthetic stand-in + active-speaker head tracker | Pi (AI HAT+ 2) |

The authoritative policy engine is the Rust `brainos-kira` in `os/`; the Pi
gate mirrors its physical rules so the 50 Hz runtime can decide locally and
produce the same verdict and spoken reason. Device acquisition in sim is Gate
1 (`brainos blur gate1`, Rust); `sil.py` shows the Pi skill runtime driving
the acquired body end to end over the same CAN frames.

## Keys

Copy `.env.example` to `.env` (gitignored) and fill `ANTHROPIC_API_KEY`,
`CARTESIA_API_KEY` (or `ELEVENLABS_API_KEY`), optional `DEEPGRAM_API_KEY`.
Then `voice/latency_test.py` measures the real cloud round-trip.
