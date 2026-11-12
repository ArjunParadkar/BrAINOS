//! BFS  LEVEL    Domain 4 substrate - the BPU seam (descriptor, mock, nerve, planner)  [crate brainos-bpu]
//! BFS  PURPOSE  The BPU integer contract: every fixed-point format and operation of a tile read and a write-verify, bit-exact with bpusim/fixed.py
//! BFS  DEFINES  ACT_BITS, W_LEVELS, GMIN_Q, GSTEP, GMAX_Q, ETA_FRAC, ETA_ONE, ETA_MAX, ACC_BITS, ADC_MAX, ADC_MIN, VERIFY_CODE, +27 more
//! BFS  USES     this crate: tables_gen
//! BFS  USED BY  bpu/mock, bpu/prng, bpu/tests/golden, bpu/tests/seam, nostd/lib
//!
//! The integer contract of a BPU tile.
//!
//! This is a line-for-line port of `bpusim/fixed.py`, whose first line
//! is "This module IS the contract": every number an integer, every
//! operation one a digital circuit performs. The KV260 FPGA tile was
//! checked against the same file. `tests/golden.rs` reproduces that
//! reference's outputs bit for bit, including the FPGA gate's own
//! vectors.
//!
//! # The read path
//!
//! ```text
//!   state            w_q  in 0..=255            one device; w = w_q / 255
//!   conductance      G_q  = 655 + 254 * w_q     16 bits; G_min = 1 uS is code 655
//!   read noise       eta_q  Q1.14, |eta_q| <= 16383
//!   noisy            G^_q = (G_q * (2^14 + eta_q) + 2^13) >> 14      17 bits, exact
//!   activation       int8 Q1.7 (one pass) or int16 Q1.15 (two passes)
//!   row drive        one pass:  d = x                      (-128..=127)
//!                    two pass:  d0 = x & 0xFF (0..=255), d1 = x >> 8 (-128..=127)
//!                               so x = 256 * d1 + d0 exactly; 9 signed bits cover both
//!   column current   I+ = sum over rows d * G^+ ,  I- likewise
//!   column sum       Y  = sum over passes (I+ - I-) << (8 * pass)
//!   ADC              y  = saturate_i8( round_half_even( Y / 2^S ) )
//! ```
//!
//! A weight is the differential pair `G+ - G-`.
//!
//! # The write path
//!
//! ```text
//!   device state     w_s  Q1.15 in 0..=32768; the read side sees w_q = min(255, (w_s + 64) >> 7)
//!   one pulse        step = (STEP[w_s >> 5] * gain * (2^14 + c2c) + 2^27) >> 28
//!   write-verify     read 8 times and sum; convert the sum to a state;
//!                    if |target - measured| > max(tol, half a pulse): aim at
//!                    w + 0.7 * error, pulse the nominal count (at least one);
//!                    repeat, at most 100 times.   tol = 0.02 -> 655 in Q1.15
//! ```
//!
//! # Geometry, and one thing the brief's numbers do not cover
//!
//! The reference is a 32 x 32 tile; the paper's tile is 256 x 256. The
//! functions here take the row and column counts as arguments, and the
//! formats are unchanged.
//!
//! The contract's accumulator is 40 bits signed. For the reference's
//! 32 rows that is exact with a wide margin. For 256 rows it depends on
//! the noise:
//!
//! * **with no read noise it still fits**, by about 1 %: the largest
//!   column sum is 64770 * 32767 * 256 = 5.43e11 against 2^39 = 5.50e11;
//! * **with read noise near the top of the dial it does not.** At
//!   sigma = 0.2 a sample can reach +0.658, which takes a full-scale
//!   device to code 108,480, and the same worst-case column reaches
//!   9.1e11.
//!
//! So `tile_read` accumulates in 64 bits, which is exact in every case,
//! and reports in `ReadResult::beyond_40_bits` how many column sums
//! left the 40-bit range. It does not saturate them, because the
//! contract does not say a 256-row accumulator saturates; it was
//! written for 32 rows and is silent. That is a gap to close before
//! 256-row silicon (one more accumulator bit, or a stated saturation),
//! and it is recorded in `docs/NERVES.md`.

use crate::tables_gen::{NDN, NUP, STEP_DN, STEP_UP};
use alloc::vec;
use alloc::vec::Vec;

