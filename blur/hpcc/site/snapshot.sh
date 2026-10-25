# snapshot.sh -- sourced by every job after pinnacle.env: copy the code tree to
# $BLUR_HOME/snap/$SLURM_JOB_ID and point BLUR_CODE/BLUR_PY there, so a later
# sync_to_pinnacle.sh can't pull files out from under a running job, and each
# run's exact code stays on disk next to its results.
if [ -n "${SLURM_JOB_ID:-}" ]; then
  _snap=$BLUR_HOME/snap/$SLURM_JOB_ID
  mkdir -p "$_snap" && cp -a "$BLUR_CODE/." "$_snap/"
  export BLUR_CODE=$_snap BLUR_PY=$_snap/hpcc/bpy
  echo "code snapshot $_snap ($(cd "$_snap" && find . -type f -name '*.py' | sort | xargs cat | sha256sum | cut -c1-12))"
fi
