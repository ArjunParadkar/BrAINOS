/* protocol.h -- the one CAN protocol for every BLUR motor node.
 *
 * This is the C mirror of os/blur/src/wire.rs and blur/sim/wire.py. All
 * three are held together by the golden vectors in test_node_core.c,
 * which assert the same bytes as os/blur/tests and blur/sim/test_wire.py.
 *
 * It is pure, freestanding C11 (no libc beyond <stdint.h>/<string.h>), so
 * it compiles unchanged on the STM32G431 (B-G431B-ESC1 nodes), the
 * ESP32-S3 (SimpleFOC-Mini nodes) and on a host for unit tests.
 *
 * CAN identifier layout (11-bit classic, 1 Mbit/s):
 *   id = function << 7 | node        (lower id wins arbitration)
 *   0x000          ESTOP      broadcast, highest priority on every bus
 *   0x080          CENSUS     broadcast: every node replies IDENTITY
 *   0x100 | node   SETPOINT   q*, tau_ff, kp, kd for one slot
 *   0x180 | node   FEEDBACK   q, dq, current, status for one slot
 *   0x200 | node   CONFIG     one calibration / envelope value
 *   0x280 | node   CONFIG_ACK the value the node now holds
 *   0x300 | node   IDENTITY   silicon UID, kind, slot count (3 pages)
 *   0x380 | node   RAW        raw encoder counts + turns + temperature
 */
#ifndef BLUR_PROTOCOL_H
#define BLUR_PROTOCOL_H

#include <stdint.h>
#include <stdbool.h>
#include <string.h>

/* function codes */
enum {
    FC_ESTOP = 0, FC_CENSUS = 1, FC_SETPOINT = 2, FC_FEEDBACK = 3,
    FC_CONFIG = 4, FC_CONFIG_ACK = 5, FC_IDENTITY = 6, FC_RAW = 7
};

/* SETPOINT flags */
#define SP_ENABLE (1u << 0)

/* FEEDBACK / RAW status bits */
#define ST_ENABLED    (1u << 0)
#define ST_ENC_OK     (1u << 1)
#define ST_OVERCURRENT (1u << 2)
#define ST_OVERTEMP   (1u << 3)
#define ST_FAULT      (1u << 4)
#define ST_ESTOPPED   (1u << 5)
#define ST_CALIBRATED (1u << 6)
#define ST_CLAMPED    (1u << 7)

/* CONFIG keys */
enum {
    CFG_OFFSET = 1, CFG_DIRECTION = 2, CFG_CURRENT_LIMIT = 3,
    CFG_QMIN = 4, CFG_QMAX = 5, CFG_TURN_OFFSET = 6,
    CFG_COMMIT = 7, CFG_PROBE_TORQUE = 8
};

/* node kinds */
#define KIND_ESC1    1u
#define KIND_ESP32S3 2u

/* the e-stop frame's magic (there is no release frame) */
#define ESTOP_MAGIC0 0xE5
#define ESTOP_MAGIC1 0x70

#define AMPS_PER_COUNT 0.01f

/* One classic CAN frame. bus in {0,1,2} = {A,B,C}. */
typedef struct {
    uint8_t bus;
    uint16_t id;     /* 11-bit */
    uint8_t len;
    uint8_t data[8];
} can_frame_t;

/* fixed-point scales, per motor kind. only winch vs. everything else differ. */
typedef struct { float q, tau, dq, kp, kd; } scales_t;

static inline scales_t blur_scales(bool is_winch) {
    scales_t s;
    if (is_winch) { s.q = 2e-6f; s.tau = 0.01f; s.dq = 1e-4f; s.kp = 20.0f; s.kd = 0.2f; }
    else          { s.q = 2.5e-4f; s.tau = 1e-3f; s.dq = 1e-2f; s.kp = 0.25f; s.kd = 0.01f; }
    return s;
}

/* round half away from zero into i16, saturating -- identical to wire.rs/py */
static inline int16_t blur_q16(float v, float scale) {
    float c = v / scale;
    if (c != c) return 0;                 /* NaN */
    if (c >= 32767.0f) return 32767;
    if (c <= -32767.0f) return -32767;
    return (int16_t)(c >= 0.0f ? (c + 0.5f) : (c - 0.5f));
}

/* clamp to [0,255], truncating after +0.5 -- identical to wire.rs/py */
static inline uint8_t blur_q8(float v, float scale) {
    float c = v / scale;
    if (c != c || c <= 0.0f) return 0;
    if (c >= 255.0f) return 255;
    return (uint8_t)(c + 0.5f);
}