/// DAC resolution per pass.
pub const ACT_BITS: u32 = 8;
/// A device stores one of 256 levels.
pub const W_LEVELS: i64 = 255;
/// Conductance code of w = 0.
pub const GMIN_Q: i64 = 655;
/// Conductance code per state level.
pub const GSTEP: i64 = 254;
pub const GMAX_Q: i64 = GMIN_Q + GSTEP * W_LEVELS;
pub const ETA_FRAC: u32 = 14;
pub const ETA_ONE: i64 = 1 << ETA_FRAC;
pub const ETA_MAX: i64 = ETA_ONE - 1;
/// The accumulator width the contract states.
pub const ACC_BITS: u32 = 40;
pub const ADC_MAX: i64 = 127;
pub const ADC_MIN: i64 = -128;
/// The activation code a write-verify read drives one row with.
pub const VERIFY_CODE: i64 = 64;

pub const WS_FRAC: u32 = 15;
pub const WS_ONE: i64 = 1 << WS_FRAC;
pub const GAIN_FRAC: u32 = 14;
pub const GAIN_ONE: i64 = 1 << GAIN_FRAC;
pub const GAIN_MIN: i64 = 819;
pub const LUT_BITS: u32 = 10;
/// Write-verify tolerance 0.02, in Q1.15.
pub const TOL_Q: i64 = 655;
/// Write-verify damping 0.7, in Q2.14.
pub const DAMP_Q: i64 = 11469;
pub const VERIFY_READS: i64 = 8;
pub const MAX_ITER: u32 = 100;
pub const SIGMA_FRAC: u32 = 14;

/// Round-half-to-even arithmetic right shift: what the SAR ADC does.
pub fn rne_shift(x: i64, s: u32) -> i64 {
    if s == 0 {
        return x;
    }
    let r = x >> s; // floor
    let rem = x - (r << s); // 0 <= rem < 2^s
    let half = 1i64 << (s - 1);
    let up = rem > half || (rem == half && (r & 1) == 1);
    r + up as i64
}

pub fn conductance_q(w_q: i64) -> i64 {
    GMIN_Q + GSTEP * w_q
}

/// `G^_q = floor((G_q * (2^14 + eta_q) + 2^13) / 2^14)`. Exact, 17 bits.
pub fn noisy_conductance_q(g_q: i64, eta_q: i64) -> i64 {
    (g_q * (ETA_ONE + eta_q) + (1 << (ETA_FRAC - 1))) >> ETA_FRAC
}

/// The row drive for one pass.
pub fn dac_pass_code(x_code: i64, passes: u32, pass: u32) -> i64 {
    if passes == 1 {
        x_code
    } else if pass == 0 {
        x_code & 0xFF
    } else {
        x_code >> 8
    }
}

/// A noise-dial setting as the contract's `sigma_q` (Q2.14), from
/// thousandths: 5 is sigma = 0.005, 200 is 0.2. Integer round half to
/// even, so no floating point is involved.
pub fn sigma_code_from_milli(milli: u32) -> i64 {
    // sigma_q = rne(milli / 1000 * 2^14) = rne(milli * 2048 / 125)
    let num = milli as i64 * 2048;
    let q = num / 125;
    let rem = num % 125;
    // 125 is odd, so a remainder is never exactly half
    q + (2 * rem > 125) as i64
}

/// What one tile read produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadResult {
    /// the ADC codes, one per column
    pub y: Vec<i8>,
    /// the column sums before the ADC
    pub acc: Vec<i64>,
    /// how many columns hit an ADC rail
    pub clipped: u32,
    /// how many column sums fell outside the contract's 40-bit signed
    /// range (always 0 for a 32-row tile; see the module note)
    pub beyond_40_bits: u32,
}

/// One tile read, bit-exact.
///
/// * `w_plus`, `w_minus`: `rows * cols` state codes, row-major
/// * `mask`: `rows * cols` connectivity
/// * `x`: `rows` activation codes (int8 range for one pass, int16 for two)
/// * `eta`: `rows * cols * 2` noise samples, index `(row * cols + col) * 2 + device`
/// * `shift`: the ADC range exponent S
#[allow(clippy::too_many_arguments)]
pub fn tile_read(
    rows: usize,
    cols: usize,
    w_plus: &[u8],
    w_minus: &[u8],
    mask: &[bool],
    x: &[i32],
    eta: &[i16],
    shift: u32,
    passes: u32,
) -> ReadResult {
    debug_assert_eq!(w_plus.len(), rows * cols);
    debug_assert_eq!(w_minus.len(), rows * cols);
    debug_assert_eq!(mask.len(), rows * cols);
    debug_assert_eq!(x.len(), rows);
    debug_assert_eq!(eta.len(), rows * cols * 2);
    let mut acc = vec![0i64; cols];
    for pass in 0..passes {
        for r in 0..rows {
            let d = dac_pass_code(x[r] as i64, passes, pass);
            if d == 0 {
                continue;
            }
            for c in 0..cols {
                let i = r * cols + c;
                if !mask[i] {
                    continue;
                }
                let gp = noisy_conductance_q(conductance_q(w_plus[i] as i64), eta[i * 2] as i64);
                let gm = noisy_conductance_q(conductance_q(w_minus[i] as i64), eta[i * 2 + 1] as i64);
                acc[c] += (d * (gp - gm)) << (ACT_BITS * pass);
            }
        }
    }
    let lim = 1i64 << (ACC_BITS - 1);
    let mut y = Vec::with_capacity(cols);
    let (mut clipped, mut beyond) = (0u32, 0u32);
    for &a in &acc {
        if a >= lim || a < -lim {
            beyond += 1;
        }
        let q = rne_shift(a, shift);
        if !(ADC_MIN..=ADC_MAX).contains(&q) {
            clipped += 1;
        }
        y.push(q.clamp(ADC_MIN, ADC_MAX) as i8);
    }
    ReadResult { y, acc, clipped, beyond_40_bits: beyond }
}

