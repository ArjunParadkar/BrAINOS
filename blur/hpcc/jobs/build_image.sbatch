#!/usr/bin/env bash
#SBATCH -J blur-image
#SBATCH -p cloud72 -q cloud -c 4 -t 02:00:00
#SBATCH -o /scrfs/storage/paradkar/blur/logs/%x-%j.out
# Builds $BLUR_SIF from blur.def. Tries `singularity build --fakeroot`; if the
# site does not allow it, does the same steps unprivileged: docker -> sandbox
# (node-local disk), apt/pip inside the writable sandbox, sandbox -> SIF.
set -euo pipefail
source /scrfs/storage/paradkar/blur/code/blur/hpcc/site/pinnacle.env
module load singularity/3.9.7
cd "$BLUR_CODE/hpcc"
mkdir -p "$(dirname "$BLUR_SIF")" "$SINGULARITY_CACHEDIR"
TMPB=/local_scratch/$SLURM_JOB_ID; [ -d "$TMPB" ] && [ -w "$TMPB" ] || TMPB=/tmp/blur-$SLURM_JOB_ID; mkdir -p "$TMPB"
export SINGULARITY_TMPDIR=$TMPB
echo "[$(date -Is)] node $(hostname) tmp $TMPB"; singularity --version
OUT=$BLUR_SIF.new
if singularity build --fakeroot "$OUT" blur.def; then
  echo "[$(date -Is)] built with --fakeroot"
else
  echo "[$(date -Is)] fakeroot build failed; unprivileged sandbox route"
  SB=$TMPB/sandbox
  singularity build --sandbox "$SB" docker://python:3.12-slim-bookworm
  mkdir -p "$SB/opt/blur"; cp requirements.txt "$SB/opt/blur/"
  # apt needs root inside the sandbox; skipped here (mujoco's own GL is only for rendering, not used on the cluster)
  singularity exec --writable --no-home --cleanenv "$SB" sh -c '
    set -e
    pip install --no-cache-dir -r /opt/blur/requirements.txt
    pip install --no-cache-dir torch==2.14.1 --index-url https://download.pytorch.org/whl/cpu
    pip freeze > /opt/blur/freeze.txt
    mkdir -p /.singularity.d/env
    printf "export PYTHONNOUSERSITE=1\nexport PYTHONDONTWRITEBYTECODE=1\n" > /.singularity.d/env/90-blur.sh'
  singularity build "$OUT" "$SB"
fi
mv -f "$OUT" "$BLUR_SIF"
singularity exec "$BLUR_SIF" cat /opt/blur/freeze.txt > "$BLUR_SIF.freeze.txt"
ls -la "$BLUR_SIF"; sha256sum "$BLUR_SIF" | tee "$BLUR_SIF.sha256"
rm -rf "$TMPB/sandbox"
echo "[$(date -Is)] done"
