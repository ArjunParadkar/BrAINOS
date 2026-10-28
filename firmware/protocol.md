# BLUR CAN protocol — one bus protocol for every motor node

This is the human-readable spec. The machine truth is three implementations
held together by shared golden vectors:

| side | file | tested by |
|---|---|---|
| Teensy (Rust, `no_std`) | `os/blur/src/wire.rs`, budget in `os/blur/src/bus.rs` | `os/blur/tests` |
| MuJoCo twin + Pi tools (Python) | `blur/sim/wire.py` | `blur/sim/test_wire.py` |
| motor nodes (C, G431 + ESP32-S3) | `firmware/node_core/protocol.h` | `firmware/node_core/test_node_core.c` |

The two golden frames below are asserted byte-for-byte in all three test
suites. Change the wire only by changing all three and re-running every suite.

## Physical layer

Classic CAN, **1 Mbit/s**, 11-bit identifiers. SN65HVD230 transceivers are
rated to 1 Mbit/s, so CAN-FD's fast data phase is **not** available (the brief
lists FD as "optional on the G431 links"; the transceiver rules it out on the
shared buses). Three buses:

| bus | carries | transceiver label |
|---|---|---|
| A (0) | front: 6 × B-G431B-ESC1 (FR/FL pitch,knee,dist) + FRONT ESP32-S3 (node 17) | can-1 / can-T (chest) |
| B (1) | rear: 6 × B-G431B-ESC1 (RR/RL pitch,knee,dist) + REAR ESP32-S3 (node 19) | can-2 (pelvis) |
| C (2) | spine: MID ESP32-S3 (node 18) alone | can-3 (pelvis) |

## Why the impedance loop is on the node, not the Teensy

Each motor costs two frames per control cycle (a SETPOINT down, a FEEDBACK
up). A worst-case-stuffed 8-byte classic frame is 135 bits, so a 1 Mbit/s bus
carries ≈ 7,407 of them per second. A 1 kHz loop closed *across* CAN for 26
motors is 52,000 frames/s = **8 buses at 100 %**; the Teensy has 3
(`bus.rs::buses_needed_for`). So:

- the joint impedance loop `τ = kp(q*−q) − kd·q̇ + τ_ff` runs **on each node**
  at its own FOC rate (≥ 20 kHz current loop, impedance at 1–5 kHz);
- the Teensy refreshes setpoints at **200 Hz** (`bus.rs::SETPOINT_HZ`), which
  holds every bus under the 70 % planning load (`bus.rs::load_at`, tested). At
  200 Hz the design satisfies the brief's "< 60 %" split-bus rule with margin
  (bus A, the busiest, 6 ESC + a 6-slot node → see the bus-load table below).

Bus load is computed in `bus.rs` and mirrored by `bus_load.py`; the numbers
are in `BRINGUP_LOG.md`.

## Identifier layout

```
id = function << 7 | node          (lower id wins arbitration)
0x000          ESTOP      broadcast, highest priority on every bus
0x080          CENSUS     broadcast request: every node replies IDENTITY
0x100 | node   SETPOINT   q*, τ_ff, kp, kd for one slot
0x180 | node   FEEDBACK   q, q̇, current, status for one slot
0x200 | node   CONFIG     one calibration / envelope value for one slot
0x280 | node   CONFIG_ACK the value the node now holds
0x300 | node   IDENTITY   silicon UID, kind, slot count (three pages)
0x380 | node   RAW        raw encoder counts + turns + temperature
```

`node` is the node id (1–12 for the ESCs, 17/18/19 for the ESP32-S3s), **not**
the joint index; a multi-slot node addresses its channels with the `slot`
byte. The brief's "node id = joint id" holds literally for the single-slot
ESCs and is generalised to (node, slot) for the shared nodes.

## Frames (payloads are 8 bytes, little-endian)

### SETPOINT 0x100|node  (Teensy → node, 200 Hz)
`slot:u8  flags:u8  q:i16  τ_ff:i16  kp:u8  kd:u8`
Fixed-point, per motor kind (`scales()`): non-winch q 2.5e-4 rad/ct, τ 1e-3
N·m/ct, kp 0.25, kd 0.01; winch q 2e-6 m/ct, τ 0.01, kp 20, kd 0.2.
`flags` bit 0 = `SP_ENABLE`. q̇_des is implicitly 0 (the node damps toward
rest between setpoints); τ_ff carries any feed-forward.

