#!/usr/bin/env bash
# Launch one BODY of Blur: core in QEMU + its own tethered cognitive limb
# (mind daemon). Two bodies (A/B) can boot from the SAME BrAIn Key image
# to demonstrate one entity moving between machines -- but never at the
# same time, since they'd be writing the same FAT16 image concurrently.
# A flock on the key image enforces that: the second launch fails fast
# with a clear message instead of racing the first.
#
# BOUNDARY (changed 2026-07-31 on soapai's direction, "make it like it
# would be in the real OS"): the entity has a NETWORK ORGAN of its own --
# a user-mode NIC, outbound only, no inbound, no host filesystem exposure.
# The firmware's HTTP stack backs the KIRA-gated web.get/web.save limbs,
# so the internet is reached through this body's hardware rather than by
# borrowing a host program. BRAINOS_NET=off boots it isolated again, and
# the net region then honestly vanishes from the body map.
#
# Otherwise unchanged: no shared folders, no USB passthrough. The other
# channels out of the GUEST are the COM3 link (mind daemon, no actuators)
# and COM1 (a passive, write-only mirror of the on-screen console text).
# The HOST-side daemon has one outbound path of its own -- HTTPS to
# Anthropic for the deliberate-cognition pathway (body_daemon.py
# CloudMind) -- gated on ANTHROPIC_API_KEY in this shell's environment.
#
# Usage: run_instance.sh <A|B> [--headless] [--key PATH]
set -euo pipefail
cd "$(dirname "$0")/.."

BODY="${1:?usage: run_instance.sh <A|B> [--headless] [--key PATH]}"
shift || true
HEADLESS=0
KEY_IMG="$PWD/brainos-key.img"
# The world disk carries the entity's files, not its identity/memory (those
# live on the key). Same entity -> same world, so both bodies share one
# image; the key flock below already serialises launches, so only one qemu
# ever has it open. Attached only if it exists: with no world disk the core's
# world_present() is false and the filesystem limb honestly disappears.
WORLD_IMG="$PWD/brainos-world.img"
while [ $# -gt 0 ]; do
  case "$1" in
    --headless) HEADLESS=1 ;;
    --key) KEY_IMG="$2"; shift ;;
    *) echo "[run] unknown arg: $1" >&2; exit 1 ;;
  esac
  shift
done

QLOCAL="$PWD/tools/qemu-local"
VENV_PY="/home/god/fellows-assessment/.venv/bin/python"
LOG_DIR="$PWD/workspace/logs"
AUDIO_DIR="$PWD/workspace/vm_audio_${BODY}"
mkdir -p "$LOG_DIR" "$AUDIO_DIR/mic_in" "$AUDIO_DIR/speaker_out"

SOCK="/tmp/brainos_link_${BODY}.sock"
QMP_SOCK="/tmp/brainos_qmp_${BODY}.sock"
DAEMON_LOG="$LOG_DIR/body_daemon_${BODY}.log"
CONSOLE_LOG="$LOG_DIR/console_${BODY}.log"
OVMF_VARS="$QLOCAL/OVMF_VARS_${BODY}.fd"
LOCK_FILE="$PWD/workspace/key.lock"

rm -f "$SOCK" "$QMP_SOCK" "$CONSOLE_LOG"

# Claim the key image lock FIRST, before paying for an 8B model load: a
# refused launch must be cheap and instant, not a wasted GPU load that
# then has to be torn down. The fd stays open across the final exec, so
# the lock releases the instant this body's qemu exits -- clean or killed.
exec {LOCK_FD}>"$LOCK_FILE"
if ! flock -n "$LOCK_FD"; then
  echo "[run] REFUSED: another BrAInOS body is already running against $KEY_IMG" >&2
  echo "[run] release it first (ESC in its window), then retry" >&2
  exit 1
fi

# Only one body is ever meant to be live at a time (the lock above is what
# enforces that). A daemon whose VM already released the body does NOT
# exit on its own -- it sits holding ~7GB VRAM waiting to reconnect a
# socket that's gone. Clean up ANY leftover daemon (from this body or the
# other one) before loading a fresh model, or the second body's 8B load
# OOMs / silently falls back to 4B. Matched by argv0 + script path only
# (never by a data value like a socket/image path) to avoid a `pkill -f`
# footgun where the pattern matches this very shell's own command line.
pkill -f "[b]ody_daemon.py" 2>/dev/null && sleep 2 || true

# Writable OVMF vars, one copy per body (independent firmware state --
# these are NOT identity/memory, just boot variables, so no sharing needed).
[ -f "$OVMF_VARS" ] || cp "$QLOCAL/usr/share/edk2/x64/OVMF_VARS.4m.fd" "$OVMF_VARS"

