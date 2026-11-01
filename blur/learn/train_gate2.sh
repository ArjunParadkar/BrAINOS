#!/usr/bin/env bash
# train_gate2.sh -- the three Gate 2 reflexes, concurrently, 6 workers each.
# Launched as blur-train-* units: yield_guard.sh freezes them while the
# machine's research jobs (InvProj1, bpusim) run; ppo.py budgets ACTIVE time.
# Righting waits for its keyframe searches to finish (it loads their designs).
set -u
cd "$(dirname "$0")"
PY=${BLUR_PY:-$HOME/Desktop/Arenda_Projects/BLUR/blur_mujoco/.venv/bin/python}   # cluster: blur/hpcc/bpy
H=${HOURS:-4}
run () { systemd-run --user --unit "blur-train-$1" --working-directory="$PWD" -p MemoryMax=5G -E OMP_NUM_THREADS=1 -E MUJOCO_GL=egl \
         $PY ppo.py --task "$1" --workers 6 --envs 4 --steps 96 --hours "$H" --curriculum --severity0 0.0 --out "runs/$1_v1"; }
run standup
run balance
while systemctl --user is-active --quiet 'blur-design-righting-left' 'blur-design-righting-right' 'blur-design-righting-back'; do sleep 60; done
run righting
