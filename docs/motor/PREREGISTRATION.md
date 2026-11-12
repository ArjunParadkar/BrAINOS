# Workstream E (motor memory): pre-registration

Written 2026-11-12, before any gate below was run. Thresholds are not
edited after a gate has been run; a threshold that turns out to be wrong
is reported as wrong in `MOTOR.md`, and the gate keeps its verdict.

Status words are the repository's: **Verified**, **Built**, **Manual**,
**Not built**, **Failing**.

## What this workstream was told to stand on, and what exists

| Dependency | State on 2026-11-12 |
|---|---|
| D0, MemoryHandle sealed | Present (`103c9a8` on `nerves-x86-bpu`). |
| D, LOCI | **Does not exist.** No `loci` branch locally or on the remote, no code, no brief. |

Consequences, fixed before work started:

- `motor` is branched from `nerves-x86-bpu`, not from `loci`.
- Preconditions, postconditions and `compose` are specified in the brief
  as LOCI codes. LOCI's code format is not known, and it is not invented
  here. `brainos-motor` carries a small symbolic stand-in
  (`motor::condition`), named as a stand-in in its own documentation.
  Anything the gates say about composition is a statement about the
  stand-in.
- E9's "stream goes into LOCI" is built against the state graph that
  exists. The LOCI half is **Not built**.

## Compute

Standing rule: nothing heavier than a single process for under a minute
runs on the laptop; the cluster is for compute. On 2026-11-12 neither
cluster is reachable (no VPN, no control socket). Every gate below
therefore has two sizes: a **laptop size** (single process, under a
minute per invocation) and a **full size** (an `sbatch` script under
`motor/hpcc/`). A gate run only at laptop size is reported with its
actual sample count and is not called Verified at the full threshold
unless the laptop size already meets the stated count.

## Simulated bodies

| Body | Source of the self-model | What the source states | What it does not state |
|---|---|---|---|
| Mini-Blur hexapod | `miniblur_core::geometry::LegGeom`, `ServoTable::bom_10_servo`, `bodyd::hexapod` | link lengths, mounts, joint limits, coupled pairs, top speed, tilt limit | **any mass, any torque** |
| Planar six-axis arm | `bodyd::arm::ArmConfig` | link lengths, masses, joint limits, torques | geometry out of the plane |
| Desk arm + gripper | `bodyd::deskarm` (new, this workstream) | everything, as *plausible, not measured* numbers shaped like a LeRobot SO-101 | nothing measured |
| BLUR quadruped | `blur/descriptor.json` + a skeleton exported from the twin | joints, limits, actuator forces, total mass, body tree | meshes, tendons, springs, contact geometry |

Masses and torques the source does not state are assumptions, written
down in the generated model as assumptions, and E2's result is a result
about a body with those assumed numbers.

## Gates

### E1: the generated MJCF matches the references

| Check | Threshold |
|---|---|
| Hexapod: foot position, MuJoCo forward kinematics of the generated model against `LegGeom::foot`, 1,000 random poses inside the servo limits | worst error at most 0.01 mm |
| Hexapod: joint ranges in the MJCF against the servo table | equal to 1e-3 degrees |
| Hexapod: the two linked swing pairs | present as equality constraints; both joints of a pair agree to 0.1 degrees after 1 s of simulation under a step command |
| Planar arm: end effector against `Arm::end_effector`, 1,000 random poses | worst error at most 0.01 mm |
| Planar arm: static shoulder torque from MuJoCo inverse dynamics against `Arm::shoulder_torque_nm`, 200 poses | within 1 % of the larger of the two, or 0.01 N·m |
| BLUR: world position of every body, generated skeleton against the twin, 200 random joint configurations inside limits | worst error at most 0.1 mm |
| BLUR: joint ranges, actuator force ranges | equal to 1e-6 |
| BLUR: total mass | within 0.1 % |

The BLUR check is circular in one respect and that is stated here: the
skeleton is exported from the twin, so it verifies that the self-model
representation and the emitter lose nothing they claim to carry. It does
not verify the twin.

### E2: a gait from the self-model alone

"Alone" means the reward reads only quantities the self-model defines
(base velocity, tilt, joint limits, actuator effort). No reference gait,
no phase clock, no foot-contact schedule.

- **Stable gait:** over 20 evaluation seeds of 10 s each, mean forward
  speed at least 0.06 m/s (half of `TOP_SPEED_MPS`), and at least 18 of
  20 episodes end without the tilt limit being crossed.
- **Reported, no threshold:** wall-clock and GPU time until the first
  checkpoint that meets "stable gait"; number of environment steps.
