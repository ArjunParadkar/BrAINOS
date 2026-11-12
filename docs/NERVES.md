# Nerves — every piece of hardware is a body part

**Written 2026-11-10, updated 2026-11-12.** Status vocabulary, used strictly: **Verified** (a
test ran and passed; the command is named), **Built** (compiles, not proven
on the target), **Manual** (needs hands; the runbook is here), **Not
built**, **Failing**.

A nerve is a protocol that connects the entity to a part. A part is any
piece of hardware: a motor, a fan, a GPU, a disk, a battery, a radio. The
entity acquires, feels and — through KIRA — uses all of them through one
model. This document is that model, where each piece of it lives, what has
been shown to work and on what, and the exact steps for everything that
still needs a human.

Nothing here is a parallel system. A part is acquired by the same five §8
steps as any body, gets its id the same way, sits in the same body map and
is gated by the same `Kira`. What was added is listed in "What changed in
existing crates" at the end.

---

## 1. The part model

Code: `os/part` (`no_std`, `forbid(unsafe)`, no new external dependency).

On acquisition a part declares a `PartSchema` (`os/part/src/schema.rs`):

| facet | type | example |
|---|---|---|
| identity | `BodyId = H(attestation key ‖ H(verbs ‖ facets))` | changes if anything below changes |
| class | `PartClass` | `Thermal` |
| substrate | `Option<Substrate>` | `Gpu` — Compute parts only |
| verbs | `CapabilitySchema` (unchanged type) | `fan.set(duty_pct 0..100)` |
| senses | `SenseSpec` — name, quantity, plausible range | `fan.rpm`, rpm, 0..9000 |
| vitals | `VitalSpec` — a sense with a warning and a critical threshold | `temp.c`: degraded at 85, critical at 95 |

Three rules follow from it, each enforced in code:

1. **A changed part is a new part.** Senses, vitals and class are hashed
   into the id (`Acquisition::describe_part`). A part that reconnects
   claiming a higher temperature limit does not match its old id;
   `Entity::still_the_same_part` releases it and it must be acquired again.
2. **No limits, no verbs.** `PartSchema::lowered` drops every verb with an
   unbounded parameter (the existing §8 rule) and every verb on a part that
   declared no vital. Such a part is still acquired and still felt; it is
   never in the body map, so nothing can be authorized through it.
3. **An implausible reading is a broken sensor.** A value outside a sense's
   plausible range is passed on marked stale, and a stale vital is `Failed`.

### The eight classes and the five domains

`os/part/src/class.rs`. Every part passes through Domain 1 (it has a key),
Domain 3 (every verb is gated) and Domain 5 (what it reports can become
memory). What differs is which domain looks after it day to day:

| class | examples | served by | encoded as |
|---|---|---|---|
| `Actuator` | motor, servo, valve | Domain 2 Autonomic | `served_by()`; verbs on consequential regions |
| `Sensor` | camera, IMU, encoder | Domain 2 Autonomic | `may_act() == false`: a verb is a schema error |
| `Thermal` | fan, pump, heater | Domain 2 Autonomic | `is_homeostatic()`: regulated by a `Homeostat` |
| `Power` | battery, rail, charger | Domain 2 Autonomic | `is_homeostatic()` |
| `Compute` | CPU, GPU, NPU, BPU, MCU | Domain 4 Cognition | `is_placement_target()`: becomes a `Resource` on a `Substrate` |
| `Comms` | NIC, radio, bus adapter | Domain 4 Cognition | regions `Network`, `Link` |
| `Storage` | RAM, flash, disk | Domain 5 State | `hosts_state()`: may hold the graph's sealed bytes, never owns it |
| `Composite` | several behind one key | its members | `served_by() == None` |

How each domain's rule is enforced:

* **Domain 1 — Silicon.** Enumeration is `Nerve::enumerate`; attestation is
  the §8 challenge (`Nerve::answer`). *Limit, stated plainly:* parts behind
  the operating system (sysfs, NVML, CAN nodes) hold no key. Their key is
  derived from a stable identifier the kernel provides
  (`nerves::stable_key`), which gives a stable **identity** and is **not
  attestation**. Hardware attestation of on-board parts (for example SPDM
  over PCIe) is **Not built**. ACPI/devicetree/PCIe enumeration in the UEFI
  core is **Not built**; the live nerves run on Linux.
