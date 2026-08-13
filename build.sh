#!/usr/bin/env bash
# Build the BrAInOS portable core for both architectures and mint the key.
set -euo pipefail
cd "$(dirname "$0")"
export PATH="$HOME/.cargo/bin:$PATH"

cargo build --release --target x86_64-unknown-uefi  --manifest-path core/Cargo.toml
cargo build --release --target aarch64-unknown-uefi --manifest-path core/Cargo.toml
# --keep-key: same entity across rebuilds — identity and memories carry over.
# Delete key/brain_key.json to mint a brand-new entity instead.
python3 tools/make_key.py --keep-key "${1:-$PWD/brainos-key.img}"
# The world disk is created once and then left alone — rebuilding the core
# must never wipe the entity's files. tools/make_world.py --force resets it.
python3 tools/make_world.py "$PWD/brainos-world.img"
