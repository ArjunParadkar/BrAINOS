/* test_node_core.c -- host unit tests for the node firmware core.
 *
 * Two kinds of check:
 *   1. GOLDEN VECTORS: the CAN codec must produce the exact bytes that
 *      blur/sim/test_wire.py and os/blur/tests assert. If these pass, the
 *      C node, the Python twin and the Rust Teensy agree on the wire.
 *   2. CONTROL LAW + ENVELOPE: the impedance law, gain caps, setpoint
 *      clamp, stale-setpoint damping, joint-limit spring, current clamp,
 *      config-lock (narrow-only), e-stop latch, and the I2t derate behave
 *      as nerves.py specifies.
 *
 * Build + run:  make test   (needs only a C11 compiler; runs in <1 s).
 */
#include "node_core.h"
#include <stdio.h>
#include <math.h>

static int fails = 0, checks = 0;
#define OK(cond, msg) do { checks++; if (!(cond)) { \
    printf("  FAIL: %s  (%s:%d)\n", msg, __FILE__, __LINE__); fails++; } } while (0)

static int bytes_eq(const uint8_t *a, const uint8_t *b, int n) {
    for (int i = 0; i < n; i++) if (a[i] != b[i]) return 0;
    return 1;
}
static void hex(const uint8_t *d, int n) { for (int i = 0; i < n; i++) printf("%02X ", d[i]); }

/* ---------------- 1. golden vectors ---------------- */
static void test_golden(void) {
    printf("golden vectors (shared with test_wire.py / blur_suite.rs):\n");

    setpoint_t sp = { .slot = 2, .flags = SP_ENABLE, .q = -1.0622f,
                      .tau_ff = 0.25f, .kp = 20.0f, .kd = 0.5f };
    can_frame_t f = setpoint_encode(0, 0x11, false, &sp);
    uint8_t want_sp[8] = { 0x02, 0x01, 0x67, 0xEF, 0xFA, 0x00, 0x50, 0x32 };
    OK(f.id == 0x111, "setpoint id == 0x111");
    OK(bytes_eq(f.data, want_sp, 8), "setpoint bytes match golden");
    if (!bytes_eq(f.data, want_sp, 8)) { printf("    got "); hex(f.data, 8); printf("\n"); }

    feedback_t fb = { .slot = 0, .status = ST_ENC_OK | ST_CALIBRATED,
                      .q = 1.5351f, .dq = -3.21f, .current = 12.34f };
    can_frame_t g = feedback_encode(1, 0x08, false, &fb);
    uint8_t want_fb[8] = { 0x00, 0x42, 0xFC, 0x17, 0xBF, 0xFE, 0xD2, 0x04 };
    OK(g.id == 0x188, "feedback id == 0x188");
    OK(bytes_eq(g.data, want_fb, 8), "feedback bytes match golden");
    if (!bytes_eq(g.data, want_fb, 8)) { printf("    got "); hex(g.data, 8); printf("\n"); }

    /* config encode->decode roundtrip through the wire */
    config_t c = { .slot = 0, .key = CFG_CURRENT_LIMIT, .value = 1.75f, .seq = 7, .ack = false };
    can_frame_t h = config_ack(2, 0x12, &c);
    config_t back = {0};
    OK(config_decode(&h, &back), "config decodes");
    OK(back.key == CFG_CURRENT_LIMIT && fabsf(back.value - 1.75f) < 1e-6f && back.seq == 7 && back.ack,
       "config roundtrip value/seq/ack");

    /* e-stop recognition */
    uint8_t em[4] = { ESTOP_MAGIC0, ESTOP_MAGIC1, 1, 0 };
    can_frame_t e = frame_make(0, FC_ESTOP, 0, em, 4);
    OK(frame_is_estop(&e), "e-stop frame recognised");
    OK(!frame_is_estop(&f), "setpoint is not an e-stop");

    /* setpoint decode from a frame drives the node */
    setpoint_t dec = {0};
    OK(setpoint_decode(&f, false, &dec), "setpoint decodes");
    OK(dec.slot == 2 && (dec.flags & SP_ENABLE), "decoded slot/flags");
}

