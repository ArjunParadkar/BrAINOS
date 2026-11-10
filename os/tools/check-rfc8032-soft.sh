#!/usr/bin/env bash
#
# Run the RFC 8032 vectors on the host with sha2's software backend forced
# — the backend the UEFI targets use.
#
# The UEFI builds cannot run tests (there is no test runner on a target
# with no operating system), so on their own they only show that the
# software path *compiles*. This shows that it *computes ed25519
# correctly*: it works on a throwaway copy of the workspace in which the
# target-scoped sha2 line in crypto/Cargo.toml is made unconditional, and
# runs the same vectors there. Nothing in the real tree is modified.
#
# What it still does not show: the behaviour of the code LLVM emits for
# the UEFI targets themselves. That needs a boot.
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
mkdir "$tmp/os"
tar -c --exclude=./target . | tar -x -C "$tmp/os"
# os/bodyd links ../miniblur/core by path
[ -d ../miniblur ] && mkdir -p "$tmp/miniblur" && tar -c -C ../miniblur --exclude=./target . | tar -x -C "$tmp/miniblur"
sed -i "s/^\[target.'cfg(target_os = \"uefi\")'.dependencies\]$/[dependencies.sha2]\nversion = \"0.10\"\ndefault-features = false\nfeatures = [\"force-soft\"]\n\n[target.'cfg(any())'.dependencies]/" "$tmp/os/crypto/Cargo.toml"
grep -q '^\[dependencies.sha2\]' "$tmp/os/crypto/Cargo.toml" || { echo "could not rewrite the manifest" >&2; exit 1; }
sed -i '/^sha2 = /d' "$tmp/os/crypto/Cargo.toml"
cd "$tmp/os"
cargo tree -p brainos-crypto -e features --offline 2>/dev/null | grep -q 'sha2 feature "force-soft"' \
  || { echo "force-soft is not active in the copy" >&2; exit 1; }
echo "sha2 force-soft active on host copy; running RFC 8032 vectors"
CARGO_TARGET_DIR="$tmp/target" cargo test --offline -p brainos-crypto --test crypto_suite rfc8032 2>&1 | tail -6
