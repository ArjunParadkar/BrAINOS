#!/usr/bin/env bash
# lane.sbatch -- one BLUR training lane: a singleton job (same name + --dependency=singleton)
# that claims the next unfinished run of a campaign (mkdir lock), runs it until done or until
# its time is nearly used (ppo checkpoints and exits 75; the next lane job resumes it
# exactly), and repeats; then resubmits itself while runs remain. Modelled on WhiteBox's
# runs/slurm/k12_lane.sbatch.
#
#   sbatch --export=NONE,CAMPAIGN=dayone -J blur-lane-1 -p comp06 -q comp -c 32 -t 6:00:00 \
#          --dependency=singleton $BLUR_HOME/campaigns/dayone/code/hpcc/jobs/lane.sbatch
#
# Guards: account running comp-QOS cores (sum over squeue, this job included) must be
# <= MAX_COMP (96) at lane start, else the lane resubmits itself 15 min later and exits.
# squeue failing or returning nothing (impossible while this job runs) counts as "busy"
# and "lock holder alive" -- never as "nothing running". Budget: node-hours of every lane
# job are logged; past CAP_NODE_H no lane resubmits (STOP_BUDGET).
#SBATCH -o /scrfs/storage/paradkar/blur/logs/%x-%j.out
set -uo pipefail
: "${HOME:=/home/paradkar}"; export HOME   # --export=NONE resubmits from inside a job can arrive without HOME (WhiteBox jobs 1367020, 1367097)
: "${USER:=paradkar}"; export USER
: "${CAMPAIGN:=dayone}"; : "${TLIM:=6:00:00}"; : "${MAX_COMP:=96}"; : "${CAP_NODE_H:=210}"; : "${MAXTRY:=4}"; : "${QOS:=comp}"; : "${MARGIN:=600}"
C=/scrfs/storage/paradkar/blur/campaigns/$CAMPAIGN
source $C/code/hpcc/site/pinnacle.env
export BLUR_CODE=$C/code BLUR_PY=$C/code/hpcc/bpy CAMPAIGN_RUNS=$C/runs
export CAMPAIGN_CODE_SHA=$(cat $C/CODE_SHA 2>/dev/null || echo unknown)
source $C/campaign.env                       # BLUR_DR, HOURS, manifest settings
ST=$C/state; mkdir -p $ST/lock $ST/tries $ST/failed $CAMPAIGN_RUNS
LANE=$C/code/hpcc/jobs/lane.sbatch
lim_s() { IFS=: read -r h m s <<< "$1"; echo $((10#$h * 3600 + 10#$m * 60 + 10#$s)); }
T0=$(date +%s); END=$((T0 + $(lim_s $TLIM) - MARGIN)); MIN_START=${MIN_START:-1500}
echo "[lane] $SLURM_JOB_NAME $SLURM_JOB_ID start $(date -Is) node=$(hostname) code=$CAMPAIGN_CODE_SHA"
echo "$SLURM_JOB_ID $SLURM_JOB_NAME start $T0" >> $ST/usage.log

my_jobs() {   # our job ids, one per line; returns 1 if squeue failed or (impossibly) listed nothing
  local out; out=$(squeue -h -u "$USER" -o %i 2>/dev/null) || return 1
  [[ -n "$out" ]] || return 1
  echo "$out"
}
comp_cores() {   # running comp-QOS cores of the account; prints 99999 if squeue fails/empty
  local out; out=$(squeue -h -u "$USER" -t R -o "%q %C" 2>/dev/null) || { echo 99999; return; }
  [[ -n "$out" ]] || { echo 99999; return; }
  echo "$out" | awk '$1=="comp"{s+=$2} END{print s+0}'
}
used_node_h() {   # node-hours of finished + this lane's jobs so far (usage.log: start/end lines)
  awk -v now="$(date +%s)" '$3=="start"{s[$1]=$4} $3=="end"{e[$1]=$4} END{for(j in s){t+=((j in e)?e[j]:now)-s[j]} printf "%.1f", t/3600}' $ST/usage.log
}
resubmit() {   # $1: extra sbatch args (e.g. --begin=now+900)
  [[ -f $C/STOP ]] && { echo "[lane] STOP file: not resubmitting"; return; }
  local extra=$1
  if [[ -z "$extra" ]]; then   # the same core check as at start
    local cc=$(comp_cores); (( cc > MAX_COMP )) && extra=--begin=now+900   # this job (still running) is replaced by the new one
  fi
  sbatch --export=NONE,CAMPAIGN=$CAMPAIGN,TLIM=$TLIM,MAX_COMP=$MAX_COMP,CAP_NODE_H=$CAP_NODE_H,MAXTRY=$MAXTRY,QOS=$QOS,MARGIN=$MARGIN,MIN_START=$MIN_START \
    -J $SLURM_JOB_NAME -p ${SLURM_JOB_PARTITION} -q $QOS -c ${SLURM_CPUS_PER_TASK} -t $TLIM --dependency=singleton \
    $extra $LANE && echo "[lane] resubmitted $extra"
}
finish() { echo "$SLURM_JOB_ID $SLURM_JOB_NAME end $(date +%s)" >> $ST/usage.log; echo "[lane] end $(date -Is) used=$(used_node_h) node-h"; }

cc=$(comp_cores)
if (( cc > MAX_COMP )); then
  echo "[lane] account comp cores running = $cc > $MAX_COMP: wait 15 min"; resubmit --begin=now+900; finish; exit 0
fi

done_run() { [[ -f $CAMPAIGN_RUNS/$1/DONE ]]; }
eligible() {
  local id=$1
  done_run $id && return 1
  [[ -f $ST/failed/$id ]] && return 1
  local n=$(cat $ST/tries/$id 2>/dev/null || echo 0); (( n >= MAXTRY )) && return 1
  return 0
}
claim() {
  local id=$1
  if mkdir $ST/lock/$id 2>/dev/null; then echo $SLURM_JOB_ID > $ST/lock/$id/job; return 0; fi
  local j=$(cat $ST/lock/$id/job 2>/dev/null) jobs
  jobs=$(my_jobs) || return 1                       # can't tell: assume the holder is alive
  if [[ -n "$j" ]] && ! grep -qx "$j" <<< "$jobs"; then   # stale lock (holder gone)
    rm -rf $ST/lock/$id && mkdir $ST/lock/$id 2>/dev/null && { echo $SLURM_JOB_ID > $ST/lock/$id/job; return 0; }
  fi
  return 1
}

while :; do
  now=$(date +%s); (( END - now < MIN_START )) && { echo "[lane] time nearly used"; break; }
  u=$(used_node_h); if awk -v u=$u -v c=$CAP_NODE_H 'BEGIN{exit !(u>c)}'; then
    echo "[lane] budget $u > $CAP_NODE_H node-h: stopping"; date -Is > $C/STOP_BUDGET; touch $C/STOP; break; fi
  picked=""; line=""
  while read -r id task seed hours; do
    [[ -z "$id" || "$id" == \#* ]] && continue
    eligible $id || continue
    claim $id && { picked=$id; line="$task $seed $hours"; break; }
  done < $C/manifest.txt
  [[ -z "$picked" ]] && { echo "[lane] nothing eligible"; break; }
  n=$(cat $ST/tries/$picked 2>/dev/null || echo 0); echo $((n + 1)) > $ST/tries/$picked
  echo "[lane] run $picked start $(date -Is) try $((n + 1)): $line"
  bash $C/code/hpcc/run_one.sh $picked $line $END
  rc=$?
  echo "[lane] run $picked end $(date -Is) rc=$rc"
  if (( rc == 75 )); then echo $n > $ST/tries/$picked        # out of time is not a failed try
  elif (( rc != 0 )) && ! done_run $picked; then
    echo "rc=$rc job=$SLURM_JOB_ID $(date -Is)" > $ST/failed/$picked; echo "[lane] run $picked marked failed"
  fi
  rm -rf $ST/lock/$picked
  (( rc == 75 )) && break
done

left=0
while read -r id rest; do [[ -z "$id" || "$id" == \#* ]] && continue; eligible $id && { left=1; break; }; done < $C/manifest.txt
(( left )) && resubmit ""
finish
