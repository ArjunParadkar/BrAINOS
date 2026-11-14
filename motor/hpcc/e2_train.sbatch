#!/usr/bin/env bash
# e2_train.sbatch -- gate E2 at the pre-registered size: train a hexapod gait from the
# generated self-model alone (PPO, CPU), export it as int8, evaluate it and the hand-tuned
# tripod gait (stable gait 20 seeds x 10 s; pushes 0.05/0.1/0.2/0.4 N s x 50), and check the
# Rust int8 runner against the Python reference on 1,000 recorded observations.
# One node, CPU only. Everything lands in motor/gait/runs/<jobid>/.
#
#   cd <repo root>          # the brainos checkout on the cluster
#   sbatch -p <partition> -c 32 -t 12:00:00 --export=ALL,MOTOR_PY=<python with mujoco+torch> \
#          motor/hpcc/e2_train.sbatch
#
# Knobs (environment; defaults in brackets):
#   REPO        repo root [$SLURM_SUBMIT_DIR]           MOTOR_PY   python [motor/pyenv.sh default]
#   ENVS        environments [256]                      WORKERS    env worker processes [cpus - THREADS]
#   THREADS     torch threads [4]                       ENV_STEPS  control-step budget [2e8]
#   HOURS       training wall budget, h [10]            EVAL_EVERY PPO iterations between evals [20]
#   SEED        [0]                                     RUN_DIR    [motor/gait/runs/$SLURM_JOB_ID]
#   CONTACT_TC  train with this contact time constant, s [empty = the model as generated, 0.02 s].
#               Smoke runs found a gait that only works on the generated model's soft contact
#               (motor/gait/README.md). The eval always reports a 0.005 s contact check beside
#               the pre-registered verdict; choosing CONTACT_TC changes what is trained, not the gate.
#   SMOKE=1     tiny sizes, single process, under a minute in total: checks the script itself
#
# Resuming: resubmit with RUN_DIR=<the same dir>; train.py picks up ckpt_latest.pt.
#SBATCH -J e2-train
#SBATCH -N 1
#SBATCH -c 32
#SBATCH -t 12:00:00
#SBATCH -o e2-train-%j.out
set -uo pipefail

REPO=${REPO:-${SLURM_SUBMIT_DIR:-$PWD}}
cd "$REPO" || exit 2
[[ -f motor/gait/train.py ]] || { echo "REPO=$REPO is not the brainos checkout"; exit 2; }
source motor/pyenv.sh
export OMP_NUM_THREADS=1 MKL_NUM_THREADS=1 MUJOCO_GL=disable PYTHONHASHSEED=0
JOB=${SLURM_JOB_ID:-local-$(date +%Y%m%d-%H%M%S)}
RUN=${RUN_DIR:-motor/gait/runs/$JOB}
mkdir -p "$RUN" && RUN=$(cd "$RUN" && pwd)
LOG=$RUN/stages.log
CPUS=${SLURM_CPUS_PER_TASK:-$(nproc)}
THREADS=${THREADS:-4}
WORKERS=${WORKERS:-$(( CPUS > THREADS + 1 ? CPUS - THREADS : 1 ))}
ENVS=${ENVS:-256}; ENV_STEPS=${ENV_STEPS:-2e8}; HOURS=${HOURS:-10}; EVAL_EVERY=${EVAL_EVERY:-20}
SEED=${SEED:-0}; CONTACT_TC=${CONTACT_TC:-}
# the pre-registered evaluation size
E_SEEDS=20; E_SECONDS=10; E_PUSHES=50; T_EVAL_SEEDS=20; T_EVAL_SECONDS=10
TRAIN_BUDGET=(--env-steps "$ENV_STEPS" --hours "$HOURS"); CALIB=4000
if [[ "${SMOKE:-0}" == 1 ]]; then
  ENVS=8; WORKERS=1; THREADS=1; EVAL_EVERY=5; TRAIN_BUDGET=(--seconds 12)
  E_SEEDS=2; E_SECONDS=3; E_PUSHES=1; T_EVAL_SEEDS=2; T_EVAL_SECONDS=3; CALIB=500
fi

stage() {   # stage NAME CMD...: run, time, log; keep going on failure (later stages say what is missing)
  local name=$1; shift
  local t0=$(date +%s)
  echo "[$(date -Is)] $name: $*" | tee -a "$LOG"
  "$@"; local rc=$?
  echo "[$(date -Is)] $name rc=$rc wall=$(( $(date +%s) - t0 )) s" | tee -a "$LOG"
  return $rc
}

