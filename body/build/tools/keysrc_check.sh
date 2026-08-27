#!/usr/bin/env bash
# boot the stick, then run a second operator instance in-guest on a scratch socket
# for 4s and capture its key-source log line (the session's own stderr is not kept).
set -uo pipefail; cd "$(dirname "$0")"; WORK="$(cd ../work && pwd)"; REPO="$(cd ../../.. && pwd)"
KEYUSE="$WORK/out/.ks_key"; install -m600 "$WORK/ssh/brainos_dev_ed25519" "$KEYUSE"
SSHOPT=(-i "$KEYUSE" -p 2224 -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o IdentitiesOnly=yes -o ConnectTimeout=8 -o LogLevel=ERROR)
./run_body.sh --serial-log "$WORK/out/serial-ks.log" --ssh-port 2224 "$REPO/brainos.img" >/dev/null 2>&1 & P=$!
trap 'kill $P 2>/dev/null; rm -f "$KEYUSE"' EXIT
for i in $(seq 1 120); do ssh "${SSHOPT[@]}" human@127.0.0.1 true 2>/dev/null && break; sleep 2; done
ssh "${SSHOPT[@]}" human@127.0.0.1 'BRAINOS_SOCK=/tmp/ks.sock RUST_LOG=info timeout 4 /opt/brainos/bin/brainos-operator 2>&1 | grep -a "\[key\]\|deliberate\|fast\]" | head -5'
