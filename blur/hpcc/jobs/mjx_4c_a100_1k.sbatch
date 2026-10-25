#!/usr/bin/env bash
#SBATCH -J blur-4c-a100-1k
#SBATCH -p agpu06 -q gpu -c 24 --gres=gpu:1 -t 00:30:00
#SBATCH -o /scrfs/storage/paradkar/blur/logs/%x-%j.out
# Re-measure the A100 1024-env points (job 1367639 recompiled inside the timed call).
set -uo pipefail
source /scrfs/storage/paradkar/blur/code/blur/hpcc/site/pinnacle.env
source "$BLUR_CODE/hpcc/site/snapshot.sh"
export JAX_COMPILATION_CACHE_DIR=$BLUR_HOME/cache/jax
OUT=$BLUR_RUNS/4c/$SLURM_JOB_ID; mkdir -p "$OUT"
cd "$BLUR_CODE/hpcc"; echo "[$(date -Is)] $(hostname)"; nvidia-smi --query-gpu=name --format=csv
timeout 1200 "$BLUR_PY" mjx_throughput.py --n 1024 --solver fast,fast_feet --out "$OUT" || echo "throughput exit $?"
echo "[$(date -Is)] done"
