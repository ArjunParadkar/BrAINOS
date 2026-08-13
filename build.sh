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
# The arch set is passed to make_key.py EXPLICITLY rather than letting it
# guess from whatever happens to sit in core/target/ -- otherwise a stale
# aarch64 binary from an earlier --arm build would silently ride along and
# a laptop-only key would claim to be universal.
set -euo pipefail
cd "$(dirname "$0")"
export PATH="$HOME/.cargo/bin:$PATH"

BUILD_ARM=""
OUT=""
for a in "$@"; do
  case "$a" in
    --arm|-a|arm) BUILD_ARM=1 ;;
    -h|--help)
      sed -n '2,14p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    -*)
      echo "build.sh: unknown option '$a' (try --help)" >&2
      exit 2
      ;;
    *) OUT="$a" ;;
  esac
done
OUT="${OUT:-$PWD/brainos-key.img}"

cargo build --release --target x86_64-unknown-uefi --manifest-path core/Cargo.toml

if [ -n "$BUILD_ARM" ]; then
  cargo build --release --target aarch64-unknown-uefi --manifest-path core/Cargo.toml
  ARCHES="both"
else
  ARCHES="x86_64"
fi

# --keep-key: same entity across rebuilds — identity and memories carry over.
# Delete key/brain_key.json to mint a brand-new entity instead.
python3 tools/make_key.py --keep-key "--arch=$ARCHES" "$OUT"
# The world disk is created once and then left alone — rebuilding the core
# must never wipe the entity's files. tools/make_world.py --force resets it.
python3 tools/make_world.py "$PWD/brainos-world.img"
