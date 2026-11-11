# BLUR bring-up in simulation

BrAInOS inhabiting BLUR v36 (the 3D-printed cheetah) in its first body,
the MuJoCo twin, through the same messages the robot will use. One
section per phase; each ends with its gate report. Numbers here are
produced by commands in this repo and the logs they write; nothing is
quoted from memory.

```sh
~/Desktop/Arenda_Projects/BLUR/blur_mujoco/.venv/bin/python blur/gen_descriptor.py   # descriptor from the MJCF
cd os && cargo test --release -p brainos-blur                                     # 31 tests, no twin needed
cd os && cargo run --release --bin brainos -- blur gate1                          # Gate 1 against the twin (~6 min)
```

## Where things are

| piece | where | runs on (robot) |
|---|---|---|
| body descriptor (generated from `blur.xml` + `blur_meta.json`, sha256-pinned) | `blur/gen_descriptor.py` → `blur/descriptor.json`, `os/blur/src/gen.rs` | — |
| wire: CAN frames, BMI270 burst, the Teensy's pin boundary | `os/blur/src/wire.rs`, mirrored in `blur/sim/wire.py` (golden vectors both sides) | Teensy, nodes |
| CAN budget | `os/blur/src/bus.rs` | — |
| census, build sheet, schema, phantom-limb check | `os/blur/src/{census,schema}.rs` | Pi (KIRA side) |
| body attestation key, owner list | `os/blur/src/firmware.rs` | Teensy |
| Autonomic acquisition procedures (census, probe, home, push, IMU level, rest) | `os/blur/src/autonomic.rs` (`no_std`, 1 kHz `step`) | Teensy |
| probe least-squares | `os/blur/src/fit.rs` | Teensy |
| calibration solver | `os/blur/src/calibrate.rs` | Pi |
| §8 driver over `brainos_body::Acquisition` | `os/blur/src/acquire.rs` | Pi |
| twin: MuJoCo + Silicon layer (node impedance loops, current caps, torque/speed derate, MT6701, CAN timing, BMI270, e-stop, faults) | `blur/sim/nerves.py` | — |
| software-in-the-loop port, Gate 1 runner | `os/host/src/{blur_sil,blur_run}.rs` | laptop |

The swap point between twin and robot is the `acquire::Port` trait. In
software-in-the-loop, `SilPort` runs the Teensy's own code (`Autonomic`,
`Firmware`) in-process against simulated pins. On the robot the same
`Autonomic` is built for the Teensy and the Pi reaches it over USB through
a port that is **not written yet**, because the Teensy firmware image it
would talk to does not exist yet. Nothing above `Port` changes.

## Phase 1 — what the files say, where they disagree with the brief

The MJCF and the BOM win. Disagreements found before any code was written:

1. **No spine ligament springs or TPU bump stops in the v36 MJCF.**
   `spine_s1` and `spine_s2` have stiffness 0 and no tendon.
2. **Spine walk torque is 2.12 N·m p95** (shipped `physics_checks`, no
   ligament springs), not 1.6–1.9. No file has the lower figure.
3. **No torque/speed curves in the MJCF.** The actuators are ideal
   position servos with a flat torque cap. The twin's nodes add a derate
   (flat to w0/2, linear to 0 at w0; the knee point is an assumption).
   The 4015's KV is ~100, an **assumption** carried over from
   `physics_checks.py`.
4. **The 4015s cannot make 3.8 N·m under the stated parts.** 3.8 N·m
   needs 2.93 A at KV 100 / 16:1 / 85 % efficiency. The brief doesn't
   name the 4015 driver; on a SimpleFOC Mini (DRV8313, 2.5 A peak) they
   deliver **3.25 N·m**. The descriptor carries `tau_deliverable` and
   the twin enforces it. This hits abduction and the spine, which is
   already the gallop's limit.
5. The 6 × 2204 are neck lift/pan/tilt, jaw, and **two tail winches**
   that pull cables through a **passive 9-segment, 18-joint sprung
   tail** (underactuated). The body also has **8 passive sprung girdle
   joints** and TPU knee-extensor cables on the **rear legs only**.
   59 DOF, 26 actuators, 3.672 kg.
6. The MJCF has **4 foot touch sensors the robot doesn't have**; they
   are never read. It has no cameras and no current sensing.
7. The 751 mm table ceiling is with the feet tucked; with the rig's
   stance it is **406 mm** (the abduction motors cap it).
8. `run_blur.sh` / `tools/run_instance.sh` defaulted the network organ
   **on** (a 2026-07-31 decision). The brief makes off-by-default
   non-negotiable; both now default off.
9. Stale comments fixed: `os/body/src/acquire.rs` `default_envelope`
   said "intersection" while the code takes the union.
   `EXECUTION_PLAN.md` said Blur has 23 actuators; it has 26.

Two findings that change the architecture:

- **A 1 kHz joint loop over CAN does not fit.**
  - Each motor needs a setpoint and a feedback frame per cycle, at 135
    bits worst case each.
  - 26 motors × 2 frames × 1 kHz is 52,000 frames/s. One 1 Mbit/s bus
    carries 7,407.
  - That's **8 buses at 100 % load**; the Teensy has 3. SN65HVD230 is
    rated to 1 Mbit/s, so CAN-FD doesn't help.
  - So the impedance loop (q*, kp, kd, τff) and the current clamp run
    **on each node**. The Teensy supervises at 1 kHz and refreshes
    setpoints at **200 Hz**, which puts every bus under 70 % (`bus.rs`,
    tested).
  - The proposed node map: bus A = front ESCs + FRONT node (6 motors);
    B = rear ESCs + REAR node; C = spine node alone. This is a proposal,
    not a wiring diagram. The FRONT ESP32-S3 drives 6 BLDCs, which is 18
    PWM outputs on paper and needs a bench check.
