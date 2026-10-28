// Teensy 4.1 body controller — the hardware shell of BLUR's Autonomic layer.
//
// 1 kHz master loop: pull the Pi's setpoint stream (50-200 Hz over USB),
// interpolate to 200 Hz, drive the three CAN buses, read FEEDBACK, run the
// safety envelope + reflexes, own the e-stop. The reflex / attitude / session
// logic is specified and tested in the no_std Rust crate os/blur
// (supervisor.rs = reflexes + Mahony attitude + balance, session.rs =
// session envelope); this file is the shell that will link that crate via a
// C ABI. Here the reflex hooks are explicit stubs with their source of truth.
//
// Builds for teensy41 (see platformio.ini). The CAN frame layout is the
// shared protocol.h, so this image and the nodes agree by construction.
#include <Arduino.h>
#include <FlexCAN_T4.h>
extern "C" {
#include "protocol.h"
}

// Teensy 4.1 has three FlexCAN controllers: CAN1/CAN2 (CAN2.0) + CAN3 (FD).
// Bus A = CAN1 (front), B = CAN2 (rear), C = CAN3 (spine).
static FlexCAN_T4<CAN1, RX_SIZE_256, TX_SIZE_64> busA;
static FlexCAN_T4<CAN2, RX_SIZE_256, TX_SIZE_64> busB;
static FlexCAN_T4<CAN3, RX_SIZE_256, TX_SIZE_64> busC;

static const int PIN_ESTOP_LATCH = 24;   // latching switch (GPIO), true = pressed
static const int PIN_RELAY_OPEN  = 25;    // our relay: can only OPEN the motor rail
static const int PIN_ARM_KEY     = 26;    // arming loop key

// Pi-link watchdog, per the brief: no command 100 ms -> hold, 500 ms -> lie+damp.
static elapsedMillis since_pi_cmd;
static elapsedMicros loop_dt;
static uint32_t tick = 0;

// latest Pi target and the previous one, for 50-200 Hz -> 200 Hz interpolation
struct JointCmd { float q, qd, kp, kd, tau_ff; };
static JointCmd pi_now[26], pi_prev[26];
static uint32_t pi_now_ms = 0, pi_prev_ms = 0;

static void send_frame(const can_frame_t *f) {
  CAN_message_t m; m.id = f->id; m.len = f->len; m.flags.extended = 0;
  memcpy(m.buf, f->data, f->len);
  if (f->bus == 0) busA.write(m);
  else if (f->bus == 1) busB.write(m);
  else busC.write(m);
}

// Reflexes (source of truth: os/blur/src/supervisor.rs). On the robot these
// call the linked Rust crate; here they are named stubs so the shell builds.
static void reflex_attitude_update() { /* Mahony on BMI270 @ >=400 Hz */ }
static bool reflex_fall_detected()   { return false; /* tilt + accel */ }
static void reflex_protective_lie()  { /* tuck legs + damp */ }
static void broadcast_estop(uint8_t reason) {
  can_frame_t f = frame_make(0, FC_ESTOP, 0,
      (const uint8_t[]){ESTOP_MAGIC0, ESTOP_MAGIC1, reason, (uint8_t)tick}, 4);
  for (uint8_t b = 0; b < 3; ++b) { f.bus = b; send_frame(&f); }
  digitalWriteFast(PIN_RELAY_OPEN, HIGH);
}

static void read_pi_usb() {
  // Framed 'D' datagram (blur/sim/wire.py down_* mirrors the format). When a
  // full target frame arrives, shift now->prev and timestamp it. Omitted:
  // the USB serial parser; it fills pi_now[] and sets pi_now_ms.
}

// linear interpolation of the Pi targets to this 200 Hz setpoint instant
static JointCmd interp(int j, uint32_t now_ms) {
  JointCmd a = pi_prev[j], b = pi_now[j];
  float span = (float)(pi_now_ms - pi_prev_ms);
  float t = span > 0.5f ? (float)(now_ms - pi_prev_ms) / span : 1.0f;
  if (t < 0) t = 0; if (t > 1) t = 1;
  JointCmd o;
  o.q = a.q + t * (b.q - a.q); o.qd = a.qd + t * (b.qd - a.qd);
  o.kp = b.kp; o.kd = b.kd; o.tau_ff = b.tau_ff;
  return o;
}

void setup() {
  pinMode(PIN_ESTOP_LATCH, INPUT_PULLUP);
  pinMode(PIN_RELAY_OPEN, OUTPUT);
  pinMode(PIN_ARM_KEY, INPUT_PULLUP);
  digitalWriteFast(PIN_RELAY_OPEN, LOW);   // rail closed only while armed + healthy
  busA.begin(); busA.setBaudRate(1000000);
  busB.begin(); busB.setBaudRate(1000000);
  busC.begin(); busC.setBaudRate(1000000);  // classic 1 Mbit (SN65HVD230 cap)
  Serial.begin(2000000);                     // USB to the Pi
}

void loop() {
  if (loop_dt < 1000) return;                // 1 kHz
  loop_dt = 0; tick++;

  read_pi_usb();
  reflex_attitude_update();

  bool estop = digitalReadFast(PIN_ESTOP_LATCH) == LOW;
  if (estop || reflex_fall_detected()) {
    if (estop) broadcast_estop(/*reason*/1);
    else reflex_protective_lie();
  }

  // Pi-link watchdog
  if (since_pi_cmd > 500) { reflex_protective_lie(); }
  else if (since_pi_cmd > 100) { /* hold last safe pose */ }

  // 200 Hz setpoint refresh: every 5th 1 kHz tick
  if ((tick % 5) == 0 && !estop) {
    uint32_t now_ms = millis();
    for (int j = 0; j < 26; ++j) {
      JointCmd c = interp(j, now_ms);
      setpoint_t sp; sp.slot = 0; sp.flags = SP_ENABLE;
      sp.q = c.q; sp.tau_ff = c.tau_ff; sp.kp = c.kp; sp.kd = c.kd;
      // node id + bus + winit per joint come from the descriptor table (TODO);
      // placeholder here keeps the shell building:
      can_frame_t f = setpoint_encode(/*bus*/0, /*node*/(uint8_t)(j + 1), /*winch*/false, &sp);
      send_frame(&f);
    }
  }

  // drain FEEDBACK/RAW and forward a telemetry summary to the Pi (omitted).
}
