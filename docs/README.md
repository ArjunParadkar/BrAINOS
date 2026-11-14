# BrAInOS documentation: where to look

Written 2026-11-13. One line per document: what it is, when it was last
true, and what to read instead if it has been overtaken. Status words
across the docs are Verified, Built, Manual, Not built, Failing; the
September documents use VERIFIED / MANUAL / BUILT / NOT BUILT with the
same meanings.

## Start here

| If you want | Read |
|---|---|
| What exists and what it has shown, with every command | [`OS_STATUS.md`](OS_STATUS.md) — start at "If you read one section" |
| What is left and by when | [`../PLAN.md`](../PLAN.md) |
| To run something now | [`OS_STATUS.md` → Run it](OS_STATUS.md#run-it), [`../os/README.md`](../os/README.md) |
| Where a piece of code lives | [`CODE_MAP.md`](CODE_MAP.md) (generated; every Rust file's five-line card) |

## Current status documents

| Document | Covers | Last updated | Notes |
|---|---|---|---|
| [`OS_STATUS.md`](OS_STATUS.md) | the `os/` workspace: crates, guarantees, the seven property domains, measurements, bugs found, what it does not show | 2026-11-13 | the status of record |
| [`NERVES.md`](NERVES.md) | every piece of hardware as a body part: the part model, the `Nerve` trait, nerve families and their status, runbooks for every Manual item, the BPU seam | 2026-11-12 | |
| [`motor/MOTOR.md`](motor/MOTOR.md) | motor memory (Workstream E): gate table E1–E9, the mug task, negative results, the first-real-arm runbook | 2026-11-13 | simulation only; four gates Failing |
| [`motor/PREREGISTRATION.md`](motor/PREREGISTRATION.md) | thresholds for E1–E9, fixed before any gate ran | 2026-11-12 | never edited; amendments are dated |
| [`BLUR_BRINGUP.md`](BLUR_BRINGUP.md) | BrAInOS on the BLUR quadruped's MuJoCo twin, six gated phases | 2026-10-23 | Gate 1 Verified, Gate 2 Failing |
| [`BPU_PAPER_CORRECTIONS.md`](BPU_PAPER_CORRECTIONS.md) | two corrections to the BPU paper found by building `os/bpu` | 2026-11-12 | |
| [`CODE_MAP.md`](CODE_MAP.md) | every Rust file, by level | generated | run `tools/bfs_cards.py`; do not edit |

## Plans and architecture

| Document | Covers | Last updated | Notes |
|---|---|---|---|
| [`../PLAN.md`](../PLAN.md) | gap list and dated plan to v1.0 | 2026-11-13 | the live plan |
| [`EXECUTION_PLAN.md`](EXECUTION_PLAN.md) | every component, the stage map, Phase 0 work packages, the hexapod wire protocol and policy | 2026-09-14 (status column 2026-11-10) | its Phase 0 dates (31 Oct, 2 Nov) have passed; for dates use `PLAN.md`. Still the reference for the stage numbering and the Phase 0 design |
| `BrAInOS_Architecture_v1.3.pdf` | the architecture | 2026-07-10 | canonical. v1.2 is kept for history |

## Runbooks (things that need hands)

| Document | For | State |
|---|---|---|
| [`BARE_METAL.md`](BARE_METAL.md) | booting the BrAIn Key on a real machine | prepared, gated in a VM, not executed on metal |
| [`MINIBLUR_WIRING.md`](MINIBLUR_WIRING.md) | wiring and bring-up of the Mini-Blur hexapod | skeleton v0.0; the robot has never been powered on |
| [`NERVES.md` §5](NERVES.md#5-runbooks-for-every-manual-item) | fan write on real hardware, real CAN bus, ROS 2, disk wear | per item |
| [`motor/MOTOR.md` → Runbook](motor/MOTOR.md#runbook-first-test-on-a-real-arm-lerobot-so-101-or-similar) | first test on a real desk arm | not started |
| [`../BRINGUP_LOG.md`](../BRINGUP_LOG.md), [`../firmware/README.md`](../firmware/README.md), [`../firmware/protocol.md`](../firmware/protocol.md) | BLUR node firmware and CAN protocol | no node flashed |

## The September stack (still accurate for what it describes)

| Document | Covers |
|---|---|
| [`STAGE3_OPERATOR.md`](STAGE3_OPERATOR.md) | the operator body: a chainloaded Linux body, three cognition tiers, voice and face (August 2026) |
| [`STAGE4_ENTITY.md`](STAGE4_ENTITY.md) | one Key across several bodies through Reach, signed skills, the model pool (August 2026) |

These describe `mind/`, `core/` and `body/operator`. The `os/` workspace
is a second implementation of the same architecture, built beside them;
`os/bodyd` links `miniblur/core`, so both agree about the hexapod by
construction. Which pieces of the September stack move onto `os/` is
item A5 of the plan.

## Evidence

- [`artifacts/`](artifacts/): the transcript of each `brainos` subcommand named in `OS_STATUS.md`.
- [`motor/artifacts/`](motor/artifacts/): the output of each motor gate.

## Open decision

The hexapod's leg kinematics (`MOTOR.md`, finding 1): the two options and
the measurements for the first are in [`../PLAN.md`](../PLAN.md),
"Decisions needed" and the log.