- **The 4015s' "built-in" MT6701 sits on the motor**, ahead of the 16:1
  gearbox. At power-on their output angle is known only modulo 22.5°, so
  **abduction and the spine must be homed at every boot.** BLUR has to
  power up in the jig, or in a known rest pose.

## Gate 1 report (2026-10-22)

`brainos blur gate1`, log `workspace/blur_logs/gate1_final.txt`:
**PASS (acquisition part).** Existing suites: see the end of this section.

**Acquisition, end to end, over the real message formats** (CAN frames
per node, BMI270 bursts, through the Autonomic's 1 kHz `step`):

| run | result | body time | zero error, worst vs truth |
|---|---|---|---|
| unit 36, first inhabitation (no record → build sheet proposed) | acquired + calibrated | 149.7 s | RL_dist 0.027 rad (76 % of 0.035) |
| unit 36, re-inhabitation from the record (sheet + recorded zeros, drift check) | acquired + calibrated | 149.7 s | RL_dist 76 % |
| unit 37 (other silicon, magnets, IMU mounting, noise) | acquired + calibrated | — | FR_pitch 79 % |
| unit 38 | acquired + calibrated | — | RR_dist 76 % |

Calibration, unit 36:
- **IMU level:** mount roll 0.18°, pitch 0.42°; gyro bias recovered.
- **Torque/encoder agreement:** all 26 joints, by a least-squares fit
  of `q = q0 + v0 t + ½ c t² + b Φ(t)` to a ± torque out-and-brake
  doublet, pooled over both directions.
- **Zero error against the twin's truth:**
  - Abduction and spine ≤ 0.0024 rad.
  - Neck and jaw ≤ 0.009 rad.
  - Legs 0.012–0.027 rad. That's systematic: the stop gives a little
    under the homing current. A load-day bench number is needed for
    printed stops.
  - Tail cables 0.0 mm.
  - The gripper has no encoder. Its error is 0.056 rad, and the twin
    gives it a noisy true position, which is generous.
- **Reach check:** the front and rear pitch and knee joints verify only
  45–47 % of their travel. Their far ends are loaded by the jump
  springs (~3 N·m at the end of FR_pitch's 244°), beyond the homing
  cap. The threshold is 35 %. That's the spring, not a fault, and it's
  the smallest margin in the procedure.
- **Tail:** has no hard stop (cable length is a smooth function of
  force through nine sprung segments), so it is zeroed from the
  force-free rest at the stand pose against the length the MJCF
  predicts.

**Adversarial bodies** (the expected step is written in the runner
before the run; each case boots a fresh twin):

| case | expected | caught at | finding |
|---|---|---|---|
| forged ESC: UID ≠ build sheet (0x05) | HANDSHAKE | HANDSHAKE | `Swapped { id: 5 }` |
| forged ESC: second responder on 0x05 | HANDSHAKE | HANDSHAKE | `TwoAnswers { bus: A, id: 5 }` |
| stranger node on bus B | HANDSHAKE | HANDSHAKE | `Stranger { id: 0x20 }` |
| entity not on the body's owner list | HANDSHAKE | HANDSHAKE | `RefusedByDevice` |
| phantom limb (RR_dist claimed, ESC absent) | SCHEMA | SCHEMA | `PhantomLimb { node: 9 }` |
| IMU disconnected at power-on | SCHEMA | SCHEMA | `NoInnerEar` |
| IMU lost during calibration | CALIBRATE | CALIBRATE | `ImuLost` |
| flipped motor direction (RR_knee) | CALIBRATE | CALIBRATE | `Flipped`, motion −0.144 rad |
| dead encoder (FL_dist) | CALIBRATE | CALIBRATE | `DeadEncoder` |

**Not detected, and said so in the gate output.** An encoder mounted
mirrored, with a motor whose FOC alignment followed it, agrees with
itself, so the probe passes. Homing then finds the opposite stop with
the same travel. This is undetected on every joint. The candidate check
for Phase 2: holding current at the jig stand pose against the MJCF's
predicted gravity torque.

**Limits of what Gate 1 shows.**
- CAN has no authentication; the census catches assembly mistakes and
  naive intrusion, not a node that copies a UID.
- The twin's tail cable is a MuJoCo fixed tendon that can push. A real
  cable can only pull.
- The jig is a kinematic hold of the chest. The IMU model accounts for
  it: specific force is Rᵀg in a rigid jig.
- The Teensy firmware, the ESC/ESP32 node firmware and the USB port are
  not written. The node behaviour they must have is what `nerves.py`
  implements, and the frame formats are fixed by tests.

**Existing suites after the change** (logs in `workspace/blur_logs/`):
- `os` workspace: 793 passed, 0 failed (762 before, plus the 31 new
  brainos-blur tests).
- `mind`: green.
- `world_test`: ALL CHECKS PASSED.
- `metal_prep`: ALL PASS 43/43 when re-run alone.
  - The first run, made while the twin and a pip install loaded the
    machine, missed its 90-second "telemetry silent" boot wait on two
    checks; every later check in that run passed.
- `tools/check-no-std.sh`: brainos-blur is in the bare-metal canary, and
  it builds for `aarch64-unknown-uefi`. The x86_64-uefi failure is the
  pre-existing `sha2` LLVM bug the script documents.
  (Fixed 2026-11-10: both UEFI targets build; see `docs/OS_STATUS.md`.)

## Phase 2 — the Autonomic layer and KIRA's physical policies

### Architecture: the Autonomic spans the nodes and the Teensy

The Phase 1 CAN finding is now the design. The Autonomic layer (Domain 2)
is not one processor:

| layer | runs on | rate | holds |
|---|---|---|---|
| joint impedance loop (q*, kp, kd, τff) | each ESC / ESP32 node | 2 kHz | the motion between setpoints |
| **physical envelope** | each node, by itself | every loop | current ceiling, joint limits, kp ≤ kp_max, setpoint q clamped into range, damping if the Teensy goes quiet |
| reflexes (balance, brace, righting, stand-up, crouch), fall detection, attitude (Mahony on the BMI270) | Teensy (`supervisor.rs`) | 1 kHz step, policies at 50 Hz, setpoints at 200 Hz | the body when cognition isn't driving it |
| session envelope (narrower than physical), e-stop broadcast | Teensy | 1 kHz | what a granted session may do |
| KIRA (validate / simulate / policy), the entity | Pi | per request | what may be asked at all |

The envelope must hold even if the Teensy dies or misbehaves, so its
physical part lives on the nodes (`blur/sim/nerves.py` `Node`, which is
the specification for the not-yet-written node firmware):

- **After Commit, ceilings only narrow.**
  - The current ceiling and joint limits recorded at Commit can be
    narrowed by the Teensy (sessions do this) but never widened.
  - Calibration (offset, direction, turn count) cannot be rewritten.
  - Both reopen only while the node is uncommitted or e-stopped, when it
    delivers no torque. That is the re-acquisition path.
- **Gains are capped and setpoints clamped.**
  - kp is capped at the envelope's `kp_max`, and kd at 0.2 × `kp_max`.
  - The setpoint q is clamped into the committed limits.
- **A setpoint older than 60 ms becomes pure damping**
  (kd = 0.24 × deliverable torque per rad/s). The body sags slowly
  instead of dropping or holding a stale stiff command.

Gate 2 now tests this directly ("2b"): the Teensy goes silent for 3 s,
then turns rogue and sends widening and recalibrating writes plus
10×-gain setpoints aimed 1 rad past every limit.

Not covered: a rogue Teensy that uncommits a node and re-runs Commit
can reset its ceilings. The defence there is §8 (Commit only comes from
the entity's acquisition), not the node.

### Why the first get-up policy failed (getup_v2 diagnosis)

Five deterministic episodes were rendered as video. In 3 of them the robot
ended upright (0.999), at height (z 0.25–0.26 m) and still, yet failed the
check. FL_dist sat on its upper stop, both hind feet on their lower stops
and both hind hips on theirs, 2.39 rad from the stand pose; the tolerance
is 0.35. The same 2.39 every episode is q_max − q_stand exactly: a pinned
stop, not an offset or wrap. The descriptor, the MJCF `stand` keyframe
and `env.q()` agree (FL_dist −0.240; joint ref 0).

It was **reward farming**:
- The pose term exp(−|err|²) was 1e-9 at that crouch, so it carried no
  gradient.
- Being upright and at height alone paid 2.42 of 3.5 per step; the policy
  averaged 2.04.
- The success check was right. It also had three weaknesses: it looked at
  a single frame with no hold, the training number was measured with
  sampled (noisy) actions, and the curriculum widened on upright + height
  rather than success.

### The rebuild

- **Two skills plus a balance reflex.**
  - *Righting*: side or back → sternal (belly down, legs tucked), held
    0.5 s.
  - *Stand-up*: sternal → stand, held 2 s.
  - *Balance*: push recovery around a ready stance.
  - A *Brace* (soft tuck until the body comes to rest) runs between a fall
    and the first skill. The policies were trained from settled starts.
- **Reward that can't be earned without success** (`blur/learn/skills.py`):
  - The true reward is paid only inside the success predicate, plus a
    bonus when the hold time is reached. Costs are bounded.
  - Everything else is potential-based shaping, γΦ(s′) − Φ(s), with Φ =
    per-joint-tolerance imitation of the keyframe reference plus a body
    term. It telescopes, so no loitering policy can farm it.
  - Check (stand-up, 6 episodes per level):

    | policy | level 0 return | level 1 return |
    |---|---|---|
    | keyframe reference, open-loop | +56.8, 5/6 succeed | +8.6, 1/6 succeed |
    | getup_v2's crouch | −4.1, 0/6 | −2.7, 0/6 |
    | frozen tuck | −4.1, 0/6 | −2.8, 0/6 |
- **Success is measured on the deterministic policy.**
  - Env 0 of every other worker acts on the mean action and is masked out
    of the update.
  - The curriculum advances only when ≥ 80 % of ≥ 24 deterministic
    episodes succeed.
  - The best checkpoint is chosen by (level, deterministic success).
- **Stand-up keyframes** (`blur/learn/ref/keyframes.py`). The sternal tuck
  is the rig's `lay` legs made symmetric with the spine straight; the
  rig's own `lay` rolls the hips with spine twist 1.12, and that is what
  flipped the body in Gate 1. Open-loop from a settled tuck, 10
  randomised bodies:

  | order | success | lowest uprightness |
  |---|---|---|
  | all four together | 10/10 | 0.99 |
  | hind first | 10/10 | 0.91 |
  | front first | 9/10 | 0.96 |
  | via the rig's `sit` | 7/10 | 0.82 |

  The reference is "all four together".
- **Righting keyframes** (`blur/learn/ref/search_righting.py`).
  - A cat-style structured family: two shaping keys (hip ab/adduction,
    leg tuck, spine twist and bend, tail winches), then the tuck.
  - The numbers are searched by CMA-ES on open-loop physics, separately
    for each resting class. BLUR is not left/right symmetric: RL_abd
    reaches −0.55 where RR_abd reaches −1.01, and spine twist runs
    −0.36..+1.29.
  - In 60 random falls, BLUR came to rest on its left side 28 times, its
    back 14, its right side 12 and its belly 6.
- **Ready stance** (`blur/learn/ref/stances.py`). Plain PD against the
  gate's push range, 72 pushes each:

  | stance | survived | holding torque | chest z |
  |---|---|---|---|
  | stand | 13/72 | 0.32× driver | 0.272 m |
  | wide | 2/72 | 0.62× (sags) | 0.236 m |
  | low | 13/72 | — | — |
  | **wide + low** | **20/72** | 0.37× | 0.243 m |

  No stance survives 50–60 N passively. Stepping is the balance policy's
  job.

### Bugs fixed

- **E-stop KIRA stage.**
  - `legs.stand` during an e-stop was refused at SIMULATE ("rollout
    predicts a fall"), not POLICY. The pipeline order is canonical
    (validate 4, simulate 5, policy 6).
  - The fault was `BlurPolicy::simulate` rolling out a motion on a body
    with no motor power. The twin predicts the slumped body "falls", a
    fact about the state, not the motion.
  - Simulate now records "rollout not run" when the rail is off or a
    reflex owns the body, and stage 6 refuses unconditionally. Test:
    `a_motion_asked_of_an_estopped_body_is_refused_at_policy_not_simulate`.
- **The tally that hid it.**
  - The gate verdict used `estop_ok && kira_estop_ok`, but the summary
    printed only `estop_ok`.
  - Every PASS/FAIL line now goes through one `Tally`. Section summaries
    and the verdict are computed from those lines only, so a section
    with no lines reads "NOT RUN (FAIL)".
- **Push criterion.** A push trial printed "upright & still" after one
  sample. It now requires 2 s of continuous upright, at height, still
  (< 0.5 rad/s) and back in the standing reflex, inside 12 s.

### Phase 4 items logged here (no hardware touched)

- **Spine torque gap.**
  - A two-leg-stance bound needs spine S1 ≥ 2.14 N·m × peak-Fz/BW,
    which is 6.6–11 N·m p95 at 1.5–2.5 m/s.
  - The 4015 is rated 3.8 N·m and delivers 3.25 N·m on a 2.5 A driver.
    The spine, not speed, caps the gallop (`BLUR/RESULTS_v36.md`).
- **Try first, before any part change:** tune spine ligament springs as
  gallop energy storage, with the motor steering and the springs carrying
  load. The v36 MJCF has none (Phase 1 finding 1), so it's a model
  addition in an experiment copy; the shipped MJCF stays untouched.

### Gate 2 report: FAIL (last run 2026-10-23 11:39)

`brainos blur gate2`, log `blur/logs/gate2_after3.txt`: **FAIL — 51
checks, 20 failed.** (This heading said "pending" until 2026-11-10; the
gate had run and failed on 23 October and the report was never written
up. The numbers below are from that log.)

| section | result | detail |
|---|---|---|
| release | PASS | |
| pushes | **FAIL** | 30 pushes of 10–60 N: 9 stayed up, 4 fell and got up, **17 failed** to be upright and still within 12 s |
| envelope clamp | PASS | worst applied torque 1.0000 × the node limit; violations clamped within one 0.5 ms node-loop step |
| node envelope without the Teensy | **FAIL** | silent Teensy: PASS (nodes fell back to damping). Rogue Teensy: 75 widening writes refused, but **1 joint went past its physical range** |
| KIRA stages | **FAIL** | six of seven asks refused at the expected stage. A harmless `neck_pan` pose to 0.3 rad was refused at *simulate* (`PredictedInvariantBreak`) when it should have been granted — so nothing was granted and the Teensy-side check could not be exercised |
| e-stop | PASS | 8 of 8: no torque with the rail off, 26/26 nodes latched |

Chain evaluations from the same training (200 deterministic episodes
each, `blur/logs/chain_*.txt`): recovering from fallen **93.5 %**;
recovering from pushes **49.5 %**.

Open, in order: the push recovery; the rogue-Teensy joint-limit case;
the false refusal of the neck pose.

#### As written before the run (2026-10-22 15:50)
Training has not run yet. The InvProj1 job `s19-grid` holds 18 of 20 CPU
cores, and the standing order is never to disrupt it.
- The three trainers (`blur-train-{standup,balance,righting}`, 4 active
  hours each) and the post-training pipeline (`blur-train-post` →
  `blur/tools/after_training.sh`: export, 200-episode chain evaluations,
  Gate 1 rerun, Gate 2) are queued.
- `blur/tools/yield_guard.sh` (unit `blur-yield-guard`) keeps them frozen
  (SIGSTOP) while any InvProj1 or bpusim job is live. They thaw by
  themselves.
- Logs: `blur/logs/`. The table goes here when the gate has run.

## Track 4 — training on the AHPCC cluster (Pinnacle)

Heavy BLUR training moves to the University of Arkansas cluster. BLUR is
research (papers in progress), which AHPCC's policy allows ("research and
instructional use ... Commercial usage is not allowed"). Arjun's order:
no large allocations until he has seen 4b/4c. So far only small jobs have
run; nothing from 4d/4e has been submitted.

### 4a — the cluster (probed 2026-10-22/23, details in `~/hpcc_notes.md` on Pinnacle)

- Slurm. Login nodes run Rocky 9; compute nodes run CentOS 7 (glibc 2.17),
  so current JAX wheels need a container.
- `module singularity/3.9.7` (no apptainer binary). `--fakeroot` is not
  allowed (no `/etc/subuid` entry).
- GPUs:
  - gpu06/gpu72: 1× V100-PCIE-32GB.
  - agpu06/agpu72: 1× A100-PCIE-40GB.
  - qgpu: 4× A100.
  - Driver 550.163.01 (CUDA 12.4).
- QOS:
  - gpu needs ≥ 24 cores per job.
  - comp needs 32 (whole node).
  - cloud is for small CPU jobs (≤ 44 cores per user).
- Walltime is 6 h on the *06 partitions; the 72 h queues wait days.
- Slurm thinks every node has ~192 MB, so `--mem` requests are refused.
  Leave `--mem` out.
- Storage: `/scrfs/storage/paradkar` (Lustre, 10 TB, not backed up).
  Compute nodes have internet.

### 4b — one code tree, two sites

- **Config layer.** `blur/blurcfg.py` is now the only place any BLUR
  script gets the descriptor from: `env.py`, `nerves.py`, `fastbody.py`,
  `trainmodel.py` and, through `env`, keyframes/gaze/babble/poseview.
  - `BLUR_MJCF_DIR` overrides the laptop path recorded in `descriptor.json`.
  - `blur.xml` must hash to the descriptor's sha256, or the process exits,
    so a cluster copy can never train a different body.
  - The two hardcoded `/home/god/...moves.npz` paths are gone.
  - `train_gate2.sh` and `after_training.sh` take `BLUR_PY` and
    `BRAINOS_ROOT`; on the laptop the defaults leave behaviour unchanged.
- **Site files.** `blur/hpcc/site/{laptop,pinnacle}.env` say where things
  live. `blur/hpcc/bpy` is "python" on either site: the laptop venv, or
  `singularity exec [--nv] --cleanenv -B /scrfs <image> python`.
- **Image.** `blur-mjx-0.12-mj3.16.sif`, 4.0 GB, sha256 `b5aa2388…`, built
  by job 1367491 (cloud72, 11 min). It is `python:3.12-slim-bookworm` plus
  the laptop venv's exact versions: mujoco / mjx / warp 3.16.0,
  jax[cuda12] 0.12.0.1, numpy 2.5.4, scipy 1.18.1, torch 2.14.1+cpu, cma,
  imageio.
  - Without fakeroot, the build job takes an unprivileged route: docker →
    node-local sandbox → pip inside `--writable` → SIF.
  - That route skips apt, so the image has no EGL. `bpy` sets
    `MUJOCO_GL=disable`; nothing renders on the cluster.
- **Verification.** Each smoke job imports everything, loads and sha-checks
  the descriptor, runs the full MJCF for 1 s on CPU MuJoCo, runs
  `BlurEnv(standup)` with a torch MLP for 25 policy steps, and runs MJX
  with 64 envs × 100 steps.
  - CPU job 1367526 (cloud72, CentOS 7): **PASS**.
  - GPU job 1367520 (A100, `cuda:0`, driver 550): **PASS**. jax 0.12's
    CUDA 12.9 wheels run on the 12.4 driver.
  - Two earlier CPU attempts failed on site quirks, now fixed: bash 4.2's
    empty-array `set -u` bug in `bpy` (job 1367519), and `MUJOCO_GL=egl`
    with no EGL in the image (job 1367525).

### 4c — MJX vs CPU MuJoCo, and what a GPU buys (A100 jobs 1367639 + 1368055, V100 job 1367917)

**What was compared** (`blur/hpcc/mjx_parity.py`):

- **Model.** The full v36 MJCF does not load in MJX at all: MJX has no
  cylinder–box collision. So the comparison uses `dev/trainmodel.py`, which
  turns cylinders into capsules and ellipsoids into spheres, and keeps
  collisions only on the paws, chest, pelvis and head.
  - Actuators are env.py's stand impedance as position servos, clamped at
    the deliverable torque.
  - 1 ms step, implicitfast integrator, elliptic cone, the stand keyframe.
  - The speed derate is left out (it rewrites the model every 5 ms), on
    both engines.
- **Two fixed target sequences, with floor contact throughout.**
  - **walk** (5 s): 1 s of standing, then the rig's walk cycle as
    open-loop joint targets. The body stumbles and ends lying down.
  - **drop** (3 s): stand targets, with the body released 12 cm up at 25°
    roll and 10° pitch. It hits the floor head- and shoulder-first and
    ends on its side.
- **Reference.** CPU MuJoCo in float64 on the same model, with MuJoCo's
  solver defaults (Newton, 100/50 iterations, tolerance 1e-8).
- **Errors.** Measured on the 26 actuated joint coordinates, the base
  pose, and per-paw normal ground-reaction force (GRF). Body weight is
  36.0 N.

**Two CPU-only yardsticks:**

| CPU vs CPU | walk: t(err > 0.01 / 0.1 rad) | walk max err | drop: t(0.01 / 0.1) | drop max err |
|---|---|---|---|---|
| initial qvel + 1e-9 (chaos floor) | never / never | 8e-11 rad | never / never | 9e-10 |
| initial qvel + 1e-6 | never / never | 8e-8 | never / never | 6.4e-4 |
| **full MJCF vs training model** | 2.09 s / 2.15 s | **0.16 rad**, base 6.4 cm | 0.54 s / 0.55 s | **0.39 rad** |

**MJX on the A100 vs that CPU reference** (same model; joint error in rad;
GRF error as RMS over the run / largest single-sample error):

| variant | walk max (1 s / 5 s) | walk joint RMS | drop max (0.5 s / 3 s) | t(err > 0.01) walk / drop | t(err > 0.1) | base pos / ori max | GRF RMS / peak (walk) |
|---|---|---|---|---|---|---|---|
| f64, 1 ms | 1e-8 / 0.0032 | 1.2e-4 | 0.009 / 0.060 | never / 0.55 s | never | 4 mm / 1.5° | 0.59 / 13 N |
| f64, 0.5 ms | 1e-8 / 0.0043 | 1.6e-4 | 0.0046 / 0.022 | never / 0.56 s | never | 2 mm / 0.5° | 0.66 / 13 N |
| f32, MuJoCo defaults | 3e-5 / 0.0053 | 2.2e-4 | 0.009 / 0.060 | never / 0.55 s | never | 4 mm / 1.5° | 0.74 / 14 N |
| f32, tolerance 1e-6 | 2e-5 / 0.0135 | 5.4e-4 | 0.009 / 0.060 | 2.26 s / 0.55 s | never | 4 mm / 2.4° | 0.74 / 14 N |
| f32, tolerance 1e-6, 0.5 ms | 5e-5 / 0.0030 | 1.7e-4 | 0.0046 / 0.022 | never / 0.56 s | never | 2 mm / 0.5° | 0.71 / 13 N |
| **f32, training solver (5 iter / 8 line-search)** | 3e-5 / **0.0205** | 1.1e-3 | 0.009 / **0.059** | 1.97 s / 0.55 s | **never** | 4 mm / 2.0° | 0.87 / 14 N |
| f32, training solver, paws-only collisions | 2e-5 / 0.048 | 1.8e-3 | 0.009 / 0.075 | 1.96 s / 0.67 s | never | 13 mm / 5.2° | 2.14 / 59 N |
| MJX-Warp backend | — | — | — | — | — | — | failed (`FFI callback IndexError`), not used |

What this says, honestly:

- **On the same model, MJX reproduces CPU MuJoCo.** No variant crossed
  0.1 rad within 5 s of open-loop walking or a 3 s fall.
  - Joint RMS error is ≤ 1e-3 rad and base error ≤ 4 mm.
  - GRF RMS error is 0.6–0.9 N, about 2 % of body weight.
  - The largest single GRF errors (13–19 N) sit on single impact samples,
    where a contact switches one step earlier or later.
- **The impact error is not precision.** In the drop, the 0.009 → 0.06 rad
  error is the same in f32 and f64, and the same with 5 or 100 solver
  iterations. It halves at a 0.5 ms step, so it is a time-discretisation
  difference between MJX's and MuJoCo's contact/integration code.
  - Nearly all of it is in the neck (neck_pan 0.06, neck_lift 0.03 rad):
    the head hits the floor.
  - Leg joints stay ≤ 0.009 rad.
  - The fix, if it matters, is the 0.5 ms step (0.022 rad), at twice the
    cost.
- **The training solver (5/8 iterations, float32) is good enough** for
  locomotion. After 2 s of walking its error grows to 0.02 rad, against
  0.003 for f64; it never reached 0.1 rad.
- **The bigger mismatch is the model, not the engine.** The MJX-able
  training model diverges from the full v36 MJCF, both on CPU, at
  0.55 s (drop) and 2.15 s (walk), reaching 0.16–0.39 rad. That is 3–20×
  the MJX error. So a policy trained in MJX has to be re-validated on the
  full MJCF behind `nerves.py`, as trainmodel.py already says.
  Paws-only collisions are not physical for falls: the body sinks through
  the floor.
- **Not tested:** the speed derate, domain randomisation, latency, and the
  observation/IMU path. Those live in env.py on the host, and an MJX
  training env would have to re-implement them in JAX.

**Throughput** (`mjx_throughput.py`: N envs × 200 physics steps of 1 ms in
one jit, stand targets plus a 2 Hz ±0.15 rad sinusoid; env-steps/s, with a
50 Hz policy-step rate = ÷20):

| GPU | contacts (MJX slots) | solver | 1024 envs | 4096 | 8192 |
|---|---|---|---|---|---|
| A100 | training model (233) | 5/8 | 1.14k (see note) | **22.3k** | **23.0k** |
| A100 | paws only (4) | 5/8 | 91.6k | **177k** | 160k |
| A100 | training model (233) | defaults 100/50 | 1.05k | — | — |
| V100 | training model (233) | 5/8 | 9.0k | **10.2k** | (timed out) |
| V100 | paws only (4) | 5/8 | 64.8k | 91.4k | **99.2k** |

Notes:
- First-call compile takes 1–4.5 min per configuration.
- The A100's 1024-env training-model point is reproducibly slow: 179 s per
  200-step call in job 1367639 and again in job 1368055, after an untimed
  warm-up call. The same call takes 36.7 s at 4096 envs. The cause is not
  known (probably how XLA tiles that batch size). Don't train at 1024 envs
  on an A100.
- The tolerance-1e-6 run (A100) and V100 8192 with the training model hit
  their timeouts. The V100's float32 parity matches the A100's in the drop
  (0.0596 rad) and is smaller in the walk (0.0056 vs 0.0205 rad). The
  chaotic part of a float32 rollout differs between GPUs.

**Comparison: CPU MuJoCo on the same model** runs 7.1–9.3k steps/s per core
(measured in the same job on the A100 node's EPYC 7543, including
Python-side logging; the comp nodes' Xeon 6130 cores are likely somewhat
slower). A 32-core comp
node is therefore about 230–300k env-steps/s, and the 24 cores that a
gpu-QOS job must reserve anyway are about 170–220k.

- MJX on the training model is ~23k/s, roughly **10× slower than the CPU
  cores of the same job**.
- 188 of the 233 contact slots are the torso's 47 boxes (box–plane = 4
  slots each). With paws only, MJX reaches ~170k/s, about the speed of the
  CPUs, but cannot simulate a fall.
- Conclusion for now: GPU training is **not** worth it for BLUR as
  modelled. Getting it there needs a training model whose torso/head
  collide through ~10 capsules/spheres instead of 47 boxes. That would be
  another model-gap measurement against the full MJCF before anyone
  trusts it. Until then, the existing CPU PPO (`learn/ppo.py`, full MJCF)
  on comp nodes is the faster path.

GPU time used in 4b/4c:

| job | GPU | wall time | outcome |
|---|---|---|---|
| 1367520 | A100 | 3.7 min | smoke |
| 1367539 | A100 | 3.4 min | parity bug, then killed by a code sync; fixed: jobs now run from a per-job code snapshot, `site/snapshot.sh` |
| 1367639 | A100 | 65 min | |
| 1367917 | V100 | 34 min | |
| 1368055 | A100 | 11 min | re-measure at 1024 envs |

Total: **1.96 GPU-hours** (1.39 on the A100, 0.56 on the V100), at 24 cores each. CPU jobs: 1367491
(image), 1367519/1367525/1367526 (smokes), 1367632/1367635 (MJX-on-CPU
code checks), all on cloud72 with 4 cores.

Raw JSON and logs: `blur/hpcc/results/4c/`.

### Proposed 4d/4e allocation (written 2026-10-24; approved 2026-10-25, CPU plan only)

**Day one:**
- 7 skills: walk, sit, lie, stand-up, small hops, get-up, balance/push
  recovery.
- × 5 seeds = 35 runs, with the wider domain randomisation from the brief.
- Each run is the existing CPU PPO (`learn/ppo.py`, full MJCF, 30 workers)
  on one 32-core comp06 node.
- The 6 h walltime needs checkpoint/resume from 4d.

**Estimated cost:**
- On the laptop, getup_v2 did 27.6M steps in 5 h with 18 workers. That is
  ≈ 85 policy steps/s per core, so ≈ 2.7k/s on a node.
- 50M steps ≈ 5–6 h, so one comp06 job per run.
- 35 node-jobs ≈ **210 node-hours ≈ 6.7k core-hours of comp QOS, 0 GPU-hours**.
- Plus the deterministic stress evals: 200 episodes per run on cloud72, small.
- At 4 nodes concurrently (the BLUR share, under lab ≤ half of the account
  limits), this is about 2.5 days of wall time.

**Optional GPU line (only if Arjun wants MJX):**
- 2 × 6 h A100 jobs (12 GPU-hours) to build a capsule/sphere torso
  collision model.
- Measure its gap to the full MJCF the same way as above.
- Re-measure throughput.
- Only if it gets ≥ 5× a comp node is an MJX trainer worth writing.

**Decision (2026-10-25).** Arjun: "do what you think is best". The CPU plan
goes ahead; the GPU line is skipped for now.
- The account's GPU share is saturated by WhiteBox, OMEN and BPU E15.
- MJX would need a new body model anyway (capsule/sphere torso, then a gap
  measurement against the full MJCF).
- **Later option:** MJX training, after a capsule-torso training model has
  been measured against the full MJCF, aiming for ≥ 5× a comp node.

### 4d — the job system (2026-10-25)

**One run = one (skill × seed × config) line** in a campaign manifest
(`blur/hpcc/campaigns/<name>/manifest.txt`). Runs are executed by a few
**singleton lanes** (`blur/hpcc/jobs/lane.sbatch`, modelled on WhiteBox's
`k12_lane.sbatch`), not by Slurm arrays.

**What a lane does:**
- It is a comp06 job (`-q comp -c 32`, 6 h, `--dependency=singleton`
  under its own name).
- It claims the next unfinished run with a `mkdir` lock and runs it.
- When it is out of time, it resubmits itself.

**Lane guards:**
- `HOME`/`USER` defaults (`: "${HOME:=/home/paradkar}"`).
- At start and at every resubmission, the account's running comp-QOS cores
  must be ≤ 96; otherwise the lane resubmits itself with `--begin=now+15min`.
- A failed or empty `squeue` counts as "busy" and "lock holder alive",
  never as "nothing running".
- Every lane job's node-time is logged. Past the campaign cap (210
  node-hours), no lane resubmits (`STOP_BUDGET`).
- A crashed run is marked failed and is not retried blindly.

**A run** (`blur/hpcc/run_one.sh`):
- **Train:** `learn/ppo.py --auto-resume`, with a checkpoint every 15 min.
  At the lane deadline it checkpoints and exits 75; the next lane job
  resumes it exactly.
- **Evaluate:** `learn/eval.py`, deterministic (policy mean action),
  200 episodes, in two modes:
  - **nominal**: no randomisation, flat ground;
  - **stress**: every 4e.1 range pinned at an edge, uneven and softest
    ground.
- Then `DONE`.
- Each skill also gets a **reference** run (`ZERO:<skill>`): its reference
  motion played open loop, evaluated the same way. A policy has to beat
  this to mean anything.

**Exact resume.**
- A checkpoint holds the network, the optimiser, the observation
  normaliser, the RNG states, the curriculum and windows, and each env's
  state.
- Each env is saved as its MuJoCo integration state, its Python state, and
  the model arrays that differ from a fresh build. The 60 MB-per-env
  MjModel itself is not stored.
- Checkpoint size: ~5 MB per 8 envs.
- At a checkpoint the envs are **canonicalised**: state written back,
  then `mj_forward`. The run that continues and the run resumed from disk
  therefore continue identically.
- **Tests:**
  - Bitwise on the laptop (balance) and on Pinnacle (job 1368258: walk,
    righting, hop). 30 iterations straight vs killed at 23 (last
    checkpoint 20) and resumed: weights and every progress row identical,
    with episodes ending and resetting inside the window.
  - End to end through a real lane deadline (cloud72 lane test, jobs
    1368260 → 1368267): righting was checkpointed at iteration 63,
    resumed in the next job, finished, and evaluated.

**Per-campaign code freeze.** `campaign_setup.sh` copies the synced tree
to `campaigns/<name>/code` and records a code hash. Every lane job runs
that copy, so a later sync can't change a run halfway.

**Results back to the repo.** `blur/hpcc/fetch_runs.sh <campaign>` (laptop)
copies run.json, progress.csv, policy_best.json, the eval JSONs and the
logs into `blur/learn/runs/hpcc_<campaign>/`. It leaves ckpt.pt and *.pt
on Pinnacle. Then `blur/hpcc/summarize.py` writes `SUMMARY.md`:
- every run ranked by deterministic nominal success, with stress beside it
  (95 % Wilson intervals), the skill's reference beside both, and peak
  torque under stress;
- the best run per skill, chosen by stress success;
- lane node-hours.

### 4e step 1 — day-one skills (launched 2026-10-25)

**Skills.** 7 skills × 5 seeds = 35 runs, plus 7 references.
- From skills.py: `standup`, `righting` (= get-up, fall → sternal),
  `balance` (push recovery).
- New in `learn/dayone.py`, same reward discipline (true reward only inside
  the success predicate, potential-based shaping toward a reference):
  - `sit`: stand → the rig's sit pose, held 2 s;
  - `lie`: stand → the rig's lay pose, held 2 s;
  - `walk`: 0.08–0.20 m/s for 8 s, judged on the last 4 s for speed,
    drift and heading, residual around the rig's walk cycle;
  - `hop`: ≥ 2 flights, then stand still for 1 s.
- Open loop on nominal physics:
  - sit and lie already succeed.
  - walk falls.
  - hop gets 2 flights but falls.
  - The references are in the table so this is visible.

**Randomisation** (`BLUR_DR=wide`, env.py `DR_RANGES['wide']`):
- link masses ±20 %;
- ground friction 0.3–1.3;
- motor strength ±25 %;
- latency 2–19 ms (20 ms would be a whole policy step);
- encoder noise to 0.006 rad, IMU bias ×2;
- soft ground: contact time constant 0.02–0.06 s;
- uneven ground on half of the episodes: a 6 m height field of smooth bumps
  up to 1.5 cm.
  - The height field has to be compiled with collisions on: MuJoCo ignores
    a geom compiled with collisions off even if they are enabled later.
  - It is toggled per episode against the plane.
- The laptop default (`v1`) is unchanged.

**Budget and running.**
- Each run is 3 active hours on 28 workers × 4 envs, about 15M policy
  steps. On cloud72, Xeon 6130 cores run ~55 policy steps/s each with the
  terrain.
- Estimate: ~120–130 node-hours total, against a cap of 210 that the lanes
  enforce.
- Lanes `blur-lane-1` (job 1368269) and `blur-lane-2` (job 1368270).
  - Lane 1 started 08:36 on 2026-10-25 (c1404) and evaluated the 7
    references in 6 min.
  - It began `walk_s0` at 08:42. Training measured ~830 policy steps/s per
    comp node (walk, wide DR), so a 3 h run is ≈ 9M steps. For comparison,
    the laptop's v2 runs were 6.5–7.5M steps.
  - Lane 2 was queued, with an estimated start of 13:18.
- With 2 lanes, ~60–65 h of lane time, so finishing around 28–29 Oct
  2026 plus queue gaps between 6 h segments.

**References** (the skill's reference played open loop; deterministic,
200 episodes each):

| skill | nominal | stress |
|---|---|---|
| balance | 24 % | 18 % |
| hop | 0 % | 2 % |
| lie | 44 % | 22 % |
| righting | 64 % | 34 % |
| sit | 44 % | 32 % |
| standup | 56 % | 49 % |
| walk | 0 % | 0 % |

A trained policy counts only by what it adds over these numbers.
- Watch: `blur/hpcc/fetch_runs.sh dayone` then
  `blur/learn/runs/hpcc_dayone/SUMMARY.md`.
- Ranked table: pending.
