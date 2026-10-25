#!/usr/bin/env bash
#SBATCH -J blur-paritycheck
#SBATCH -p cloud72 -q cloud -c 4 -t 00:30:00
#SBATCH -o /scrfs/storage/paradkar/blur/logs/%x-%j.out
# A 2 %-length MJX-on-CPU run of mjx_parity.py: catches code errors before a GPU job.
set -uo pipefail
source /scrfs/storage/paradkar/blur/code/blur/hpcc/site/pinnacle.env
source "$BLUR_CODE/hpcc/site/snapshot.sh"   # run from a per-job copy of the code
cd "$BLUR_CODE/hpcc"; OUT=$BLUR_RUNS/4c/check-$SLURM_JOB_ID
for v in f64 f32_fast_feet; do JAX_PLATFORMS=cpu BLUR_PARITY_TSCALE=0.02 "$BLUR_PY" mjx_parity.py --variant $v --out "$OUT"; done
JAX_PLATFORMS=cpu "$BLUR_PY" mjx_throughput.py --n 8 --k 5 --reps 1 --solver fast,fast_feet --out "$OUT"
