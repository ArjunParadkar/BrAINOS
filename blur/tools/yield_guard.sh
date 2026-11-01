#!/usr/bin/env bash
# yield_guard.sh -- BLUR training never competes with Arjun's research jobs.
# Every 5 s: if any InvProj1 / bpusim job is live (their systemd units),
# freeze every blur-train-* unit
# (SIGSTOP: yields the CPU at once, loses nothing); thaw when they are gone.
busy() {
  systemctl --user list-units --state=active --no-legend 'bpu-*' 's1[0-9]-*' 's2[0-9]-*' 'omen-rerun*' '*invproj*' '*bpusim*' 2>/dev/null | grep -q . && return 0
  # the research jobs run as systemd units; a shell (or another Claude
  # session) merely sitting in those folders is not a job
  return 1
}
state=""
while true; do
  if busy; then want=STOP; else want=CONT; fi
  if [ "$want" != "$state" ]; then
    echo "[$(date -Is)] research jobs $( [ $want = STOP ] && echo LIVE: freezing || echo idle: thawing ) blur-train-*"
    state=$want
  fi
  for u in $(systemctl --user list-units --state=active --no-legend 'blur-train-*' 2>/dev/null | awk '{print $1}'); do
    systemctl --user kill -s "SIG$want" "$u"
  done
  sleep 5
done
