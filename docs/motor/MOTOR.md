# Motor memory (Workstream E)

Branch `motor`, off `nerves-x86-bpu`. Written 2026-11-13.
Thresholds were fixed in [`PREREGISTRATION.md`](PREREGISTRATION.md)
before any gate was run. Evidence is in [`artifacts/`](artifacts/).

**Everything here is simulation.** No motor has moved. The list of what
that leaves unknown is in "Verified only in simulation" below.

## If you read one section

A desk arm in simulation was shown **one** demonstration of taking a
brush, wiping the inside of a mug and putting the brush back. From that
it wrote a three-step recipe (grasp, wipe, place) with its lengths tied
to the mug, and ran it on 100 mugs it had not seen, of other sizes, in
other places: **99 succeeded.** A network cloned from the same
demonstration succeeded on **0**, and on 0 when given 50 demonstrations.

Every one of those runs went through the gate. KIRA was asked 4 times
per run (once to hold posture, once per primitive) and never during a
movement; in between, the arm's own loop checked every tick against what
had been granted, and in 100,000 adversarial ticks let none through
that was outside it.

Four gates are **failing** on at least one pre-registered row and are
reported that way: E1, E3, E6, E7. One (E2) could not be run. One (E5)
was run in a reduced form. Two things this work was told to stand on do
not exist or are not as described, and both matter:

- **LOCI does not exist.** Preconditions and composition use a stand-in
  (`motor::condition`). Nothing here says anything about LOCI.
- **The hexapod's reference kinematics describe a leg that cannot be
  built** from its own bill of materials (found by E1; details below).
  The hand-tuned gait does a third of its planned speed as a result.

## Gate table

| Gate | Status | Result | Evidence |
|---|---|---|---|
| E1 self-model matches references | **Failing** (2 of 8 rows) | BLUR skeleton: 73 bodies within 0.0001 mm, 53 joint ranges, 24 force ranges, mass exact. Hexapod linked pairs and joint ranges pass. Hexapod foot position off by up to 42 mm and arm shoulder torque off in 57/200 poses; both traced to the reference, not the generator | `artifacts/e1_check.txt` |
| E2 gait from the self-model | **Built, not run** at full size | Pipeline complete; int8 runner bit-exact 1000/1000. A 45 s smoke run learns no gait on firm ground | `motor/gait/README.md`, `artifacts/e2_eval_firm_ground.json` |
| E3 primitive success rates | **Failing** (2 of 6 below threshold, 1 not run) | grasp 100/100, place 100/100, wipe 99/100, push 82/100 (needs 85), turn 82/100 (needs 85), pour not run | `artifacts/e3_gate.txt` |
| E4 one demonstration (headline) | **Verified in simulation** | recipe 99/100 (threshold 80), cloning 0/100 (claimed margin 30 points) | `artifacts/e4_gate.txt` |
| E5 the curve | **Not run as registered**; reduced run on one task | recipe needs 1 demonstration once grasp, place and wipe are in the library; cloning does not reach 80 % with 50 | `artifacts/e5_reduced.txt` |
| E6 fluidity | **Failing** (1 of 4 rows) | pauses 0.06 of baseline, peak force 0.63, success 18/20 against 7/20: pass. Integrated squared jerk 0.76 against a threshold of 0.50: fail | `artifacts/e6_gate.txt` |
| E7 reflexes, cognition killed | **Failing** (1 of 3 rows) | touch 12/12 in the same tick, shove recovered in 35 to 41 ms, identical after a real SIGKILL. Slip answered in 9/12 | `artifacts/e7_gate.txt` |
| E8 mandate structure | **Verified** | compile-fail tests; 100,000 random ticks over 264 real grants, 0 applied outside; 0 gate calls during a run | `artifacts/e8_e9_rust.txt` |
| E9 standing behaviours and the stream | **Verified** for posture and the stream; gaze and balance **Not built** | posture and brush held 10 s after SIGKILL, 10/10; 599,000 ticks add 0 memories; 100/100 injected faults kept | `artifacts/e9_gate.txt`, `artifacts/e8_e9_rust.txt` |

`os/` suite: **957 → 1035 passed, 0 failed.** Both UEFI targets build.
No dependency was added.

## Design

```
cognition (a separate process; may die)
   |  "ask arm.wipe radius=... box=... 16 s, 12 N"          one line per primitive
   v
entity + KIRA  -- Granted --> part::Mandate --> motor::RunMandate
   |                                              (duration, force, rate, box)
   v
bodyd::armloop, 1 kHz:  reflex arcs -> path -> own kinematics -> Runner.admit -> efforts
```

