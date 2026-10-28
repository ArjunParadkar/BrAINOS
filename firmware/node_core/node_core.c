/* node_core.c -- see node_core.h. Faithful to blur/sim/nerves.py Node. */
#include "node_core.h"
#include <math.h>

static const float TWO_PI = 6.283185307179586f;

void node_reset_config(node_t *n) {
    n->dir = 1.0f;
    n->offset = 0.0f;
    n->turn_offset = 0.0f;
    n->qmin = -BIG_LIMIT;
    n->qmax = BIG_LIMIT;
    n->cur_limit = PROBE_CAP_FRAC * n->peak;
    n->committed = false;
    n->estopped = false;
    n->has_ceil = false;
    n->probe_tau = 0.0f;
    n->probe_t_ms = -1000000000;
    n->has_sp = false;
    n->sp_t_ms = -1000000000;
}

void node_init(node_t *n, bool is_winch, bool has_kt, float kt, float peak,
               float tau_max, float w0, float enc_div,
               float i_cont, float r_phase, float r_th, float tau_th_s,
               float ambient_c, float warn_c) {
    n->is_winch = is_winch;
    n->has_kt = has_kt;
    n->kt = kt;
    n->peak = peak;
    n->tau_max = tau_max;
    n->w0 = w0;
    n->enc_div = enc_div;
    float deliv = node_deliverable(n);
    n->kp_max = is_winch ? 3000.0f : 12.0f * deliv;      /* supervisor.rs Envelope::physical */
    n->kd_damp = is_winch ? 5.0f : DAMP_KD_PER_TAU * deliv;
    n->q_prev = 0.0f; n->has_qprev = false; n->dq = 0.0f;
    n->clamped = false; n->refused = 0;
    n->last_current = 0.0f; n->last_tau_cmd = 0.0f;
    n->i_cont = i_cont; n->r_phase = r_phase; n->r_th = r_th;
    n->tau_th_s = tau_th_s; n->ambient_c = ambient_c;
    n->winding_c = ambient_c; n->warn_c = warn_c; n->overtemp = false;
    node_reset_config(n);
}

float node_apply_config(node_t *n, uint8_t key, float value) {
    /* committed and live: the physical envelope is the node's own. Limits
     * may only narrow, calibration may not change; both reopen only while
     * uncommitted or e-stopped. Mirrors nerves.py Node.config(). */
    bool locked = n->committed && !n->estopped && n->has_ceil;
    if (locked && (key == CFG_OFFSET || key == CFG_DIRECTION || key == CFG_TURN_OFFSET)) {
        n->refused++;
        return NAN;                       /* acknowledged as refused */
    }
    switch (key) {
    case CFG_OFFSET: n->offset = value; break;
    case CFG_DIRECTION: n->dir = (value >= 0.0f) ? 1.0f : -1.0f; break;
    case CFG_CURRENT_LIMIT: {
        float v = value < 0.0f ? 0.0f : (value > n->peak ? n->peak : value);
        if (locked && v > n->ceil_cur) { n->refused++; v = n->ceil_cur; }
        n->cur_limit = v;
        return v;
    }
    case CFG_QMIN:
        if (locked && value < n->ceil_qmin) { n->refused++; value = n->ceil_qmin; }
        n->qmin = value;
        return value;
    case CFG_QMAX:
        if (locked && value > n->ceil_qmax) { n->refused++; value = n->ceil_qmax; }
        n->qmax = value;
        return value;
    case CFG_TURN_OFFSET: n->turn_offset = roundf(value); break;
    case CFG_COMMIT:
        n->committed = (value >= 0.5f);
        if (n->committed) {
            n->estopped = false;          /* re-acquisition is the only way back from e-stop */
            n->ceil_cur = n->cur_limit; n->ceil_qmin = n->qmin; n->ceil_qmax = n->qmax;
            n->has_ceil = true;
        } else {
            n->has_ceil = false;
        }
        break;
    case CFG_PROBE_TORQUE: n->probe_tau = value; break;
    default: break;
    }
    return value;
}

void node_on_setpoint(node_t *n, const setpoint_t *sp, int32_t t_ms) {
    n->sp = *sp; n->has_sp = true; n->sp_t_ms = t_ms;
}

void node_on_estop(node_t *n) { n->estopped = true; }

float node_q(const node_t *n, float enc_cont) {
    return n->dir * enc_cont + n->offset + n->turn_offset * (TWO_PI / n->enc_div);
}

