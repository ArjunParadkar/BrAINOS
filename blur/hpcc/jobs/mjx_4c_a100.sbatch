#!/usr/bin/env bash
#SBATCH -J blur-4c-a100
#SBATCH -p agpu06 -q gpu -c 24 --gres=gpu:1 -t 02:00:00
#SBATCH -o /scrfs/storage/paradkar/blur/logs/%x-%j.out
# Track 4c on one A100: MJX vs CPU parity (all variants) + throughput.
# Every piece has its own timeout, so one slow setting cannot eat the job.
set -uo pipefail
source /scrfs/storage/paradkar/blur/code/blur/hpcc/site/pinnacle.env
source "$BLUR_CODE/hpcc/site/snapshot.sh"   # run from a per-job copy of the code
export JAX_COMPILATION_CACHE_DIR=$BLUR_HOME/cache/jax
OUT=$BLUR_RUNS/4c/$SLURM_JOB_ID; mkdir -p "$OUT"
cd "$BLUR_CODE/hpcc"; echo "[$(date -Is)] $(hostname)"; nvidia-smi --query-gpu=name,memory.total,driver_version --format=csv
JAX_PLATFORMS=cpu timeout 900 "$BLUR_PY" mjx_parity.py --variant cpu --out "$OUT"
for v in ${VARIANTS:-f32_fast f32_fast_feet f64 f32_tol6 f64_dt05 f32_tol6_dt05 f32_warp f32}; do
  echo "[$(date -Is)] parity $v"; timeout 600 "$BLUR_PY" mjx_parity.py --variant "$v" --out "$OUT" || echo "parity $v exit $?"
done
echo "[$(date -Is)] throughput"
timeout 1500 "$BLUR_PY" mjx_throughput.py --n 1024,4096,8192 --solver fast,fast_feet,tol6 --out "$OUT" || echo "throughput exit $?"
timeout 600 "$BLUR_PY" mjx_throughput.py --n 1024 --solver default --k 50 --tag _default --out "$OUT" || echo "throughput default exit $?"
timeout 600 "$BLUR_PY" mjx_throughput.py --n 1024,4096,8192 --solver fast,fast_feet --impl warp --tag _warp --out "$OUT" || echo "throughput warp exit $?"
echo "[$(date -Is)] done"
