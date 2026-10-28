// ESP32-S3 node — on-target wrapper around the proven node_core. Staged
// (SimpleFOC + Arduino-ESP32 not installed on the dev laptop). One node_t
// per driven slot; TWAI carries the shared CAN protocol. The control law
// and codec are host-tested in firmware/node_core/test_node_core.c.
#include <Arduino.h>
#include <SimpleFOC.h>
#include "driver/twai.h"
extern "C" {
#include "node_core.h"
#include "protocol.h"
}

#ifndef BLUR_NODE_ID
#define BLUR_NODE_ID 17
#endif

// slot count per node: 17 FRONT=6, 18 MID/spine=3, 19 REAR=5
static const int N = (BLUR_NODE_ID == 18) ? 3 : (BLUR_NODE_ID == 19) ? 5 : 6;
static node_t node[6];

void setup() {
  for (int s = 0; s < N; ++s) {
    // 4015 on a SimpleFOC Mini (DRV8313): kt_out 1.299, peak 2.5, tau_max
    // 3.8, w0 13.09, enc_div 16 (built-in MT6701 is motor-side -> home every
    // boot). The 2204 neck/jaw and the N20 use their own init values.
    node_init(&node[s], /*winch*/false, /*has_kt*/true, 1.299f, 2.5f,
              3.8f, 13.09f, 16.0f,
              /*i_cont*/1.75f, /*r_phase*/5.0f, /*r_th*/4.9f, /*tau_th*/20.0f,
              25.0f, 100.0f);
    // driver[s].init(); motor[s].initFOC();  // one SimpleFOC Mini per slot
  }
  // twai_init(1'000'000);  // classic 1 Mbit
}

void loop() {
  const float dt = 5e-4f;
  int32_t t_ms = (int32_t)millis();
  bool rail_on = true;
  for (int s = 0; s < N; ++s) {
    // float enc = sensor[s].getAngle() / 16.0f;
    // ... poll TWAI for SETPOINT/CONFIG/ESTOP/CENSUS addressed to (NODE_ID,s) ...
    // float tau = node_step(&node[s], enc, t_ms, rail_on, dt);
    // motor[s].move(tau);
    (void)t_ms; (void)rail_on; (void)dt; (void)s;
  }
}
