#!/usr/bin/env bash
# Launch Blur (BrAInOS instance): core in QEMU + the tethered cognitive
# limb (mind daemon). QEMU/OVMF live user-locally in tools/qemu-local.
#
# BOUNDARY (changed 2026-07-31 on soapai's direction, "make it like it
# would be in the real OS"): the entity now has a NETWORK ORGAN of its
# own. The guest gets a user-mode NIC — outbound only, no inbound, no
# host filesystem exposure, no shared folders, no USB passthrough, no
# clipboard agent. The firmware's own HTTP stack backs the KIRA-gated
# web.get/web.save limbs, so the entity reaches the internet through its
# body's hardware rather than by borrowing a host program. Set
# BRAINOS_NET=off to boot it isolated again (the net region then honestly
# disappears from the body map and every web verb is refused at authz).
#
# Everything else the entity can do still lives INSIDE the VM. The mind
# daemon on the tether has no actuators — it thinks, transcribes and
# synthesizes, and answers every action request with a formal error. Sight
# follows hearing exactly: BRAINOS_CAMERA=on opens a virtual lens jack
# (workspace/vm_video/cam_in), and the daemon never opens a real camera
# device — a file appearing at the jack IS the light, same as wav files
# are for the ears. The
# HOST-side daemon has one sanctioned egress of its own (outbound HTTPS
# to Anthropic for deliberate cognition — tools/body_daemon.py CloudMind).
set -euo pipefail
cd "$(dirname "$0")"

QLOCAL="$PWD/tools/qemu-local"
VENV_PY="/home/god/fellows-assessment/.venv/bin/python"
SOCK="/tmp/link.sock"
LOG_DIR="$PWD/workspace/logs"
# The entity's file storage, separate from the key (identity/memory live on
# the key; files live here). Attached only if it exists -- absent, the core's
# filesystem limb honestly disappears rather than pretending to be there.
WORLD_IMG="$PWD/brainos-world.img"
mkdir -p "$LOG_DIR"

# Stale daemon = OOM on the 8B load (silent 4B fallback). Always clear it.
pkill -f "tools/body_daemon.py" 2>/dev/null && sleep 2 || true
pkill -f "brainos-key.img" 2>/dev/null && sleep 1 || true

# Writable OVMF vars (CODE stays pristine)
[ -f "$QLOCAL/OVMF_VARS.fd" ] || cp "$QLOCAL/usr/share/edk2/x64/OVMF_VARS.4m.fd" "$QLOCAL/OVMF_VARS.fd"

# ANTHROPIC_API_KEY is inherited from this shell's environment (never read
# from a file, never written to one) -- export it before running this
# script to light up the deliberate pathway. Without it Blur still boots
# and thinks fine, just fast-pathway-only (Qwen).
if [ -n "${ANTHROPIC_API_KEY:-}" ]; then
  echo "[run] ANTHROPIC_API_KEY set -- deliberate pathway (Fable 5) will be available"
else
  echo "[run] ANTHROPIC_API_KEY not set -- fast pathway (Qwen) only this session"
fi

# The lens is OPT-IN, like the network organ: BRAINOS_CAMERA=on opens the
# virtual camera jack (drop a png/jpg into workspace/vm_video/cam_in and it
# is light arriving at the lens). Without it the entity genuinely has no
# eyes, its self-model says so, and KIRA refuses vision.look at authz.
CAM_ARGS=()
if [ "${BRAINOS_CAMERA:-off}" = "on" ]; then
  CAM_ARGS=(--camera --camera-dir "$PWD/workspace/vm_video")
  echo "[run] camera jack ON ($PWD/workspace/vm_video/cam_in)"
fi

echo "[run] starting body daemon (loads whisper + Qwen3-8B + kokoro, takes a bit)..."
nohup "$VENV_PY" tools/body_daemon.py --socket "$SOCK" "${CAM_ARGS[@]}" \
  > "$LOG_DIR/body_daemon.log" 2>&1 &
DAEMON_PID=$!
echo "[run] daemon pid $DAEMON_PID — log: $LOG_DIR/body_daemon.log"

# Boot the core only once the mind is loaded, so the tether is live
# at first link ping instead of needing F2.
for i in $(seq 1 120); do
  grep -q "\[mind\] ready" "$LOG_DIR/body_daemon.log" && break
  kill -0 "$DAEMON_PID" 2>/dev/null || { echo "[run] daemon died, see log"; exit 1; }
  sleep 2
done
grep -q "\[mind\] ready" "$LOG_DIR/body_daemon.log" || echo "[run] mind not ready after 4min, booting anyway (F2 to retry link)"

export LD_LIBRARY_PATH="$QLOCAL/usr/lib"
export QEMU_MODULE_DIR="$QLOCAL/usr/lib/qemu"

WORLD_ARGS=()
if [ -f "$WORLD_IMG" ]; then
  WORLD_ARGS=(-drive format=raw,file="$WORLD_IMG")
  echo "[run] world disk attached ($WORLD_IMG)"
else
  echo "[run] no world disk -- filesystem limb will be absent"
fi

# The entity's network organ (see the boundary note above).
NET_ARGS=(-nic user,model=virtio-net-pci)
if [ "${BRAINOS_NET:-on}" = "off" ]; then
  NET_ARGS=(-nic none)
  echo "[boot] BRAINOS_NET=off — booting with no network organ"
fi

exec "$QLOCAL/usr/bin/qemu-system-x86_64" \
  -enable-kvm -m 1G -cpu host \
  -L "$QLOCAL/usr/share/qemu" \
  -drive if=pflash,format=raw,readonly=on,file="$QLOCAL/usr/share/edk2/x64/OVMF_CODE.4m.fd" \
  -drive if=pflash,format=raw,file="$QLOCAL/OVMF_VARS.fd" \
  -drive format=raw,file=brainos-key.img \
  "${WORLD_ARGS[@]}" \
  -chardev socket,id=mm,path="$SOCK",server=on,wait=off \
  -device isa-serial,chardev=mm,index=2 \
  -device qemu-xhci -device usb-tablet \
  "${NET_ARGS[@]}" \
  -display gtk \
  "$@"
