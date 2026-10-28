# BLUR bring-up log

Deadline Friday 30 Oct 2026. One running log; newest block on top. Nothing
here is a hardware result — no robot is connected to this machine, and the
firmware has never touched a motor. Sim/host results say so explicitly.

Priority order (from the brief): leg-node firmware → Teensy safety/reflexes →
Pi skill runtime + laptop control → voice → vision → sim/training.

---

## Block 2 — 2026-10-28 (real firmware builds, Pi stack + SIL, control, voice, vision, retarget)

### Firmware — real on-target builds (PlatformIO in a repo-local venv)

PlatformIO 6.1.16 installed in `firmware/.venv` with `PLATFORMIO_CORE_DIR`
inside the repo (`firmware/.pio-core`); both gitignored. No system packages.

| project | target | result |
|---|---|---|
| `esc_g431` | B-G431B-ESC1 (ststm32 ^19, Arduino + SimpleFOC) | **BUILT** — Flash 25,908 B (19.8 %), RAM 2,220 B (6.8 %) |
| `esp32s3_node` | ESP32-S3 (SimpleFOC 2.3.2 + TWAI) | **BUILT** — Flash 233,597 B (7.0 %), RAM 19,064 B (5.8 %) |
| `node_core.c` (cross) | Cortex-M4F (STM32G431) | **BUILT** `arm-none-eabi-gcc -O2 -Werror -ffreestanding`, .text **2060 B** |
| `node_core.c` (cross) | Cortex-M7 (Teensy 4.1) | **BUILT** same flags, .text **1960 B** |
| `teensy_body` | Teensy 4.1 | blocked: this box's GitHub clone of FlexCAN_T4 keeps resetting (TLS/connection); core cross-builds for M7 above. Builds on a stable connection. |

So **both motor-node images build** (G431 and ESP32-S3), and the portable node
control core compiles for both real MCUs (≈2 KB of code). Only the Teensy body
image is outstanding, blocked solely by this environment's unreliable GitHub
access when cloning the FlexCAN_T4 library — not our code; it links the same
proven core (M7 cross-build above) and builds once the connection is stable.
Fix applied: `esc_g431` pinned to `ststm32@^19` (the earlier 17.6 pin's
framework package would not resolve here).

### Pi stack + SIL — "BrAInOS learns its devices in sim first" (brief 3, 5)

New `blur/pi/` package (app venv `blur/pi/.venv`: websockets, numpy, httpx):

- `kira_gate.py` — Pi-side mirror of the Rust KIRA physical-state deny rules
  (e-stopped, uncommitted, low battery, on-back, tethered, missing spine/abd,
  bad precondition), each with a one-sentence sayable reason.
- `skill_runtime.py` — ONNX-optional policy loader + a stand/sit/lie/stand-up/
  righting sequencer with smooth transitions, every request KIRA-gated. 50 Hz
  **timing self-check: a policy forward is ~14 µs vs the 20 ms budget
  (1434× headroom)** — the Pi CPU is not the constraint.
- `sil.py` — the Teensy's role in Python driving the MuJoCo twin
  (`nerves.py --sock`) over the **real CAN protocol**, on the **final body**:
  acquire (zero-at-rest offset + limits + Commit per node) → KIRA gate → a
  200 Hz SETPOINT stream. Result: **2000 ticks, body holds stand (+0.98 g
  body-z)**. The rigorous homing probe remains the Rust Autonomic (Gate 1);
  this shows the Pi skill path end to end over the same frames.
- `test_pi.py` — gate + sequencer + timing: **ALL PASS**.

### Laptop control (brief 3) — built + latency measured

`control_server.py` (WebSocket + served page) + `web/index.html`: velocity,
skill buttons, head pan/tilt, live telemetry (posture, battery, joint temps/
torques, IMU upright, pose), a big software e-stop, a **mock eye-camera video**
(server-generated BMP frames), browser **Gamepad API** + WASD. Every command
is KIRA-gated; velocity implies locomotion and is denied until the 4015s are
fitted. **Localhost command round-trip: p50 0.40 ms, p95 1.06 ms, max 2.05 ms**
(server path only; well inside the < 100 ms Wi-Fi target). `latency_probe.py`
is the measurement tool.

### Voice (brief 4) — built against mocks; real latency pending keys