* **Domain 2 — Autonomic.** `os/part/src/homeostat.rs`. A `Homeostat` is
  installed with one `Mandate` (KIRA granted a regulate verb with a `min`
  and `max`) and from then on commands only values inside that range, with
  no model call. It imports nothing from the entity, the state graph or
  cognition. With no trustworthy reading it commands its failsafe.
* **Domain 3 — KIRA.** `Nerve::apply` takes a `Mandate`, whose only
  constructor consumes and verifies a `Granted`. There is no other way to
  reach a part's verb. `Granted` still has exactly one constructor.
* **Domain 4 — Cognition.** `Substrate` (`Mcu | Cpu | Gpu | Npu | Bpu`) is in
  `os/cognition/src/placement.rs`. Acquiring a Compute part with a
  `ComputeOffer` adds a `Resource` on that substrate; `Plan::prefer` states
  which tissue a tier would rather use; `ResourcePool::pick` honours it and
  falls through when that tissue is unavailable.
* **Domain 5 — State.** Vitals reach the state graph through
  `Entity::feel`, written by the entity with its own `MemoryHandle`, the
  part as provenance. No nerve can write memory (section 4).

### The vitals invariant

`os/part/src/policy.rs`. **KIRA refuses any verb whose predicted effect
breaks another part's vital.**

Every bound policy hears every part's readings (`BodyPolicy::feel`, a new
defaulted method). `PartPolicy` keeps the latest value of exactly the
senses it guards. Whoever wires the machine states the cross-part rule as a
`Guard`, because a fan does not know which processor it cools:

```rust
Guard::sustain(fan_set, "duty_pct", gpu, "temp.c", 85.0, 60.0)
// while the GPU reads >= 85 C, fan.set's duty_pct must be >= 60
```

| request | GPU reading | result |
|---|---|---|
| fan to 0 % | 45 °C | granted |
| fan to 0 % | 91 °C | refused at **simulate**, `PredictedInvariantBreak` |
| fan to 100 % | 91 °C | granted |
| fan to 0 % | never heard, stale, too old, or not a number | refused at **simulate**, `Unpredictable` |

"Unknown" is never read as "cool". A `Meter` does the same for a finite
budget (write cycles): a request that would spend below the reserve is
refused at simulate as `IrreversibleBeyondBudget`.

### Interoception

`os/part/src/intero.rs`. Readings do not flood memory. Threshold crossings
and faults are remembered with their value; a window of routine becomes one
summary line per sense; a value hovering at a threshold is called unstable
once and then muted, except that a crossing into critical or failed is
never muted. In the entity suite, 5,000 rounds of steady readings from two
parts write **zero** memories, and closing the window writes five.

### Envelope packs

`os/part/src/pack.rs`. The limits most hardware does not state exist
somewhere: in a datasheet, or in the head of whoever built the machine. An
`EnvelopePack` is how they get in — a small **signed** profile attached to
a part at acquisition:

```text
label      hwmon hp                         # or one exact part, by its key
source     human                            # or: vendor
reference  measured at full speed, 2026-11-12
limit      fan1.rpm  faults  rising  6000 7000
limit      fan2.rpm  faults  rising  6000 7000
```

```sh
cd os && cargo run --release --bin brainos -- parts --pack fans.pack
```

| rule | enforced by |
|---|---|
| accepted only if the entity's Key signed it, or approved a vendor's signature. A vendor's signature alone is not enough: nobody but the owner widens what an entity may do to its own body | `EnvelopePack::verify` |
| an approval names the issuer, so it cannot be moved to another issuer's copy of the same numbers | `approved_by` signs digest ‖ issuer key |
| it can fill silence and tighten; it can never loosen a limit the hardware stated, or flip its direction | `apply` → `WouldLoosen` |
| it cannot invent a sense | `apply` → `NoSuchSense` |
| a pack that is offered and not accepted means the part is **not acquired**, not acquired without limits | `PartError::Pack` |
| the limits are part of the part's identity; who vouched is written into memory when the part joins | facets digest; the join record |

