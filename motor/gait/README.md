# motor/gait: gate E2, a hexapod gait from the self-model alone

**Status: Built, not run at full size.** Every stage runs, at smoke size,
on the laptop. The full-size training run is `motor/hpcc/e2_train.sbatch`
and has not been submitted (no cluster on 2026-11-12). Read the smoke
numbers below as smoke numbers. One of them meets the pre-registered
stable-gait criterion at the full evaluation size, and a check that is
not part of the gate shows that this gait is most likely an artifact of
soft contact in the simulator (see "What the smoke runs found").

The gate text is in `docs/motor/PREREGISTRATION.md` (E2) and is not
changed here.

The body is `motor/models/hexapod_servo.xml`, written by
`brainos motor self-model hexapod --servo`. Every mass (0.374 kg in
total) and the servo torque (0.18 N·m) are **assumed** numbers, listed in
that file's header. Any result here is a result about a body with those
assumed numbers.

## Files

| file | what |
|---|---|
| `env.py` | the MuJoCo environment: observation, action, reward, termination, domain randomisation, the servo-rate limit, and vector envs (in-process, or worker processes when asked) |
| `ppo.py` | actor 36-64-64-10 ReLU with a linear output (the shape the int8 runner runs), critic, GAE, and the PPO update |
| `train.py` | the training driver: checkpoints, `log.csv` (wall, CPU, env steps), periodic stable-gait evals, and `first_stable.{pt,json}` |
| `export_int8.py` | quantises a checkpoint to the `BMOTI8` blob and records 1,000 observations plus the reference outputs for the Rust check |
| `int8_ref.py` | numpy reference of `os/motor/src/policy.rs` (parse, `act_int`, `act`), and the golden-set reader and writer |
| `eval.py` | the stable-gait test and the push test, for `int8:`, `float:` or `tripod[:speed]` policies, with two reported checks that are not part of the gate |
| `make_golden.py` | regenerates `os/motor/tests/golden/` (a seeded, untrained network) |
| `../hpcc/e2_train.sbatch` | the full-size run: train, export, eval, then the Rust check |
| `os/motor/tests/policy_golden.rs` | the Rust int8 runner against the reference, bit for bit; malformed blobs refused |
| `os/host/src/motor_gait.rs` | `brainos motor gait-reference [SPEED_MPS] [SECONDS]`: the hand-tuned tripod as JSON lines (wired into the CLI separately) |

## Design

**Observation** (36 floats, physical units, things a real body can sense,
with no world position, heading or clock): gravity direction in the body
frame (3), body angular velocity (3), positions of the 10 driven joints
(10), their velocities (10), and the previous action (10). The network
sees `clip(obs / OBS_RANGE, -1, 1)`, where the full scales are fixed and
physical: 1, 8 rad/s, the joint range, 12 rad/s, and 1. There is no
running mean, because the int8 runner has no input offset.

**Action:** 10 numbers in [-1, 1]. Joint target = action × 0.6 rad,
clipped to ±70°. Control runs at 50 Hz, physics at 1 kHz.

**Servo rate (added after a smoke run; see findings).** The self-model
assumes 600°/s per driven joint (`bodyd::selfmodel::HEX_ASSUMED_SERVO_DPS`),
but the MJCF emitter does not put any rate into the model. `env.py`
therefore enforces it: each physics step, the setpoint given to the
position servo moves toward the target by at most 0.6°. The rate is read
from the model file's header. The hand-tuned gait goes through the same
limiter.

**Reward**, reading only base velocity, tilt, joint limits and actuator
effort (no reference gait, no phase clock, no contact schedule):

```
+1.0 * clip(v_fwd, ±0.12)/0.12      forward base velocity, capped at TOP_SPEED_MPS
-0.1 * min((v_lat/0.12)^2, 1)       sideways base velocity
-0.1 * min(yaw_rate^2, 1)           turning rate
-0.2 * min((v_z/0.12)^2, 1)         vertical base velocity  (added after smoke run 1)
-0.1 * min(roll_rate^2+pitch_rate^2, 1)                     (added after smoke run 1)
-1.0 * (tilt/20deg)^2
+0.2                                each step inside the tilt limit
-0.5 * sum(max(0, |q|/q_max - 0.9)) joints near their limits
-0.02 * mean((tau/tau_max)^2)       actuator effort
```

