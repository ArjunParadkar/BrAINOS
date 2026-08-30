#!/usr/bin/env bash
# run_body.sh — boot a BrAInOS body in QEMU (KVM, bochs-display DRM, OVMF).
#
# Two modes:
#   (default) boot a full GPT image (the core agent's brainos.img):
#       ./run_body.sh [--headless] [path/to/brainos.img]
#   direct body test (no core image needed) — wrap body.ext4 + BODY.EFI in a
#   throwaway ESP+GPT disk and boot that:
#       ./run_body.sh --body ../work/out/body.ext4 --efi ../work/out/BODY.EFI [--headless]
#
# Flags:
#   --headless   -display none, serial on stdio (console=ttyS0 already in UKI)
#   --graphical  force gtk gl display (default when a DISPLAY is present)
#   --ssh-port N host-forward for guest sshd (default 2222)
#   --reach-port N host-forward for the operator's Reach listener (default 7777)
#   --mem G --smp N
#   --audio-wav F  capture everything the guest plays to host wav F (default: a
#                  silent dummy backend; the HDA card is present either way)
set -euo pipefail
SELF="$(readlink -f "$0")"
cd "$(dirname "$SELF")"
TOOLS="$PWD"
BUILD="$(cd .. && pwd)"
REPO="$(cd ../../.. && pwd)"
WORK="$BUILD/work"
QLOCAL="$REPO/tools/qemu-local"

MODE_DIRECT=0 HEADLESS=0 GRAPHICAL=-1 SSH_PORT=2222 REACH_PORT=7777 MEM=4G SMP=8 SERIAL_LOG="" AUDIO_WAV=""
BODY="" EFI="" IMG=""
while [ $# -gt 0 ]; do case "$1" in
  --headless)  HEADLESS=1 ;;
  --graphical) GRAPHICAL=1 ;;
  --body)      MODE_DIRECT=1; BODY="$2"; shift ;;
  --efi)       MODE_DIRECT=1; EFI="$2"; shift ;;
  --ssh-port)  SSH_PORT="$2"; shift ;;
  --reach-port) REACH_PORT="$2"; shift ;;
  --serial-log) SERIAL_LOG="$2"; HEADLESS=1; shift ;;
  --mem)       MEM="$2"; shift ;;
  --smp)       SMP="$2"; shift ;;
  --audio-wav) AUDIO_WAV="$2"; shift ;;
  -h|--help)   sed -n '2,20p' "$SELF" | sed 's/^# \{0,1\}//'; exit 0 ;;
  -*)          echo "run_body.sh: unknown option '$1'" >&2; exit 2 ;;
  *)           IMG="$1" ;;
esac; shift; done

[ "$GRAPHICAL" = "-1" ] && { [ -n "${WAYLAND_DISPLAY:-}${DISPLAY:-}" ] && [ "$HEADLESS" = 0 ] && GRAPHICAL=1 || GRAPHICAL=0; }

# ---------------------------------------------------------------------------
# Assemble a bootable disk if in direct mode.
if [ "$MODE_DIRECT" = "1" ]; then
  BODY="${BODY:-$WORK/out/body.ext4}"; EFI="${EFI:-$WORK/out/BODY.EFI}"
  [ -f "$BODY" ] || { echo "no body.ext4 at $BODY" >&2; exit 1; }
  [ -f "$EFI" ]  || { echo "no BODY.EFI at $EFI" >&2; exit 1; }
  IMG="$WORK/out/test.img"
  echo ">> assembling throwaway GPT test disk (ESP + BRAINOS-BODY) -> $IMG"
  docker run --rm --network host \
    -v "$WORK":/work -v "$BODY":/in/body.ext4:ro -v "$EFI":/in/BODY.EFI:ro \
    -v "$REPO/body/build/work/cache/pacman":/var/cache/pacman/pkg \
    archlinux:base-devel bash -euo pipefail -c '
      pacman -Sy --noconfirm --needed gptfdisk dosfstools mtools e2fsprogs >/dev/null
      cd /work/out
      bodysz=$(stat -c%s /in/body.ext4)
      bodysec=$(( (bodysz + 511) / 512 ))
      efisz=$(stat -c%s /in/BODY.EFI)
      # ESP holds two copies of the UKI (BOOTX64 + BRAIN) + slack, min 128 MiB
      espmib=$(( (efisz * 2) / 1048576 + 48 )); [ $espmib -lt 128 ] && espmib=128
      espsec=$(( espmib * 2048 ))
      start1=2048; start2=$(( start1 + espsec ))
      total=$(( start2 + bodysec + 2048 ))
      rm -f test.img
      truncate -s $(( total * 512 )) test.img
      # ESP
      truncate -s $(( espsec * 512 )) esp.img
      mkfs.fat -F32 -n BRAINOSBOOT esp.img >/dev/null   # FAT vol label max 11 chars; GPT PARTLABEL below is the real one
      mmd -i esp.img ::/EFI ::/EFI/BOOT ::/EFI/BRAIN
      mcopy -i esp.img /in/BODY.EFI ::/EFI/BOOT/BOOTX64.EFI
      mcopy -i esp.img /in/BODY.EFI ::/EFI/BRAIN/BODY.EFI
      # partition table
      sgdisk -a 1 \
        -n 1:${start1}:$(( start2 - 1 )) -t 1:ef00 -c 1:BRAINOS-BOOT \
        -n 2:${start2}:$(( start2 + bodysec - 1 )) -t 2:8300 -c 2:BRAINOS-BODY \
        test.img >/dev/null
      dd if=esp.img of=test.img bs=512 seek=${start1} conv=notrunc status=none
      dd if=/in/body.ext4 of=test.img bs=512 seek=${start2} conv=notrunc status=none
      rm -f esp.img
      echo "test.img assembled ($(du -h test.img | cut -f1))"
    '
