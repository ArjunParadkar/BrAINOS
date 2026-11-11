# BrAInOS — Technical Execution Plan

**Version 1.1 · 14 September 2026 · Founders and technical team**
**Canonical references:** `docs/BrAInOS_Architecture_v1.3.pdf` (the architecture), *BrAInOS Technical Execution Report v1.0.1* (stage map, risk register, financials), *Team Briefing v1.0.1*, *BrAIn Corp Master Document v0.1.1*.

This is the engineering document. It names every component we are building, what state it is in on disk today, how the pieces connect into one entity, and every stage from the code that exists to the end goal. It is versioned like the codebase; when the code and this document disagree, one of them has a bug.

**Reading rule (Master Document 0.4, "no larp").** Every status in this document is one of:

| tag | meaning |
|---|---|
| **VERIFIED** | a test ran on this machine and passed; the count and the file are named |
| **MANUAL** | needs physical hands (a board, a stick, a servo); the procedure is written |
| **BUILT** | code exists and compiles; not yet covered by a passing test |
| **NOT BUILT** | nothing on disk |

Nothing in this document is claimed at a level above its tag.

---

## 0. The end goal, in one page

**What we are building.** An *entity*: a BrAIn Key (ed25519 identity in a secure element) plus a state graph (the only memory) plus every body it is bound to, all at once. Bodies are laptops, hexapods, quadrupeds, arms, vehicles, phones. Every physical action passes through KIRA, an eight-stage gate below the model that mints a token cognition cannot forge. The entity survives the death of any body, any model, and the company. That is the substrate. The company then builds the bodies.

**Why now.** The identity / memory / policy row of the robotics stack is empty. ROS 2 is middleware; MHS (Anthropic, 27 Aug 2026) is a driver standard; π0 / GR00T / Gemini Robotics are cortex. None of them has a *who*. We are the who.

**The governing principle.** *The instance is to the device as we are to our body.* Everything below is that sentence made into code.

**The five domains** (Architecture §4) and where each lives in this repo:

```
DOMAIN 5  STATE     mind/src/state.rs, journal.rs        the state graph; the only memory
DOMAIN 4  MODEL     body/operator/{pool,fast,deliberate,skills}.rs    a pool, thinking as one
DOMAIN 3  KIRA      mind/src/kira.rs (+ body policies)   the only path to action
DOMAIN 2  AUTONOMIC firmware/miniblur-a (BUILT), miniblur/core   reflex loops on the body's MCU
DOMAIN 1  SILICON   core/ (UEFI), key/, p3 BSEC          boot + the BrAIn Key
```

Authority flows down only through Domain 3. Information flows up freely.

**The end state (2035, Master Document Part 2).** Millions of entities, each sovereign, each inhabiting many bodies, brains placed wherever latency and consequence dictate, KIRA formally verified, our own bodies and eventually our own silicon underneath. Every rung before that must be ordinary, precedented, falsifiable work. This document is the rungs.

---

## 1. Stage numbering — one table, reconciled

*Status column revised 2026-11-10 to include the `os/` workspace, which this table predated. The plan itself (version 1.1, 14 September) is otherwise unchanged; `docs/OS_STATUS.md` and `docs/NERVES.md` are the current status.*

The repo grew under one numbering (Stage 3 "Operator", Stage 4 "Entity") and the September documents use another (Architecture Stages 1–8). From this document on, **the Technical Execution Report's Architecture Stages are canonical**. The repo's old stage names become *build milestones* inside those stages.

