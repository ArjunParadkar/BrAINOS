#!/usr/bin/env bash
# run_one.sh RUN_ID TASK SEED HOURS DEADLINE_EPOCH -- one (skill x seed x config) run, resumable.
# Phases, each skipped once its output exists:
#   train   learn/ppo.py --auto-resume (exact resume from ckpt.pt; checkpoints every 15 min;
#           exits 75 at the deadline after checkpointing)            -> TRAINED
#   eval    learn/eval.py on policy_best.json, deterministic (mean action), 200 episodes:
#           nominal (no randomisation) and stress (every Track 4e.1 range at an edge,
#           uneven + soft ground)                                   -> eval_nominal.json, eval_stress.json
#   DONE
# A run with TASK=ZERO:<skill> is the reference baseline: no training, the skill's
# reference played open loop (zero residual), evaluated the same way.
# Exit: 0 done, 75 out of time (resume later), anything else = failure.
set -uo pipefail
ID=$1; TASK=$2; SEED=$3; HOURS=$4; DEADLINE=$5
D=$CAMPAIGN_RUNS/$ID; mkdir -p "$D"; cd "$BLUR_CODE/learn"
PY=$BLUR_PY
[ -f "$D/run.json" ] || printf '{"id":"%s","task":"%s","seed":%s,"hours":%s,"dr":"%s","code":"%s"}\n' \
  "$ID" "$TASK" "$SEED" "$HOURS" "$BLUR_DR" "$CAMPAIGN_CODE_SHA" > "$D/run.json"
left() { echo $(( DEADLINE - $(date +%s) )); }
if [[ "$TASK" != ZERO:* ]] && [ ! -f "$D/TRAINED" ]; then
  echo "[run $ID] train $(date -Is)"
  "$PY" ppo.py --task "$TASK" --seed "$SEED" --workers ${WORKERS:-28} --envs ${ENVS:-4} --steps 96 --hours "$HOURS" \
     --curriculum --severity0 0.0 --out "$D" --auto-resume --ckpt-min ${CKPT_MIN:-15} --deadline "$DEADLINE" \
     ${ITERS:+--iters $ITERS} >> "$D/train.log" 2>&1
  rc=$?; echo "[run $ID] train rc=$rc $(date -Is)"
  (( rc == 75 )) && exit 75
  (( rc != 0 )) && exit $rc
fi
POL=$D/policy_best.json; [[ "$TASK" == ZERO:* ]] && POL=$TASK
for mode in nominal stress; do
  [ -f "$D/eval_$mode.json" ] && continue
  (( $(left) < ${EVAL_NEED_S:-1200} )) && { echo "[run $ID] no time left for eval $mode"; exit 75; }
  echo "[run $ID] eval $mode $(date -Is)"
  timeout $(( $(left) - 60 )) "$PY" eval.py "$POL" --episodes ${EVAL_EPISODES:-200} --procs ${EVAL_PROCS:-25} --$mode \
     --out "$D/eval_$mode.json.tmp" >> "$D/eval.log" 2>&1
  rc=$?; (( rc == 124 )) && exit 75; (( rc != 0 )) && exit $rc
  mv "$D/eval_$mode.json.tmp" "$D/eval_$mode.json"
done
date -Is > "$D/DONE"; echo "[run $ID] DONE"
