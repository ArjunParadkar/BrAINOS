#!/usr/bin/env bash
# build_body.sh — build the BrAInOS Linux body: body.ext4 (p2) + BODY.EFI (UKI).
#
# Rootless on the host: all privileged work (pacstrap, chroot, mkinitcpio, ukify,
# mkfs owning uid/gid) happens inside a privileged archlinux container. The host
# needs only docker (no sudo, host packages untouched).
#
# Usage:
#   ./build_body.sh                 full build (models + dev ssh)
#   ./build_body.sh --no-dev-ssh    production image (no harness key, sshd off)
#   ./build_body.sh --skip-models   fast iterate (no gguf/whisper downloads)
#   ./build_body.sh --rebuild       wipe work/rootfs and pacstrap fresh
#   ./build_body.sh --no-vulkan     build llama.cpp CPU-only (skip shader compile)
#
# Outputs: body/build/work/out/{body.ext4,BODY.EFI}, body/build/PACKAGES.txt
set -euo pipefail
cd "$(dirname "$0")"
BUILD_DIR="$PWD"
REPO="$(cd ../.. && pwd)"
WORK="$BUILD_DIR/work"
IMAGE="${BRAINOS_BUILD_IMAGE:-archlinux:base-devel}"

WITH_MODELS=1 WITH_DEV_SSH=1 FORCE_ROOTFS=0 LLAMA_VULKAN=1
for a in "$@"; do case "$a" in
  --no-dev-ssh)  WITH_DEV_SSH=0 ;;
  --skip-models) WITH_MODELS=0 ;;
  --rebuild)     FORCE_ROOTFS=1 ;;
  --no-vulkan)   LLAMA_VULKAN=0 ;;
  -h|--help)     sed -n '2,17p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
  *) echo "build_body.sh: unknown option '$a'" >&2; exit 2 ;;
esac; done

mkdir -p "$WORK/cache/pacman" "$WORK/cache/models" "$WORK/out" "$WORK/ssh"

echo ">> BrAInOS body build"
echo "   image=$IMAGE  models=$WITH_MODELS  dev_ssh=$WITH_DEV_SSH  rebuild=$FORCE_ROOTFS  vulkan=$LLAMA_VULKAN"

exec docker run --rm --privileged --network host \
  -e WITH_MODELS="$WITH_MODELS" -e WITH_DEV_SSH="$WITH_DEV_SSH" \
  -e FORCE_ROOTFS="$FORCE_ROOTFS" -e LLAMA_VULKAN="$LLAMA_VULKAN" \
  -v "$REPO":/repo \
  -v "$WORK":/work \
  -v "$WORK/cache/pacman":/var/cache/pacman/pkg \
  "$IMAGE" bash /repo/body/build/build_in_container.sh