/* a representative 4015 spine/abduction node on a SimpleFOC-Mini (DRV8313) */
static node_t make_4015(void) {
    node_t n;
    /* kt=1.299 N.m/A (eta 0.85), peak 2.5 A, tau_max 3.8, w0 13.09 rad/s,
     * enc_div 16, i_cont 1.75 A, r_phase 5 ohm, r_th 2.5 K/W,
     * tau_th 20 s, ambient 25, warn 100 C (thermal numbers = placeholders) */
    node_init(&n, false, true, 1.299f, 2.5f, 3.8f, 13.09f, 16.0f,
              1.75f, 5.0f, 4.9f, 20.0f, 25.0f, 100.0f);
    return n;
}

/* ---------------- 2. control law + envelope ---------------- */
static void test_impedance_and_current_clamp(void) {
    printf("impedance law + current clamp:\n");
    node_t n = make_4015();
    OK(fabsf(n.kp_max - 12.0f * 3.2475f) < 1e-2f, "kp_max = 12 * deliverable");

    /* fresh enabled setpoint, huge gain, large error -> current clamps */
    setpoint_t sp = { .slot = 0, .flags = SP_ENABLE, .q = 10.0f,
                      .tau_ff = 0.0f, .kp = 1000.0f, .kd = 0.0f };
    node_on_setpoint(&n, &sp, 0);
    float tau = node_step(&n, 0.0f, 0, true, 5e-4f);   /* q=0, q*=10 */
    OK(n.clamped, "over-limit current is flagged clamped");
    /* uncommitted cap = 0.9*peak = 2.25 A -> tau = 2.25*1.299 = 2.923 */
    OK(fabsf(tau - 2.25f * 1.299f) < 2e-2f, "torque clamped to 0.9*peak*kt");

    /* a modest fresh setpoint tracks toward q* (restoring sign) */
    node_t m = make_4015();
    setpoint_t sp2 = { .slot = 0, .flags = SP_ENABLE, .q = 0.05f,
                       .tau_ff = 0.0f, .kp = 20.0f, .kd = 0.0f };
    node_on_setpoint(&m, &sp2, 0);
    float t2 = node_step(&m, 0.0f, 0, true, 5e-4f);
    OK(t2 > 0.0f, "positive error -> positive restoring torque");
}

static void test_watchdog_damping(void) {
    printf("stale-setpoint watchdog -> damping:\n");
    node_t n = make_4015();
    node_apply_config(&n, CFG_COMMIT, 1.0f);            /* committed so damping path is live */
    setpoint_t sp = { .slot = 0, .flags = SP_ENABLE, .q = 0.5f,
                      .tau_ff = 0.0f, .kp = 30.0f, .kd = 0.0f };
    node_on_setpoint(&n, &sp, 0);
    /* still body, setpoint now 100 ms old (> SP_WATCHDOG_MS) */
    float tau = node_step(&n, 0.0f, 100, true, 5e-4f);
    OK(fabsf(tau) < 0.2f, "stale setpoint holds nothing when still (no stiff pull to q*)");
}

static void test_config_lock(void) {
    printf("config lock (narrow-only after commit):\n");
    node_t n = make_4015();
    node_apply_config(&n, CFG_CURRENT_LIMIT, 2.0f);
    node_apply_config(&n, CFG_QMIN, -0.3f);
    node_apply_config(&n, CFG_QMAX, 0.3f);
    node_apply_config(&n, CFG_COMMIT, 1.0f);

    float r1 = node_apply_config(&n, CFG_CURRENT_LIMIT, 2.4f);  /* widen current: refused->ceil */
    OK(fabsf(r1 - 2.0f) < 1e-6f, "current limit cannot widen past the committed ceiling");
    float r2 = node_apply_config(&n, CFG_QMAX, 0.9f);           /* widen qmax: clamped to ceil */
    OK(fabsf(r2 - 0.3f) < 1e-6f, "qmax cannot widen past the committed ceiling");
    float r3 = node_apply_config(&n, CFG_OFFSET, 1.0f);         /* recalibrate: refused */
    OK(isnan(r3), "offset write refused while committed");
    OK(n.refused >= 3, "three refusals counted");

    /* narrowing still works */
    float r4 = node_apply_config(&n, CFG_CURRENT_LIMIT, 1.0f);
    OK(fabsf(r4 - 1.0f) < 1e-6f, "current limit can still narrow");
}

