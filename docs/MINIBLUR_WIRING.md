# Mini-Blur — wiring and bring-up (v0.0, skeleton)

**Owner:** Nischal (WP0.7 / WP0.8). **Standard:** a stranger rebuilds a second Mini-Blur from this document alone. Fill every `[ ]`; replace every *hypothesis* with a measurement. The entity side (firmware protocol, seat policy, simulator) is built against the numbers in `miniblur/core/src/schema.rs` — when a number here differs, change the code, not the robot.

## 1. Bill of materials (Technical Execution Report §3.10)

| # | part | qty | role | source / cost |
|---|---|---|---|---|
| 1 | ESP32-S3 dev board (A) | 1 | Domain 2: servos, gait, audio | [ ] |
| 2 | ESP32-S3 dev board (B), camera variant | 1 | vision capture | [ ] |
| 3 | MG90S metal-gear micro servo | 10 | legs | [ ] |
| 4 | 2S LiPo 7.4 V | 1 | servo rail | [ ] mAh |
| 5 | UBEC 5 V | 1 | servo rail regulator | [ ] A continuous — **3 A is tight**: the default gait peaks at 2.69 A with 7 servos moving (`miniblur-sim bench`); 5 A recommended |
| 6 | INMP441 I²S mic | 1 | ears | [ ] |
| 7 | MAX98357A I²S amp + speaker | 1 | voice | [ ] |
| 8 | 3D-printed frame, 6 legs | 1 | body | [ ] STL files → `hardware/` |
| 9 | wire, headers, 1000 µF cap on the servo rail | — | — | [ ] |

## 2. Servo channels — the joint layout

*Hypothesis in the code (`ServoTable::bom_10_servo`)*: every leg has its own lift servo (6), the front and rear swing of each side are linked by a bar (2), the middle legs swing on their own (2). Linked pairs {L1,L3} and {R1,R3} are each inside one tripod, which is what makes 10 servos enough for a tripod gait.

| ch | joint | GPIO (board A) | MCPWM | measured center ° | min ° | max ° | dir |
|---|---|---|---|---|---|---|---|
| 0 | L1 lift | 4 | 0 / op0 A | [ ] | [ ] | [ ] | [ ] |
| 1 | R1 lift | 5 | 0 / op0 B | [ ] | [ ] | [ ] | [ ] |
| 2 | L2 lift | 6 | 0 / op1 A | [ ] | [ ] | [ ] | [ ] |
| 3 | R2 lift | 7 | 0 / op1 B | [ ] | [ ] | [ ] | [ ] |
| 4 | L3 lift | 15 | 0 / op2 A | [ ] | [ ] | [ ] | [ ] |
| 5 | R3 lift | 16 | 0 / op2 B | [ ] | [ ] | [ ] | [ ] |
| 6 | L1+L3 swing (linked) | 17 | 1 / op0 A | [ ] | [ ] | [ ] | [ ] |
| 7 | R1+R3 swing (linked) | 18 | 1 / op0 B | [ ] | [ ] | [ ] | [ ] |
| 8 | L2 swing | 8 | 1 / op1 A | [ ] | [ ] | [ ] | [ ] |
| 9 | R2 swing | 3 | 1 / op1 B | [ ] | [ ] | [ ] | [ ] |

If the real layout is different (12 servos, or 4 legs × 2 + 2), change `bom_10_servo()` and the pin map in `firmware/miniblur-a/src/main.rs`; the gait, the policy and the simulator take the table from there.

## 3. Leg geometry (mm)

| coxa | femur | tibia | body width | body length | CoM offset (x aft −, y left +) |
|---|---|---|---|---|---|
| [ ] (code: 12) | [ ] (30) | [ ] (45) | [ ] (70) | [ ] (110) | [ ] (−5, 0) |

## 4. Power

- Servo rail: LiPo → UBEC → servo V+ rail, **isolated from the ESP32 3V3/5V**. Common ground only. [ ] verified with a meter before the first servo is plugged.
- [ ] 1000 µF across the servo rail at the servo end.
- [ ] Battery divider to an ADC pin for `vbat` — until wired, the board reports `vbat n/a` and the policy cannot refuse on battery.

## 5. Bring-up procedure (each step is a MANUAL check in the plan)

1. `./build.sh --esp32` → `firmware/miniblur-a/target/xtensa-esp32s3-none-elf/release/miniblur-a`. Flash: `espflash flash --monitor <elf>`.
2. Board alone, no servos: the monitor shows `[miniblur-a] paired with 55ca4abd..` and a hello line every 2 s.
3. Seat on the laptop: `python3 miniblur/tools/serial-reach-bridge.py --port /dev/ttyACM0 --seat 127.0.0.1:7777 --echo`. The face shows body `miniblur` joined (class legs). `where are you` lists it.
4. `on miniblur, begin calibration` → one servo at a time: `on miniblur, servo 0 to 90` … record with `on miniblur, record servo 0 center 91 min 25 max 155 dir 1`. `on miniblur, end calibration`. Restart the operator and the board: the calibration comes back from the state graph (H5 in `hexapod_test.py`).
5. Six-leg sweep: [ ] video.
6. `on miniblur, stand up` → six feet down. `on miniblur, walk forward 1 steps` at speed 0.3.
7. Pull the USB cable mid-walk → the board stops and stands within 500 ms on its own (`[miniblur-a] reflex: link lost`). [ ]
8. Power-cut the body mid-walk → the seat's journal is intact (it never lived on the body). [ ]
9. Run `python3 body/operator/tests/hexapod_test.py` with the bridge instead of the sim: same checks, real board.

## 6. Open questions

- [ ] the true joint layout of ten servos
- [ ] MCPWM direct vs a PCA9685 (the `ServoBus` trait is the seam)
- [ ] Wi-Fi (esp-radio) vs the serial bridge for the 31 Oct demo
- [ ] camera board B in the demo or not
