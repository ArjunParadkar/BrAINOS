#!/usr/bin/env bash
# fetch_runs.sh [CAMPAIGN] -- (laptop) copy a campaign's results back into the repo:
#   blur/learn/runs/hpcc_<campaign>/<run>/{run.json, progress.csv, policy_best.json,
#   eval_nominal.json, eval_stress.json, TRAINED, DONE, *.log}
# plus the lane state (usage.log, failed/). Heavy files stay on Pinnacle: ckpt.pt (exact
# resume state), policy*.pt (torch; policy_best.json is the exported actor the twin and
# the Rust side load). Then writes SUMMARY.md with blur/hpcc/summarize.py.
set -euo pipefail
N=${1:-dayone}; HOST=${PINNACLE:-pinnacle}
HERE=$(cd "$(dirname "$0")" && pwd); DEST=$HERE/../learn/runs/hpcc_$N
C=/scrfs/storage/paradkar/blur/campaigns/$N
mkdir -p "$DEST"
timeout 600 ssh -o BatchMode=yes -o ConnectTimeout=20 "$HOST" \
  "cd $C && tar cz --exclude='*.pt' --exclude='*.pt.tmp' --exclude='policy.json' --exclude='*.json.tmp' runs state/usage.log state/failed CODE_SHA manifest.txt campaign.env 2>/dev/null" \
  | tar xz -C "$DEST"
python3 "$HERE/summarize.py" "$DEST" > "$DEST/SUMMARY.md"
echo "fetched $N -> $DEST ($(du -sh "$DEST" | cut -f1)); summary: $DEST/SUMMARY.md"