A pack does not make a number true. It makes it attributable: the gate
enforces the limit and the memory says who vouched for it.

**Status.** The type, signing, approval and application: **Verified** (9
unit tests, `nerves_live` fixture and live). The text file is unsigned and
is signed by the session's Key when given to `brainos parts --pack`: that
is the human case. A wire format for a pack that arrives already signed by
a vendor is **Not built**, and so is any distribution of packs.

---

## 2. The Nerve trait

`os/part/src/nerve.rs`, `no_std`.

```text
enumerate   what parts are on this nerve?                  §8 step 1
answer      the part proves it holds its key               §8 step 2
describe    what the part says it is        -> PartSchema  §8 step 3
read        what it feels now               -> Sensed
apply       do this                         <- Mandate     (a verified Granted)
reflex      one step of a regulator         <- ReflexAct   (from a Homeostat)
safe        stop, now
```

---

## 3. Nerve families

| family | nerve | status | evidence or gap |
|---|---|---|---|
| **Platform interfaces** | | | |
| Linux hwmon / thermal zones, read | `host/src/nerves/hwmon.rs` | **Verified** live | `nerves_live`: 10 chips, 10 temperatures read and acquired on this machine |
| Linux fan write (`pwm1_enable`) | same | **Verified** on a fixture file; **Manual** on a real fan | the real fan is acquired *with its verb* once an envelope pack states its limits (`nerves_live`, live); the write itself needs root; runbook 5.1 |
| CPU, disk, battery (`/proc`, `/sys`) | `host/src/nerves/machine.rs` | **Verified** live | `nerves_live`: Ryzen AI 9 365, `nvme0n1`, `BAT0` |
| NVML (NVIDIA) | `host/src/nerves/nvml.rs` | **Verified** live | `nerves_live`: RTX 5070 Laptop GPU; agrees with `nvidia-smi` |
| NVMe / SMART wear | — | **Not built** | needs an admin ioctl (root); runbook 5.5 states the gap |
| ROCm-SMI (AMD) | — | **Not built** | the AMD iGPU's temperature is read through hwmon only |
| ACPI (direct), IPMI / Redfish | — | **Not built** | |
| **Field buses and robot middleware** | | | |
| CAN, BLUR codec over an in-memory bus | `host/src/nerves/socketcan.rs` | **Verified** | `nerves_can`: census, feedback, e-stop, golden vector |
| CAN over a real SocketCAN socket | same + `can_ffi.rs` | **Verified** live on a kernel `vcan0` | `os/tools/can-live.sh`: real `AF_CAN` sockets on a real kernel interface, in a private rootless network namespace. Not a physical bus; a real `can0` with nodes is **Manual**, runbook 5.3 |
| ROS 2, live graph via the `ros2` tool | `host/src/nerves/ros2.rs` | **Verified** live against turtlesim | `os/tools/ros2-live.sh`: a running `turtlesim_node` (ROS 2 Jazzy, in a container), read with the real `ros2` tool, through §8 and the gate |
| ROS 2, publishing a granted command | — | **Not built** | the reader produces the graph and stops |
| ROS 2, a described graph | `os/adapters/src/ros2.rs` | **Verified** (pre-existing) | `brainos stack`, adapters suite |
| Zenoh, micro-ROS, CANopen, EtherCAT, Dynamixel, Modbus, MAVLink | — | **Not built** | |
| **Anthropic MHS** | `os/adapters/src/mhs.rs` | **Verified** against described devices (pre-existing) | the file is the spec of record here. It was not compared with any public MHS document in this work, so any difference is unknown, not absent |
| **On-board buses** | | | |
| PCIe/CXL, USB, I²C/SMBus, SPI, UART, GPIO/PWM, MIPI | — | **Not built** as nerves | the kernel's drivers stand in front of them; hwmon and NVML read through those drivers |
| **Simulated** | `os/part/src/sim.rs` | **Verified** (it is a simulation) | a fan and a GPU with a thermal model; the bench for every test that must be able to stop a fan |
| **BPU** | `os/bpu` | see section 6 | mock only; the silicon does not exist |