The episode ends when tilt exceeds 20° (training) or after 10 s.

**Domain randomisation** (training only): each link's mass and inertia
×U(0.8, 1.2), all friction ×U(0.7, 1.3) (one factor per episode), and
each servo's kp (and kv with it) ×U(0.8, 1.2). Evaluation uses the model
as generated.

**Evaluation** (`eval.py`):

- *Stable gait.* N seeds × T s. Each seed draws a start yaw and joint
  offsets of ±0.05 rad. Episodes are not cut at the tilt limit; each one
  records whether and when the limit was crossed. Speed is displacement
  along the starting heading divided by T, averaged over all episodes.
  The test passes when mean speed is at least 0.06 m/s and at least
  18/20 episodes stay upright (at reduced N: at least 90 %, and the
  result is marked `*`).
- *Push test.* 8 s episodes. At a time drawn from U[2, 6] s, a horizontal
  force is applied to the base for 10 ms in a random direction, sized so
  that force × 10 ms equals the impulse. "Survived" means the tilt limit
  was never crossed. Pushes are paired across policies.
- Two extra checks, **not pre-registered** and reported next to the
  verdict, never inside it: `--dt-check 2` reruns the stable test at a
  0.5 ms physics step, and `--contact-tc 0.005` reruns it with every
  contact time constant set to 0.005 s. The generated model keeps
  MuJoCo's default of 0.02 s.
- `tripod[:s]` replays `miniblur_core::gait` open loop at its own sample
  rate. The poses come from `brainos motor gait-reference` when the
  binary has that command, and otherwise from the Python port in
  `eval.py`. The two agree to 5.0e-6° at gait speeds 1.0, 0.5 and 0.35
  (`--check-port`, run against the module built in a scratch crate).

**Int8 export.** Input scale is 127/OBS_RANGE, so the integer input is
127·clip(obs/range). Weights use one symmetric scale per layer. Hidden
activations map their 99.99th percentile on calibration observations to
127. Outputs use out_scale = 1. mult/shift has mult < 2^15, checked
against int64 overflow. Calibration observations come from the float
policy (nominal model plus randomised model). The 1,000 recorded
observations come from the int8 policy driving the nominal model on
fresh seeds.

## How to run

```sh
source motor/pyenv.sh; export OMP_NUM_THREADS=1
# laptop sizes (each one process, under a minute)
$MOTOR_PY motor/gait/train.py --out motor/gait/runs/smoke --seconds 45 --envs 16 --eval-every 10 --eval-seeds 4 --eval-seconds 5
$MOTOR_PY motor/gait/export_int8.py motor/gait/runs/smoke/first_stable.pt --out motor/gait/runs/smoke/policy_first_stable.bmoti8 --golden-dir motor/gait/runs/smoke/golden_first_stable --record 1000
$MOTOR_PY motor/gait/eval.py --policy tripod --policy int8:motor/gait/runs/smoke/policy_first_stable.bmoti8 --seeds 20 --seconds 10 --skip-push
(cd os && BRAINOS_POLICY_GOLDEN_DIR=$PWD/../motor/gait/runs/smoke/golden_first_stable cargo test --release -p brainos-motor --test policy_golden -- --nocapture)
$MOTOR_PY motor/gait/make_golden.py          # regenerates os/motor/tests/golden (deterministic)
SMOKE=1 bash motor/hpcc/e2_train.sbatch     # the whole cluster script at toy size, ~27 s
# cluster
sbatch -p <partition> -c 32 -t 12:00:00 --export=ALL,MOTOR_PY=<python> motor/hpcc/e2_train.sbatch
```

