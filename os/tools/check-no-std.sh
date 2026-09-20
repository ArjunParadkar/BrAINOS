#!/usr/bin/env bash
#
# The real no_std check: build the canary for a target that has no
# operating system.
#
# The workspace claims every crate below `host` compiles with nothing
# beneath it. `cargo test -p brainos-nostd-check` checks that
# mechanically at the source level, which catches the way the discipline
# actually breaks. This script is the stronger check, and it needs a bare
# target installed:
#
#   rustup target add thumbv7em-none-eabihf
#   rustup target add x86_64-unknown-uefi        # what the boot core uses
#
# If none is present it says so and exits 2 — deliberately not 0, so a CI
# job cannot report "no_std verified" on a machine that never ran the
# verification.

set -uo pipefail
cd "$(dirname "$0")/.."

CANDIDATES=(
    thumbv7em-none-eabihf
    thumbv6m-none-eabi
    riscv32imac-unknown-none-elf
    aarch64-unknown-none
    x86_64-unknown-uefi
    aarch64-unknown-uefi
)

have_target() {
    rustc --print target-libdir --target "$1" >/dev/null 2>&1 \
        && [ -d "$(rustc --print target-libdir --target "$1" 2>/dev/null)" ]
}

FOUND=()
for t in "${CANDIDATES[@]}"; do
    if have_target "$t"; then
        FOUND+=("$t")
    fi
done

if [ ${#FOUND[@]} -eq 0 ]; then
    cat >&2 <<'MSG'
no bare-metal target is installed, so the strong no_std check did not run.

  what did run:   cargo test -p brainos-nostd-check
                  (source-level: no crate below `host` mentions std)
  what did not:   a build against a target with no operating system

to run it:

  rustup target add thumbv7em-none-eabihf
  tools/check-no-std.sh

exiting 2 rather than 0: a check that did not happen must not report
success.
MSG
    exit 2
fi

status=0
for t in "${FOUND[@]}"; do
    echo "── building the canary for $t"
    if cargo build -p brainos-nostd-check --target "$t" --offline; then
        echo "   ok"
    else
        echo "   FAILED on $t" >&2
        status=1
    fi
done

if [ $status -eq 0 ]; then
    echo
    echo "no_std verified against: ${FOUND[*]}"
fi
exit $status