"Live" means a real signal during the test run: this machine's hardware,
a kernel CAN interface, a running ROS 2 node. **All five of the reference
nerves the brief names are now Verified on a live signal** — hwmon read,
CPU and storage, NVML, CAN, ROS 2. Two of those are live in a narrower
sense, and it is worth being exact:

* **CAN** is a real SocketCAN socket on a real kernel `vcan` interface. It
  is not a physical bus: no transceiver, no BLUR node.
* **ROS 2** is a real running ROS 2 node read with the real tool. ROS runs
  in a container because it is not installed on the host. The reader reads
  the graph and the gate rules on it; nothing is published back into ROS.

Reproduce:

```sh
cd os
cargo run --release --bin brainos -- parts                       # the live survey
cargo test --release -p brainos-host --test nerves_live -- --nocapture
cargo test --release -p brainos-host --test nerves_can  -- --nocapture
cargo test --release -p brainos-host --test nerves_ros2 -- --nocapture
tools/can-live.sh                                                # CAN on a real kernel vcan0, no root
tools/ros2-live.sh                                               # ROS 2 against turtlesim in a container
cargo test --release -p brainos-host --test autonomic_survives -- --nocapture
cargo test --release -p brainos-entity --test parts_suite
cargo test --release -p brainos-proof --test vitals_domain
```

A live test that finds no hardware prints `SKIPPED (...)` and returns. Run
with `--nocapture` to see which ones actually ran; a skipped test is not
evidence.

### What the live survey found on this machine (2026-11-10)

14 parts, all acquired through §8: eight hwmon temperature chips (NVMe,
both DDR5 SPD sensors, the Wi-Fi radio, the AMD iGPU, `k10temp`, `acpitz`),
two fan chips (`acpi_fan`, `hp`), one thermal zone, the CPU, the NVMe disk,
the battery and the NVIDIA GPU.

Two things it showed that are worth knowing:

* **Most chips state no limit.** Only the NVMe drive, the two memory
  sensors and the GPU (through NVML: slowdown 102 °C, shutdown 105 °C) say
  what temperature is too hot. `k10temp`, `acpitz` and the others report a
  number and no threshold. The nerves do not invent one, so those are
  senses and not vitals.
* **The fans state no limit either, so they have no verb.** Rule 2 applied
  to real hardware: sysfs gives fan speed and a mode switch and no fault or
  maximum, so the entity is given nothing to command. The verb becomes
  usable when the fan is acquired with an *envelope pack* (section 1) that
  states its limits and is signed by the entity's Key. With one, this
  laptop's `hp` fan chip is acquired as a commandable part with the verb
  `fan.full` (`nerves_live`, live; nothing is commanded).

---

## 4. Guarantees, and how each is checked

| guarantee | check | status |
|---|---|---|
| nothing reaches a part's verb without a `Granted` | compile-fail doctests in `part/src/nerve.rs`: a `Mandate` cannot be written, a `Granted` cannot be written, a token cannot be used twice | **Verified** `cargo test -p brainos-part --doc` (each confirmed to fail for the intended reason: E0451, private fields, E0382) |
| a forged command is refused at authenticate; a grant cannot be reused for another command, under another key, or late | `parts_suite` | **Verified** |
| a nerve cannot write memory | the types: a graph write needs a `MemoryHandle`; a handle is issued only to the holder of the entity's `BrainKey` (`MemoryHandle::issue(&key)`); the graph refuses, on every write, a handle for any entity but its owner. Compile-fail doctests + `memory_suite` (a handle minted from another key cannot add, link, revise, forget, decay or load). `nostd/tests/discipline.rs` remains as a second line | **Verified**. Until 2026-11-12 `issue` took a public entity id and the graph did not check whose handle it was given; both are fixed. |
| fan-to-0 over the limit is refused; the autonomic loop keeps cooling | `parts_suite`, `vitals_domain`, `autonomic_survives` | **Verified** on the simulated bench |
| the loop survives cognition being killed | `autonomic_survives`: a separate cognition process is killed with SIGKILL; 600 further loop steps at full load | **Verified** on the simulated bench. The killed process is the *model*; the gate is on the body side. `parts_suite` separately drops the entire entity. |
| a part that declares no limits gets no verbs | `parts_suite`, `nerves_live` (fixture and the real fans) | **Verified** |
| a part that changes its schema mid-session is re-acquired as a new part | `parts_suite` | **Verified** |
| the policy agrees with an independent oracle on all 504 combinations, and a policy with no guard is caught | `vitals_domain`; `brainos verify` (seventh domain) | **Verified**. Bounded checking, not a proof. |

