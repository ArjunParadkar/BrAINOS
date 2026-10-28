// B-G431B-ESC1 node — on-target wrapper around the proven node_core.
// Staged: this compiles against SimpleFOC + the Arduino-STM32 HAL, which
// are NOT installed on the dev laptop (see firmware/README.md). The control
// law, the envelope and the CAN codec it calls are host-tested in
// firmware/node_core/test_node_core.c.
#include <Arduino.h>
#include <SimpleFOC.h>
extern "C" {
#include "node_core.h"
#include "protocol.h"
}

// --- hardware (B-G431B-ESC1 fixed pinout) ---
static BLDCMotor motor(7);                       // 2806.5 pole pairs TBD on bench
static BLDCDriver6PWM driver(A_PHASE_UH, A_PHASE_UL, A_PHASE_VH, A_PHASE_VL,
                             A_PHASE_WH, A_PHASE_WL);
// MT6701 over the encoder header as ABZ incremental (index homing at boot).
static Encoder encoder(A_HALL1, A_HALL2, 1024, A_HALL3);

static node_t node;
static uint8_t NODE_ID = 1;                      // set per board at flash time

void setup() {
  // FOC current loop >= 20 kHz; impedance handled by node_core each loop.
  driver.voltage_power_supply = 20.0f;           // V_SAG
  driver.init(); motor.linkDriver(&driver);
  encoder.init(); motor.linkSensor(&encoder);
  motor.torque_controller = TorqueControlType::foc_current;
  motor.controller = MotionControlType::torque; // node_core sets the torque
  motor.init(); motor.initFOC();

  // 2806.5 on the ESC1: kt_out 0.0999 N.m/A (eta 0.85), tau_max 4.0,
  // w0 170 rad/s, enc_div 16. Thermal params are load-day placeholders.
  node_init(&node, /*winch*/false, /*has_kt*/true, 0.0999f, 40.0f,
            4.0f, 170.17f, 16.0f,
            /*i_cont*/6.0f, /*r_phase*/0.1f, /*r_th*/3.0f, /*tau_th*/15.0f,
            25.0f, 90.0f);

  // can_init(1'000'000);  // FDCAN classic 1 Mbit — HAL glue on bench
}

void loop() {
  motor.loopFOC();                               // >= 20 kHz current loop

  // 1-5 kHz impedance tick:
  const float dt = 5e-4f;
  int32_t t_ms = (int32_t)millis();
  float enc = encoder.getAngle() / 16.0f;        // joint angle = motor / 16
  bool rail_on = true;                           // from the e-stop GPIO on the robot

  // ... poll CAN: SETPOINT -> node_on_setpoint, CONFIG -> node_apply_config,
  //     ESTOP -> node_on_estop, CENSUS -> reply IDENTITY ...

  float tau = node_step(&node, enc, t_ms, rail_on, dt);
  motor.move(tau);                               // commanded motor torque

  // every 5 ms: feedback_encode(...) -> CAN; every 100 ms: RAW.
}