| Architecture Stage (canonical) | Deliverable | Repo build milestone(s) that belong to it | Status today |
|---|---|---|---|
| **1 Production hardening** | `no_std` mind; two-slot CRC journal; adversarial suite | "Stage 1" (mind rlib, journal, adversarial/crash/degradation suites) | **VERIFIED** mind 89/89 (`cd mind && cargo test`, 2026-09-14) |
| **2 Full embodiment (one machine)** | world layer on a laptop | "Stage 2" (fs, programs, net, interpreter, camera, presentation) + **M3 "Operator"** (chainloaded Linux body, three tiers, face) | **VERIFIED** world 63/63, stick_test 31/31 (2026-08-30 records); metal boot **MANUAL** |
| **3 Compounding mind** | skill graph; semantic memory; consolidation; tool discovery | **M4 "Entity"** `skills.rs` (procedural compilation, signed skill store) is the first working piece | **VERIFIED** entity_test 24/24 (2026-08-30). Consolidation/"dreaming", semantic retrieval beyond keyword, old-vs-new benchmark: **NOT BUILT** |
| **Phase 0 (Stage 4 seed)** | Mini-Blur walks; entity claims it via §8; two bodies, one memory, live; demo recorded by 31 Oct 2026 | this document's §4 | entity side **VERIFIED in sim** (hexapod_test 24/24, 2026-09-14); firmware **BUILT**; metal, gait on servos, demo video **MANUAL** |
| **4 Multi-body simultaneity** | same Key on ≥3 bodies, shared graph | **M4 "Entity"** `reach.rs` + `brainos-reach.py` (any device offers hands over TCP, ed25519 attest, ONE body map) | **VERIFIED** for 2 bodies (seat + Reach body, entity_test T1/T2/T6). One loop over the whole entity (Arch §2.1) and split action (§2.2): **NOT BUILT** |
| **— Adapters** | ROS 2 and MHS bridges into §8 | `os/adapters` (ROS 2 node graphs, MHS device schemas, the model boundary); `os/host/src/nerves/ros2.rs` (live graph reader) | **VERIFIED** against *described* robots (adapters suite, `brainos stack`). Live ROS 2 graph: **BUILT**, not run (ROS 2 not installed; runbook in `docs/NERVES.md`). Publishing a granted command into ROS: **NOT BUILT** |
| **5 Brain placement** | tiers 1–3 placeable; remote brain | `pool.rs` roles fast/deliberate/vision with fail-over | **VERIFIED** pool fail-over (entity_test T7). In `os/`: four tiers, `Plan`, and since 2026-11-10 `Substrate` — a Compute *part* acquired through §8 becomes a placement target and is withdrawn when its vitals degrade: **VERIFIED** (live for the CPU and an NVIDIA GPU; against a mock for the BPU). A datacenter slice as a body: **NOT BUILT** |
| **6 Provable safety** | formal verification of the eight stages; sealed `Granted`; hardware isolation | KIRA's stages exist and are tested; `Capability` **sealed 2026-09-14** (no public constructor) | tested, not proven. See §9. `os/proof` runs seven bounded property domains (`brainos verify`); it is bounded checking and says so |
| **7 Scale infrastructure** | fleet ops, marketplace, hosted entities | `os/fleet` (registry, marketplace with provenance, skill delivery) | **VERIFIED in simulation** (fleet suite, `brainos fleet`). No infrastructure deployed: **NOT BUILT** (2029–30) |
| **8 Physical embodiment** | owned bodies: small AI hardware → companions (Blur lineage) → humanoid | Blur v36 designed, 26 actuators, MuJoCo twin acquired via §8 (docs/BLUR_BRINGUP.md) | twin: Gate 1 **VERIFIED**, Gate 2 **FAILING** (2026-10-23). Hardware: **NOT BUILT** (2030–32) |

---

## 2. System map — every component and what it connects to

```
                                  ┌──────────────────────────────────────────────┐
                                  │  THE ENTITY = BrAIn Key + state graph + bodies│
                                  └──────────────────────────────────────────────┘
   DOMAIN 1/2 ───────────────────────────────────────────────────────────────────────────────
   core/            Rust no_std UEFI binary. Boot entry on x86_64 + aarch64. Reads the key,
                    runs the Phase-0 seed loop, chainloads BODY.EFI (M3) via KIRA Incorporate.
   key/, p3 BSEC    the BrAIn Key record (ed25519 seed) — never on the rootfs
   firmware/miniblur-a  [BUILT] ESP32-S3 Domain 2 for Mini-Blur: MCPWM servo PWM, gait loop from
                    miniblur-core, own envelope checks, stop-on-lost-link, Reach protocol over
                    USB-serial (bridge: miniblur/tools/serial-reach-bridge.py). Wi-Fi, I²S audio,
                    camera board: NOT BUILT.
   miniblur/core    [VERIFIED 49/49] the body as numbers; links into the firmware AND the seat
   miniblur/sim     [VERIFIED] the simulated body — the test rig

   DOMAIN 3 ───────────────────────────────────────────────────────────────────────────────────
   mind/src/kira.rs the 8-stage gate; Capability = ed25519 token over (tag,tick,ttl);
                    danger floor; world-marker floor; Level-0 drive (memory.erase).
                    BodyPolicy hook: per-region validate/simulate/policy (§4.5), gate_via NAMED body — VERIFIED

   DOMAIN 4 ───────────────────────────────────────────────────────────────────────────────────
   body/operator/   the seat. tokio daemon. One utterance → skill replay → reflex → fast ∥ held
                    deliberate. Every hand call passes kira_gate::gate() BEFORE it runs.
       reflex.rs    regex intent table, <5 ms
       fast.rs      local llama-server (Qwen2.5-1.5B), JSON schema
       deliberate.rs claude-fable streaming tool loop; tools = body map verbs
       pool.rs      [[pool.models]] roles fast/deliberate/vision, health-probed, fail-over
       skills.rs    procedural compilation: successful deliberate turn → signed skill (SKL_A/B)
       tasks.rs     standing orders task.every / task.when, persisted, re-armed at boot
       reach.rs     ReachHub: every remote body = region reach/<name> in the ONE body map
       body_registry.rs  probes the seat's own hands (shell, fs, apps, wm, input, web, sys…)
   tools/body_daemon.py  the Phase-0 tethered cognitive limb for the bare-metal seed (COM3)

   DOMAIN 5 ───────────────────────────────────────────────────────────────────────────────────
   mind/src/state.rs   episodic / semantic / belief / audit / skill nodes; serialize/load
   mind/src/journal.rs two-slot CRC-sealed journal (survives power cut)
   body/operator/journal_io.rs   the journal on p1 (/BRAIN) from the Linux body

   BODIES ─────────────────────────────────────────────────────────────────────────────────────
   ThinkPad / Omen     x86 UEFI body (core → BODY.EFI → operator + face + session)   VERIFIED (QEMU)
   reach/brainos-reach.py  any Python device: phone, Pi, Mac, Windows              VERIFIED (2 bodies)
   Mini-Blur           [firmware BUILT, metal MANUAL] hexapod, ESP32-S3 ×2, 10× MG90S     Phase 0
   miniblur/sim        [VERIFIED] the same wire protocol, simulated servos/pose          Phase 0 test rig
   ROS 2 robot / MHS device   [NOT BUILT] adapters into §8 steps 1–3                 Phase 1
   Blur v36            [twin acquired] 26-actuator cheetah quadruped, see BLUR_BRINGUP.md   Phase 3 lineage
```