None of the simulated-bench results is a hardware result. No real fan has
been commanded by this code.

---

## 5. Runbooks for every Manual item

### 5.1 Fan write on real hardware — **Manual** (root)

Reading needs nothing. Writing `pwm1_enable` needs root, and the meaning
of its values is per driver. For `hp-wmi` (this laptop): `0` = full speed,
`2` = automatic. **Check your driver's documentation before using this on
another machine**; on `thinkpad_acpi` fan control must first be enabled
with the module option `fan_control=1` and the interface is
`/proc/acpi/ibm/fan`, which this nerve does not drive.

```sh
# 1. confirm the file and its current value (2 = automatic on hp-wmi)
cat /sys/class/hwmon/hwmon*/pwm1_enable
# 2. confirm you can hear the difference by hand, and put it back
echo 0 | sudo tee /sys/class/hwmon/hwmonN/pwm1_enable    # fans to full
echo 2 | sudo tee /sys/class/hwmon/hwmonN/pwm1_enable    # back to automatic
# 3. state the fan's limits in an envelope pack (sysfs states none) and run
#    the gated path as root. `brainos parts --pack FILE` acquires the fan
#    with its verb; there is no CLI yet that goes on to *apply* a Mandate.
#    For that, write a small harness that acquires the fan with the pack
#    and a Guard against the part it cools, and applies the Mandate.
#    host/tests/nerves_live.rs::fixture_the_fan_write_goes_through_kira_...
#    is that harness against a temporary directory; point it at /sys.
```

Leaving the fans in automatic mode is always safe: the firmware's own
control stays in charge.

### 5.2 CAN on a virtual interface — no longer Manual

```sh
cd os && tools/can-live.sh
# expect: "LIVE socketcan: node N found on vcan0, feedback [...]"
```

The script runs the test inside `unshare -Urn`, a private user and network
namespace, where creating `vcan0` needs no root and leaves no interface on
the host. To have `vcan0` on the host itself instead:
`sudo modprobe vcan && sudo ip link add dev vcan0 type vcan && sudo ip link set up vcan0`.

### 5.3 CAN on the real robot — **Manual** (hardware)

Needs a CAN adapter and BLUR nodes with flashed firmware (none exist yet:
see `BRINGUP_LOG.md`). Bring the interface up at the bus rate in
`firmware/protocol.md`, 1 Mbit/s:

```sh
sudo ip link set can0 up type can bitrate 1000000
```

Then `SocketCanBus::open("can0", Bus::A)`. The nerve only *feels* nodes and
can e-stop them. It does not move joints: that goes through the session
envelope in `os/blur`. **`safe()` sends ESTOP, which latches every node
until power-cycle** — do not call it on a robot you want to keep running.

### 5.4 ROS 2, a real running graph — no longer Manual

```sh
cd os && tools/ros2-live.sh
# expect: "LIVE ros2: 3 topics (1 commands: ["/turtle1/cmd_vel"]), 7 services"
#         "LIVE ros2: as running, section 8 refuses it (NothingToDo)"
#         "LIVE ros2: with limits, `tool.cmd_vel` ... -> granted; ... -> Some((Validate, OutOfBounds))"
```

