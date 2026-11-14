# BrAInOS: plan to a finished product

Written 2026-11-13 from `docs/OS_STATUS.md`, `docs/EXECUTION_PLAN.md`,
`docs/NERVES.md` and `docs/motor/MOTOR.md`. This file is the live plan;
the four documents remain the status of record. Status words are the
ones `NERVES.md` uses: Verified, Built, Manual, Not built, Failing.

**What "finished" means here.** Someone who has never seen this
repository can put BrAInOS on a laptop in one command, on a ROS 2 robot
or a servo-bus arm in an afternoon, and on a microcontroller body by
following one document; every claim in the docs is backed by a command
they can run; and there is a tagged v1.0 with a licence.

## Where it stands (2026-11-13)

- `os/`: 21 crates, 1035 tests passing on branch `motor`, `no_std`
  below `host`, builds for both UEFI targets. Seven bounded property
  domains. Never booted on bare metal.
- Three branches stacked and unmerged: `main` ← `nerves-x86-bpu` ←
  `motor`. `main` is the October BLUR state.
- Live nerves: hwmon, CPU/disk/battery, NVML, CAN on `vcan`, ROS 2 read
  against turtlesim. Reading is live; **no part has been commanded on
  real hardware.**
- Motor memory: one-demonstration recipe 99/100 in simulation; four
  gates failing (E1, E3, E6, E7), E2 not run, E5 reduced.
- **No motor has moved. Mini-Blur has never been powered on.**

## Gap list

Each gap names what closes it. "Needs" marks what is not software.

### A. Repository and release

| # | Gap | Closes when |
|---|---|---|
| A1 | Three stacked branches; `main` is two branches behind | `nerves-x86-bpu` then `motor` merged to `main`; one branch to build from |
| A2 | No CI in this repository | A workflow runs `cargo test --workspace --release`, `check-no-std.sh`, the BFS card check and `brainos verify` on every push |
| A3 | No licence file (the plan says Apache 2.0, "chosen once") | `LICENSE` committed. **Needs Arjun's decision** |
| A4 | No versioning, changelog or release artefact | Tagged releases with a static `brainos` binary for x86_64 and aarch64 Linux |
| A5 | Two implementations side by side (`mind/` + `core/` + `body/operator` and `os/`) with no stated relationship for a newcomer | One page saying which is the product, which is the September proof harness, and the migration of the operator's live pieces (voice, pool, skills) onto `os/` |
| A6 | `EXECUTION_PLAN.md` is dated 14 September; its Phase 0 dates (31 Oct, 2 Nov) have passed | Revised to v1.2 against this plan |

### B. Laptop: drop in with one command

| # | Gap | Closes when |
|---|---|---|
| B1 | `brainos` is a set of demonstrations (`demo`, `stack`, `verify`, `parts` …), not a service | `brainos init` mints or adopts a Key and writes a journal; `brainos up` runs the entity as a long-lived process with the parts of this machine acquired; a systemd user unit ships with it |
| B2 | Where the Key lives on an ordinary laptop is not settled in `os/` (secure element in the architecture; a file on a stick today) | Key storage trait with a TPM 2.0 backend and the stick backend; the fallback is stated, not silent |
| B3 | No part has been commanded: the fan write is Manual (root) and there is no CLI that applies a `Mandate` | `brainos part apply` exists; runbook 5.1 run on this laptop and recorded. **Needs** root on the machine |
| B4 | Limits for common hardware must be typed by hand (most chips state none) | A small set of signed envelope packs for common laptops, and the vendor-signed pack wire format |
| B5 | OS-level parts are identified, not attested | Stated in the quickstart as a limit; SPDM stays Not built for v1.0 |
| B6 | NVMe/SMART wear, ROCm | Not in v1.0; listed |
| B7 | The `os/` UEFI build has never been booted | Boots in QEMU on x86_64 and aarch64 and passes a self-test; then on one real machine (Manual) |

### C. Robots: drop in within an afternoon

| # | Gap | Closes when |
|---|---|---|
| C1 | A grant has no wire format, so seat and gate must share a process | A signed, single-use, time-bound grant frame, with the adversary domain extended to it |
| C2 | ROS 2: the graph is read; a granted command is never published | Publish path through `Nerve::apply`; turtlesim driven live through the gate; refusal at validate shown |
| C3 | No servo-bus nerve (Feetech, Dynamixel) | One nerve behind the existing trait, tested against a bus simulator; hardware run is Manual |
| C4 | MHS adapter never compared with the public specification | Compared, differences listed or fixed |
| C5 | Zenoh, micro-ROS, CANopen, EtherCAT, Modbus, MAVLink | Not in v1.0; the integrator guide shows how to add one |
| C6 | An entity does not fit the ESP32-S3 (153 KiB only with the audit window shrunk; leaves must go to flash) | A stated MCU profile: body-only on the MCU with the entity on the seat, leaves to flash. **Needs** the Phase 0 hardware decision |
| C7 | No "write a nerve" or "describe a body" guide | Integrator guide with one worked nerve, tested by building it from the guide alone |

### D. Motor memory