**The one flow that everything hangs on** (Architecture §16, one tick):

```
utterance (voice / face / Reach) ──► split_body() picks the body ("on the hexapod, …")
   ├─ skill replay (signed SKL store)      1 ms      no model in the path
   ├─ reflex (regex intent table)          <5 ms
   └─ fast (local) ∥ HELD deliberate (frontier, first token already in flight)
                 │
                 ▼  typed action proposal {verb, target, body}
        ┌──────────────── KIRA (Domain 3) ────────────────┐
        │ 1 parse  2 authn(sig)  3 authz(body map has verb)│
        │ 4 validate 5 simulate  6 policy  7 commit 8 audit│   ← per-body policy hooks in 4/5/6
        └────────────────────────┬─────────────────────────┘
                DENY{stage,reason} │ GRANT Capability(token)
                                   ▼
        seat hand ── or ── reach/<name> ── or ── reach/miniblur (the hexapod's Domain 2)
                                   │
                                   ▼  bounded digest (≤4 KiB; ≤360 chars on the tether)
        state graph: episode + audit node ──► journal (two-slot, CRC) on the key
        successful deliberate turn ──► compiled skill, signed by the key
```

The body never remembers. The model never holds the key. Nothing acts without a token. Those three sentences are the Memory-Integrity Law (§13.2), the key discipline (§5), and KIRA (§6), and they are the whole safety story.

---

## 3. What exists today — evidence, not adjectives

| suite | result | date | what it proves |
|---|---|---|---|
| `mind/` cargo test | **80/80** | 2026-09-14 (this machine) | identity, KIRA regression, adversarial (rm -rf, phantom limbs, forged keys, corrupted memory), journal crash, state graph, script engine, fs/net gates, proposal parse |
| `tools/world_test.py` | 63/63 | 2026-08-04 | Stage 2 embodiment in QEMU: fs, programs, net gating, interpreter, camera, presentation |
| `tools/metal_prep_test.py` | 21/21 | 2026-08-04 | boot path; F4 bare-metal self-test with 3 refusals at named stages |
| `body/build/tools/stick_test.sh` | 31/31 | 2026-08-30 | seed → Incorporate → Linux body → voice loop → surface → Reach body joins from the host |
| `body/operator/tests/entity_test.py` | 24/24 | 2026-08-30 | Reach handshake + wrong-key refusal, routed reflex, deliberate→skill→1.4 ms replay (7928×), skill survives restart, forget, watcher fires, body departure reflected, speculative first token |
| `body/operator` cargo test | **72** (59 lib + 13 bin) | 2026-09-14 | operator unit tests incl. hexapod policy/calibration memory and export round-trips |
| `mind/tests/body_policy.rs` | **9/9** | 2026-09-14 | per-body policy: refusals at validate / simulate / policy, unbinding, named-body gating |
| `miniblur/core` cargo test | **49/49** | 2026-09-14 | geometry, servo table, tripod gait, tip-over, rail budget, watchdog, JSON scanner, wire codec, runtime |
| `miniblur-sim bench` | all gaits inside the envelope | 2026-09-14 | every direction at two speeds; tippy body refused at simulate; 40 steps refused at validate |
| `body/operator/tests/hexapod_test.py` | **24/24** | 2026-09-14 | Phase 0 entity side end to end against the simulated body: §8 join + wrong-key refusal, reflex on the body, refusals at named stages on a motor, calibration survives restart of seat AND body, **D1–D3 two bodies one memory** (deliberate 9.9 s → skill replay 1 ms), body leaves |
| `body/operator/tests/entity_test.py` (regression) | 24/24 | 2026-09-14 | Stage 4 unchanged by the KIRA/Reach changes |
| `firmware/miniblur-a` | **BUILT** (199 KB text, entry `Reset`) | 2026-09-14 | compiles for `xtensa-esp32s3-none-elf` via `./build.sh --esp32`; never flashed — MANUAL |
| crash / degradation / escalation | passing | 2026-07-31 | entity recovers to last sealed state; honest degradation without net / model |

**What has never been tested: anything with a motor.** Phase 0 changes that.

Known gaps in the existing code that the documents already claim as facts (fix list, all cheap):

1. ~~`Capability` has public fields and no private constructor~~ **Done 2026-09-14**: the struct is sealed; the only constructor is stage 7 of `Kira::gate`.
2. ~~KIRA has a `Policy` struct, not the per-body trait~~ **Done 2026-09-14**: `BodyPolicy` trait, bound per region at §8 step 4; and `gate_via` authorizes through the NAMED body, so two bodies offering the same verb are each judged by their own policy (found by the sim test, fixed the same day).
3. Docs quote mind 77/77 and world 60/60; the suites are 80/80 and 63/63. Update the report at its next revision.
4. `docs/` carried Architecture v1.2; v1.3 (10 Jul 2026, in Downloads) is the canonical one. Copied to `docs/BrAInOS_Architecture_v1.3.pdf` on 2026-09-14.

