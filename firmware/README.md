# BLUR firmware

Firmware for the two motor-node types and the body controller, plus the one
CAN protocol they share. Written so the protocol and the node control law are
**proven on the host today** (no hardware, no cross toolchain), and the
on-target projects link that proven core unchanged.

Nothing here has touched a motor. Host unit tests are not a hardware result.

## Layout

```
firmware/
  protocol.md          the CAN protocol spec (authoritative prose)
  node_core/           portable C: the protocol codec + node control law
    protocol.h         CAN frame codec, scales, structs   (mirror of wire.rs/wire.py)
    node_core.{c,h}    impedance loop + local envelope + I2t (mirror of nerves.py Node)
    test_node_core.c   host unit tests (golden vectors + control law + envelope)
    Makefile           `make test`  -> builds with cc, runs in <0.2 s
  torque/              first-principles per-joint torque limits
    torque_limits.py   KV / 16:1 / eta / driver-current -> continuous & peak
    torque_limits.json generated; merge into BLUR_final/mujoco/blur_meta.json
    TORQUE.md          the numbers, the sources, and the skills ruled out
  esc_g431/            B-G431B-ESC1 node project (PlatformIO)   [config staged]
  esp32s3_node/        ESP32-S3 multi-Mini node project (PlatformIO) [config staged]
  teensy_body/         Teensy 4.1 body controller                 [spec = os/blur]
  miniblur-a/          OLD hexapod ESP32-S3 firmware (Rust) — unrelated to BLUR v42
```

## What builds and runs on this laptop now

```sh
cd firmware/node_core && make test     # 26 checks, 0 failed, <0.2 s
python3 firmware/torque/torque_limits.py
```

`node_core` compiles with `-std=c11 -O2 -Wall -Wextra -Werror -pedantic` and
its two golden frames match `blur/sim/test_wire.py` and `os/blur/tests`
byte-for-byte, so the C node, the Python twin and the Rust Teensy agree on the
wire. The control-law tests cover the impedance law, gain caps, setpoint
clamp, stale-setpoint damping, the joint-limit spring, the current clamp, the
config-lock (narrow-only after Commit), the e-stop latch and the I²t derate.

## On-target node projects (toolchain pinned, not yet compiled here)

This laptop has **no PlatformIO or arduino-cli** (checked 2026-10-28). Both
node projects are provided as pinned `platformio.ini` configs plus a thin
`src/main.cpp` that wraps `node_core`. They are staged, not yet built, because
installing PlatformIO needs network and is a heavy install Arjun should opt
into. Once PlatformIO is in a venv:

```sh
python3 -m venv ~/.pio-venv && ~/.pio-venv/bin/pip install 'platformio==6.1.16'
~/.pio-venv/bin/pio run  -d firmware/esc_g431        # build the G431 node
~/.pio-venv/bin/pio run  -d firmware/esp32s3_node    # build the ESP32-S3 node
```

### B-G431B-ESC1 node (2806.5 legs)
- **Framework:** Arduino-STM32 (`ststm32`) + **SimpleFOC** for the ≥ 20 kHz FOC
  current loop and impedance. SimpleFOC builds on the G431 today; ST MCSDK is
  the fallback if the current loop needs more than SimpleFOC gives.
- **Encoder:** MT6701 on the encoder header, ABZ incremental + one-time index
  alignment at boot (homing routine), joint angle = motor angle / 16 with a
  zero offset stored in flash. (The 2806.5s have a real MT6701; the 4015s do
  not — theirs is on the motor, so they home every boot.)
- **CAN:** FDCAN peripheral in classic 1 Mbit mode.
- **Flash:** the ST-LINK strip snaps off after the first flash. Flash all 12
  boards **before** snapping (`pio run -t upload`), and the firmware carries a
  **CAN/UART bootloader** entry so later updates need no SWD.

### ESP32-S3 node (4015 + 2204 + N20)
- **Framework:** Arduino-ESP32 + SimpleFOC, one ESP32-S3 driving several
  SimpleFOC Minis (DRV8313) and, on #2/#3, the N20 gripper via a DRV8833.
- **CAN:** TWAI peripheral, classic 1 Mbit.
- The same `node_core` runs one instance per slot.

## Teensy 4.1 body controller

The body controller's **logic already exists and is tested** as the `no_std`
Rust crate `os/blur` (Autonomic acquisition, `supervisor.rs` reflexes +
Mahony attitude + balance, `session.rs` envelope, `firmware.rs` attestation),
with 31 crate tests inside the workspace's 793. What does **not** exist is the
Teensy *image* and the Pi↔Teensy USB port (see `docs/BLUR_BRINGUP.md`). The
`teensy_body/` project will host the Teensyduino build that embeds that crate
(via its C ABI) behind FlexCAN3, the BMI270 SPI driver and the e-stop GPIO.
This is the next build after the node projects.