**Golden:** `setpoint(bus A, node 0x11, non-winch, slot 2, ENABLE, q −1.0622,
τ_ff 0.25, kp 20, kd 0.5)` → id `0x111`, data
`02 01 67 EF FA 00 50 32`.

### FEEDBACK 0x180|node  (node → Teensy, 200 Hz)
`slot:u8  status:u8  q:i16  q̇:i16  current:i16`  (current 0.01 A/ct, signed
with the torque).
`status` bits: ENABLED, ENC_OK, OVERCURRENT, OVERTEMP, FAULT, ESTOPPED,
CALIBRATED, CLAMPED.

**Golden:** `feedback(bus B, node 0x08, non-winch, slot 0,
ENC_OK|CALIBRATED, q 1.5351, q̇ −3.21, I 12.34)` → id `0x188`, data
`00 42 FC 17 BF FE D2 04`.

### CONFIG 0x200|node / CONFIG_ACK 0x280|node
`slot:u8  key:u8  value:f32  seq:u8  _:u8`. Keys: 1 Offset, 2 Direction,
3 CurrentLimit (A — **the envelope, enforced at the node**), 4 QMin, 5 QMax,
6 TurnOffset, 7 Commit, 8 ProbeTorque. The node replies CONFIG_ACK with the
value it now holds; a refused write (see "envelope" below) is ACKed as NaN.

### RAW 0x380|node (node → Teensy, 10 Hz)
`slot:u8  status:u8  counts:u16  turns:i16  temp:i8  _:u8`. The MT6701's
14-bit count, the node's power-on turn counter, driver temperature — for
calibration and the I²t estimate.

### IDENTITY 0x300|node (node → Teensy, on CENSUS)
Three pages carrying `page:u8 kind:u8 slots:u8 fw:u8 uid[4]`; kind 1 = ESC1,
2 = ESP32-S3; the UID is the STM32G431 96-bit id or the ESP32-S3 MAC padded.

### ESTOP 0x000 (broadcast, highest priority)
`E5 70 reason seq`. **There is no release frame.** A node that sees an ESTOP
stays disabled until power-cycled or re-acquired (CONFIG Commit), so a forged
or replayed frame can only ever stop the robot, never start it. Heartbeat is
implicit: a node whose last SETPOINT is older than `SP_WATCHDOG_MS` (60 ms)
drops to pure damping; the Teensy's own Pi-link watchdog is in section 2 of
the brief.

## The node-local envelope (the safety that survives a dead Teensy)

Enforced inside every node (`node_core.c`, spec `blur/sim/nerves.py::Node`):

- **current clamp** to `CurrentLimit`; before Commit, never above
  0.9 × driver peak (homing the spine needs most of it). Over the clamp sets
  `CLAMPED`/`OVERCURRENT`.
- **gain caps**: kp ≤ `kp_max` (12 × deliverable torque, or 3000 for a
  winch), kd ≤ 0.2 × kp_max.
- **setpoint clamp** into [QMin, QMax] once committed; **joint-limit spring**
  (40 N·m/rad) pushes back beyond the limits whatever the setpoint says.
- **stale setpoint** (> 60 ms) → pure damping (a slow sag, not a drop or a
  held stale command).
- **CAN timeout** (the brief's "> 20 ms → damped stop"): the 60 ms watchdog is
  the hard floor; the Teensy refreshes at 200 Hz (5 ms), so 20 ms = four
  missed frames is the practical alarm and is where the node begins damping on
  the robot. Both are configurable; `SP_WATCHDOG_MS` is the enforced number.
- **ceilings only narrow after Commit**: CurrentLimit can be lowered but not
  raised past the committed ceiling, QMin only raised, QMax only lowered;
  Offset / Direction / TurnOffset cannot change at all. They reopen only while
  the node is uncommitted or e-stopped (when it makes no torque). This is the
  re-acquisition path and is tested in Gate 2's "2b" (rogue Teensy).
- **I²t** winding-temperature estimate; over the warn threshold the current
  caps at the continuous rating (`node_core.c`; thermal parameters are
  load-day placeholders).

## Python codec

`blur/sim/wire.py` is the reference Python codec (encode/decode for every
frame, the BMI270 burst, and the `NerveDown`/`NerveUp` pin-boundary
datagrams). Import it on the Pi and in tools; do not re-implement the packing.
