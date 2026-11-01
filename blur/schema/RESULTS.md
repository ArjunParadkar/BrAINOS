# Track B: BLUR's body schema, "inspired by OMEN (Claudi & Fiete)" (2026-10-22)

Pre-registration, with three amendments all made before any model latent's
homology was computed: `PREREGISTRATION.md`. Everything below is produced
by the scripts in this directory. Raw outputs are in `runs/`.

## Gate B, in one paragraph
**Gate B is not passed.**
- OMEN (B0) is a supervised decoder with fixed, untrained attractors, so it
  cannot be "run in reverse". The model built instead uses OMEN's temporal
  CNN encoder and no topology prior.
- End-to-end learned routing of channels into modules did not reproduce
  across seeds. A deterministic predictive-information grouping did, and
  it is coherent: a joint's command, encoder and current land together,
  the IMU forms its own module, and the camera groups with neck pan.
- The persistent-homology (PH) check fails its own attitude control. Its
  module results, read as observations, falsify Arjun's predictions P1–P3:
  - the orientation module is contractible, not S² or SO(3);
  - leg modules are contractible, as the physics said they must be;
  - nothing encodes head yaw.
- One unpredicted loop reproduces across seeds.
- The grown gaze reflex is indistinguishable from no reflex. The
  hand-tuned PD helps under pushes.

## B0: what OMEN is (read-only; `/home/god/MIT Fiete Lab/OMEN_rerun/OMEN-main`)
- The authors are Federico Claudi and Ila Fiete (MIT). Arjun re-ran their
  code; it is not his model.
- A "can" is a **CAN**, a continuous attractor network: 48 units on a ring
  or a line with hand-set weights W_ij = k(d(θ_i, θ_j)) − 1, **never
  trained** (`omen.py` `get_params` returns only the encoder and heads).
  There is one CAN per labelled behaviour variable, and the config's
  `periodic` flag chooses ring or line.
- M (behaviour) is assumed known and is a product of rings and lines. N
  (neural) is not modelled. The training loss is R² against **behaviour
  labels**, so OMEN is supervised.
- Running it in reverse is not possible:
  - with no labels there is no loss;
  - a ring CAN can only output S¹, so finding S¹ in it would be circular;
  - SO(3) cannot be built from rings and lines.
- Reused: `ConvLayer` and `tCNN` (`omen_tcnn.py`, attributed). The repo has
  no LICENSE file; check with the authors before any release.

## B1: babbling (`babble.py`, `data/babble_s0.npz`)
- 10 sim-minutes at 50 Hz, 30,000 samples, 6,000 camera frames.
- The body is held on a gimbal that a handler turns through uniformly
  random attitudes. A fixed stand would never let it see its orientation
  change.
- Joints babble (Ornstein–Uhlenbeck) inside 10–90 % of their range at half
  the deliverable torque.
- Channels: efference copy 26, encoders 26, currents 26, IMU 6, and a head
  camera (16 principal components). The camera and coloured landmarks are
  added in memory; the MJCF has no camera and is untouched.

## B2: the modules
**End-to-end learned routing (not judged; Amendment 3):**

| model | joints with cmd/enc/cur together | seeds agree (ARI) |
|---|---|---|
| v1, one-step autoencoder | 27 % | — |
| v2, 200 ms forward model with hard routing | 58 % / 46 % | 0.056 |
| v3, faster routing | collapsed to 2 modules | — |

**Judged: the predictive-information grouping (`anchor_groups.py`).**
- Affinity is how much channel i's past 0.64 s improves the 200 ms-ahead
  linear prediction of channel j.
- Spectral clustering gives K = 12, the smallest K in 8..16 whose largest
  module holds ≤ 30 % of the channels. ARI is 1.0 across clustering seeds.
- Each module then gets a 6-D latent from the forward model (fixed
  assignment). Validation error: 0.557 and 0.561 on two seeds, against
  0.893 for the channel-shuffled control.

| module | channels | best-decoded variable (5-fold kNN R²) |
|---|---|---|
| m0 | spine s1/s2 + tail pitch winch (cmd/enc/cur) | spine_s1 0.85 |
| m1 | camera PCs 1–15 + neck_pan + FL_abd + gripper | gravity direction 0.46 (the camera sees the horizon) |
| m2 | hip abductions + rear pitch + spine twist | RR_abd 0.58, spine_twist 0.54 |
| m3 / m5 | RR_knee / RL_knee (cmd, enc, cur) | 0.93 / 0.90 |
| m4 | FL pitch + knee | FL_knee 0.93 |
| m6 | FR_pitch | 0.91 |
| m7 | tail yaw winch | 0.39 |
| m8 / m10 | RR_dist / RL_dist | 0.99 / 0.94 |
| m9 | front knees and feet + neck lift/tilt + jaw + camera PC0 | FL_dist 0.84 |
| **m11** | **all 6 IMU channels, alone** | **gravity direction 0.81** |

## B3: persistent homology
- Vietoris–Rips via ripser over Z/2 and Z/3, 400 farthest-point samples
  per cloud, maxdim 2. The rule and its amendments are in the
  pre-registration.
