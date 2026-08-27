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
GUESTWAV="$WORK/out/guest-audio.wav"; rm -f "$GUESTWAV"
[ -f "$IMG" ] || { echo "no image at $IMG"; exit 1; }
[ -f "$KEY" ] || { echo "no dev key at $KEY"; exit 1; }
KEYUSE="$WORK/out/.stick_key"; install -m600 "$KEY" "$KEYUSE"
SSHOPT=(-i "$KEYUSE" -p "$PORT" -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null
        -o IdentitiesOnly=yes -o ConnectTimeout=8 -o LogLevel=ERROR)
sshc(){ ssh "${SSHOPT[@]}" human@127.0.0.1 "$@"; }

echo ">> booting production stick $IMG (headless, snapshot)"
"$TOOLS/run_body.sh" --serial-log "$SERIAL" --ssh-port "$PORT" --audio-wav "$GUESTWAV" "$IMG" >"$WORK/out/runbody-stick.log" 2>&1 &
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
# the ssh session is outside the Hyprland session: borrow its display + instance
export WAYLAND_DISPLAY="$(ls "$XDG_RUNTIME_DIR" | grep -m1 '^wayland-[0-9]')"
export HYPRLAND_INSTANCE_SIGNATURE="$(ls "$XDG_RUNTIME_DIR/hypr" 2>/dev/null | head -1)"
OPLOG="$XDG_RUNTIME_DIR/brainos/operator.log"
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

# ── voice loop: real audio through the body's own PipeWire graph ──────────────
# A null sink's monitor becomes the default source (the "microphone"); the body's
# own mouth synthesises the prompt; push-to-talk goes through the real face ctl
# socket; the operator records, transcribes, answers, and speaks the answer to
# the HDA card (captured host-side as guest-audio.wav).
ck "audio-card-present"   bash -c 'wpctl status | grep -q -i "hda\|alsa"'
ck "mic-null-sink"        bash -c 'pactl load-module module-null-sink sink_name=voicein >/dev/null && sleep 1 && pactl set-default-source voicein.monitor && pactl get-default-source | grep -q voicein'
ck "mouth-synth-prompt"   bash -c 'echo "what time is it" | /opt/brainos/piper/piper --model /opt/brainos/voices/en_US-lessac-medium.onnx --output_file /tmp/prompt.wav >/dev/null 2>&1 && test -s /tmp/prompt.wav'
rm -f /tmp/brainos-tts.wav /tmp/brainos-voice-capture.wav
ck "voice-turn"           python3 - <<'PYV'
import json,os,socket,subprocess,time
rt=os.environ["XDG_RUNTIME_DIR"]
obs=socket.socket(socket.AF_UNIX); obs.connect(os.path.join(rt,"brainos","operator.sock")); obs.settimeout(1)
def face(cmd):
    c=socket.socket(socket.AF_UNIX); c.connect(os.path.join(rt,"brainos","face.sock")); c.sendall(cmd.encode()); c.close()
face("ptt-on"); time.sleep(1.2)                         # recorder attaches to the default source
subprocess.run(["pw-play","--target","voicein","/tmp/prompt.wav"],check=True)
time.sleep(0.4); face("ptt-off")
heard=None; said=[]; t0=time.time(); buf=b""
while time.time()-t0<60:
    try: buf+=obs.recv(65536)
    except socket.timeout: continue
    lines=buf.split(b"\n"); buf=lines.pop()
    for l in lines:
        if not l.strip(): continue
        m=json.loads(l)
        if m.get("t")=="heard": heard=m["text"]
        if m.get("t")=="say" and m.get("text"): said.append(m["text"])
        if m.get("t")=="say" and m.get("done") and heard is not None:
            print("heard=%r -> said=%r" % (heard, " ".join(said)[:80])); raise SystemExit(0)
print("heard=%r said=%r (timeout)" % (heard, said)); raise SystemExit(1)
PYV
ck "voice-heard-words"    bash -c "grep -o '\[voice\] heard .* in [0-9]* ms' '$OPLOG' | tail -1 | grep ."
for i in $(seq 1 30); do grep -q "\[voice\] speaking" "$OPLOG" 2>/dev/null && break; sleep 1; done
ck "voice-reply-spoken"   bash -c "grep -q '\[voice\] speaking' '$OPLOG' && test -s /tmp/brainos-tts.wav && echo tts wav \$(stat -c%s /tmp/brainos-tts.wav) bytes"

# ── surface: the entity builds UI through the face (WebKitGTK inside the body) ─
ck "webkitgtk-6.0-loads"  python3 -c 'import gi; gi.require_version("WebKit","6.0"); from gi.repository import WebKit; print(WebKit.get_major_version(), WebKit.get_minor_version())'
ck "surface-turn"         python3 - <<'PYS'
import json,os,socket,time
rt=os.environ["XDG_RUNTIME_DIR"]
s=socket.socket(socket.AF_UNIX); s.connect(os.path.join(rt,"brainos","operator.sock")); s.settimeout(1)
html="<h2>Wi-Fi</h2><p>Pick a network.</p><button onclick=\"brainos.emit('click',{a:1})\">Join</button>"
s.sendall((json.dumps({"t":"utter","text":"Use the ui.build tool exactly once to build a surface named wifi whose html is exactly: "+html+" Then reply with one short sentence. Do nothing else."})+"\n").encode())
t0=time.time(); buf=b""; surf=False
while time.time()-t0<90:
    try: buf+=s.recv(65536)
    except socket.timeout: continue
    lines=buf.split(b"\n"); buf=lines.pop()
    for l in lines:
        if not l.strip(): continue
        m=json.loads(l)
        if m.get("t")=="surface" and m.get("html"): surf=True; print("surface %r html %d bytes" % (m["name"], len(m["html"])))
        if m.get("t")=="say" and m.get("done") and surf: raise SystemExit(0)
raise SystemExit(1)
PYS
sleep 2
ck "surface-layer-window" bash -c 'hyprctl layers | grep -m1 "brainos-surface"'
ck "surface-screenshot"   bash -c 'grim /tmp/surface.png && test -s /tmp/surface.png && echo "$(stat -c%s /tmp/surface.png) bytes"'
echo "SUMMARY p=$p f=$f"
REMOTE
)"
echo "$report"
rf="$(echo "$report" | sed -n 's/.*SUMMARY p=\([0-9]*\) f=\([0-9]*\).*/\2/p')"
scp "${SSHOPT[@]/#-p/-P}" -q human@127.0.0.1:/tmp/surface.png "$WORK/out/surface.png" 2>/dev/null && echo ">> surface screenshot -> $WORK/out/surface.png"
echo ">> guest audio captured host-side:"
python3 - "$GUESTWAV" <<'PYW' || true
import sys,array
try:
    d=open(sys.argv[1],"rb").read()[44:]; a=array.array("h",d[:len(d)//2*2])   # header may be unfinalised (QEMU killed)
    peak=max((abs(x) for x in a), default=0)
    print("  %s: %d samples, peak %d -> %s" % (sys.argv[1].split("/")[-1], len(a), peak, "REAL AUDIO reached the sound card" if peak>1000 else "silence"))
except Exception as e: print("  (no guest audio file: %s)" % e)
PYW
echo ">> operator key source (guest journal/session log):"
sshc 'grep -h "\[key\]\|\[voice\]\|tier=" "$XDG_RUNTIME_DIR/brainos/operator.log" 2>/dev/null | tail -6' || true
total=$(( f + ${rf:-1} ))
echo ">> stick: seed-checks-failing=$f body-checks-failing=${rf:-?}"
[ "$total" = 0 ]