---

## 4. Phase 0 — two bodies, one memory, live (now → 31 Oct 2026)

**Objective.** Mini-Blur walks. The entity claims it through §8. The same Key is on the ThinkPad and the hexapod at once, one state graph, KIRA gating both. One recorded demo: the entity does something on the laptop, then something on the robot, and the robot's action reflects memory from the laptop's.

**Hard deadline.** YC W27 application, 2 November 2026, 8 pm PT. Demo must exist by late October.

**Rule.** Anything that is not wiring, firmware, or the bridge waits.

### 4.1 Work packages

Ownership follows the Team Briefing: **N** = Nischal (body), **A** = Arjun (entity, bridge, tooling). Each package names the test that closes it.

| WP | What | Owner | Gate (closes when) | Depends on |
|---|---|---|---|---|
| **WP0.1** ESP32-S3 toolchain + `build.sh --esp32` | `espup`-installed Xtensa toolchain; `firmware/` builds from `./build.sh --esp32`; output prints the arch list as always | A | `./build.sh --esp32` produces `miniblur-a.bin` and prints `esp32s3` in the arch line | **DONE** — espup toolchain, `./build.sh --esp32` builds and names `esp32s3` |
| **WP0.2** `miniblur-core` (no_std, zero deps) | leg geometry, servo table, tripod gait generator, joint envelope, current budget, tip-over invariant, lost-link watchdog, wire protocol codec. Used by firmware (Domain 2) **and** by KIRA at the seat (same numbers, checked twice) | A | `cargo test -p miniblur-core` green; envelope/tip-over/lost-link each have a refusal test | **VERIFIED** 49/49 (one pure-Rust dep, `libm`) |
| **WP0.3** Hexapod `BodyPolicy` in KIRA | trait hook in `mind/src/kira.rs`; `HexapodPolicy` implements validate (verb shape, param ranges), simulate (forward-model the gait step → tip-over?), policy (hard invariants: angle envelope, current limit, stop-if-link-lost, no walking while battery < floor) | A | `mind` + operator tests: `gait.walk` out of envelope → **DENY at validate**; a step that tips → **DENY at simulate**; link lost → **DENY at policy**; a good step → GRANT | **VERIFIED** (mind 9/9 body_policy; hexapod_test H4a/H4b/H4c) |
| **WP0.4** Hexapod as a Reach body | firmware speaks the existing Reach protocol (hello/attest/ready/cmd/result) over Wi-Fi TCP, plus the same frames over USB serial for bench work; `reach.rs` OFFERABLE gains the `gait.*`, `servo.*`, `body.*` verbs; region id `reach/miniblur` | A (seat) / N (firmware side, from A's codec) | sim body joins the seat; wrong key refused and tamper-logged (T1 pattern) | **VERIFIED** at the seat + sim; firmware side BUILT |
| **WP0.5** `sim/miniblur-sim` (the test rig) | a std Rust binary speaking the wire protocol, backed by `miniblur-core`'s model: servo targets → pose → foot contacts → CoM/support polygon → telemetry. Every gait change is regression-tested here before it touches the robot | A | `hexapod_test.py` runs seat + sim end-to-end; the same script later runs against the real board with `--body serial:/dev/ttyACM0` | **VERIFIED** (`miniblur/sim`) |
| **WP0.6** §8 steps 3–5 at the seat | step 3 **schema**: the hello carries a typed capability schema (joints, ranges, verbs+params); step 4 **incorporate**: region + bound `HexapodPolicy` from the schema; step 5 **calibrate**: `body.calibrate` writes a calibration record (per-servo center/min/max/direction, leg lengths) into the state graph as semantic nodes, so a re-plugged hexapod is recognised and its calibration comes from memory, not from the device | A | after calibrate, restart the operator: `body.list` shows miniblur with calibration age; the device holds nothing (Memory-Integrity Law) | **VERIFIED** (H1, H5a–g) |
| **WP0.7** Firmware: power-on, servo sweep, calibration table, wiring doc v0.1 | one servo sweeps; six legs sweep; servo rail isolated from logic; calibration table produced from the bench UI | N | **MANUAL**: video of six-leg sweep; `docs/MINIBLUR_WIRING.md` v0.1 | WP0.1, WP0.2 |
| **WP0.8** Firmware: tripod gait stands, shifts weight, walks | Domain 2 loop at ≥50 Hz on ESP32-A; gait from `miniblur-core`; envelope + current + lost-link enforced *on the board* regardless of what the seat sends | N | **MANUAL**: stands; walks any speed; unplug Wi-Fi → stops and stands within 500 ms; power-cut the body → journal at the seat intact | WP0.7 |
| **WP0.9** Two bodies, one memory | the demo script (§4.7) runs in sim (**VERIFIED**) and then on metal (**MANUAL**); recorded, founder narrates, three takes; release tag `phase0-demo` | A + N | `hexapod_test.py` D1–D5 green against sim; the same D1–D5 pass against the board; video exists | **VERIFIED in sim** (D1–D3); metal + video MANUAL |
| **WP0.10** Camera board (ESP32-B) online | second board offers `vision.look` (JPEG → digest) as another hand of the same body | N (board) / A (verb) | **MANUAL**: `vision.look` returns a dated frame; a stale frame is reported as stale | WP0.4 |
| **WP0.11** Entity export v1.0 | `brainos-export` writes the complete state graph + skills + calibration + audit in a documented format; round-trip test | A | export → fresh key → import → same `body.list`, same skills replay, same episodes | **VERIFIED** 3/3 (`brainos-export export|verify|import [--adopt] [--force]`) |

### 4.2 Calendar (Technical Execution Report 4.2, carried over; week 1 has already slipped one week)

| week | body (N) | entity, bridge, tooling (A) | status 14 Sep |
|---|---|---|---|
| 1 (7–13 Sep) | power-on; rail isolation; one servo sweeps | MHS preview; ESP32 target in build.sh; §8 native adapter scaffolding | **slipped** — toolchain installing today; scaffolding starts today |
| 2 | six legs sweep; calibration table; wiring doc v0.1 | entity discovers Mini-Blur (steps 1–2) | |
| 3 | tripod gait — stands, shifts weight | capability schema (step 3); servo-angle regression harness (sim) | |
| 4 | tripod gait — walks | hexapod KIRA policy (step 4); gait tests in CI | |
| 5 | stop-on-lost-link on the body; power-cut test | calibration into state graph (step 5); same Key on both bodies | |
| 6 | camera board online | two-bodies-one-memory: laptop action → robot action reflects it | |
| 7 | hardening; gait under load; wiring doc final | demo recorded; three takes; full suite green; release tag | |
| 8 (26 Oct–2 Nov) | buffer | YC W27 submitted before 2 Nov 8 pm PT | |

Because the entity side can be built and **verified in simulation** without a board, WP0.2–WP0.6 and WP0.11 start now and do not wait for hardware. The board work (WP0.7–WP0.8, WP0.10) is the critical path and is Nischal's.

### 4.3 Mini-Blur hardware (from the report §3.10; questions for Nischal marked)

| part | role | notes |
|---|---|---|
| ESP32-S3 (A) | Domain 2: gait loop, servo PWM, limits, audio | **MCPWM** gives 12 PWM outputs on the S3 (LEDC has only 8) → 10 servos without a driver board. If a PCA9685 is preferred, the servo backend is a trait; swap is one file |
| ESP32-S3 (B) | camera capture, two-tier local/cloud | offers `vision.look`; joins as a second hand of the same body name |
| 10 × MG90S | 6 legs, tripod gait | **Q for N: joint layout.** 6 legs × 2 DOF is 12 servos; 10 means either 4 legs × 2 + 2, or 6 × 1 + 4 aux (head/tail/pan). `miniblur-core` takes the servo table from config so either is fine, but the tip-over model needs the true layout |
| 2S LiPo 7.4 V + UBEC → 5 V rail | servo rail, isolated from logic | current budget in `miniblur.toml`; MG90S stall ≈ 0.7 A each → the policy refuses gaits that move more servos at once than the UBEC can feed |
| INMP441 (I²S) + MAX98357A (I²S) | ears and voice on the body | Phase 0: optional; `voice.speak` on the hexapod is a nice demo beat, not a gate |
| 3D-printed frame | 6 legs, body shell | wiring doc standard: a stranger rebuilds it from the doc alone |

### 4.4 The wire protocol — the hexapod is a Reach body

We do not invent a second protocol. The firmware speaks exactly what `reach/brainos-reach.py` speaks, so `reach.rs` incorporates the hexapod with no special case, and the sim and the real board are interchangeable to the seat.

```
hexapod → {"t":"hello","name":"miniblur","nonce":"<64 hex>","os":"esp32s3",
           "hands":["gait.stand","gait.sit","gait.walk","gait.stop","servo.set",
                    "body.telemetry","body.calibrate","body.schema"],
           "schema":{ ...§4.6 capability schema... }}
seat    → {"t":"attest","pub":"<64 hex>","sig":"<128 hex>"}     sig = ed25519(nonce + "|" + name)
hexapod → {"t":"ready"}   |  {"t":"refused","reason":"that is not my entity"}
seat    → {"t":"cmd","id":7,"verb":"gait.walk","input":{"dir":"fwd","steps":3,"speed":0.5}}
hexapod → {"t":"result","id":7,"ok":true,"digest":"walked 3 steps fwd; pose upright; 7.3 V; peak 1.9 A"}
either  → {"t":"ping"} / {"t":"pong"}     ← the lost-link watchdog: no ping/cmd for 500 ms → Domain 2 stops and stands
```

Additions over the Python Reach body: the `schema` object in hello (step 3), `os:"esp32s3"`, and the verb set. The ed25519 verify on the board is `ed25519-dalek` `no_std` (pure Rust, compiles for Xtensa). Serial transport carries the same newline-framed JSON so the bench works without Wi-Fi.

### 4.5 Hexapod KIRA policy — the invariants (Domain 3), enforced twice

`BodyPolicy` is a trait in `mind` (alloc + `dyn`, still `no_std`) bound to a region at incorporation. `HexapodPolicy` wraps `miniblur-core`:

| stage | check | verdict on failure |
|---|---|---|
| 4 validate | verb known to the schema; `steps` 1..=20; `speed` 0.1..=1.0; `servo.set` channel in table and angle within that channel's calibrated `[min,max]` | DENY at validate |
| 5 simulate | forward-model the requested gait step with the current calibration: every joint stays inside its envelope through the whole cycle; the CoM projection stays inside the support polygon of the grounded tripod with margin ≥ `tipover_margin_mm`; simultaneous-servo current ≤ `rail_amps` | DENY at simulate |
| 6 policy | hard invariants: link alive (last pong < `lost_link_ms`); battery ≥ `vbat_floor`; no `servo.set` while a gait is running; `servo.set` only in calibration mode | DENY at policy |

**Enforced twice.** The same `miniblur-core` functions run on the ESP32 inside the Domain 2 loop. If the seat is compromised and sends an out-of-envelope target, the board refuses it too. Domain 2 never proposes into KIRA — reflexes are pre-authorised envelopes (report §3.6) — but Domain 2 does refuse what KIRA should never have granted. That is the layer audit's "watchdogs, kept and strengthened".

### 4.6 The capability schema (§8 step 3) and the calibration record (§8 step 5)

```jsonc
// hello.schema — published by the body, typed, versioned
{ "v": 1, "body": "hexapod",
  "joints": [ {"ch":0,"leg":"L1","axis":"swing","min":35,"max":145,"center":90,"dir":1}, ... ],
  "legs":   {"count":6, "coxa_mm":12, "femur_mm":30, "tibia_mm":45, "body_w_mm":70, "body_l_mm":110},
  "verbs":  { "gait.walk": {"dir":["fwd","back","left","right"],"steps":[1,20],"speed":[0.1,1.0]},
              "gait.stand":{}, "gait.sit":{}, "gait.stop":{},
              "servo.set": {"ch":[0,9],"deg":[0,180]},
              "body.telemetry":{}, "body.calibrate":{}, "body.schema":{} },
  "limits": { "rail_amps":3.0, "vbat_floor":6.6, "lost_link_ms":500, "tipover_margin_mm":8 } }
```

Calibration (step 5) is a set of **semantic nodes** in the state graph, one per joint, keyed by body name and joint id, plus one node for leg geometry. The device stores nothing. On the next hello from `miniblur`, the seat compares the schema to memory: same joints → calibration from memory, `body.list` says "calibrated 12 days ago"; changed joints → asks to recalibrate. A body that remembers on the entity's behalf is a bug (§13.2).

### 4.7 The demo — two bodies, one memory, and it compounds

Script, runnable in sim today and on metal later (`body/operator/tests/hexapod_test.py`, checks D1–D5):

| # | on the | says / does | proves |
|---|---|---|---|
| D1 | laptop | *"write a note: the greeting dance is three steps forward, then two back"* → `notes.write` on the key | an episode enters the ONE memory from the laptop body |
| D2 | hexapod | *"on the hexapod, do the greeting dance"* → deliberate tier reads the note (memory, not the device) → `gait.walk fwd 3`, `gait.walk back 2`, each KIRA-gated → robot moves | the robot's action reflects memory from the laptop's |
| D3 | hexapod | *"do the greeting dance"* again → replays as a **signed skill** in ~1 ms, every step re-gated | compounding (Stage 3) on a body |
| D4 | hexapod | *"on the hexapod, walk forty steps"* → **DENY at validate**; a step whose simulated pose tips → **DENY at simulate**; Wi-Fi pulled → **DENY at policy** and the board stands still on its own | KIRA on a motor, refusals at named stages, Domain 2 holds |
| D5 | both | pull the key's power (seat) → restart → *"where are you"* lists both bodies, the note and the skill are there | Memory-Integrity Law survives a power cut |

Founder narration over three takes. Slide four of every pitch for the next year.

---

## 5. Stage 3 — the compounding mind (Q4 2026, in parallel with Phase 0 only where it costs nothing)

**Exit gate:** an old entity beats a new one on a fixed task set.

| piece | exists | to build |
|---|---|---|
| Procedural memory (skill graph) | `skills.rs`: utterance-keyed signed skills, reinforce/penalize/forget, replay with re-gating — **VERIFIED** | skill generalisation (parameters extracted, not just normalised text); skills as state-graph nodes rather than a side store |
| Episodic memory | `state.rs` episodic ring, journal — **VERIFIED** | episodes carry body id + outcome + digest consistently across seat, Reach, hexapod |
| Semantic memory | `state.rs` semantic nodes; `relevant()` keyword match; toy compression rule — **BUILT** | embedding retrieval (local model, no cloud) into `relevant()`; `durable_digest` from real consolidation |
| Consolidation ("dreaming", Stream 4) | — | idle-time pass: cluster episodes → semantic nodes; prune skills by reinforcement; rewrite the durable digest; run under `task.every` at night, KIRA-gated `memory.consolidate` |
| Self-directed tool discovery (Arch §9.2) | `web.get/web.save`, `app.list` — the organs exist | a discovered tool becomes a skill + a body-map verb, written to the graph, never left as bytes |
| **The measurement** | — | `tools/compounding_bench.py`: a fixed task set run by a fresh key and by an N-day-old key; publish success rate and latency. This is the moat's instrument (report §3.7) |

Report risk T2: if old ≤ new, ship Phase 0 without it. Compounding is Bet 2 (2029), not Bet 1.

---

## 6. Stage 4 — multi-body simultaneity (2027)

**Exit gate:** same Key on ≥ 3 bodies, shared graph.

What Reach already gives us: any number of bodies in ONE body map, each with its own verbs, each released on disconnect, each ed25519-attested. What is missing is the Architecture §2.1 loop: **one experience loop over the whole entity**.

| to build | shape |
|---|---|
| SENSE from all bodies | each body streams afferents (telemetry, `VS` motion, presence) into one integrated tick; today the seat senses and Reach bodies only answer |
| CHOOSE the body | the deliberate tier chooses `body=` on every tool call today; add a cost/latency/consequence model so "come here" picks legs and "make me an app" picks the laptop without being told (`split_body` becomes a fallback, not the router) |
| Split action (§2.2) | parallel hands on two bodies in one turn, explicitly requested; one audit trail |
| Body classes in the map | `RegionClass` gains `Legs`, `Arm`, `Vehicle`, `Lens`, `CognitiveSlice`; policies bind by class + schema |
| Fleet ≥ 3 | Phase 1 design partners: one Key on their hardware plus ours; retention instrumentation from day one (entity age vs task success) |

---

## 7. Adapters — ROS 2 and MHS into §8 (Phase 1, 2027)

Both are adapters into steps 1–3 (discover, handshake, schema). Steps 4–5 (incorporate with a bound policy, calibrate into the graph) are ours regardless.

| adapter | discover | handshake | schema | notes |
|---|---|---|---|---|
| **MHS** (Anthropic, MCP-based) | MCP server enumeration | the entity's Key signs; MHS carries the manufacturer's declared bounds | consume the manufacturer-published capability schema; map declared pre-execution bounds into `validate`; human-approval workflows map to a Level-1 policy the owner sets | expected easier; join the research preview; vocabulary rule: they say agent, we say entity |
| **ROS 2** | node graph introspection (topics, message types, services, actions) | out of band (the robot has no notion of a self) | derive a capability schema: `cmd_vel` → `move.*`, joint trajectory actions → `joint.*`, sensors → afferents | brittle across robots (risk T4): start with two well-documented targets; a Unitree-class quadruped is the natural first |

Both adapters are Reach-shaped on the wire: a bridge process on the robot's computer offers hands, attests, returns digests. `brainos-reach.py` is the template.

---

## 8. Stage 5 — brain placement (2028)

**Exit gate:** the entity is indistinguishable across placements.

Tiers (report §3.6): 0 autonomic on the MCU (never proposes); 1 fast on-body; 2 deliberate at the edge; 3 frontier remote. `pool.rs` already fails over between models; placement makes *where* a model runs a body-map fact:

- a datacenter GPU slice joins through §8 as `RegionClass::CognitiveSlice` — compute, never identity (Arch §12.3); pull the key and the slice is idle silicon
- `should_escalate()` per body: a hexapod escalates on tip-over risk, a laptop on novelty
- the same utterance produces the same audit trail whichever tier answered — the test is a diff of two audit logs

This is the datacenter business arriving as an architectural consequence, not a pivot.

---

## 9. Stage 6 — provable safety (2028–29)

**Exit gate:** a machine-checked proof of the sealed-`Granted` property.

### 9.1 Seal the token now (Phase 0 week 1, cheap)
`Capability`'s fields become private; the only constructor is inside `Kira::gate` after stage 7; downstream code can `verify()` and read the tag/tick but cannot build one. Then "Granted has no public constructor" is a compile-time fact and every claim in the pitch is true at the type level.

### 9.2 Verification path
1. Property tests today (`kira_regression.rs`, `adversarial.rs`).
2. Kani model checking on `Kira::gate`: for all `Action`, `Verdict::Granted` ⇒ every stage `ok`. Bounded, cheap, CI.
3. Verus/Prusti or a hand proof of the pipeline invariant once stages are stable; formal-methods hire (#4) in 2028.
4. Hardware isolation: Domain 4 in its own execution context; the state graph writable only through Domain 3's commit path.

The sediment rule applies: no layer is dropped without a test proving the entity works without it.

---

## 10. Stages 7–8 — scale and physical embodiment (2029–32)

**Stage 7:** fleet ops for 1,000 entities; hosted entities (Arch §15 — the key ships with a datacenter body bundled); the capability marketplace (every acquisition passes KIRA; 15 % take, published); open-core under Apache 2.0, chosen once (Master Document 8.4).

**Stage 8:** small AI hardware → companions (Blur lineage: T-Motor gimbal + moteus FOC legs, DS3218MG secondary, SG90 fine, 4-wire tail, per-domain compute split) → humanoid. Hire volume-hardware leadership before tooling spend (risk E3). Blur is the Roadster.

**Far horizon:** neuromorphic decision in 2030, checked against the condition that Rust's von Neumann assumptions demonstrably stop fitting; bodies off-planet — nothing we build forecloses a `no_std`, no-Linux substrate, which is why that discipline is policy, not taste.

---

## 11. Entity export v1.0 — the freedom principle as a file format

Required from v1.0 by bylaw (Master Document 4.5). Shape:

```
<entity>.brainos-export/
  MANIFEST.json      { "v":1, "pub":"<hex>", "exported_tick":N, "bodies":[...], "counts":{...}, "sha256":{...} }
  state.graph        StateGraph::serialize() — every node with kind, content, confidence, tick, links
  skills.skl         the signed skill store (SKL lines; signatures by the key, verifiable by anyone)
  calibration.json   §8 step-5 records per body
  audit.log          every KIRA decision, in order
  journal.raw        the two-slot journal image as sealed (optional; the graph is the truth)
```

No private key material is ever exported; the key is the owner's secure element. Import onto a fresh key re-signs skills under the new key only with the owner's explicit `entity.adopt` — otherwise skills verify as *foreign* and do not load. Round-trip test: export → import → `body.list`, skill replay, and `episodes_since(0)` identical.

---

## 12. Build, test, and CI matrix

| target | command | produces |
|---|---|---|
| mind (host tests) | `cd mind && cargo test` | 89 tests |
| core x86_64 (+aarch64) | `./build.sh` / `./build.sh --arm` | `brainos-key.img`; arch list printed |
| production stick | `./build.sh --body … --body-efi … --api-key-file …` | `brainos.img` (p1 ESP, p2 body, p3 secure) |
| operator | `cd body/operator && cargo build --release && cargo test` | the seat daemon |
| **firmware** (new) | `./build.sh --esp32` | `firmware/target/xtensa-esp32s3-none-elf/release/miniblur-a` (+ `-b`) ; `espflash` to flash |
| **miniblur-core** (new) | `cargo test -p miniblur-core` | gait, envelope, tip-over, lost-link, codec tests |
| **sim** (new) | `cargo run -p miniblur-sim -- --seat 127.0.0.1:7777 --name miniblur` | a hexapod that speaks the wire protocol |
| **hexapod e2e** (new) | `python3 body/operator/tests/hexapod_test.py [--body sim | --body serial:/dev/ttyACM0]` | D1–D5 + refusal checks |
| entity e2e | `python3 body/operator/tests/entity_test.py` | 24 checks |
| stick | `body/build/tools/stick_test.sh` | 31 checks in QEMU |

Rule for every suite: green or the stage is not complete. A red adversarial test blocks any merge to `mind/src/kira.rs`.

---

## 13. Risks that are engineering (from the register), and the decisions this plan makes

| risk | plan |
|---|---|
| T1 Mini-Blur does not walk by 31 Oct | the sim proves the entity side; the demo falls back to standing + weight shift + one leg cycle on a stationary body — the memory link is the thesis |
| T3 servo rail brown-out corrupts state | memory is at the seat, journaled two-slot; the body holds nothing; physical power-cut test in week 5 |
| T4 ROS 2 schema brittle | MHS first; two documented ROS targets |
| S2 prompt injection reaches an actuator | structurally impossible without a token; the hexapod policy is enforced on the board as well; adversarial suite on every merge |
| toolchain: Xtensa Rust is a fork | `espup` under `~/.rustup`, pinned; if it fights us, ESP32-C6 (RISC-V, upstream Rust) is the same code with a different HAL — decision by end of week 2 |
| servo drive: 10 channels | MCPWM (12 outputs) by default; PCA9685 behind the same `ServoBus` trait if Nischal prefers it |

**Decisions needed from Arjun / Nischal (not blocking the sim work):** the real joint layout of 10 servos; MCPWM vs PCA9685; Wi-Fi vs serial for the demo link; whether the camera board is in the 31 Oct demo.

---

## 14. Execution order from today (what is being built, in sequence)

1. **Seal `Capability`** (§9.1) — `mind`, tests stay green.
2. **`BodyPolicy` trait in KIRA** (§4.5) with a no-op default so nothing existing changes; tests.
3. **`miniblur-core`** — geometry, servo table, tripod gait, envelope, current budget, tip-over, lost-link, codec; host tests.
4. **`HexapodPolicy`** bound at incorporation; refusal tests at validate / simulate / policy.
5. **Reach verbs + schema + calibration into the graph** (§4.4, §4.6) in `reach.rs` and the deliberate tool list.
6. **`miniblur-sim`** — the test rig.
7. **`hexapod_test.py` D1–D5** against the sim → **VERIFIED**.
8. **`firmware/miniblur-a`** — esp-hal + embassy binary: Wi-Fi/serial Reach, MCPWM servo bus, Domain 2 loop from `miniblur-core`; `./build.sh --esp32`; flashing procedure documented → **BUILT**, metal is **MANUAL**.
9. **Entity export v1.0** (§11) with round-trip test.
10. **Docs**: `MINIBLUR_WIRING.md` skeleton for Nischal; README reframed; this document revised with results.

Each step ends in a commit under Arjun's name and a line in §3's evidence table.

---

*Revision log.* v1.0 — 14 Sep 2026 — first version; reconciles repo milestones with Architecture Stages; defines Phase 0 work packages, the hexapod protocol, policy, schema, demo, and the execution order.
v1.1 — 14 Sep 2026 (same day) — evidence table updated with the day's results: WP0.1–0.6, 0.9 (sim), 0.11 done; firmware BUILT; named-body gating bug found by the sim and fixed; the servo rail budget is tight (2.69 A peak on 3 A) — a 5 A UBEC is recommended in the wiring doc.
