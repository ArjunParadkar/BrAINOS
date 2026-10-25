#!/usr/bin/env bash
#SBATCH -J blur-smoke-gpu
#SBATCH -p agpu06,gpu06 -q gpu -c 24 --gres=gpu:1 -t 00:30:00
#SBATCH -o /scrfs/storage/paradkar/blur/logs/%x-%j.out
set -euo pipefail
source /scrfs/storage/paradkar/blur/code/blur/hpcc/site/pinnacle.env
source "$BLUR_CODE/hpcc/site/snapshot.sh"   # run from a per-job copy of the code
cd "$BLUR_CODE/hpcc"; echo "[$(date -Is)] $(hostname)"; nvidia-smi --query-gpu=name,memory.total,driver_version --format=csv
"$BLUR_PY" smoke.py