The script starts a ROS 2 Jazzy container (it pulls `ros:jazzy-ros-base`,
about 1.3 GB, the first time), runs `turtlesim_node` headless, and points
the reader at it through `tools/ros2-in-docker/ros2`. With ROS 2 installed
on the host, no container is needed: `BRAINOS_NET=on cargo test --release
-p brainos-host --test nerves_ros2 -- --nocapture`.

Without limits the graph is read and §8 refuses to incorporate it. Limits
are a file of `topic field min max` lines (`nerves::ros2::parse_limits`).
`BRAINOS_NET=on` is required because ROS 2 discovery opens network
sockets. Still **Not built**: publishing a granted command into ROS, and
Zenoh.

### 5.5 Disk wear — **Not built**

NVMe "percentage used" and SATA SMART attributes need an admin ioctl and
root. What is read without privilege: capacity, bytes written since boot,
the drive's temperature and its own temperature limits. To see wear by
hand: `sudo nvme smart-log /dev/nvme0n1` or `sudo smartctl -a /dev/nvme0n1`.

---

## 6. The BPU seam

Code: `os/bpu` (`no_std`, `forbid(unsafe)`, no new external dependency) and
`os/cognition/src/bpu.rs`. **The silicon does not exist.** This is a seam
and a faithful mock, and each piece says which it is.

| piece | where | status |
|---|---|---|
| `Substrate::Bpu`, `BpuDescriptor` (tiles, tiers, levels, ADC bits, contract version, dial range, endurance, dead tolerance) | `os/cognition` | **Verified** (unit tests) |
| the integer contract: tile read, noise generator, write path, write-verify | `os/bpu/src/{contract,prng}.rs` | **Verified** bit-exact against `bpusim/fixed.py` at commit `0af2b7aebd8c` |
| `BpuDevice`: the trait a real driver implements; every write takes a `Mandate` | `os/bpu/src/device.rs` | **Built** (one implementation: the mock) |
| `MockBpu` | `os/bpu/src/mock.rs` | **Verified** as a mock. Its arithmetic is the contract; wear, dead devices, drift and heat are set by the test, not modelled |
| `BpuNerve`: a BPU as a Compute part with five verbs | `os/bpu/src/nerve.rs` | **Verified** against the mock |
| model → tiles planner | `os/bpu/src/planner.rs` | **Verified** as arithmetic. **It is a planner, not a compiler.** |
| a driver for a real BPU over PCIe/CXL | — | **Not built**; nothing to drive |
| `sample(k)` experimental read | — | **Not built** |

### The contract is matched bit for bit

`os/bpu/tools/gen_golden.py` reads the reference (it never writes to that
repository) and produces the lookup tables and `tests/golden/`. Every
comparison in `tests/golden.rs` is integer equality:

* 69 tile reads — the FPGA gate's own generator, seeds 0–47, plus every
  contract corner case: 42 one-pass, 27 two-pass, five noise levels, masked
  tiles, ADC rails;
* the noise generator's stream for five seeds and dial settings;
* each write-path function on 400-device arrays;
* five whole write-verify runs, state for state, with the iteration and
  pulse counts.

```sh
cd os && cargo test --release -p brainos-bpu           # 29 tests
python3 os/bpu/tools/gen_golden.py                     # regenerate from the reference
```

### What KIRA refuses (Gate C)

| request | refused at | code |
|---|---|---|
| a load signed by anyone but the entity | authenticate | `BadSignature` |
| a load whose pulse estimate would go below the reserve | simulate | `IrreversibleBeyondBudget` |
| a noise dial outside 0.005–0.2 | validate | `OutOfBounds` |
| a load onto tiles past the dead-device tolerance, stated honestly | validate | `OutOfBounds` |
| the same, with the dead fraction understated | policy | `BodyInvariant` (the part is not fit) |

There is no way to write a weight without a token: every `BpuDevice`
method that writes takes a `&Mandate`. And the device checks for itself
whatever the gate concluded — it refuses a load onto tiles that really
exceed the tolerance, stops a write that really overruns the budget, and
refuses an image other than the one the grant names
(`tests/seam.rs::the_device_refuses_a_bad_load_even_if_the_gate_was_told_a_comfortable_story`).

