#!/usr/bin/env bash
# sync_to_pinnacle.sh -- (laptop) copy the code tree and the MJCF to Pinnacle.
# Code: brainos/blur -> $BLUR_HOME/code/blur (no runs/, logs/, caches, schema data),
#       unpacked into code/blur.new and swapped in, so the copy is exact.
#       Don't sync while a blur- job that is still importing code is running.
# MJCF: BLUR/blur_mujoco_v36 -> $BLUR_HOME/mjcf/blur_mujoco_v36 (a copy; the
#       laptop original is never touched; blurcfg refuses a copy whose sha differs).
# tar over ssh (the laptop has no rsync); rides the open ControlMaster and
# fails fast (BatchMode) if it is not open.
set -euo pipefail
HOST=${PINNACLE:-pinnacle}
SRC=$(cd "$(dirname "$0")/.." && pwd)
MJCF=${BLUR_MJCF_DIR:-$HOME/Desktop/Arenda_Projects/BLUR/blur_mujoco_v36}
RB=/scrfs/storage/paradkar/blur
SSH=(ssh -o BatchMode=yes -o ConnectTimeout=20 "$HOST")
timeout 60 "${SSH[@]}" "mkdir -p $RB/{code,mjcf,images,runs,logs,cache} && rm -rf $RB/code/blur.new && mkdir $RB/code/blur.new"
tar -C "$SRC" -cz --exclude='__pycache__' --exclude='./learn/runs' --exclude='./schema/runs' --exclude='./logs' \
    --exclude='./schema/data' --exclude='*.mp4' . | timeout 600 "${SSH[@]}" "tar -C $RB/code/blur.new -xz"
timeout 60 "${SSH[@]}" "cd $RB/code && rm -rf blur.old && { [ ! -d blur ] || mv blur blur.old; } && mv blur.new blur && rm -rf blur.old"
want=$(sha256sum "$MJCF/blur.xml" | cut -d' ' -f1)
have=$(timeout 60 "${SSH[@]}" "sha256sum $RB/mjcf/blur_mujoco_v36/blur.xml 2>/dev/null | cut -d' ' -f1" || true)
if [ "$want" != "$have" ]; then
  tar -C "$(dirname "$MJCF")" -cz --exclude='MUJOCO_LOG.TXT' "$(basename "$MJCF")" | timeout 600 "${SSH[@]}" "tar -C $RB/mjcf -xz"
  echo "MJCF copied (sha ${want:0:12})"
fi
echo "synced $(git -C "$SRC" rev-parse --short HEAD 2>/dev/null) + working tree -> $HOST:$RB/code/blur"