{
  echo "job=$JOB host=$(hostname) cpus=$CPUS workers=$WORKERS threads=$THREADS envs=$ENVS"
  echo "python=$MOTOR_PY smoke=${SMOKE:-0} contact_tc=${CONTACT_TC:-generated}"
  git rev-parse HEAD 2>/dev/null; git status --porcelain -- motor os/motor 2>/dev/null | head -20
  sha256sum motor/models/hexapod_servo.xml
} | tee "$RUN/job.txt"

# ---- 1. train
RESUME=(); [[ -f $RUN/ckpt_latest.pt ]] && RESUME=(--resume)
TC=(); [[ -n "$CONTACT_TC" ]] && TC=(--contact-tc "$CONTACT_TC")
stage train "$MOTOR_PY" motor/gait/train.py --out "$RUN" --seed "$SEED" --envs "$ENVS" \
  --workers "$WORKERS" --threads "$THREADS" "${TRAIN_BUDGET[@]}" --eval-every "$EVAL_EVERY" \
  --eval-seeds "$T_EVAL_SEEDS" --eval-seconds "$T_EVAL_SECONDS" "${TC[@]}" "${RESUME[@]}"

# ---- 2. export: the first checkpoint that met "stable gait" (if any) and the final one
POLICIES=(--policy tripod)
for tag in first_stable final; do
  [[ -f $RUN/$tag.pt ]] || { echo "no $tag.pt" | tee -a "$LOG"; continue; }
  stage "export-$tag" "$MOTOR_PY" motor/gait/export_int8.py "$RUN/$tag.pt" --out "$RUN/policy_$tag.bmoti8" \
    --calib "$CALIB" --record 1000 --golden-dir "$RUN/golden_$tag" \
    && POLICIES+=(--policy "int8:$RUN/policy_$tag.bmoti8")
done

# ---- 3. evaluate at the pre-registered size (plus the two reported, non-gate checks)
stage eval "$MOTOR_PY" motor/gait/eval.py "${POLICIES[@]}" --seeds "$E_SEEDS" --seconds "$E_SECONDS" \
  --pushes "$E_PUSHES" --dt-check 2 --contact-tc 0.005 --json "$RUN/eval.json" | tee "$RUN/eval.txt"

# ---- 4. Rust int8 runner vs the Python reference, bit for bit
if command -v cargo >/dev/null 2>&1; then
  for g in "$RUN"/golden_*; do
    [[ -d $g ]] || continue
    ( cd os && BRAINOS_POLICY_GOLDEN_DIR="$g" stage "rust-$(basename "$g")" \
        cargo test --release -p brainos-motor --test policy_golden -- --nocapture ) 2>&1 \
      | grep -E "bit-exact|test result|rc=|error" | tee -a "$RUN/rust_check.txt"
  done
else
  echo "cargo not on this node: run on any machine with the repo:" | tee "$RUN/rust_check.txt"
  for g in "$RUN"/golden_*; do
    echo "  cd os && BRAINOS_POLICY_GOLDEN_DIR=$g cargo test --release -p brainos-motor --test policy_golden -- --nocapture" \
      | tee -a "$RUN/rust_check.txt"
  done
fi

# ---- 5. one page
{
  echo "E2 run $JOB  ($( [[ ${SMOKE:-0} == 1 ]] && echo 'SMOKE SIZE, not a gate run' || echo 'pre-registered size'))"
  echo; echo "training summary:"; cat "$RUN/summary.json" 2>/dev/null; echo
  echo; echo "first checkpoint meeting stable gait (float policy, training-time eval):"
  cat "$RUN/first_stable.json" 2>/dev/null || echo "  none"
  echo; echo "evaluation:"; cat "$RUN/eval.txt" 2>/dev/null
  echo; echo "rust bit-exact check:"; cat "$RUN/rust_check.txt" 2>/dev/null
} > "$RUN/RESULT.txt"
[[ -n "${SLURM_JOB_ID:-}" && -f "$REPO/e2-train-$SLURM_JOB_ID.out" ]] && cp "$REPO/e2-train-$SLURM_JOB_ID.out" "$RUN/"
echo "[$(date -Is)] done: $RUN/RESULT.txt"