`voice/pipeline.py`: streaming STT → Claude **claude-haiku-5-5** (streaming,
tool use) → **Cartesia Sonic** streaming TTS, with an energy **VAD**,
**barge-in**, a **local stop-word spotter** (works with no network), an
offline fallback, and end-of-speech→first-audio **latency logging**. Tool
calls (`sit`, `come_here`, `look_at_me`, `follow`, `lie_down`, `stop`) route
through KIRA → the skill runtime. `voice/latency_test.py` (one command) and
`.env.example` added.

- Mock run: "sit" granted and spoken; "come here" **KIRA-denied with a gentle
  spoken reason** ("my spine and hip motors aren't fitted yet"); stop-word
  fires offline; barge-in detected; simulated TTFA 90 ms (Cartesia ballpark).
- **TTS provider: Cartesia Sonic** (lowest streaming TTFB over WebSocket →
  the < 800 ms budget + barge-in); ElevenLabs Flash v2.5 fallback.
- **3 stock-voice candidates** (warm, gentle, soft-spoken young-adult female;
  stock-library, never a clone): [0] Cartesia soft warm conversational,
  [1] Cartesia calm even gentle, [2] ElevenLabs youthful bright-but-soft. A
  sampling render for A/B/C choice runs once a TTS key is set.

### Vision (brief 5) — scaffold, cheap stand-in

`vision/vision.py`: a `Detector` interface with `HailoDetector` (real path on
the AI HAT+ 2 / Hailo-8L, imported lazily) and a dependency-free
`StandInDetector` (synthetic moving speaker), plus a `HeadTracker` that picks
the **active speaker** (audio/lip-motion flag, else most confident face) and
emits neck pan/tilt. Verified: tracker drives toward the target and selects
the speaking face over a higher-confidence silent one. No OpenCV/Picamera2
dependency pulled in (kept cheap); real fps is Hailo inference on the HAT.

### Retarget to the final body (brief 0.3) + staged campaign (brief 5)

- `blur/retarget_check.py` (run): the final `blur.xml` stand keyframe matches
  `descriptor_final.json` to **0.000 rad** (self-consistent), all 18 reference
  motions load; v36→final gap is **17/26 joints beyond the 0.02 rad tolerance,
  max 0.94 rad** (spine_twist), mass 3.6715→3.5754 kg → the v36 (day-one)
  policies do not transfer; a new campaign on the final body is required.
- `blur/blurcfg.py` gained a `BLUR_DESCRIPTOR` override so the final campaign
  uses `descriptor_final.json` without touching the committed v36 descriptor;
  verified it loads the final body (3.575 kg, correct sha) and passes the
  MJCF sha-check.