| Piece | Where | What it is |
|---|---|---|
| Self-model | `motor::model`, `motor::mjcf`, `bodyd::selfmodel` | Links, joints and limits, every number `Stated` or `Assumed`; emitted as MJCF with the assumptions listed at the top |
| Run mandate | `motor::mandate` | What one grant allows: how long, how hard, how fast, inside what box. Read from the granted arguments only. Not `Clone` |
| Runner | `motor::run` | Checks every tick against the mandate; the first breach ends the run. Nothing is clamped |
| Primitive | `motor::primitive` | Strokes in an object's own frame, parameters with ranges, what it needs and achieves, an envelope, success tests. Six for the desk arm |
| Path and stiffness | `motor::control` | Minimum-jerk strokes that overlap; per-stroke stiffness; a joint law with gravity fed forward from the arm's own masses |
| Reflex arcs | `motor::reflex` | Slip tightens the grip, an unexpected touch softens the arm, a shove stops the path's clock |
| Int8 policy | `motor::policy` | Integer arithmetic between input rounding and output scaling |
| One-shot learner | `motor::recipe` | Cuts a demonstration at changes of grip and contact; binds lengths to the object; checks the steps chain |
| Conditions | `motor::condition` | **Stand-in for LOCI codes** |
| The arm's loop | `bodyd::armloop` | The tick. A task run and a standing run |
| Motor memory | `entity::entity::motor` | Primitives stored as skill nodes; a recipe believed only after its trial passed |
| Seat, cognition | `host::motor_seat`, `host::motor_mind` | `brainos motor seat`, `brainos motor mind` |
| Physics | `motor/sim/arm_sim.py` | MuJoCo and the wire. Decides nothing |

Three rules, and what enforces each:

- **KIRA is below everything.** A `Runner` starts only with a
  `RunMandate` taken by value; a `RunMandate` is made only from a
  `part::Mandate`; that is made only from a verified `Granted`.
  Compile-fail tests cover each link. No second way to make a `Granted`
  was added: the seat and the gate share a process precisely so that a
  grant never has to be decoded from a wire.
- **Bodies hold no memory.** `brainos-motor` and `brainos-bodyd` do not
  depend on `brainos-memory` outside tests, checked by
  `nostd/tests/discipline.rs` and by compile-fail tests. The seat reads
  the primitives *out of the graph* at start-up.
- **`no_std`, no unsafe, no new dependency.** `brainos-motor` builds for
  both UEFI targets.

One thing was added below `host`: `Channel::Force` in `brainos-rt`, so
that a force bound is not written down as a current.

## The mug, worked through

**The demonstration.** `motor/sim/mug_task.py: demonstrate` drives the
arm directly from a list of hand-written waypoints: no seat, no
primitives. It is logged at 100 Hz: tool position, finger opening, grip
force, tool force, and where the named things are (`brush.grip`,
`mug.mouth`, `holder.rest`, each with its lengths). 1,706 samples, 17 s.

**Cutting it.** The learner looks only for the moments the hand's
relation to the world changes: the fingers start to feel something, stop
feeling it, or the tool starts or stops touching.

- fingers close near `brush.grip` → **grasp** the brush
- the tool touches, while holding, near `mug.mouth` → **wipe** the mug
  (two contact episodes merged into one)
- fingers let go near `holder.rest` → **place** the brush

**Binding it to the object.** What was learned:

```
grasp frame=brush.grip  width_m  = width  * 0.99
wipe  frame=mug.mouth   radius_m = radius - 0.0074   depth_m = depth + 0.0014   tool_m = 0.075
place frame=holder.rest width_m  = width  * 0.99     tool_m  = 0.0757
```

The wipe's radius is "the mug's radius less 7.4 mm", not "27.6 mm". One
demonstration cannot tell "0.79 of the radius" from "7.4 mm less than
the radius"; they differ only on another mug. The learner uses a
difference for lengths that end where a held tool meets the object,
because the difference is the tool's size. **That is a prior written
into the learner, not something it learned.**

**Checking it chains.** Wipe needs something in the hand; grasp
provides it. Place needs the brush held; it is. A demonstration that
ends still holding something, or closes the fingers on nothing, is
refused with the reason.

**Running it.** For each step the cognition process works out the
strokes, the box they stay inside, and asks. The gate grants or
refuses. The arm runs the grant. In the fluid style the next step is
asked for while the current one is still running, from where things are
expected to be, and the two movements are joined without a stop.

