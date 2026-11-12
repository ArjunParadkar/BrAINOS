#!/usr/bin/env bash
#
# Run the SocketCAN live test on a real kernel vcan interface, without
# root and without changing the host's network.
#
# `unshare -Urn` starts a private user + network namespace. Inside it
# this process may create interfaces; vcan0 is created there, the test
# opens real AF_CAN raw sockets on it, and the namespace (and vcan0
# with it) disappears when the command exits. The one lasting effect
# is that the kernel's `vcan` module is loaded.
#
# This is a real SocketCAN socket on a real kernel CAN interface. It is
# not a physical bus: there are no transceivers and no other nodes.
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"
cargo test --offline --release -p brainos-host --test nerves_can --no-run 2>/dev/null
bin="$(ls -t target/release/deps/nerves_can-* | grep -v '\.d$' | head -1)"
unshare -Urn sh -c "ip link add dev vcan0 type vcan && ip link set up vcan0 && exec '$bin' --nocapture live_"