## What was run on the laptop (2026-11-12), and what was not

Every run below was a single process under 60 s, with
`OMP_NUM_THREADS=1` and one torch thread. Wall times are as measured.

| run | wall |
|---|---|
| `make_golden.py` | 0.5 s |
| `cargo test --release -p brainos-motor --test policy_golden` (10 tests) | 9.7-17 s (mostly compiling) |
| `train.py` smoke, final code (`runs/smoke`) | 47.4 s (45 s budget) |
| `train.py` smoke, reward before the v_z/rocking terms (`runs/smoke_r1`) | 47.8 s |
| `train.py` smoke, `--contact-tc 0.005` (`runs/smoke_tc005`) | 42.8 s |
| `export_int8.py` on `runs/smoke/first_stable.pt` | 3.6 s, including a diagnostic |
| `eval.py` tripod, 20×10 s + 50 pushes × 4 levels + dt check (`runs/tripod`) | 47.2 s |
| `eval.py` int8 + tripod, 20×10 s, dt check and contact check | 24.0 s |
| `eval.py` int8, 50 pushes × 4 levels | 28.1 s |
| `eval.py` tripod at 0.5 and 0.35, 20×10 s, both checks | 24.2 s |
| resume check (two sequential 6 s `train.py` runs) | 15.8 s |
| `SMOKE=1 bash motor/hpcc/e2_train.sbatch` (`runs/sbatch_smoke`) | 26.8 s |

**Not run:**

- Full-size training.
- `--workers > 1`. That would be several processes on the laptop.
  Instead, the real `ProcVecEnv` and `_worker` code was driven with
  threads standing in for processes; over 100 steps, with 12 automatic
  resets, it was bit-identical to the in-process `VecEnv`.
- `--device cuda`. There is no CUDA torch here; "GPU time" is logged as
  0 on CPU.
- `--resume` was smoke-tested with the final code: 6 s fresh (29
  iterations), then 6 s resumed (continued from iteration 29 to 57;
  cumulative clocks carried over). It ran in a scratch directory that
  has since been deleted.

## Numbers (smoke size unless stated)

**Bit-exactness** (`policy_golden.rs`):

- Untrained golden set: 1000/1000 rows, 32/32 crafted edge rows (ties to
  even, NaN, ±inf, ±1e30, -0.0, a subnormal), and float outputs
  hash-equal.
- Trained smoke policy (`runs/smoke/golden_first_stable`): 1000/1000.
- Mutation check: one flipped byte in an expected output gives 999/1000,
  and the test fails. Under round-half-away input rounding instead of
  ties-to-even, 5 of the 32 edge rows would change their output.
- Float vs int8 action on the 1,000 recorded observations: max 0.0089,
  mean 0.0023. One int8 step is 0.0079.

**Hand-tuned tripod in the generated model** (pre-registered evaluation
size, 20 seeds × 10 s, 50 pushes per level):