if [ -n "${ANTHROPIC_API_KEY:-}" ]; then
  echo "[run] body $BODY: ANTHROPIC_API_KEY set -- deliberate pathway (Fable 5) available"
else
  echo "[run] body $BODY: ANTHROPIC_API_KEY not set -- fast pathway (Qwen) only"
fi

echo "[run] body $BODY: starting cognitive limb (whisper + Qwen3-8B + kokoro)..."
# {LOCK_FD}>&- : the daemon outlives this body's qemu process (it doesn't
# exit on its own -- see the pkill comment above), so it must NOT inherit
# the key-image lock fd. Only qemu, which actually has the image open via
# -drive, should hold it -- otherwise the lock stays wedged after a clean
# shutdown because the orphaned daemon still holds a copy of the fd.
# The lens is OPT-IN, like the network organ: BRAINOS_CAMERA=on opens the
# virtual camera jack. Without it the entity genuinely has no eyes, its
# self-model says so, and KIRA refuses vision.look at authz.
# BRAINOS_NO_VOICE=1 skips whisper+kokoro. On this 8GB box the audio
# stack plus the 8B mind sits within ~600MB of the ceiling, so a test that
# doesn't need ears/voice should not pay for them -- see camera_test.py.
VOICE_ARGS=()
if [ "${BRAINOS_NO_VOICE:-0}" = "1" ]; then
  VOICE_ARGS=(--no-voice)
  echo "[run] body $BODY: voice stack OFF (whisper+kokoro not loaded)"
fi

CAM_ARGS=()
if [ "${BRAINOS_CAMERA:-off}" = "on" ]; then
  CAM_ARGS=(--camera --camera-dir "$PWD/workspace/vm_video_$BODY")
  echo "[run] body $BODY: camera jack ON ($PWD/workspace/vm_video_$BODY/cam_in)"
fi
nohup "$VENV_PY" tools/body_daemon.py --socket "$SOCK" --audio-dir "$AUDIO_DIR" \
  "${CAM_ARGS[@]}" "${VOICE_ARGS[@]}" \
  > "$DAEMON_LOG" 2>&1 {LOCK_FD}>&- &
DAEMON_PID=$!
echo "[run] body $BODY: daemon pid $DAEMON_PID -- log: $DAEMON_LOG"

for i in $(seq 1 120); do
  grep -q "\[mind\] ready" "$DAEMON_LOG" && break
  kill -0 "$DAEMON_PID" 2>/dev/null || { echo "[run] daemon died, see log"; exit 1; }
  sleep 2
done
grep -q "\[mind\] ready" "$DAEMON_LOG" || echo "[run] mind not ready after 4min, booting anyway (F2 to retry link)"

export LD_LIBRARY_PATH="$QLOCAL/usr/lib"
export QEMU_MODULE_DIR="$QLOCAL/usr/lib/qemu"

DISPLAY_ARGS=(-display gtk)
[ "$HEADLESS" = "1" ] && DISPLAY_ARGS=(-display none)

WORLD_ARGS=()
if [ -f "$WORLD_IMG" ]; then
  WORLD_ARGS=(-drive format=raw,file="$WORLD_IMG")
  echo "[run] body $BODY: world disk attached ($WORLD_IMG)"
else
  echo "[run] body $BODY: no world disk -- filesystem limb will be absent"
fi

# The entity's network organ (see the boundary note above).
NET_ARGS=(-nic user,model=virtio-net-pci)
if [ "${BRAINOS_NET:-on}" = "off" ]; then
  NET_ARGS=(-nic none)
  echo "[boot] BRAINOS_NET=off — booting with no network organ"
fi

exec "$QLOCAL/usr/bin/qemu-system-x86_64" \
  -name "BrAInOS -- Body ${BODY}" \
  -enable-kvm -m 1G -cpu host \
  -L "$QLOCAL/usr/share/qemu" \
  -drive if=pflash,format=raw,readonly=on,file="$QLOCAL/usr/share/edk2/x64/OVMF_CODE.4m.fd" \
  -drive if=pflash,format=raw,file="$OVMF_VARS" \
  -drive format=raw,file="$KEY_IMG" \
  "${WORLD_ARGS[@]}" \
  -chardev socket,id=mm,path="$SOCK",server=on,wait=off \
  -device isa-serial,chardev=mm,index=2 \
  -serial file:"$CONSOLE_LOG" \
  -qmp unix:"$QMP_SOCK",server,nowait \
  -device qemu-xhci -device usb-tablet \
  "${NET_ARGS[@]}" \
  "${DISPLAY_ARGS[@]}"
