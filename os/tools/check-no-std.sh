#!/usr/bin/env bash
#
# The real no_std check: build the canary for a target that has no
# operating system.
#
# The workspace claims every crate below `host` compiles with nothing
# beneath it. `cargo test -p brainos-nostd-check` checks that
# mechanically at the source level — no crate below `host` mentions
# `std`, every one declares `no_std` and forbids `unsafe`, and the
# external dependency list is still two crates long — which catches the
# way the discipline actually breaks in review.
#
# This is the stronger check, and it needs a bare target. On this
# machine the *system* rustc (/usr/bin/rustc, from the distribution) has
# none installed, while the rustup toolchain has the UEFI ones — so the
# script looks in both rather than concluding from the first that there
# is nothing to run. That distinction cost a session's worth of
# "verified at source level only" in the documentation.
#
# Every installed target must build. There was a standing exception for
# x86_64-unknown-uefi until 2026-11-10; it is gone, and a failure on any
# target is now a failure of the check.
#
# If no target is found anywhere it says so and exits 2 — deliberately
# not 0, so a CI job cannot report "no_std verified" on a machine that
# never ran the verification.

set -uo pipefail
cd "$(dirname "$0")/.."

CANDIDATES=(
    thumbv7em-none-eabihf
    thumbv6m-none-eabi
    riscv32imac-unknown-none-elf
    aarch64-unknown-none
    aarch64-unknown-uefi
    x86_64-unknown-uefi
)

# Each runner is a way of invoking cargo. The default toolchain first,
# then rustup's, because they are different installations with different
# targets and the one on PATH is not necessarily the one that can do it.
RUNNERS=("cargo")
if [ -x "$HOME/.cargo/bin/rustup" ]; then
    RUNNERS+=("$HOME/.cargo/bin/rustup run stable cargo")
fi

rustc_for() {
    case "$1" in
        cargo) echo "rustc" ;;
        *) echo "$HOME/.cargo/bin/rustup run stable rustc" ;;
    esac
}

FOUND=()   # "runner|target"
for runner in "${RUNNERS[@]}"; do
    rc="$(rustc_for "$runner")"
    for t in "${CANDIDATES[@]}"; do
        dir="$($rc --print target-libdir --target "$t" 2>/dev/null)"
        if [ -n "$dir" ] && [ -d "$dir" ]; then
            FOUND+=("$runner|$t")
        fi
    done
done

if [ ${#FOUND[@]} -eq 0 ]; then
    cat >&2 <<'MSG'
no bare-metal target is installed, so the strong no_std check did not run.

  what did run:   cargo test -p brainos-nostd-check
                  (source-level: no crate below `host` mentions std)
  what did not:   a build against a target with no operating system

to run it:

  rustup target add aarch64-unknown-uefi
  tools/check-no-std.sh

exiting 2 rather than 0: a check that did not happen must not report
success.
MSG
    exit 2
fi

passed=()
failed=()
for entry in "${FOUND[@]}"; do
    runner="${entry%%|*}"
    t="${entry##*|}"
    echo "── building the canary for $t"
    if $runner build -p brainos-nostd-check --target "$t" --offline >/dev/null 2>&1; then
        echo "   ok"
        passed+=("$t")
    else
        echo "   failed"
        failed+=("$t")
    fi
done

echo
if [ ${#passed[@]} -gt 0 ]; then
    echo "no_std verified against: ${passed[*]}"
fi
if [ ${#failed[@]} -gt 0 ]; then
    cat <<MSG

did not build: ${failed[*]}

Every installed bare target is expected to build, in the debug profile
this script uses. x86_64-unknown-uefi used to fail here inside \`sha2\`
and \`curve25519-dalek\` (their AVX2 paths on a target with SSE off);
that is fixed by crypto/Cargo.toml's UEFI-only \`force-soft\` line and
os/.cargo/config.toml's serial backend. If it is failing again, check
those two first — and that this was run from inside os/, since the
cargo config is only read from there.
MSG
    exit 1
fi

exit 0