### Placement

A healthy BPU is preferred for the tier that prefers it; when its vitals
go critical its resource is withdrawn and work falls to the GPU, then the
CPU. The state graph is intact throughout and the entity still exports and
verifies (`tests/seam.rs::placement_prefers_a_healthy_bpu_...`).

### Three things this work found, none of them in the brief

1. **Write-verify does not settle at tile scale under read noise.** The
   contract stops when a round finds *no* device outside the band. Every
   round re-measures with fresh noise, so in an array of 2,048 devices a
   few measurements fall outside each round and the run goes to the
   100-round limit. The devices are well written either way (worst true
   error 0.024 in both cases); the noisy run spends pulses on false
   alarms. Measured on a 32 × 32 tile: 69,177 pulses and settled at dial
   0.005; 73,269 pulses and not settled at the contract's 0.02 — about 6 %
   more wear. `WriteReport::settled` reports this honestly rather than
   calling it convergence. *A per-device stopping rule would remove it.*
2. **The 40-bit column sum is exact for 32 rows and tight for 256.** With
   no read noise a 256-row tile still fits, by about 1 %. With noise near
   the top of the dial (σ = 0.2) the worst case does not. The mock
   accumulates in 64 bits and counts columns that left the 40-bit range
   (`ReadResult::beyond_40_bits`); it does not saturate them, because the
   contract — written for 32 rows — does not say what a 256-row
   accumulator does. *Needs one more bit, or a stated saturation, before
   256-row silicon.*
3. **No re-verify threshold or temperature limit is published.** The
   paper gives the write band (0.02) and says retention drifts toward
   G_min. The nerve's drift thresholds (degraded at 0.05, critical at
   0.10) are its own assumption, derived and labelled in
   `os/bpu/src/nerve.rs`; temperature is a sense with no vital.

### What is deliberately absent

Nothing here assumes device noise improves results. The dial is exposed
because the hardware has one. Nothing schedules it or tunes it.

The energy figures (4 fJ per multiply-accumulate, 2.56 pJ per conversion,
100 pJ per write pulse) are **projections** from the paper. They leave out
the controller, the row drivers and everything off-tile. They are for
comparing placements and should not be quoted as measurements. The
endurance figures in a `BpuDescriptor` are parameters: on-chip endurance
is unproven and no value is published.

---

## 7. What changed in existing crates

| crate | change | effect on existing behaviour |
|---|---|---|
| `body` | `vital.rs` (`Felt`, `Health`, `Band`); `BodyPolicy::feel` with a default that ignores it; `Acquisition::describe_part` | none: `describe` produces the same ids as before; existing policies are unchanged |
| `cognition` | `bpu.rs` (`BpuDescriptor`, projected energy constants); `Substrate`; `Resource::substrate` (defaults from the site), `on`, `costing`; `Plan::prefer`; `ResourcePool::pick`, `set_available` | none: a plan with no preference picks exactly as `cheapest_for` did |
| `entity` | `parts`, `intero` fields; `acquire_part`, `feel`, `summarise_parts`, `still_the_same_part`, `release_part` | none for entities with no parts |
| `proof` | seventh domain `vitals`; `Report::full` includes it | `brainos verify` prints one more section |
| `host` | `nerves/`, `brainos parts [--pack FILE]`, features `nvml` and `socketcan` | none |
| `memory` | `MemoryHandle::issue` takes the `BrainKey`; `StateGraph` is bound to its owner and refuses other entities' handles | a handle for the wrong entity, which used to be accepted on writes, is now refused |
| `nostd` | canary touches `part` and `bpu`; three new discipline tests | — |
| new: `part`, `bpu` | the part model and nerve trait; the BPU seam | — |

`unsafe` was added in exactly two files, both in `host`, both named for it:
`nerves/nvml_ffi.rs` (dlopen of the vendor library) and
`nerves/can_ffi.rs` (the `AF_CAN` socket). Everything below `host` still
forbids it.