- **Robustness:** horizontal pushes of 0.05, 0.1, 0.2 and 0.4 N·s at a
  random time and direction, 50 pushes per level; survival rate of the
  learned gait and of the hand-tuned tripod gait
  (`miniblur_core::gait`) replayed in the same model. Reported whatever
  it shows. The gate does not require the learned gait to win.
- **Export:** the int8 policy run by `brainos-motor` produces the same
  actions as the Python int8 reference on 1,000 recorded observations,
  bit for bit.
- Order: hexapod first; BLUR only if the hexapod gate passes.

### E3: primitive success rates

Desk arm + gripper, 100 randomized instances per primitive (object
size ±30 %, mass ×0.5 to ×2, friction 0.6 to 1.2, pose anywhere in the
stated workspace, yaw uniform).

| Primitive | Success test | Threshold |
|---|---|---|
| grasp | object held 5 cm above the table for 1 s | 90 % |
| place | object upright, within 1.5 cm of the target, released | 90 % |
| push | object within 2 cm of the target, not toppled | 85 % |
| wipe | at least 80 % of the target patch cells touched, normal force never above the mandate's limit | 85 % |
| turn | knob angle within 10 degrees of the target | 85 % |
| pour | at least 80 % of the grains end in the receiving vessel | 75 % |

### E4 (headline): one demonstration

Task: grasp the brush from its holder, move to the mug, wipe the inside
of the mug, place the brush back. One demonstration on one mug.

- **Test set:** 100 novel mugs: radius and height each ±30 % of the
  demonstration mug, position anywhere in the workspace, yaw uniform.
  None equal to the demonstration.
- **Success:** at least 80 % of the interior wall cells touched by the
  brush, the mug moved less than 3 cm and not toppled, the brush back
  within 3 cm of its holder.
- **Threshold:** recipe success at least 80 %.
- **Baseline:** behaviour cloning from the same single demonstration (a
  network regressing the demonstrated action from the state, same
  observation the recipe has). **Claim:** the recipe beats it by at
  least 30 points. Both numbers reported whatever they are.
- The recipe is written to memory as a belief only after its own success
  test has passed in simulation; a test asserts that a failing recipe
  leaves no belief.

### E5: the curve

Four tasks not used to build the library. For library sizes 0 to 6
primitives (added in a fixed order written into the script before the
run), the number of demonstrations from {1, 2, 5, 10, 20, 50} needed to
reach 80 % on 50 trials; where a needed primitive is missing, the
missing segment is learned by behaviour cloning from the demonstrations.
**Claim:** demonstrations needed falls toward 1 as the library grows.
The curve is reported whatever it shows, including "never reaches 80 %".

### E6: fluidity

Same task (E4's), same body, two controllers: the fluid one (variable
impedance, blended chunks, predictive execution) and a baseline (fixed
stiff position control, full stop between segments).

| Metric | Threshold |
|---|---|
| integrated squared jerk of the tool tip | fluid at most 50 % of baseline |
| pauses (tool speed under 5 % of its peak for more than 100 ms, mid-task) | fluid at most half the baseline's count |
| peak contact force | fluid at most 70 % of baseline |
| task success | fluid no more than 5 points below baseline |

### E7: reflexes

Tick is 1 ms. Latency is counted in ticks from the first tick at which
the sensor frame contains the event to the first tick whose command
reflects the response.

| Reflex | Threshold |
|---|---|
| slip, then grip increase | at most 20 ms |
| unexpected contact, then stiffness drop | at most 10 ms |
| perturbation, then back within 2 cm of the path | at most 500 ms |

Each is measured twice: with the cognition process alive, and after a
real `SIGKILL` of it. The second set must meet the same thresholds.

### E8: structure

- A primitive cannot start without a mandate: compile-fail test.
- A mandate cannot be constructed except from a KIRA grant: compile-fail
  test.
- Adversarial: for every bound of the mandate (angle, rate, force,
  workspace, duration) a tick that exceeds it is refused by the body
  runtime and the run stops; 10,000 random ticks, zero applied outside.
- The body runtime's types hold no memory: compile-fail test plus the
  source-level discipline test.
- No call into `brainos-kira` occurs per tick: asserted by counting gate
  evaluations across a run.

### E9: standing behaviours and the stream

- Posture and balance, running under standing grants, keep their
  invariants for 10 s after `SIGKILL` of cognition.
- Memory growth: 10 simulated minutes of a steady 1 kHz stream adds at
  most 64 nodes; an injected fault or novel reading is kept, every time,
  in 100 injections.
- Gaze is specified in the brief; no simulated body here has a camera.
  Gaze is **Not built** unless one is added.
