#!/usr/bin/env bash
#SBATCH -J blur-4c-v100
#SBATCH -p gpu06 -q gpu -c 24 --gres=gpu:1 -t 01:00:00
#SBATCH -o /scrfs/storage/paradkar/blur/logs/%x-%j.out
# Track 4c on one V100: throughput (training solver, full and paws-only contacts)
# plus the float32 training-solver parity check.
set -uo pipefail
source /scrfs/storage/paradkar/blur/code/blur/hpcc/site/pinnacle.env
source "$BLUR_CODE/hpcc/site/snapshot.sh"   # run from a per-job copy of the code
export JAX_COMPILATION_CACHE_DIR=$BLUR_HOME/cache/jax
OUT=$BLUR_RUNS/4c/$SLURM_JOB_ID; mkdir -p "$OUT"
cd "$BLUR_CODE/hpcc"; echo "[$(date -Is)] $(hostname)"; nvidia-smi --query-gpu=name,memory.total,driver_version --format=csv
timeout 1500 "$BLUR_PY" mjx_throughput.py --n 1024,4096,8192 --solver fast_feet,fast --out "$OUT" || echo "throughput exit $?"
timeout 600 "$BLUR_PY" mjx_parity.py --variant f32_fast --out "$OUT" || echo "parity exit $?"
echo "[$(date -Is)] done"
