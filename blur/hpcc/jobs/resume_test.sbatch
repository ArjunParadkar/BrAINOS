#!/usr/bin/env bash
#SBATCH -J blur-resumetest
#SBATCH -p cloud72 -q cloud -c 8 -t 01:00:00
#SBATCH -o /scrfs/storage/paradkar/blur/logs/%x-%j.out
# Bitwise resume test of learn/ppo.py on the cluster, per task: run A straight for 30
# iterations; run B is killed hard at iteration 23 (last checkpoint 20, rows 21-23 lost)
# and resumed with --auto-resume. Both checkpoint every 5 iterations (a checkpoint
# canonicalises the env states, so both runs must take the same ones). Pass = final
# weights and every progress row except wall time identical. Episodes end and reset
# inside the window (envs=2, steps=24, wide DR with terrain).
set -uo pipefail
source /scrfs/storage/paradkar/blur/code/blur/hpcc/site/pinnacle.env
source "$BLUR_CODE/hpcc/site/snapshot.sh"
export BLUR_DR=wide OMP_NUM_THREADS=1
O=$BLUR_RUNS/resumetest/$SLURM_JOB_ID; mkdir -p $O; cd "$BLUR_CODE/learn"
for task in walk righting hop; do
  C="--task $task --workers 4 --envs 2 --steps 24 --iters 30 --ckpt-iters 5 --hours 5 --curriculum --severity0 0.0 --seed 3"
  "$BLUR_PY" ppo.py $C --out $O/$task/A > $O/$task.A.log 2>&1
  "$BLUR_PY" ppo.py $C --out $O/$task/B --hard-stop-after 23 > $O/$task.B1.log 2>&1
  "$BLUR_PY" ppo.py $C --out $O/$task/B --auto-resume > $O/$task.B2.log 2>&1
  "$BLUR_PY" -c "
import torch, csv, sys
a=torch.load('$O/$task/A/policy.pt',weights_only=False)['net']; b=torch.load('$O/$task/B/policy.pt',weights_only=False)['net']
w=all(torch.equal(a[k],b[k]) for k in a)
ra=[r[:1]+r[2:] for r in csv.reader(open('$O/$task/A/progress.csv'))]; rb=[r[:1]+r[2:] for r in csv.reader(open('$O/$task/B/progress.csv'))]
eps=sum(1 for r in ra[1:] if r[2] not in ('','nan'))
print('RESUME $task', 'PASS' if (w and ra==rb) else 'FAIL', 'weights', w, 'rows', ra==rb, len(ra)-1, 'iters with finished episodes', eps)
"
  grep -h "resumed" $O/$task.B2.log
done
ls -la $O/walk/B/ckpt.pt
