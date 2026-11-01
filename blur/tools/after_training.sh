#!/usr/bin/env bash
# after_training.sh -- once the three Gate 2 reflexes have trained: export
# them for the Teensy, evaluate the chain (200 fallen starts, 200 pushes),
# rerun Gate 1 (stricter nodes) and Gate 2. Runs as blur-train-post so
# yield_guard.sh freezes it whenever the research jobs are live.
set -u
R=${BRAINOS_ROOT:-$(cd "$(dirname "$0")/../.." && pwd)}; L=$R/blur/learn; LOG=$R/blur/logs; mkdir -p "$LOG"
PY=${BLUR_PY:-$HOME/Desktop/Arenda_Projects/BLUR/blur_mujoco/.venv/bin/python}   # cluster: blur/hpcc/bpy
while systemctl --user is-active --quiet blur-train-standup blur-train-balance blur-train-righting; do sleep 60; done
echo "[$(date -Is)] trainers done" >> "$LOG/after_training.txt"
cd "$L"
for k in righting standup balance; do
  $PY export.py runs/${k}_v2/policy_best.json policies/$k.mlp >> "$LOG/after_training.txt" 2>&1
done
OMP_NUM_THREADS=1 MUJOCO_GL=egl $PY eval_chain.py --mode fallen --episodes 200 --procs 16 --out runs/chain_fallen.json > "$LOG/chain_fallen.txt" 2>&1
OMP_NUM_THREADS=1 MUJOCO_GL=egl $PY eval_chain.py --mode push --episodes 200 --procs 16 --out runs/chain_push.json > "$LOG/chain_push.txt" 2>&1
cd "$R/os"
cargo test -q -p brainos-blur > "$LOG/blur_tests.txt" 2>&1
BLUR_TWIN_START_S=300 cargo run -q --release --bin brainos -- blur gate1 > "$LOG/gate1_after.txt" 2>&1
BLUR_TWIN_START_S=300 cargo run -q --release --bin brainos -- blur gate2 > "$LOG/gate2_after.txt" 2>&1
echo "[$(date -Is)] all done" >> "$LOG/after_training.txt"
