#!/usr/bin/env bash
#SBATCH -J blur-smoke-cpu
#SBATCH -p cloud72 -q cloud -c 4 -t 00:30:00
#SBATCH -o /scrfs/storage/paradkar/blur/logs/%x-%j.out
set -euo pipefail
source /scrfs/storage/paradkar/blur/code/blur/hpcc/site/pinnacle.env
source "$BLUR_CODE/hpcc/site/snapshot.sh"   # run from a per-job copy of the code
cd "$BLUR_CODE/hpcc"; echo "[$(date -Is)] $(hostname) $(cat /etc/redhat-release 2>/dev/null)"
JAX_PLATFORMS=cpu "$BLUR_PY" smoke.py