- Staged: `blur/hpcc/campaigns/finalbody/{manifest.txt,campaign.env}` (7 skills
  × 5 seeds + 7 refs, walk/hop longer) and **`blur/hpcc/submit_finalbody.sh`**
  — ONE command to sync code + the final MJCF, freeze the campaign, and launch
  lanes `blur-fb-1/2`. It **refuses cleanly while SSH is down** (verified) and
  leaves the day-one lanes untouched. **Not submitted** (Pinnacle VPN is
  Arjun's; SSH still timing out).

### What needs Arjun (updated)

Unchanged from Block 1 (confirm 7×4015; keys; voice choice; cluster VPN+2FA;
hardware checklist), plus: the two blocked firmware builds are **network**
blockers (GitHub/registry), not code — they build once the mirrors are stable;
no action needed beyond a stable connection. `allow_spine_abd` / locomotion in
KIRA stays off until the 4015s are confirmed fitted.

### Friday outlook (updated)

On track: firmware core (host + ARM cross + ESP32 image), the Pi KIRA-gated
skill runtime, SIL on the final body, laptop control (measured), the voice
pipeline (mock; real on keys), vision scaffold, and the staged final-body
campaign. Still gated on Arjun/hardware: real-robot motion (no hardware;
4015s missing), retarget training (cluster), flashed G431/Teensy images
(stable network for the frameworks).

---

## Block 1 — 2026-10-28 (inventory, firmware core, torque, BLUR_final retarget)

### What already existed (inventory, no fork)

A large, tested stack is already in place; this block builds on it, it does
not replace it.

| piece | where | state |
|---|---|---|
| body descriptor (from `blur.xml`+`blur_meta.json`, sha-pinned) | `blur/gen_descriptor.py` → `blur/descriptor.json`, `os/blur/src/gen.rs` | v36 body |
| CAN frame codec (golden-vectored) | `os/blur/src/wire.rs` (Rust), `blur/sim/wire.py` (Python) | tested both sides |
| CAN bus budget | `os/blur/src/bus.rs` | tested |
| node behaviour spec (impedance, envelope, derate, I²t absent, faults) | `blur/sim/nerves.py` `Node` | the twin / firmware spec |
| Autonomic acquisition (census, probe, home, push, IMU level) | `os/blur/src/autonomic.rs` (`no_std`, 1 kHz) | Gate 1 PASS |
| reflexes + Mahony attitude + balance + sessions | `os/blur/src/{supervisor,attitude,session}.rs` | tests green |
| §8 acquisition driver (Pi side) | `os/blur/src/acquire.rs` | tested |
| KIRA policy engine (validate/simulate/policy, evidence, explain) | `os/` workspace (18 `no_std` crates, 793 tests) | green |
| MuJoCo twin behind the real CAN/BMI270 formats | `blur/sim/nerves.py` | Gate 1 runner `brainos blur gate1` |
| RL training (skills + PPO + eval + exact-resume lanes) | `blur/learn/*`, `blur/hpcc/*` | day-one campaign on Pinnacle |
| Gate 1 (acquisition) report | `docs/BLUR_BRINGUP.md` | PASS |
| Gate 2 (push recovery) | `os/host/src/blur_gate2.rs` | pending training |

The Teensy *image*, the node firmware *images*, and the Pi↔Teensy USB port do
not exist yet. `blur/sim/nerves.py` is the executable spec for the node side;
`os/blur` is the executable spec for the Teensy side.

### Facts that changed the brief (from the coordinator), and the prompt diff

`BLUR_final/CLAUDE_CODE_PROMPT_BrAInOS.md` was compared to the brief given to
this agent: **the bodies of sections 0–6 are byte-identical** (empty `diff`),
same "3.58 kg", same hardware map, same priorities. The operative overrides
are the coordinator's, not the prompt's:

1. **BLUR_final arrived mid-block** at `~/Desktop/Arenda_Projects/BLUR_final`
   (was absent at start; firmware/torque/protocol work was done first, as
   instructed). Treated as read-only source.
2. **No API keys** in the environment (no `ANTHROPIC_API_KEY`, no TTS/STT
   key). Everything is built to read keys from env / an untracked `.env`;
   latency is measured only once Arjun provides keys.
3. **The 7 missing motors are assumed to be the 7 × 4015** (4 abduction +
   spine S1, twist, S2). **Arjun, please confirm.** Without them abduction and
   the whole spine do not move, so walking, running and jumping on the real
   body depend on them.
4. **No hardware** on this laptop; firmware is proven host-side only.
5. **"Learn before the bot"** = run the Pi/Teensy/node stack against the twin
   end to end (the Gate 1 acquisition pattern), extended to the new protocol.

### BLUR_final vs the v36 training body (retarget delta)

The repo descriptor (`blur/descriptor.json`, sha `9c7a5d24…`, 3.6715 kg,
`…/blur_mujoco_v36`) is **v36**, not the final model (sha `d27a66d4…`,
3.5754 kg). A final-model descriptor was generated to a side file without
touching the v36 tree (verified bit-identical restore):
**`blur/descriptor_final.json`**.

- Same **topology**: nq 60, nv 59, nu 26, identical actuator names, identical
  `tau_max` (legs 4.0, 4015 3.8, neck/jaw 0.8).
- Different **kinematics**: 3.575 vs 3.672 kg; README: legs **9 % longer**
  (F 125/120/53, R 110/120/70), straighter stance, abduction moved **inside
  the ribcage**, pelvis/spine shifted. Joint range/pose deltas up to **0.94
  rad** (spine_twist q_max 1.287 → 0.347).
- **Consequence:** every v36-trained policy — i.e. the entire day-one campaign
  — is on the wrong body. Retargeting (brief §0.3) is required and real; the
  gap is larger than the 0.16–0.39 rad the brief quoted. New training must be
  a **new campaign** on the final body; the running day-one lanes are left
  untouched.
- The driver/motor **torque limits are body-independent** (same motors), so
  the torque work below holds; the **required** torques per skill must be
  recomputed on the final model (`physics_checks.py` on BLUR_final), not v36.

### Firmware — the one CAN protocol (done)

`firmware/protocol.md` is the authoritative prose spec, reconciled with the
existing `wire.rs`/`wire.py`/`bus.rs`. The protocol was already defined and
golden-vectored; this block documented it and added the third implementation
(C, below). Classic CAN 1 Mbit, 11-bit ids, `id = function<<7 | node`;
ESTOP/CENSUS/SETPOINT/FEEDBACK/CONFIG/CONFIG_ACK/IDENTITY/RAW.

**Bus load (computed, `bus.rs` formulas; 135 bits/frame worst case):**

| bus | motors | load @ 200 Hz | load @ 1 kHz across-bus |
|---|---|---|---|
| A (front: 6 ESC + FRONT node) | 12 | **64.8 %** | 324 % (impossible) |
| B (rear: 6 ESC + REAR node) | 11 | 59.4 % | 297 % |
| C (spine: MID node) | 3 | 16.2 % | 81 % |

- A 1 kHz loop across CAN needs **7.02 buses**; there are 3 → the impedance
  loop runs **on each node**, Teensy refreshes setpoints at **200 Hz**. This
  was already the design; confirmed numerically.
- **Bus A is 64.8 % at 200 Hz — over the brief's 60 % guideline** (under the
  70 % plan load). Recommendation: refresh the FRONT node's low-bandwidth
  slots (abduction, neck) at 100 Hz, dropping bus A under 60 %. No wiring
  change needed. (Open item.)