| gait speed scalar (body's m/s claim) | measured m/s | upright | 0.5 ms step | contact 0.005 s |
|---|---|---|---|---|
| 1.0 (0.12) | **0.0354** | 20/20 | 0.0354 | 0.0365 |
| 0.5 (0.06) | 0.0167 | 20/20 | 0.0167 | 0.0166 |
| 0.35 (0.042) | 0.0117 | 20/20 | 0.0117 | 0.0114 |

- Push survival at 0.05 / 0.1 / 0.2 / 0.4 N·s: 50/50 at every level.
  The worst tilt per level was 0.6 / 1.1 / 4.4 / 11.8°.
- The hand-tuned gait is stable but reaches 0.0354 m/s, so it fails the
  0.06 m/s speed criterion. Ground speed is 0.28-0.30 of the body's
  m/s scaling at every cycle speed tried.

**Learned smoke policies:**

| run | first eval that passed (4 seeds × 5 s) | then at 20 × 10 s |
|---|---|---|
| `smoke` (final code) | iter 60: 61,440 control steps (1.23 M physics steps), **23.8 s wall, 23.7 s CPU**, no GPU | int8: **0.1929 m/s, 20/20 upright: meets the criterion**. Float: 0.1937, 20/20. 0.5 ms step: 0.1922. **Contact 0.005 s: 0.0007 m/s** |
| `smoke_r1` (no v_z/rocking terms) | iter 80: 81,920 steps, 36.6 s wall | float: 0.0669 m/s, 20/20 (passes); 0.5 ms step: 0.1231 |
| `smoke_tc005` (trained at contact 0.005 s) | iter 70: 71,680 steps, 27.8 s wall | float, model as generated: 0.2278, 20/20. 0.5 ms step: 0.2465. Contact 0.005 s: 0.0784. Contact 0.002 s: 0.0846 |

- Push survival of the `smoke` int8 policy: 50/50 at every level; worst
  tilt 4.0 / 4.5 / 5.8 / 10.8°.
- The "time to first stable checkpoint" above uses the reduced training
  eval (4 seeds × 5 s). The pre-registered size was applied afterwards
  to that checkpoint only, not to every iteration.

## What the smoke runs found

1. **The generated MJCF has no servo rate.** Without a rate limit, a
   smoke policy chattered at 22.6 Hz with about 3,000°/s joint speed,
   five times the assumed 600°/s, hopping forward at 0.13-0.15 m/s. Hence
   the limiter in `env.py`. With the limiter, that checkpoint scores
   -0.0050 m/s. Even with the setpoint limited, joints briefly reach
   about 1,000°/s through servo tracking transients. The proper fix is
   for `motor/src/mjcf.rs` to emit the rate.
2. **The learned "gaits" so far are fast shuffles, not walks.** The
   `smoke` policy:
   - chatters at 12.6 Hz with 8-20° joint excursions;
   - averages 1.7 feet in contact;
   - has no contact at all in 25 % of control steps;
   - drags its shins;
   - sinks a foot up to 5.4 mm into the ground (the foot radius is
     3.15 mm; the tripod's worst is 0.9 mm).

   Its speed collapses from 0.193 to 0.0007 m/s when contact is
   stiffened to 0.005 s, while the tripod is unaffected. It meets the
   pre-registered criterion on the model as generated, so **the
   criterion as written cannot tell a soft-contact artifact from a
   gait**. That is a property of the gate text and is reported here, not
   fixed. Training on stiffer contact (`smoke_tc005`) still produced a
   shuffle above 0.06 m/s at every contact stiffness tried, but its speed
   changes about 3× with stiffness. Speeds above `TOP_SPEED_MPS`
   (0.12 m/s) come from these shuffles, even though the reward stops
   increasing at 0.12.
3. **The deterministic policy's speed swings from one eval to the
   next** (for example +0.186, +0.072, -0.036, -0.057 m/s across
   consecutive evals in `smoke`). This looks like a closed-loop limit
   cycle that appears and changes direction as the weights move, not a
   steadily improving gait.
4. **The push levels do not discriminate on this body.** At
   0.4 N·s the base takes 0.347 N·s of momentum in the 20 ms step
   (friction takes the rest) and slides 5.8 cm, but the 0.37 kg, 48 mm
   tall body slides rather than tips. Both gaits, and a policy that
   barely moves, survive 50/50 at every level.
5. **The hand-tuned gait's speed scaling is about 3.4× optimistic** in
   this model: 0.0354 m/s at the cycle the body calls 0.12 m/s.
   `bodyd::hexapod` already says the scaling is not a measurement.

## Before the full run, someone has to decide

- Whether to train on the model as generated (contact 0.02 s) or with
  `CONTACT_TC` set. The cleaner fix is to have the generator emit
  contact parameters and the servo rate, which changes the model hash.
- That the stable-gait verdict will be accompanied by the dt and contact
  checks. They are already in `eval.json` and `RESULT.txt`.

`runs/` holds the smoke artifacts quoted above. It should probably be
git-ignored except for what a write-up cites.