static float clampf(float x, float lo, float hi) {
    return x < lo ? lo : (x > hi ? hi : x);
}

float node_step(node_t *n, float enc_cont, int32_t t_ms, bool rail_on, float dt) {
    float qn = node_q(n, enc_cont);
    if (!n->has_qprev) { n->q_prev = qn; n->has_qprev = true; }
    n->dq += 0.25f * ((qn - n->q_prev) / dt - n->dq);   /* same filter as nerves */
    n->q_prev = qn;

    float tau = 0.0f;
    bool enabled = n->has_sp && (n->sp.flags & SP_ENABLE);
    if (rail_on && !n->estopped) {
        if ((t_ms - n->probe_t_ms) <= PROBE_WATCHDOG_MS && n->probe_tau != 0.0f) {
            tau = n->probe_tau;
        } else if (enabled && (t_ms - n->sp_t_ms) <= SP_WATCHDOG_MS) {
            float kp = clampf(n->sp.kp, 0.0f, n->kp_max);
            float kd = clampf(n->sp.kd, 0.0f, 0.2f * n->kp_max);
            float q = n->committed ? clampf(n->sp.q, n->qmin, n->qmax) : n->sp.q;
            tau = kp * (q - qn) - kd * n->dq + n->sp.tau_ff;
        } else if (n->committed && enabled) {
            tau = -n->kd_damp * n->dq;    /* Teensy went quiet: a slow sag */
        }
    }
    /* the node's own joint-limit guard, whatever the setpoint says */
    if (rail_on && !n->estopped && n->committed && enabled) {
        if (qn < n->qmin) {
            float push = LIMIT_K * (n->qmin - qn) - 0.1f * LIMIT_K * (n->dq < 0.0f ? n->dq : 0.0f) * 0.02f;
            if (push > tau) tau = push;
        } else if (qn > n->qmax) {
            float push = -LIMIT_K * (qn - n->qmax) - 0.1f * LIMIT_K * (n->dq > 0.0f ? n->dq : 0.0f) * 0.02f;
            if (push < tau) tau = push;
        }
    }
    n->last_tau_cmd = tau;

    /* current cap (uncommitted: never above the probe cap) */
    float cap = n->committed ? n->cur_limit
                             : (n->cur_limit < PROBE_CAP_FRAC * n->peak ? n->cur_limit
                                                                        : PROBE_CAP_FRAC * n->peak);
    /* I2t over-temperature derate: once hot, cap at the continuous rating */
    if (n->overtemp && n->has_kt) {
        float tcap = n->i_cont;
        if (tcap < cap) cap = tcap;
    }

    float i;
    if (n->has_kt) {
        i = tau / n->kt;
        n->clamped = fabsf(i) > cap;
        i = clampf(i, -cap, cap);
        tau = i * n->kt;
    } else {
        i = tau / (n->tau_max > 1e-9f ? n->tau_max : 1e-9f) * n->peak;
        n->clamped = false;
    }

    /* torque-speed derate (descriptor's flat-to-w0/2, linear-to-0-at-w0) */
    float w = fabsf(n->dq);
    float avail;
    if (w <= 0.5f * n->w0) avail = n->tau_max;
    else if (w >= n->w0) avail = 0.0f;
    else avail = n->tau_max * (n->w0 - w) / (0.5f * n->w0);
    tau = clampf(tau, -avail, avail);

    /* I2t winding-temperature estimate: first-order, driven by I^2*R */
    float p = i * i * n->r_phase;
    float t_inf = n->ambient_c + p * n->r_th;
    n->winding_c += (dt / n->tau_th_s) * (t_inf - n->winding_c);
    n->overtemp = n->winding_c > n->warn_c;

    n->last_current = i;
    return n->dir * tau;                  /* motor-side command */
}

feedback_t node_feedback(const node_t *n, uint8_t slot, float enc_cont, bool rail_on) {
    feedback_t fb;
    fb.slot = slot;
    uint8_t s = 0;
    bool enabled = n->has_sp && (n->sp.flags & SP_ENABLE);
    if (enabled && rail_on && !n->estopped) s |= ST_ENABLED;
    s |= ST_ENC_OK;
    if (n->clamped) s |= ST_OVERCURRENT;
    if (n->overtemp) s |= ST_OVERTEMP;
    if (n->estopped) s |= ST_ESTOPPED;
    if (n->committed) s |= ST_CALIBRATED;
    fb.status = s;
    fb.q = node_q(n, enc_cont);
    fb.dq = n->dq;
    fb.current = n->last_current;
    return fb;
}