### Firmware — the node control core (done, host-tested)

`firmware/node_core/` is portable C11 implementing the protocol codec
(`protocol.h`) and the per-node control law + local envelope (`node_core.c`),
the executable form of `nerves.py::Node` plus the I²t estimator the brief
requires (nerves.py does not model temperature).

```
cd firmware/node_core && make test      # 26 checks, 0 failed, 0.19 s
```

Covered: the two **golden frames match `wire.py`/`wire.rs` byte-for-byte**
(so C ⇄ Python ⇄ Rust agree on the wire — re-verified `blur/sim/test_wire.py`
still green); the impedance law `τ = kp(q*−q) − kd·q̇ + τ_ff`; gain caps
(kp ≤ kp_max = 12×deliverable, kd ≤ 0.2 kp_max); setpoint clamp into
[qmin,qmax]; stale-setpoint (> 60 ms) → damping; the 40 N·m/rad joint-limit
spring; the current clamp (→ CLAMPED); the config-lock (ceilings only narrow
after Commit; calibration frozen); the e-stop latch (no release frame); and
the I²t over-temperature derate to the continuous rating.

**Builds host-side with `-Werror -pedantic`.** This is not a hardware result.

### Firmware — the on-target node projects (staged, not built here)

This laptop has **no PlatformIO / arduino-cli**. Both node projects are
provided as pinned configs wrapping `node_core`:
- `firmware/esc_g431/` — B-G431B-ESC1, `ststm32@17.6.0`, Arduino + SimpleFOC
  2.3.4, FDCAN classic, MT6701 ABZ + index homing, flash-before-snap +
  CAN/UART bootloader note.
- `firmware/esp32s3_node/` — ESP32-S3, `espressif32@6.9.0`, SimpleFOC + TWAI,
  one `node_core` per slot (node 17 FRONT ×6, 18 MID/spine ×3, 19 REAR ×5).

They are not yet compiled (needs a PlatformIO install Arjun opts into; command
in `firmware/README.md`). The control core they link is proven.

### Firmware — torque limits (done)

`firmware/torque/torque_limits.py` → `torque_limits.json` + `TORQUE.md`.
First principles: `τ = (9.5493/KV)·16·η·I`, η = 0.75 (brief) primary, 0.85
cross-check reproduces the descriptor. Driver ratings: ESC1 40 A **forced-air
peak** (no official continuous — placeholder 6 A, measure); DRV8313 2.5 A
peak / 1.75 A RMS; DRV8833 2 A / 1.5 A.

Deliverable at η = 0.75: **4015 abd/spine 2.00 continuous / 2.87 peak N·m**
(vs 3.8 design); 2806.5 legs ~3.5 peak but **thermally** limited for sustained
torque (I²t governs); neck/jaw ~0.8.

Ruled out / at risk by the real 4015 limits: **gallop/bound RULED OUT** (spine
needs 6.6–11 N·m p95, 2–3× short); **table-jump AT RISK** (remeasure on the
final body); **walk feasible but at the continuous cap** (keep the torque
penalty); sit/lie/stand-up/righting/balance feasible. `torque_limits.json` is
for merge into `blur_meta.json` and the retargeted env caps.

### Cluster

`ssh -o BatchMode=yes pinnacle` **timed out** (needs Arjun's VPN + one
interactive 2FA, per the standing notes). Per the rules, cluster work is
stopped and not retried faster than every 10 min. The day-one campaign is
running there untouched; I cannot read its status until SSH is up. **No new
training was submitted** (also correct: it must wait for the final-body
retarget and a new campaign).