**Believing it.** The recipe is written to memory as a belief only
after a trial passes; a failed trial leaves an episode and no belief
(`entity/tests/motor_memory.rs`; shown live for one passing and one
failing mug).

**Result.** 100 novel mugs (radius and height each ±30 %, any position
the arm can work over, any heading, mass ×0.7 to ×1.4, friction 0.7 to
1.2): 99 succeed. The one failure is the runner ending the wipe because
the felt force passed the 12 N that was granted. An earlier run of the
same 100, one bug-fix earlier, gave 97; the cognition process is
asynchronous, so runs are not bit-for-bit repeatable.

## Negative results and things found

**1. The hexapod's reference kinematics keep the shin vertical (E1).**
`LegGeom::foot` computes `z = -tibia + femur·sin(lift)`: the shin stays
vertical whatever the thigh does. A leg with two servos has the shin
fixed to the thigh. At the gait's 28° lift the two differ by 21 mm
outward and 5 mm up, on a body whose tip-over margin floor is 4 mm. The
generated model with a rigid shin matches rigid-shin kinematics to
0.00002 mm; a model with a joint that keeps the shin vertical matches
`LegGeom::foot` to 0.00002 mm. So the generator is right and the
reference describes a mechanism the bill of materials does not contain.
`miniblur-core` was **not** changed: it is shared with the firmware and
its stability check depends on it. This needs a decision.

**2. The hand-tuned gait does 0.038 m/s, not 0.12 (E2).** In the
generated rigid-shin model on firm ground, 20/20 upright. Consistent
with finding 1.

**3. A learned "gait" that swims in the floor (E2).** On the
simulator's default contact a 45 s smoke run produced a policy that met
the pre-registered stable-gait criterion at full evaluation size
(0.19 m/s, 20/20 upright) by sinking a 3 mm foot 5 mm into the ground.
On firm contact it does 0.001 m/s. The criterion as registered cannot
tell that from walking. The generated hexapod now has firm contact, and
on it a 45 s smoke run learns nothing (125k steps).

**4. `Arm::shoulder_torque_nm` is an upper bound, not the moment (E1).**
It adds `|m·g·x|` per link. Never exceeded in 200 poses; equal when
every link is on one side. The pre-registered row fails as written.

**5. Jerk did not halve (E6).** The fluid style has 6 % of the
baseline's pauses and 63 % of its peak force and takes half the time,
but its integrated squared jerk is 76 % of the baseline's. Most of the
jerk in both is contact chatter during the wipe, which blending does not
touch.

**6. The slip reflex misses slow slides (E7).** When the handle turns
slippery the brush slides at up to 4.2 mm/s; the threshold is 4 mm/s.
In 3 of 12 instances it stayed under and nothing answered; in 1 the
brush was dropped. The threshold was not retuned on the test instances.
During wiping the slip signal is above threshold most of the time and
the grip sits at the granted cap.

**7. Push and turn are under threshold (E3).** All 18 push failures are
the runner ending the run on felt force: a round puck's top rim catches
the finger's face and is pressed into the table. Of 18 turn failures,
13 are the wrist's roll limit (reported as unreachable, not attempted)
and 5 are felt force.

**8. Cloning never worked (E4, E5).** 0/100 from one demonstration and
0/50 from each of 1, 2, 5, 10, 20, 50. **The baseline is weak**: a
two-layer network on hand-chosen features, plain regression, no action
chunking. It does not finish the task on its own training instance. A
stronger learner would do better with 50 demonstrations; what this
shows is that one trajectory is not enough for regression, not that
cloning cannot do the task.

**9. The runner earned its keep during development.** Every one of
these was caught as a refused tick rather than as a crash: a standing
hold started from a posture nobody had sensed; a wrist asked to turn at
187°/s under a 180°/s grant; a descent blended into an approach that
clipped a mug's rim; a new run begun with no time elapsed.

## Verified only in simulation

Everything motor. Specifically, none of these has met hardware:

- every dimension, mass, torque and gain of the desk arm (`DeskArmConfig`
  is "plausible, not measured")
- every assumed mass and the servo torque of the hexapod
- the reflex thresholds, and the **slip sensor itself**: the simulator
  reports relative sliding speed at the fingers, which a real SO-101
  cannot sense directly
- felt force: the simulator sums contact forces; a real arm infers it
  from motor current
- perception: object frames and their lengths come from the simulator's
  ground truth, delayed 150 ms and sampled at 5 Hz, with no noise