static inline uint16_t frame_function(const can_frame_t *f) { return (uint16_t)(f->id >> 7); }
static inline uint8_t frame_node(const can_frame_t *f) { return (uint8_t)(f->id & 0x7F); }

static inline can_frame_t frame_make(uint8_t bus, uint16_t fc, uint8_t node,
                                     const uint8_t *data, uint8_t n) {
    can_frame_t f;
    f.bus = bus;
    f.id = (uint16_t)((fc << 7) | (node & 0x7F));
    if (n > 8) n = 8;
    f.len = n;
    memset(f.data, 0, 8);
    memcpy(f.data, data, n);
    return f;
}

/* ---- decoded payloads ---- */
typedef struct { uint8_t slot, flags; float q, tau_ff, kp, kd; } setpoint_t;
typedef struct { uint8_t slot, status; float q, dq, current; } feedback_t;
typedef struct { uint8_t slot, key, seq; float value; bool ack; } config_t;

static inline can_frame_t setpoint_encode(uint8_t bus, uint8_t node, bool is_winch,
                                          const setpoint_t *sp) {
    scales_t s = blur_scales(is_winch);
    int16_t q = blur_q16(sp->q, s.q), t = blur_q16(sp->tau_ff, s.tau);
    uint8_t d[8] = { sp->slot, sp->flags,
                     (uint8_t)(q & 0xFF), (uint8_t)((q >> 8) & 0xFF),
                     (uint8_t)(t & 0xFF), (uint8_t)((t >> 8) & 0xFF),
                     blur_q8(sp->kp, s.kp), blur_q8(sp->kd, s.kd) };
    return frame_make(bus, FC_SETPOINT, node, d, 8);
}

static inline bool setpoint_decode(const can_frame_t *f, bool is_winch, setpoint_t *out) {
    if (frame_function(f) != FC_SETPOINT || f->len != 8) return false;
    scales_t s = blur_scales(is_winch);
    const uint8_t *d = f->data;
    out->slot = d[0]; out->flags = d[1];
    out->q = (int16_t)(d[2] | (d[3] << 8)) * s.q;
    out->tau_ff = (int16_t)(d[4] | (d[5] << 8)) * s.tau;
    out->kp = d[6] * s.kp; out->kd = d[7] * s.kd;
    return true;
}

static inline can_frame_t feedback_encode(uint8_t bus, uint8_t node, bool is_winch,
                                          const feedback_t *fb) {
    scales_t s = blur_scales(is_winch);
    int16_t q = blur_q16(fb->q, s.q), v = blur_q16(fb->dq, s.dq),
            i = blur_q16(fb->current, AMPS_PER_COUNT);
    uint8_t d[8] = { fb->slot, fb->status,
                     (uint8_t)(q & 0xFF), (uint8_t)((q >> 8) & 0xFF),
                     (uint8_t)(v & 0xFF), (uint8_t)((v >> 8) & 0xFF),
                     (uint8_t)(i & 0xFF), (uint8_t)((i >> 8) & 0xFF) };
    return frame_make(bus, FC_FEEDBACK, node, d, 8);
}

static inline bool config_decode(const can_frame_t *f, config_t *out) {
    uint16_t fc = frame_function(f);
    if ((fc != FC_CONFIG && fc != FC_CONFIG_ACK) || f->len != 8) return false;
    const uint8_t *d = f->data;
    out->slot = d[0]; out->key = d[1];
    uint32_t bits = (uint32_t)d[2] | ((uint32_t)d[3] << 8) |
                    ((uint32_t)d[4] << 16) | ((uint32_t)d[5] << 24);
    memcpy(&out->value, &bits, 4);
    out->seq = d[6];
    out->ack = (fc == FC_CONFIG_ACK);
    return true;
}

static inline can_frame_t config_ack(uint8_t bus, uint8_t node, const config_t *c) {
    uint32_t bits; float v = c->value; memcpy(&bits, &v, 4);
    uint8_t d[8] = { c->slot, c->key,
                     (uint8_t)(bits & 0xFF), (uint8_t)((bits >> 8) & 0xFF),
                     (uint8_t)((bits >> 16) & 0xFF), (uint8_t)((bits >> 24) & 0xFF),
                     c->seq, 0 };
    return frame_make(bus, FC_CONFIG_ACK, node, d, 8);
}

static inline bool frame_is_estop(const can_frame_t *f) {
    return f->id == 0 && f->len >= 2 &&
           f->data[0] == ESTOP_MAGIC0 && f->data[1] == ESTOP_MAGIC1;
}

#endif /* BLUR_PROTOCOL_H */
