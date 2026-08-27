#!/usr/bin/env bash
# body_smoke.sh — prove the body boots and its services are alive.
# Boots the body headless in QEMU, waits for sshd, then runs every check over a
# SINGLE ssh connection (sshd throttles rapid reconnects), and reports the
# firmware->reachable time. Exit 0 iff all pass.
#
#   ./body_smoke.sh                 # direct-mode: uses work/out/{body.ext4,BODY.EFI}
set -uo pipefail
cd "$(dirname "$0")"
TOOLS="$PWD"; BUILD="$(cd .. && pwd)"; REPO="$(cd ../../.. && pwd)"
WORK="$BUILD/work"
KEY="$WORK/ssh/brainos_dev_ed25519"
PORT="${SSH_PORT:-2222}"
SERIAL="$WORK/out/serial-smoke.log"
: > "$SERIAL"

[ -f "$KEY" ] || { echo "no dev key at $KEY (build with dev ssh)"; exit 1; }
# ssh needs a private key that isn't group/world-readable
KEYUSE="$WORK/out/.smoke_key"; install -m600 "$KEY" "$KEYUSE"
SSHOPT=(-i "$KEYUSE" -p "$PORT" -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null
        -o IdentitiesOnly=yes -o ConnectTimeout=8 -o LogLevel=ERROR)
sshc(){ ssh "${SSHOPT[@]}" human@127.0.0.1 "$@"; }

echo ">> launching body (headless)"
pkill -f "qemu-system-x86_64.*${WORK//\//\\/}" 2>/dev/null || true
"$TOOLS/run_body.sh" --body "$WORK/out/body.ext4" --efi "$WORK/out/BODY.EFI" \
    --serial-log "$SERIAL" --ssh-port "$PORT" >"$WORK/out/runbody-smoke.log" 2>&1 &
RUNPID=$!
cleanup(){ pkill -f "qemu-system-x86_64.*test.img" 2>/dev/null || true;
           kill "$RUNPID" 2>/dev/null || true; rm -f "$KEYUSE"; }
trap cleanup EXIT

echo ">> waiting for sshd (up to ~200s)"
t0=$(date +%s.%N); ok=0
for i in $(seq 1 100); do
  if sshc true 2>/dev/null; then ok=1; break; fi
  sleep 2
done
t1=$(date +%s.%N)
if [ "$ok" != 1 ]; then
  echo "!! sshd never came up. serial tail:"; tail -30 "$SERIAL" | strings | tail -20; exit 1
fi
boot_s=$(awk "BEGIN{printf \"%.1f\", $t1-$t0}")
echo ">> ssh reachable after ${boot_s}s (firmware->reachable)"

echo ">> running checks over one connection ..."
report="$(sshc 'bash -s' <<'REMOTE'
p=0; f=0
ck(){ local l="$1"; shift; if out="$("$@" 2>&1)"; then printf "  PASS  %-26s %s\n" "$l" "$(echo "$out"|head -1)"; p=$((p+1)); else printf "  FAIL  %-26s %s\n" "$l" "$(echo "$out"|head -1)"; f=$((f+1)); fi; }
echo "system: $(systemctl is-system-running)"
echo "failed-units:"; systemctl --failed --no-legend || true
ck "NetworkManager"   systemctl is-active NetworkManager
ck "internet"         bash -c 'curl -sS --max-time 15 -o /dev/null -w %{http_code} http://example.com | grep -q 200'
ck "llama-health"     bash -c 'curl -sS --max-time 10 http://127.0.0.1:8080/health | grep -q ok'
ck "brain-mounted"    findmnt -no TARGET /brain
ck "firstboot-grown"  test -f /var/lib/brainos/firstboot-done
ck "ydotoold-socket"  test -S /run/ydotoold/socket
ck "hyprland-running" pgrep -f "Hyprland|sway"
ck "operator-running" pgrep -f brainos-operator
ck "chromium-cdp"     pgrep -f "remote-debugging-port=9222"
ck "whisper-cli"      bash -c 'whisper-cli 2>&1 | grep -qi "no input\|usage\|error"'
ck "piper"            bash -c '/opt/brainos/piper/piper --help 2>&1 | grep -qi usage'
ck "chromium-bin"     command -v chromium
echo "llama-latency:"
curl -sS --max-time 60 -o /dev/null -w "  20tok http=%{http_code} total=%{time_total}s\n" \
  -H 'Content-Type: application/json' -X POST http://127.0.0.1:8080/completion \
  -d '{"prompt":"Say hi.","n_predict":20,"temperature":0}'
echo "SUMMARY p=$p f=$f"
REMOTE
)"
echo "$report"
echo
echo ">> firmware->login serial markers:"
strings "$SERIAL" | grep -aE 'brainos login:|Reached target Multi-User' | tail -3 || true
fails="$(echo "$report" | sed -n 's/.*SUMMARY p=[0-9]* f=\([0-9]*\).*/\1/p')"
echo ">> boot=${boot_s}s  failing-checks=${fails:-?}"
[ "${fails:-1}" = 0 ]
