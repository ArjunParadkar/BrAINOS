#!/usr/bin/env bash
# submit_finalbody.sh -- ONE command to launch the final-body retarget
# campaign on Pinnacle, once Arjun has the VPN + ControlMaster up. It does
# NOT run while SSH is down (it checks first and refuses).
#
# It is deliberately conservative and leaves the day-one campaign untouched:
#   * new campaign directory (finalbody), new lane names (blur-fb-1/2);
#   * obeys the comp-QOS <=96 core gate the lane enforces;
#   * syncs the FINAL mjcf to $RB/mjcf/blur_final and the final descriptor
#     rides in the code tree (blur/descriptor_final.json);
#   * freezes a per-campaign code copy, so a later sync can't change a run.
#
# Usage:  blur/hpcc/submit_finalbody.sh            # sync + setup + launch 2 lanes
#         blur/hpcc/submit_finalbody.sh --dry-run  # print what it would do
set -euo pipefail
HOST=${PINNACLE:-pinnacle}
HERE=$(cd "$(dirname "$0")" && pwd)
SRC=$(cd "$HERE/../.." && pwd)                 # brainos/
RB=/scrfs/storage/paradkar/blur
FINAL_MJCF=/home/god/Desktop/Arenda_Projects/BLUR_final/mujoco
SSH=(ssh -o BatchMode=yes -o ConnectTimeout=20 "$HOST")
DRY=${1:-}

say() { echo "[finalbody] $*"; }

# 0. SSH must be up (VPN + ControlMaster). Refuse cleanly otherwise.
if ! timeout 25 "${SSH[@]}" true 2>/dev/null; then
  say "Pinnacle SSH is DOWN (need Arjun's VPN + one interactive 2FA ssh pinnacle)."
  say "Nothing submitted. Re-run this script once SSH is up."
  exit 3
fi

if [ "$DRY" = "--dry-run" ]; then
  say "would: sync code + final MJCF, campaign_setup finalbody, submit blur-fb-1/2"
  exit 0
fi

# 1. sync code tree (carries blur/descriptor_final.json) + the FINAL mjcf
say "syncing code tree ..."
BLUR_MJCF_DIR="$FINAL_MJCF" "$HERE/sync_to_pinnacle.sh"
say "syncing final MJCF -> $RB/mjcf/blur_final ..."
want=$(sha256sum "$FINAL_MJCF/blur.xml" | cut -d' ' -f1)
have=$(timeout 60 "${SSH[@]}" "sha256sum $RB/mjcf/blur_final/blur.xml 2>/dev/null | cut -d' ' -f1" || true)
if [ "$want" != "$have" ]; then
  timeout 60 "${SSH[@]}" "mkdir -p $RB/mjcf/blur_final"
  tar -C "$FINAL_MJCF" -cz --exclude='MUJOCO_LOG.TXT' . | timeout 600 "${SSH[@]}" "tar -C $RB/mjcf/blur_final -xz"
  say "final MJCF copied (sha ${want:0:12})"
fi

# 2. freeze the campaign code + manifest
say "freezing campaign code ..."
timeout 120 "${SSH[@]}" "bash $RB/code/blur/hpcc/campaign_setup.sh finalbody"

# 3. launch two singleton lanes (blur-fb-1/2), respecting the core gate in lane.sbatch
LANE=$RB/campaigns/finalbody/code/hpcc/jobs/lane.sbatch
for n in 1 2; do
  say "submitting blur-fb-$n ..."
  timeout 60 "${SSH[@]}" "cd $RB && sbatch --export=NONE,CAMPAIGN=finalbody -J blur-fb-$n \
     -p comp06 -q comp -c 32 -t 6:00:00 --dependency=singleton $LANE"
done
say "launched. Watch: blur/hpcc/fetch_runs.sh finalbody then runs/hpcc_finalbody/SUMMARY.md"