static void test_estop_latch(void) {
    printf("e-stop latch (no release frame; only re-commit):\n");
    node_t n = make_4015();
    node_apply_config(&n, CFG_COMMIT, 1.0f);
    setpoint_t sp = { .slot = 0, .flags = SP_ENABLE, .q = 0.2f,
                      .tau_ff = 0.0f, .kp = 30.0f, .kd = 0.0f };
    node_on_setpoint(&n, &sp, 0);
    node_on_estop(&n);
    float tau = node_step(&n, 0.0f, 0, true, 5e-4f);
    OK(fabsf(tau) < 1e-6f, "e-stopped node makes no torque");
    feedback_t fb = node_feedback(&n, 0, 0.0f, true);
    OK(fb.status & ST_ESTOPPED, "feedback reports ST_ESTOPPED");
    /* re-commit is the only way back */
    node_apply_config(&n, CFG_COMMIT, 1.0f);
    OK(!n.estopped, "re-commit clears the e-stop latch");
}

static void test_limit_spring(void) {
    printf("node joint-limit spring:\n");
    node_t n = make_4015();
    node_apply_config(&n, CFG_QMIN, -0.3f);
    node_apply_config(&n, CFG_QMAX, 0.3f);
    node_apply_config(&n, CFG_COMMIT, 1.0f);
    /* setpoint wants to push past qmax; node must resist */
    setpoint_t sp = { .slot = 0, .flags = SP_ENABLE, .q = 2.0f,
                      .tau_ff = 2.0f, .kp = 30.0f, .kd = 0.0f };
    node_on_setpoint(&n, &sp, 0);
    float tau = node_step(&n, 0.5f, 0, true, 5e-4f);   /* q = 0.5 > qmax 0.3 */
    OK(tau < 0.0f, "beyond qmax the node pushes back (negative torque)");
}

static void test_i2t_derate(void) {
    printf("I2t winding-temperature derate:\n");
    node_t n = make_4015();
    node_apply_config(&n, CFG_COMMIT, 1.0f);
    setpoint_t sp = { .slot = 0, .flags = SP_ENABLE, .q = 10.0f,
                      .tau_ff = 0.0f, .kp = 1000.0f, .kd = 0.0f };
    /* hold a large error so current pins at the cap (2.25 A > i_cont 1.75);
     * refresh the setpoint so it never goes stale, and advance time for the
     * thermal integrator. The winding heats until the estimate trips. */
    int tripped_at = -1;
    for (int k = 0; k < 2000000 && tripped_at < 0; k++) {
        node_on_setpoint(&n, &sp, k / 2);              /* t in ms, 2 kHz loop */
        node_step(&n, 0.0f, k / 2, true, 5e-4f);
        if (n.overtemp) tripped_at = k;
    }
    OK(tripped_at >= 0, "sustained over-continuous current trips the I2t estimate");
    /* with the trip latched, the next step derates current to the continuous rating */
    node_on_setpoint(&n, &sp, (tripped_at + 1) / 2);
    node_step(&n, 0.0f, (tripped_at + 1) / 2, true, 5e-4f);
    OK(fabsf(n.last_current) <= n.i_cont + 1e-3f, "once hot, current derates to the continuous rating");
}

int main(void) {
    test_golden();
    test_impedance_and_current_clamp();
    test_watchdog_damping();
    test_config_lock();
    test_estop_latch();
    test_limit_spring();
    test_i2t_derate();
    printf("\n%d checks, %d failed\n", checks, fails);
    return fails ? 1 : 0;
}
