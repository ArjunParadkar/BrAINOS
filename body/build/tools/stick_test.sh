#!/usr/bin/env bash
# stick_test.sh — prove the PRODUCTION stick (brainos.img) boots end to end:
# UEFI seed (core) -> KIRA-gated incorporate -> chainload BODY.EFI -> Linux body
# -> session/operator up -> operator holds the p3 key -> fast tier answers.
# Headless QEMU; serial for the seed's console, ssh (dev key) for the body.
#   ./stick_test.sh [path/to/brainos.img]
set -uo pipefail
cd "$(dirname "$0")"
TOOLS="$PWD"; BUILD="$(cd .. && pwd)"; REPO="$(cd ../../.. && pwd)"
WORK="$BUILD/work"; IMG="${1:-$REPO/brainos.img}"
KEY="$WORK/ssh/brainos_dev_ed25519"; PORT="${SSH_PORT:-2223}"
SERIAL="$WORK/out/serial-stick.log"; : > "$SERIAL"
[ -f "$IMG" ] || { echo "no image at $IMG"; exit 1; }
[ -f "$KEY" ] || { echo "no dev key at $KEY"; exit 1; }
KEYUSE="$WORK/out/.stick_key"; install -m600 "$KEY" "$KEYUSE"
SSHOPT=(-i "$KEYUSE" -p "$PORT" -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null
        -o IdentitiesOnly=yes -o ConnectTimeout=8 -o LogLevel=ERROR)
sshc(){ ssh "${SSHOPT[@]}" human@127.0.0.1 "$@"; }

echo ">> booting production stick $IMG (headless, snapshot)"
"$TOOLS/run_body.sh" --serial-log "$SERIAL" --ssh-port "$PORT" "$IMG" >"$WORK/out/runbody-stick.log" 2>&1 &
RUNPID=$!
cleanup(){ kill "$RUNPID" 2>/dev/null || true; pkill -f "qemu-system-x86_64.*brainos.img" 2>/dev/null || true; rm -f "$KEYUSE"; }
trap cleanup EXIT

t0=$(date +%s.%N); ok=0
for i in $(seq 1 120); do sshc true 2>/dev/null && { ok=1; break; }; sleep 2; done
t1=$(date +%s.%N)
[ $ok = 1 ] || { echo "!! body never reachable. serial tail:"; strings "$SERIAL" | tail -30; exit 1; }
echo ">> body reachable after $(awk "BEGIN{printf \"%.1f\", $t1-$t0}")s (firmware -> seed -> chainload -> sshd)"

echo ">> seed console (serial) markers:"
p=0; f=0
ck(){ local l="$1"; shift; if out="$("$@" 2>&1)"; then printf "  PASS  %-28s %s\n" "$l" "$(echo "$out"|head -1)"; p=$((p+1)); else printf "  FAIL  %-28s %s\n" "$l" "$(echo "$out"|head -1)"; f=$((f+1)); fi; }
S="$(strings "$SERIAL")"
ck "seed-banner"          bash -c "echo '$S' | grep -m1 -i 'brain'"
ck "seed-incorporate"     bash -c "echo '$S' | grep -m1 '\[incorporate\] reading the production body'"
ck "seed-no-fallback"     bash -c "! echo '$S' | grep -q 'did not take the machine'"
ck "linux-login"          bash -c "echo '$S' | grep -m1 'brainos login:'"

echo ">> body checks over ssh:"
report="$(sshc 'bash -s' <<'REMOTE'
p=0; f=0
ck(){ local l="$1"; shift; if out="$("$@" 2>&1)"; then printf "  PASS  %-28s %s\n" "$l" "$(echo "$out"|head -1)"; p=$((p+1)); else printf "  FAIL  %-28s %s\n" "$l" "$(echo "$out"|head -1)"; f=$((f+1)); fi; }
echo "  system: $(systemctl is-system-running)"
ck "p1-brain-mounted"     findmnt -no SOURCE /brain
ck "p3-secure-partlabel"  test -e /dev/disk/by-partlabel/BRAINOS-SECURE
ck "p3-owned-by-human"    bash -c 'stat -Lc %U /dev/disk/by-partlabel/BRAINOS-SECURE | grep -q human'
ck "p3-has-BSEC-records"  bash -c 'head -c 4 /dev/disk/by-partlabel/BRAINOS-SECURE | grep -q BSEC'
ck "seed-hex-on-p1"       test -s /brain/BRAIN/SEED.HEX
ck "hyprland-running"     pgrep -x Hyprland
ck "operator-running"     pgrep -f brainos-operator
ck "operator-socket"      test -S "$XDG_RUNTIME_DIR/brainos/operator.sock"
ck "face-running"         pgrep -f brainos-face
ck "chromium-cdp"         pgrep -f "remote-debugging-port=9222"
# llama loads a 1 GB model; wait for it honestly instead of racing it
for i in $(seq 1 60); do curl -sS --max-time 5 http://127.0.0.1:8080/health 2>/dev/null | grep -q ok && break; sleep 3; done
ck "llama-health"         bash -c 'curl -sS --max-time 10 http://127.0.0.1:8080/health | grep -q ok'
ck "llama-completion"     bash -c 'curl -sS --max-time 90 -H "Content-Type: application/json" -X POST http://127.0.0.1:8080/completion -d "{\"prompt\":\"Say hi.\",\"n_predict\":8,\"temperature\":0}" | grep -q content'
# operator IPC: a reflex turn straight through the real socket
ck "operator-reflex-time" python3 - <<'PY'
import json,os,socket,time
s=socket.socket(socket.AF_UNIX); s.connect(os.path.join(os.environ["XDG_RUNTIME_DIR"],"brainos","operator.sock"))
s.sendall((json.dumps({"t":"utter","text":"what time is it"})+"\n").encode())
s.settimeout(5); buf=b""; t0=time.time()
while time.time()-t0<5:
    buf+=s.recv(4096)
    for l in buf.decode(errors="replace").splitlines():
        if l.strip():
            m=json.loads(l)
            if m.get("t")=="say" and m.get("done"): print(m["text"]); raise SystemExit(0)
raise SystemExit(1)
PY
echo "SUMMARY p=$p f=$f"
REMOTE
)"
echo "$report"
rf="$(echo "$report" | sed -n 's/.*SUMMARY p=\([0-9]*\) f=\([0-9]*\).*/\2/p')"
echo ">> operator key source (guest journal/session log):"
sshc 'grep -h "\[key\]\|tier=" "$XDG_RUNTIME_DIR/brainos-session.log" 2>/dev/null | tail -4' || true
total=$(( f + ${rf:-1} ))
echo ">> stick: seed-checks-failing=$f body-checks-failing=${rf:-?}"
[ "$total" = 0 ]