/// The current one device contributes to a write-verify read: its noisy
/// conductance times `VERIFY_CODE`.
pub fn verify_current(w_q: i64, eta_q: i64) -> i64 {
    noisy_conductance_q(conductance_q(w_q), eta_q) * VERIFY_CODE
}

// ---------------------------------------------------------------- write path

fn lut_index(w_s: i64) -> usize {
    ((w_s >> (WS_FRAC - LUT_BITS)) as usize).min((1 << LUT_BITS) - 1)
}

/// The state the read side sees.
pub fn ws_to_wq(w_s: i64) -> i64 {
    ((w_s + 64) >> (WS_FRAC - 8)).min(W_LEVELS)
}

/// The size of one pulse, Q1.15. `direction` > 0 potentiates.
pub fn step_q(w_s: i64, gain_q: i64, c2c_q: i64, direction: i64) -> i64 {
    let idx = lut_index(w_s);
    let base = if direction > 0 { STEP_UP[idx] } else { STEP_DN[idx] } as i64;
    let prod = base * gain_q * (ETA_ONE + c2c_q);
    (prod + (1i64 << (GAIN_FRAC + ETA_FRAC - 1))) >> (GAIN_FRAC + ETA_FRAC)
}

/// Apply signed pulse counts one pulse at a time. Each round, every
/// device that still has pulses to go takes one; `c2c` is asked for one
/// cycle-to-cycle sample per device per round (for all devices, in
/// index order, as the reference draws them).
pub fn pulse(
    w_s: &[i64],
    n: &[i64],
    gain_q: &[i64],
    mut c2c: impl FnMut(usize) -> Vec<i64>,
    dead: &[bool],
) -> Vec<i64> {
    let mut w = w_s.to_vec();
    let mut remaining: Vec<i64> = n
        .iter()
        .zip(dead)
        .map(|(n, d)| if *d { 0 } else { *n })
        .collect();
    while remaining.iter().any(|r| *r != 0) {
        let samples = c2c(w.len());
        for i in 0..w.len() {
            if remaining[i] == 0 {
                continue;
            }
            let dir = remaining[i].signum();
            let s = step_q(w[i], gain_q[i], samples[i], dir);
            w[i] = (w[i] + dir * s).clamp(0, WS_ONE);
            remaining[i] -= dir;
        }
    }
    w
}

/// The nominal signed pulse count from `w_s` to `w_target_s`.
pub fn pulses_for(w_s: i64, w_target_s: i64) -> i64 {
    let (i0, i1) = (lut_index(w_s), lut_index(w_target_s));
    if w_target_s >= w_s {
        rne_shift(NUP[i1] as i64 - NUP[i0] as i64, 8)
    } else {
        -rne_shift(NDN[i1] as i64 - NDN[i0] as i64, 8)
    }
}

/// The state measured from the sum of `VERIFY_READS` verify currents.
pub fn w_meas_from_sum(g_sum: i64) -> i64 {
    let n = VERIFY_READS;
    let den = n * VERIFY_CODE * GSTEP * W_LEVELS;
    let num = ((g_sum - n * VERIFY_CODE * GMIN_Q) * WS_ONE).max(0);
    ((num + den / 2) / den).min(WS_ONE)
}

/// What a write-verify run did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Programmed {
    pub w_s: Vec<i64>,
    /// verify-and-correct rounds that pulsed something
    pub iterations: u32,
    /// write pulses applied: what the endurance budget is charged
    pub pulses: u64,
    /// did the run reach a round in which *no* device needed a pulse?
    ///
    /// This is the contract's stopping rule, and it is stricter than
    /// "the devices are well written". Each round re-measures every
    /// device with fresh read noise, so with noise on, a large array
    /// nearly always has a few devices whose *measurement* falls
    /// outside the band even though their state is fine. Those get one
    /// corrective pulse, and the run goes on to `MAX_ITER`. A run that
    /// did not settle has still written its devices; it has also spent
    /// pulses on false alarms. See `tests/seam.rs` for the measured
    /// cost.
    pub settled: bool,
}

