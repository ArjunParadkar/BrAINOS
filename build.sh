#!/usr/bin/env bash
# Build the BrAInOS portable core and mint the key.
#
# x86_64 is built by default. aarch64 is OPT-IN via --arm, so the everyday
# dev loop is fast and does NOT require the aarch64 rustup target to be
# installed. Pass --arm when you want a universal key that also boots the
# aarch64 targets (Pi 5, Jetson Orin).
#
#   ./build.sh                 x86_64 only  -> laptop-only key
#   ./build.sh --arm           x86_64 + aarch64 -> universal key
#   ./build.sh out.img         x86_64 only, custom image path
#   ./build.sh out.img --arm   both, custom image path
#
# STAGE 3 -- produce the full production image brainos.img:
#   ./build.sh --body body.ext4 --body-efi BODY.EFI
#     builds the core, mints a key whose p1 carries /EFI/BRAIN/BODY.EFI, then
#     wraps it with the ext4 body into brainos.img (p1 ESP + p2 body + p3 secure).
#   --esp-mib N          size p1 (default 256, enough for a 60-150 MiB body)
#   --api-key-file PATH  embed an Anthropic key in p3 (default ~/.brainos_api_key)
#
# The arch set is passed to make_key.py EXPLICITLY rather than letting it
# guess from whatever happens to sit in core/target/ -- otherwise a stale
# aarch64 binary from an earlier --arm build would silently ride along and
# a laptop-only key would claim to be universal.
set -euo pipefail
cd "$(dirname "$0")"
export PATH="$HOME/.cargo/bin:$PATH"

BUILD_ARM=""
OUT=""
BODY=""
BODY_EFI=""
ESP_MIB="256"
API_KEY_FILE=""
while [ $# -gt 0 ]; do
  a="$1"
  case "$a" in
    --arm|-a|arm) BUILD_ARM=1 ;;
    --body) BODY="$2"; shift ;;
    --body=*) BODY="${a#*=}" ;;
    --body-efi) BODY_EFI="$2"; shift ;;
    --body-efi=*) BODY_EFI="${a#*=}" ;;
    --esp-mib) ESP_MIB="$2"; shift ;;
    --esp-mib=*) ESP_MIB="${a#*=}" ;;
    --api-key-file) API_KEY_FILE="$2"; shift ;;
    --api-key-file=*) API_KEY_FILE="${a#*=}" ;;
    -h|--help)
      sed -n '2,20p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    -*)
      echo "build.sh: unknown option '$a' (try --help)" >&2
      exit 2
      ;;
    *) OUT="$a" ;;
  esac
  shift
done
OUT="${OUT:-$PWD/brainos-key.img}"

cargo build --release --target x86_64-unknown-uefi --manifest-path core/Cargo.toml

if [ -n "$BUILD_ARM" ]; then
  cargo build --release --target aarch64-unknown-uefi --manifest-path core/Cargo.toml
  ARCHES="both"
else
  ARCHES="x86_64"
fi

# --keep-key: same entity across rebuilds -- identity and memories carry over.
# Delete key/brain_key.json to mint a brand-new entity instead.
MK_ARGS=(--keep-key "--arch=$ARCHES" "--esp-mib=$ESP_MIB")
[ -n "$BODY_EFI" ] && MK_ARGS+=("--body-efi=$BODY_EFI")
[ -n "$API_KEY_FILE" ] && MK_ARGS+=("--api-key-file=$API_KEY_FILE")
python3 tools/make_key.py "${MK_ARGS[@]}" "$OUT"

# The world disk is created once and then left alone -- rebuilding the core
# must never wipe the entity's files. tools/make_world.py --force resets it.
python3 tools/make_world.py "$PWD/brainos-world.img"

# Stage 3: with a body, wrap the key into the full production image.
if [ -n "$BODY" ]; then
  if [ -z "$BODY_EFI" ]; then
    echo "build.sh: --body needs --body-efi too (the p1 launcher payload)" >&2
    exit 2
  fi
  IMG="$PWD/brainos.img"
  python3 tools/make_image.py --key "$OUT" --body "$BODY" -o "$IMG"
  echo "[build] production image: $IMG ($(stat -c%s "$IMG") bytes)"
fi
echo "[build] key: $OUT ($(stat -c%s "$OUT") bytes)"
