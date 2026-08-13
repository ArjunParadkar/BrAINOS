#!/usr/bin/env bash
# One-command launch: Body B, booting off the shared BrAIn Key image.
# Same entity as Body A (run_vm_a.sh) -- same identity/memory on the key,
# different machine, different ears/voice. See tools/run_instance.sh.
exec "$(dirname "$0")/tools/run_instance.sh" B "$@"