/// Write-verify toward a band.
///
/// `read` returns, for the current states, the sum of `VERIFY_READS`
/// verify currents per device (read noise included). `c2c` is as in
/// `pulse`.
pub fn program_target(
    w_s: &[i64],
    w_target_s: &[i64],
    gain_q: &[i64],
    mut read: impl FnMut(&[i64]) -> Vec<i64>,
    mut c2c: impl FnMut(usize) -> Vec<i64>,
    dead: &[bool],
) -> Programmed {
    let mut w = w_s.to_vec();
    let tgt: Vec<i64> = w_target_s.iter().map(|t| (*t).clamp(0, WS_ONE)).collect();
    let mut total: u64 = 0;
    for it in 1..=MAX_ITER {
        let sums = read(&w);
        let mut n = vec![0i64; w.len()];
        let mut any = false;
        for i in 0..w.len() {
            let err = tgt[i] - w_meas_from_sum(sums[i]);
            let idx = lut_index(w[i]);
            let res = (STEP_UP[idx].max(STEP_DN[idx]) as i64 * gain_q[i]) >> (GAIN_FRAC + 1);
            if err.abs() > TOL_Q.max(res) && !dead[i] {
                any = true;
                let aim = (w[i] + ((DAMP_Q * err) >> GAIN_FRAC)).clamp(0, WS_ONE);
                let mut k = pulses_for(w[i], aim);
                if k == 0 {
                    k = err.signum();
                }
                n[i] = k;
            }
        }
        if !any {
            return Programmed { w_s: w, iterations: it - 1, pulses: total, settled: true };
        }
        total += n.iter().map(|k| k.unsigned_abs()).sum::<u64>();
        w = pulse(&w, &n, gain_q, &mut c2c, dead);
    }
    Programmed { w_s: w, iterations: MAX_ITER, pulses: total, settled: false }
}

/// **A proposal, not the contract.** Write-verify with a per-device
/// stopping rule.
///
/// Identical to `program_target` except for one thing: a device whose
/// measurement lands inside the band is *done*, and is not measured
/// again. `program_target` re-measures every device every round and
/// stops only when a whole round finds nothing to correct, which under
/// read noise does not happen at tile scale (`tests/golden.rs` and
/// `tests/seam.rs` measure it).
///
/// This is here so the fix can be measured against the thing it fixes
/// with the same tables and the same arithmetic. Nothing in the mock
/// uses it, the reference does not define it, and no golden vector
/// covers it. If the contract adopts a per-device rule, this is
/// replaced by a port of whatever the contract says.
pub fn program_target_per_device(
    w_s: &[i64],
    w_target_s: &[i64],
    gain_q: &[i64],
    mut read: impl FnMut(&[i64]) -> Vec<i64>,
    mut c2c: impl FnMut(usize) -> Vec<i64>,
    dead: &[bool],
) -> Programmed {
    let mut w = w_s.to_vec();
    let tgt: Vec<i64> = w_target_s.iter().map(|t| (*t).clamp(0, WS_ONE)).collect();
    let mut done: Vec<bool> = dead.to_vec();
    let mut total: u64 = 0;
    for it in 1..=MAX_ITER {
        let sums = read(&w);
        let mut n = vec![0i64; w.len()];
        let mut any = false;
        for i in 0..w.len() {
            if done[i] {
                continue;
            }
            let err = tgt[i] - w_meas_from_sum(sums[i]);
            let idx = lut_index(w[i]);
            let res = (STEP_UP[idx].max(STEP_DN[idx]) as i64 * gain_q[i]) >> (GAIN_FRAC + 1);
            if err.abs() > TOL_Q.max(res) {
                any = true;
                let aim = (w[i] + ((DAMP_Q * err) >> GAIN_FRAC)).clamp(0, WS_ONE);
                let mut k = pulses_for(w[i], aim);
                if k == 0 {
                    k = err.signum();
                }
                n[i] = k;
            } else {
                done[i] = true;
            }
        }
        if !any {
            return Programmed { w_s: w, iterations: it - 1, pulses: total, settled: true };
        }
        total += n.iter().map(|k| k.unsigned_abs()).sum::<u64>();
        w = pulse(&w, &n, gain_q, &mut c2c, dead);
    }
    Programmed { w_s: w, iterations: MAX_ITER, pulses: total, settled: false }
}