- the success tests: judged from ground truth
- "wiped": a wall cell counts when the brush head passes within 2 mm of
  it (a rigid cylinder standing in for bristles; decided on development
  instances before the gate run)
- friction, contact stiffness, and the brush head's friction of 0.3

## Not built

- LOCI codes and LOCI `compose` (Workstream D does not exist)
- gaze (no simulated body has a camera)
- legged balance as a standing behaviour (the hexapod has no loop like
  the arm's)
- pour: the stroke program exists; no scene, never run
- a VLM segmenter, and learning from video
- int8 policies for the arm primitives (`Policy::Int8` is a slot; all six
  are stroke programs)
- the BLUR gait (pre-registered to follow the hexapod's, which has not
  run)
- push and turn in the one-shot learner (push is detected; turn is not)
- a wire format for a grant: seat and gate must share a process

## Running it

```sh
cd os && cargo test --workspace --release            # 1035
cd os && ./tools/check-no-std.sh
source motor/pyenv.sh                                 # MuJoCo python, brainos binary
$MOTOR_PY motor/e1_check.py                           # E1
cd motor/sim
$MOTOR_PY e4_gate.py setup                            # demonstration, recipe, clone
$MOTOR_PY e4_gate.py recipe 0 20                      # 20 at a time stays under a minute
$MOTOR_PY e4_gate.py clone 0 50
$MOTOR_PY e3_gate.py grasp 0 100                      # place, push, turn
$MOTOR_PY e6_gate.py run 0 10
$MOTOR_PY e7_gate.py run 0 12
$MOTOR_PY e9_gate.py 0 10
sbatch motor/hpcc/e2_train.sbatch                     # E2, on the cluster
```

Test seeds are 0 to 99. Seeds from 1000 were used while writing the
primitives and scenes and are never test seeds.

## Runbook: first test on a real arm (LeRobot SO-101 or similar)

Nothing below has been done. It is ordered so that each step can only
fail safely.

1. **Measure, do not trust.** Replace every number in
   `bodyd::deskarm::DeskArmConfig` with a measurement: link lengths with
   calipers, masses on a scale, joint limits by moving each joint by
   hand with torque off and reading the encoder. Run
   `brainos motor self-model desk-arm` and confirm the header lists no
   `ASSUMED` line you have not consciously kept.
2. **Check the sign of every joint**, torque off, by hand, against
   `DeskArmConfig::tool`. Pan and both pitch joints first.
3. **Check the kinematics on the bench.** Command nothing. Place the
   tool tip on three marked points and compare `tool()` of the encoder
   reading with a ruler. Stop if any is off by more than 5 mm.
4. **Write the nerve.** A `Nerve` for the servo bus (Feetech serial) in
   `os/host/src/nerves/`, behind the existing trait. It reads position,
   velocity, current and temperature; it writes one thing, a torque or
   position setpoint, and only through `apply` with a `Mandate`.
5. **Acquire it as a part** through the ordinary path, with vitals for
   servo temperature and current. No vitals, no verbs: that is existing
   behaviour, keep it.
6. **Float first.** With no grant, the loop feeds forward gravity and
   damps. The arm should stay where you leave it and yield to a finger.
   If it drifts, the masses are wrong: go back to step 1.
7. **Hold.** Ask for `arm.hold` for 10 s with a force bound of 5 N and
   the smallest box that contains the arm. Push it gently. It should
   resist, and the run should end at 10 s on its own.
8. **Prove the runner on hardware before any primitive.** Ask for a
   hold with a box that does not contain the arm: it must refuse to
   start. Ask for 1 s: it must stop at 1 s. These are the two tests that
   matter most; do them with a hand on the power switch.
9. **Felt force.** Calibrate force from motor current against a kitchen
   scale. Until this is done the force bound is not a bound; keep the
   torque limit low instead.
10. **One primitive in air.** `arm.grasp` on nothing, at 25 % of the
    rate bound, 3 N. It should end with "nothing gripped". Then on a
    foam block.
11. **Reflex thresholds.** There is no slip sensor. Either fit one
    (a cheap optical flow sensor on a finger) or leave the slip arc off
    and say so. Set the touch threshold from the current noise measured
    in step 9, not from the simulator's 1.5 N.
12. **Only then** the mug: demonstrate by moving the arm by hand with
    torque off (kinesthetic), log the same six channels, run
    `brainos motor learn`. Perception of `mug.mouth` and its radius is
    the piece with no implementation; a fiducial on the mug is the
    honest first version.

What will most likely be wrong first: the gravity model (step 6), the
sign of the wrist roll (step 2), and felt force (step 9).