| # | Gap | Closes when |
|---|---|---|
| D1 | `LegGeom::foot` keeps the shin vertical; the bill of materials has a two-servo rigid-shin leg; tip-over margins are for the wrong leg | **Needs Arjun's decision** (below). Then `miniblur-core`, firmware and the seat change together |
| D2 | E1 failing (2 of 8 rows): foot position follows from D1; shoulder torque row tests an upper bound as if it were the moment | D1 fixed; the torque row re-registered as a bound, as an amendment, not an edit |
| D3 | E2 never run at full size | Run on the cluster against the firm-contact model, with a criterion that cannot be met by sinking into the floor (amendment) |
| D4 | E3: push 82, turn 82 (need 85); pour never run | Finger-face geometry for push, wrist-roll reach for turn, fixed on development seeds (1000+) and re-run once on 0–99 |
| D5 | E6: jerk 0.76 against 0.50 | Contact chatter in the wipe addressed, or the row reported as failed for good |
| D6 | E7: slip answered 9/12 | A slip signal a real arm can sense (the simulator's cannot be), threshold from development seeds |
| D7 | E5 not run as registered | Run as registered |
| D8 | LOCI does not exist; a stand-in is used | Stays a stand-in; said so in every doc that mentions conditions |
| D9 | Gaze, legged balance, int8 arm policies | Not in v1.0; listed |

### E. Hardware (none of this is software)

| # | Gap | Needs |
|---|---|---|
| E1h | Mini-Blur never powered on; WP0.7, WP0.8, WP0.10 Manual | Nischal and the bench |
| E2h | No desk arm; the SO-101 runbook has never been started | An arm on the bench |
| E3h | BLUR twin Gate 2 failing (get-up policy 27/30) | Cluster time; no hardware exists |
| E4h | Real CAN bus with BLUR nodes | Flashed nodes |

### F. Proof and safety

| # | Gap | Closes when |
|---|---|---|
| F1 | Seven domains are bounded checking, not proof | Kani harness on the gate's commit path in CI (the plan's step 2). A machine-checked proof stays Stage 6 |
| F2 | No written threat model | One document: what is defended, what is not (OS-level identity, a physics model nobody checked), with the test that backs each line |

### G. Documentation

| # | Gap | Closes when |
|---|---|---|
| G1 | No quickstart | Five minutes from clone to `brainos up` on a laptop, tested on a clean machine image |
| G2 | No single map of the docs | `docs/README.md` index; every doc states its date and what supersedes it |
| G3 | No API reference | `cargo doc` builds warning-free and is published with each release |
| G4 | README leads with October and September notices | Rewritten to say what it is, how to run it, where the status is |
| G5 | BrAInOS-From-Scratch page in progress | BFS cards kept current (`tools/bfs_cards.py` after every Rust edit) |

## Dated plan

Dates are for the software. Hardware rows move when the hardware does.

| By | Milestone | Contents | Exit test |
|---|---|---|---|
| **16 Nov** | M0 hygiene | CI workflow (A2); docs index (G2); this plan; decisions D1 and A3 raised | CI green on `motor` |
| **30 Nov** | M1 one tree | A1 merges; A5 page; A6; D2 torque amendment; D3 submitted to the cluster; C4 | `main` builds and passes 1035+; E2 has a result |
| **21 Dec** | M2 laptop v0.9 | B1, B2, B3 (CLI), B4, B7 in QEMU; G1, G4; A4 first tagged pre-release | A clean Linux image runs the quickstart end to end unattended |
| **18 Jan 2027** | M3 robot v0.9 | C1, C2, C3, C7; D4, D6, D7 re-runs | turtlesim driven through the gate over the wire; a nerve built from the guide alone |
| **8 Feb** | M4 small bodies | C6 MCU profile; firmware against the corrected leg (after D1); bare-metal boot on one real machine | `hexapod_test.py` D1–D5 against the simulator on the corrected geometry |
| **1 Mar** | M5 hardware gates | E1h, E2h runbooks executed where hardware exists; anything not run is recorded as Manual with its date | Recorded runs, or an explicit "not run" |
| **22 Mar** | M6 release candidate | F1, F2; G2, G3, G5; full docs pass by a reader who has not seen the repo | v1.0-rc tagged; docs reviewed |
| **31 Mar 2027** | **v1.0** | Tag, binaries, licence, changelog | Quickstart, robot guide and MCU guide each pass from a clean checkout |

What v1.0 will not claim: a proof of the gate, attestation of on-board
parts, any result on a real motor that has not been recorded, LOCI,
fleet infrastructure.

## Decisions needed from Arjun

1. **`LegGeom::foot` (D1).** The reference keeps the shin vertical; a
   two-servo leg cannot. Options: (a) change `miniblur-core` to
   rigid-shin kinematics, re-derive the tip-over margins and retune the
   gait (firmware and seat change together; the tripod gait's real
   speed is 0.038 m/s, not 0.12, until retuned); (b) change the leg to
   three servos or a parallel linkage so the shin does stay vertical
   (changes the bill of materials: 10 servos is not enough). The
   recommendation is (a): it matches the robot that exists.
2. **Licence (A3).** Apache 2.0 as the Master Document says, or hold.
3. **The Phase 0 MCU (C6).** Keep the ESP32-S3 as a body-only board
   with the entity on the seat (recommended, no purchase), or move the
   entity onto a larger board.

## Log

- 2026-11-13: plan written. CI workflow added (A2; each step passes
  locally, first hosted run pending). `docs/README.md` index added (G2).
  Option (a) of decision 1 was built and measured on a local branch, not
  merged and not published until the decision is made: os suite 1035
  passed, `miniblur-core` 54 passed, firmware builds, the E1 foot row
  passes; the default gait's worst tip-over margin goes from 24.80 mm to
  23.72 mm against a limit of 8 mm.
