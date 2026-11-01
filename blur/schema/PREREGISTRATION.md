# Track B: pre-registration (written 2026-10-22, before any latent was analysed)

Body schema "inspired by OMEN (Claudi & Fiete)". This file records what is
predicted and how it will be judged, before the model's latents are looked at.

## Data (B1)
10 sim-minutes of motor babbling at 50 Hz on a gimbal stand (`babble.py`,
seed 0). The handler turns the gimbal through uniformly random attitudes, so
the chest's orientation covers SO(3). Joints babble inside 10 %..90 % of their
range at half the deliverable torque. Channels: efference copy 26,
encoders 26, currents 26, IMU 6, head camera 16 PCs (6 coloured posts at
irregular bearings, so headings are distinguishable).

## Arjun's predictions (from the brief)
P1 head yaw → S¹; P2 a single leg's joints → torus; P3 body orientation → SO(3)
(or its projection); P4 tail → interval.

## What the physics allows (stated before running)
- **P2 cannot hold for joint space.** No leg joint turns fully round (the
  largest travel is 244°, the front pitch). A leg's joint space is a product
  of intervals, which is contractible: b0=1, b1=b2=0. A torus in a leg module
  would be an artefact of the model, not the body.
- **P1 for the neck alone cannot hold.** `neck_pan` travels 82°, so head yaw
  relative to the body is an interval. Head yaw in the WORLD is S¹ only
  because the gimbal turns the whole body through full yaw. It is visible only
  to the camera (the IMU has no magnetometer, and integrated gyro yaw drifts).
- **P3 depends on the sensors a module takes.** The accelerometer alone sees
  only the gravity direction, which is S² (b0=1, b1=0, b2=1 over any field).
  Gravity plus a heading cue from the camera determines the full attitude,
  SO(3) ≅ RP³:
  - over Z/2: b1=b2=b3=1;
  - over Z/3: b1=b2=0, b3=1.

  Both coefficient fields are computed for every module (Arjun's
  non-circular signature).
- **P4:** the tail is cable-driven, with two winch channels of 0.098 m and
  0.039 m travel. Each is an interval, and both together are contractible.

## Judging rule (fixed now)
For each module that is assigned at least one channel:
1. Standardise the latent. Take 1500 points by farthest-point sampling (800
   for maxdim 3).
2. Vietoris–Rips persistent homology with ripser over Z/2 and Z/3, maxdim 2.
   Use maxdim 3 if the module's PCA dimension at 95 % variance is ≥ 3.
3. A bar counts as a feature if its persistence is at least 0.15 × the
   cloud's diameter AND at least 5 × the median persistence in that degree.
   The Betti estimate is the count of such bars. Dimension 0 counts
   components the same way: bars that die late, plus the infinite one.
4. Controls run through the identical pipeline:
   - **positive:** the ground-truth gravity direction (expect S²) and the
     ground-truth chest rotation matrices (expect RP³);
   - **negative:** 6-D Gaussian noise (expect a point) and the latent of a
     model trained on time-shuffled data.

   If the controls fail, the pipeline is wrong and no module result is
   reported as a finding.
5. What a module encodes is reported from data: the cross-validated
   k-nearest-neighbour R² from its latent to each ground-truth variable
   (gravity direction, world head yaw as cos/sin, each joint angle). The
   model never sees these.

A prediction holds only if the module that best encodes the variable shows
the predicted Betti numbers by the rule above. Anything else is reported as
what emerged.

## B4 gaze lock: what is controllable (stated before the test)
BLUR's neck has three joints:
- `neck_lift` and `neck_tilt` both rotate about the body's y axis (pitch);
- `neck_pan` rotates about z (yaw).

**There is no roll joint.** No neck controller, hand-tuned or grown, can
reduce head roll, so head roll RMS is reported but cannot separate the two.
The comparison is decided on head pitch RMS and gaze drift (the angle
between the head's forward axis and its initial direction).

Ranges limit the neck's reach to what the test body tilts can use: lift
−0.62..0.88, tilt −0.90..0.76, pan −0.87..0.56 rad.

Arms, all evaluated in the same seeded episodes:
1. neck held at the stand pose (no stabilisation);
2. a hand-tuned PD stabiliser on the Mahony attitude, with gains chosen by a
   small grid search on separate calibration episodes;
3. the grown reflex, which sees only the orientation module's latent and the
   neck encoders.

## Amendment 1 (2026-10-22, before any model latent was analysed)
The first run of the pipeline validation (synthetic spaces only) was
OOM-killed at 15.5 GB: Vietoris–Rips up to H2 on 1500 points is too large
for this machine. Changes:
- 400 points per cloud, and maxdim 2 everywhere. H3 is not computed.
- This loses nothing the predictions need. Over H1/H2, S² reads
  (b1, b2) = (0, 1) over both fields, while RP³ reads (1, 1) over Z/2 and
  (0, 0) over Z/3.
- The significance rule is unchanged.
- The validation on known spaces (S¹, S², T², cube, SO(3)) must pass before
  any module is judged.

## Amendment 2 (2026-10-22, before any model latent was analysed)
The validation on known spaces exposed a defect in the significance rule.
S¹ came out (1,0,0) and S² (1,0,0); T², the cube and SO(3) were correct,
including Z/2 (1,1,1) against Z/3 (1,0,0) for SO(3). When a degree has a
single bar, "≥ 5 × the median persistence in that degree" compares the bar
with itself and fails.
- The rule is now: ≥ 0.15 × diameter AND ≥ 5 × the median of the OTHER bars
  in that degree. With no other bars, the second condition is vacuous.
- The validation is re-run with the amended rule. It must pass all five
  known spaces before any module is judged.

## Amendment 3: which module model is judged (2026-10-22, before any PH on model latents)
Topology-blind model selection, recorded before any latent's homology is
computed.
- **End-to-end learned routing failed on its own terms.**
  - v1 (one-step autoencoder): only 27 % of joints kept their cmd/enc/cur
    together.
  - v2 (200 ms forward model, hard routing): 58 % and 46 % coherent on two
    seeds, but cross-seed ARI 0.056, so the grouping does not reproduce.
  - v3 (fast routing logits, stronger pruning): collapsed to 2 modules.
  - None of these is judged as "the body schema".
- **The assignment judged is the predictive-information grouping**
  (`anchor_groups.py`).
  - Affinity is how much one channel's past 0.64 s improves the linear
    prediction of another 200 ms ahead, symmetrised; then spectral
    clustering. It is learned from the babbling, unsupervised,
    topology-free and deterministic: ARI 1.0 across clustering seeds.
  - K is the smallest in 8..16 whose largest module holds ≤ 30 % of the
    channels, which gives K = 12. The eigengap picks K = 8, but that
    makes a 60-channel catch-all.
- Each module's 6-D latent is then learned by the forward model (OMEN's
  tCNN encoder) with this assignment fixed. The same model on
  channel-shuffled data is the negative control.
