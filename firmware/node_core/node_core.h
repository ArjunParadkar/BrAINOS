/* node_core.h -- the per-channel motor-node control law and local
 * envelope, as portable C. This is the executable form of the node
 * behaviour specified in blur/sim/nerves.py (class Node) and required by
 * os/blur/src/wire.rs / bus.rs: the joint impedance loop runs ON the node
 * (the 1 kHz loop does not fit across CAN -- see bus.rs), and the
 * physical envelope is enforced on the node so it holds even if the
 * Teensy dies.
 *
 * One node_t per actuator slot. A B-G431B-ESC1 node owns one; an
 * ESP32-S3 node owns several (one per SimpleFOC-Mini). The same code
 * backs both; only the FOC/driver glue around it differs.
 *
 * Law (per loop, matching nerves.py torque()):
 *   tau = kp*(q* - q) - kd*dq + tau_ff          (kp<=kp_max, kd<=0.2*kp_max)
 *   q*  clamped into [qmin,qmax] once committed
 *   setpoint older than SP_WATCHDOG_MS -> pure damping (slow sag)
 *   outside [qmin,qmax] -> a stiff limit spring pushes back
 *   current clamped to cur_limit (ceiling can only narrow after commit)
 *   torque derated by the motor's torque-speed curve
 *
 * I2t winding-temperature estimate and over-temperature derate are added
 * here (nerves.py does not model them); the thermal parameters are
 * placeholders flagged for load-day measurement.
 */
#ifndef BLUR_NODE_CORE_H
#define BLUR_NODE_CORE_H

#include "protocol.h"

#define PROBE_CAP_FRAC   0.9f
#define PROBE_WATCHDOG_MS 50
#define SP_WATCHDOG_MS   60
#define LIMIT_K          40.0f   /* N.m/rad node joint-limit spring */
#define DAMP_KD_PER_TAU  0.24f
#define BIG_LIMIT        1e9f

typedef struct {
    /* immutable motor/driver spec */
    bool is_winch;
    bool has_kt;       /* false for the brushed N20 gripper */
    float kt;          /* N.m/A at the output */
    float peak;        /* driver peak current, A */
    float tau_max;     /* mechanical/design torque ceiling */
    float w0;          /* no-load output speed (rad/s), for the derate */
    float enc_div;     /* output units per encoder turn = 2*pi/enc_div */
    float kp_max, kd_damp;

    /* calibration + envelope (CONFIG) */
    float dir, offset, turn_offset, qmin, qmax, cur_limit;
    bool committed, estopped;
    float ceil_cur, ceil_qmin, ceil_qmax; bool has_ceil;
    float probe_tau; int32_t probe_t_ms;

    /* live setpoint */
    setpoint_t sp; bool has_sp; int32_t sp_t_ms;

    /* runtime */
    float q_prev; bool has_qprev; float dq;
    bool clamped; uint32_t refused;
    float last_current;   /* signed, A */
    float last_tau_cmd;   /* requested torque before caps */

    /* I2t thermal estimate (extension; parameters are load-day placeholders) */
    float i_cont;         /* continuous RMS current rating, A */
    float r_phase;        /* phase resistance, ohm */
    float r_th;           /* winding->ambient thermal resistance, K/W */
    float tau_th_s;       /* thermal time constant, s */
    float ambient_c, winding_c, warn_c;
    bool overtemp;
} node_t;

/* deliverable torque = min(mechanical, electrical peak) */
static inline float node_deliverable(const node_t *n) {
    return n->has_kt ? (n->tau_max < n->kt * n->peak ? n->tau_max : n->kt * n->peak)
                     : n->tau_max;
}

void node_init(node_t *n, bool is_winch, bool has_kt, float kt, float peak,
               float tau_max, float w0, float enc_div,
               float i_cont, float r_phase, float r_th, float tau_th_s,
               float ambient_c, float warn_c);
void node_reset_config(node_t *n);

/* apply one CONFIG write; returns the value the node now holds, NaN if a
 * locked field refused the write (acknowledged as refused). */
float node_apply_config(node_t *n, uint8_t key, float value);

void node_on_setpoint(node_t *n, const setpoint_t *sp, int32_t t_ms);
void node_on_estop(node_t *n);

/* joint angle in output units from the continuous encoder reading */
float node_q(const node_t *n, float enc_cont);

/* run one control step; enc_cont is the encoder's continuous output-unit
 * angle, dt the loop period (s). Returns commanded MOTOR torque (dir
 * applied); updates last_current, clamped, winding_c, overtemp. */
float node_step(node_t *n, float enc_cont, int32_t t_ms, bool rail_on, float dt);

/* build the FEEDBACK payload from the last step */
feedback_t node_feedback(const node_t *n, uint8_t slot, float enc_cont, bool rail_on);

#endif /* BLUR_NODE_CORE_H */
