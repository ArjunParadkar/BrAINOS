#!/usr/bin/env bash
# campaign_setup.sh NAME -- (Pinnacle login node; file copies only) freeze the synced code
# tree as the campaign's code, and install its manifest + settings:
#   $BLUR_HOME/campaigns/NAME/{code/, CODE_SHA, manifest.txt, campaign.env, runs/, state/}
# Every lane job of the campaign runs this frozen copy, so a later sync can't change a
# run halfway (exact resume needs the same code). Re-running it on a live campaign is refused.
set -euo pipefail
N=$1; B=/scrfs/storage/paradkar/blur; C=$B/campaigns/$N; SRC=$B/code/blur
[ -d "$C/code" ] && { echo "campaign $N already has frozen code ($C/code); refusing"; exit 1; }
mkdir -p "$C"/{runs,state}
cp -a "$SRC" "$C/code"
(cd "$C/code" && find . -type f \( -name '*.py' -o -name '*.sh' -o -name '*.sbatch' -o -name '*.json' \) -not -path './hpcc/results/*' \
   | sort | xargs sha256sum | sha256sum | cut -c1-12) > "$C/CODE_SHA"
cp "$SRC/hpcc/campaigns/$N/manifest.txt" "$SRC/hpcc/campaigns/$N/campaign.env" "$C/"
echo "campaign $N: code $(cat $C/CODE_SHA), $(grep -vc '^#' $C/manifest.txt) runs"