- The pipeline on known spaces (`ph_validate.json`) passes all five: S¹,
  S², T², the cube, and SO(3), including SO(3)'s Z/2 (1,1,1) against
  Z/3 (1,0,0).

**Controls on this data:**

| control | Z/2 (b0,b1,b2) | Z/3 | verdict |
|---|---|---|---|
| true gravity direction (S²) | 1,0,1 | 1,0,1 | pass |
| true chest attitude (RP³) | 1,1,2 | 1,0,1 | **fail**: b1 drops Z/2→Z/3 as RP³ should, but both fields carry an extra b2 |
| 6-D Gaussian | 1,0,0 | 1,0,0 | pass |
| 12 modules trained on channel-shuffled data | all 1,0,0 | all 1,0,0 | pass |

The attitude control fails. **By the pre-registered rule, no module result
below is a finding**; they are reported as observations. The likely cause
is the gimbal's uneven coverage of SO(3) at 400 points; that has not been
tested.

| prediction | module | Z/2 | Z/3 | outcome |
|---|---|---|---|---|
| P3: orientation → SO(3) or S² | m11 (IMU) | 1,0,0 | 1,0,0 | **fails**: contractible (seed 1 the same) |
| P2: a leg → torus | m3, m4, m5, m6, m8, m10 | 1,0,0 | 1,0,0 | **fails**, as the physics predicted (no leg joint turns fully round) |
| P1: head yaw → S¹ | none encodes world head yaw | — | — | **fails**: not represented |
| P4: tail → interval | m7 | 1,0,0 | 1,0,0 | consistent, but Betti numbers can't tell an interval from a point |
| (not predicted) | m2, hips + spine twist | 1,1,0 | 1,1,0 | **a loop, reproduced on seed 1**; physically this joint space is contractible, so it is more likely the encoder folding it than a feature of the body; unexplained |

## B4: gaze lock (`gaze.py`, `grown.py`)
The neck has no roll joint, so roll can't separate the arms (pre-registered).

The grown reflex sees only m11's latent and the neck encoders. It cancels
retinal slip with slip ≈ J_b·Δz + J_n·Δneck, fitted on the babbling log
with slip = the image shift by phase correlation, then Δneck =
−pinv(J_n)·J_b·Δz with a 2 s leak. The first version used camera-PC change
as slip; it fit R² 0.006 and was dropped before any test. The version
tested fits R² 0.13, with gain norm 0.057.

The hand-tuned PD has gains from a grid search on 4 calibration episodes
judged on ground truth, which gives it an advantage the grown reflex does
not have.

| scenario, 20 episodes | arm | pitch RMS | gaze drift RMS | roll RMS | fell |
|---|---|---|---|---|---|
| push 5–15 N (PD-held stance; no balance policy yet) | none | 0.131 | 0.178 | 0.113 | 2 |
| | hand-tuned PD | **0.109** | **0.116** | 0.089 | 0 |
| | grown | 0.138 | 0.180 | 0.109 | 2 |
| walk gait on a compliant harness (stand-in: no locomotion policy exists; the rig's open-loop walk falls) | none | **0.038** | 0.162 | 0.010 | 0 |
| | hand-tuned PD | 0.128 | **0.142** | 0.019 | 0 |
| | grown | 0.039 | 0.161 | 0.010 | 0 |

**The grown reflex loses. It is indistinguishable from no stabilisation.**
Why:
1. Its slip model explains 13 % of the image motion: 10 Hz frames at
   48×64, with the gimbal turning up to about 2.4 rad/s.
2. The orientation module it reads is not a clean orientation map. Its
   latent is contractible, and gravity decodes only at R² 0.81.
3. The learned gain is therefore tiny.

What would fix it, in order:
1. Measure slip at the control rate: a 50 Hz camera, or optical flow
   instead of a whole-image shift.
2. Babble the neck while the body turns slowly, so head and body motion
   are decorrelated in the data that J_n is fitted on.
3. Fit the slip model to the gyro channels inside m11 rather than the 6-D
   latent, with a state-dependent J.
4. More babbling than 10 minutes, so the attitude coverage passes its own
   control.

The hand-tuned PD's mixed result (better gaze, worse pitch on the walk)
says the baseline also needs more than a 4-episode grid.

## B5: architecture (built so far)
- `Entity::learn_body_schema(body, parts)` stores the schema in the state
  graph: one summary node and one node per module, of kind `Calibration`
  (not prunable), provenanced to the body, linked in order. It is
  test-covered (`entity_suite`: `a_body_schema_is_kept_in_the_state_graph_…`).
- Model weights do not fit a 1 KiB graph node. What the graph holds is the
  assignment, the per-module summaries and the topology readings. The
  weights are a cache that can be regenerated from the babbling run.
- A grown reflex would run in the Teensy supervisor like the hand-made
  ones, and its targets would pass the same envelope clamp (`send()`) and
  the nodes' own limits. None is deployed, because the one built lost.
- **Not done:** the host step that renders `ph.json` into graph parts after
  §8 Calibrate. Proposed next grown reflexes (tail counterbalance, a
  righting orientation sense) are not proposed, because the gate's
  condition for proposing them (the grown reflex matches or beats
  hand-tuned) was not met.