else
  IMG="${IMG:-$REPO/brainos.img}"
  [ -f "$IMG" ] || { echo "no image at $IMG (build the core image, or use --body/--efi)" >&2; exit 1; }
fi

# ---------------------------------------------------------------------------
# OVMF: writable vars copy
mkdir -p "$WORK/ovmf"
VARS="$WORK/ovmf/OVMF_VARS.fd"
[ -f "$VARS" ] || cp "$QLOCAL/usr/share/edk2/x64/OVMF_VARS.4m.fd" "$VARS"
CODE="$QLOCAL/usr/share/edk2/x64/OVMF_CODE.4m.fd"

export LD_LIBRARY_PATH="$QLOCAL/usr/lib"
export QEMU_MODULE_DIR="$QLOCAL/usr/lib/qemu"
QEMU="$QLOCAL/usr/bin/qemu-system-x86_64"

# The built images are root-owned (written by the privileged build container) and
# this harness runs unprivileged. `snapshot=on` opens the base read-only (mode 644
# is fine) and diverts writes to an ephemeral temp, so no ownership fight and every
# test run starts from a pristine image. Set BRAINOS_PERSIST=1 to write through.
SNAP="snapshot=on"; [ "${BRAINOS_PERSIST:-0}" = 1 ] && SNAP="snapshot=off"

COMMON=(
  -machine q35,accel=kvm -cpu host -smp "$SMP" -m "$MEM"
  -L "$QLOCAL/usr/share/qemu"
  -drive if=pflash,format=raw,readonly=on,file="$CODE"
  -drive if=pflash,format=raw,file="$VARS"
  -drive file="$IMG",format=raw,if=none,id=disk0,cache=writeback,"$SNAP"
  -device virtio-blk-pci,drive=disk0
  -netdev user,id=n,hostfwd=tcp:127.0.0.1:${SSH_PORT}-:22,hostfwd=tcp:127.0.0.1:${REACH_PORT}-:7777
  -device virtio-net-pci,netdev=n
  -device qemu-xhci -device usb-tablet
  -vga none
  -rtc base=utc
)
# Sound card: the body's ears and mouth (PipeWire -> snd_hda_intel) need a real
# HDA codec. `wav` backend writes guest playback to a host file for the harness.
if [ -n "$AUDIO_WAV" ]; then
  COMMON+=(-audiodev "wav,id=snd,path=$AUDIO_WAV")
else
  COMMON+=(-audiodev none,id=snd)
fi
COMMON+=(-device intel-hda -device hda-duplex,audiodev=snd)

if [ "$GRAPHICAL" = "1" ]; then
  echo ">> booting GRAPHICAL (bochs-display DRM, gtk, software render). ssh: 127.0.0.1:${SSH_PORT}"
  exec "$QEMU" "${COMMON[@]}" \
    -device bochs-display -display gtk
elif [ -n "$SERIAL_LOG" ]; then
  echo ">> booting HEADLESS (serial -> $SERIAL_LOG). ssh: 127.0.0.1:${SSH_PORT}"
  exec "$QEMU" "${COMMON[@]}" \
    -device bochs-display -display none -serial "file:$SERIAL_LOG" -monitor none
else
  echo ">> booting HEADLESS (serial on stdio). ssh: 127.0.0.1:${SSH_PORT}"
  echo "   (login prompt + kernel log below; Ctrl-A X to quit)"
  exec "$QEMU" "${COMMON[@]}" \
    -device bochs-display -display none -serial mon:stdio
fi