### Not started this block (status + plan)

- **Teensy body controller image** (brief §2): logic exists and is tested in
  `os/blur` (reflexes, attitude, session envelope, acquisition). The missing
  piece is the Teensyduino image embedding that crate behind FlexCAN3 +
  BMI270 SPI + e-stop GPIO, and the Pi↔Teensy USB port. Next build after the
  node images.
- **Pi skill runtime + laptop control + vision** (brief §3): the policies
  exist (`blur/learn`), export is `blur/learn/export.py`; KIRA gating is in
  `os/`. The ONNX-at-50 Hz runtime, the WebSocket + gamepad control page, and
  the AI-HAT person/face detector are not yet built. Design is standard; these
  can be demoed independently of locomotion (brief §6.5).
- **Voice** (brief §4): design + provider recommendation below; not built
  (no keys → no latency measurement).

### Voice: recommendation (to be measured once keys exist)

Two architectures to benchmark, both with mic EC + VAD + barge-in and local
"stop/freeze" keyword spotting that works offline:
- **A. STT → Claude `claude-haiku-5-5` (streaming, tool use) → streaming TTS.**
- **B. a speech-to-speech realtime API.** Keep whichever hits the < 800 ms
  (end-of-speech → first audio) target; log both.

**TTS provider pick (by latency + stock-voice quality): Cartesia Sonic**,
with **ElevenLabs Flash v2.5** as the quality fallback. Reason: Cartesia Sonic
has the lowest published streaming time-to-first-audio (~tens of ms) over a
WebSocket, which is what the < 800 ms budget and barge-in need; ElevenLabs has
the richer stock library if the voice quality matters more than the last
50 ms. **STT:** a streaming provider (Deepgram-class) with endpointing, local
Whisper as the offline fallback.

**Three stock-voice candidates for Arjun to choose by ear** (warm, gentle,
soft-spoken young adult female; a stock library voice, never a clone of any
real or fictional voice actor):
1. a soft, warm, slightly breathy conversational voice (Cartesia stock);
2. a calm, even, gentle "assistant" voice (Cartesia stock);
3. a youthful, bright-but-soft voice (ElevenLabs stock).

Exact voice IDs depend on the live library; a sampling script will render the
same three persona lines from each once the key is set, for an A/B/C choice.
**Persona:** polite, warm, a little shy, quietly protective, dry deadpan
humour, short spoken sentences — BLUR, not a copy of any character.

### `.env` contract (gitignored; never committed)

```
ANTHROPIC_API_KEY=...        # Claude (model claude-haiku-5-5)
CARTESIA_API_KEY=...         # or ELEVENLABS_API_KEY
DEEPGRAM_API_KEY=...         # streaming STT (optional; Whisper is the fallback)
```

### What needs Arjun

1. **Confirm the 7 missing motors = the 7 × 4015** (4 abduction + spine
   S1/twist/S2).
2. **API keys** in `.env` (above) so voice latency can be measured.
3. **Voice choice** from the 3 samples (once a TTS key is set).
4. **PlatformIO opt-in** (network install in a venv) so the node images build;
   or confirm the host-tested core + staged configs are enough for now.
5. **Cluster access** (VPN + 2FA) before any retarget training; and a decision
   to start a **new** campaign on the final body (day-one lanes left to
   finish).
6. The **hardware bring-up checklist** (brief §6) is Arjun's to run with the
   e-stop in hand; nothing here can be flashed or run on the robot.

### What will / won't work by Friday

- **Will:** the one CAN protocol (documented + three-way golden-vectored); the
  node control core, host-built and unit-tested; the first-principles torque
  limits + ruled-out skills; the final-body descriptor staged for retarget;
  the voice design + provider pick + 3 voice candidates + `.env` contract.
- **Probably, if time/keys allow:** the voice pipeline end-to-end against
  mocks (and measured if keys arrive); the laptop WebSocket control page; the
  Pi ONNX skill runtime timing check.
- **Won't by Friday:** anything on the real robot (no hardware here, and the
  7 × 4015 are missing → no spine/abduction, so no walk/run/jump on the body
  regardless); retargeted policies (needs cluster + a new campaign); flashed
  node/Teensy images (needs PlatformIO + boards).

### Next step

Build the Pi-side skill runtime + laptop control against the twin
(independent of locomotion and of the missing motors), and scaffold the voice
pipeline with recorded fixtures so it is ready the moment keys land.
