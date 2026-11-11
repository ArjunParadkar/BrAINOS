//! BFS  LEVEL    Domain 2 Autonomic - the BLUR quadruped body (wire, acquisition, supervisor)  [crate brainos-blur]
//! BFS  PURPOSE  Least-squares fit of joint motion under a known torque pulse, separating the motor's effect from gravity and springs to find which way the motor turns it
//! BFS  DEFINES  DOUBLET, SLOT_MS, DOUBLET_MS, profile, phi, MAX_SAMPLES, Fit, FitResult, combine
//! BFS  USES     nothing outside core/alloc
//! BFS  USED BY  blur/acquire, blur/autonomic, blur/calibrate, blur/tests/blur_suite
//!
//! The probe's arithmetic: which way did the joint go *because of the
//! motor*?
//!
//! A hanging leg is a coupled pendulum with springs in it. Under a torque
//! pulse it moves because of the pulse, because of gravity, because of
//! the springs, and because it was still swinging from the last thing
//! that happened to it. Only the first is the motor's doing.
//!
//! So the probe drives a known torque doublet τ(t) and fits
//!
//! ```text
//!   q(t) = q0 + v0·t + ½·c·t² + b·Φ(t)        Φ = ∬ τ dt dt
//! ```
//!
//! over the feedback samples. `q0, v0` absorb where the joint was and
//! where it was going; `c` absorbs any roughly constant load (gravity, a
//! spring near its set point); `b` is 1/inertia as the motor sees it, and
//! its *sign* is the answer. The profile is out-and-brake (+, +, −, −):
//! zero net impulse, so the joint stops near where it went, and its Φ is
//! not collinear with 1, t, t², so the fit can tell them apart.
//!
//! It is run twice, with +τ and with −τ, because a hanging joint usually
//! rests against one of its stops and a push into a stop is no
//! information. The verdict is taken from the direction it was free to
//! move in (the larger |t|).

/// The torque profile, as a fraction of the probe torque, per 10 ms slot.
pub const DOUBLET: [f32; 8] = [1.0, 1.0, -1.0, -1.0, 0.0, 0.0, 0.0, 0.0];
pub const SLOT_MS: u32 = 10;
pub const DOUBLET_MS: u32 = SLOT_MS * DOUBLET.len() as u32;

/// τ(t)/τ at `t_ms` into the doublet.
pub fn profile(t_ms: u32) -> f32 {
    let k = (t_ms / SLOT_MS) as usize;
    if k < DOUBLET.len() {
        DOUBLET[k]
    } else {
        0.0
    }
}

/// Φ(t)/τ, the double integral of the profile, in s².
pub fn phi(t_s: f32) -> f32 {
    // integrate piecewise-constant slots exactly
    let slot = SLOT_MS as f32 * 0.001;
    let mut v = 0.0f32; // ∫τ
    let mut x = 0.0f32; // ∬τ
    let mut t = 0.0f32;
    for &u in DOUBLET.iter() {
        let dt = if t_s - t < slot { (t_s - t).max(0.0) } else { slot };
        x += v * dt + 0.5 * u * dt * dt;
        v += u * dt;
        t += slot;
        if t >= t_s {
            return x;
        }
    }
    // after the doublet: coasting with whatever ∫τ is left (zero by design)
    x + v * (t_s - t)
}

pub const MAX_SAMPLES: usize = 64;

/// Collected (t, q) samples and the fit.
#[derive(Clone, Copy, Debug)]
pub struct Fit {
    n: usize,
    t: [f32; MAX_SAMPLES],
    q: [f32; MAX_SAMPLES],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FitResult {
    /// the motor term's coefficient (rad per N·m·s²); its sign is the direction
    pub b: f32,
    /// b's standard error
    pub se: f32,
    /// b × the probe torque × max|Φ|: how far the motor moved it, rad
    pub motion: f32,
    pub n: usize,
}

impl FitResult {
    pub fn t_stat(&self) -> f32 {
        if self.se > 0.0 {
            self.b / self.se
        } else if self.b == 0.0 {
            0.0
        } else {
            f32::INFINITY * self.b.signum()
        }
    }
}

impl Default for Fit {
    fn default() -> Self {
        Fit::new()
    }
}

impl Fit {
    pub const fn new() -> Fit {
        Fit {
            n: 0,
            t: [0.0; MAX_SAMPLES],
            q: [0.0; MAX_SAMPLES],
        }
    }

    pub fn push(&mut self, t_s: f32, q: f32) {
        if self.n < MAX_SAMPLES && q.is_finite() {
            self.t[self.n] = t_s;
            self.q[self.n] = q;
            self.n += 1;
        }
    }

    pub fn len(&self) -> usize {
        self.n
    }

    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    /// Least squares on [1, t, t², Φ(t)·τ]. `None` with too few samples or
    /// a singular design.
    pub fn solve(&self, tau: f32) -> Option<FitResult> {
        const P: usize = 4;
        if self.n < P + 3 {
            return None;
        }
        let row = |i: usize| -> [f32; P] {
            let t = self.t[i];
            [1.0, t, 0.5 * t * t, phi(t) * tau]
        };
        let mut a = [[0.0f64; P]; P];
        let mut y = [0.0f64; P];
        for i in 0..self.n {
            let r = row(i);
            for u in 0..P {
                y[u] += r[u] as f64 * self.q[i] as f64;
                for v in 0..P {
                    a[u][v] += r[u] as f64 * r[v] as f64;
                }
            }
        }
        let inv = invert4(a)?;
        let mut beta = [0.0f64; P];
        for u in 0..P {
            for v in 0..P {
                beta[u] += inv[u][v] * y[v];
            }
        }
        let mut rss = 0.0f64;
        for i in 0..self.n {
            let r = row(i);
            let mut pred = 0.0f64;
            for u in 0..P {
                pred += beta[u] * r[u] as f64;
            }
            let e = self.q[i] as f64 - pred;
            rss += e * e;
        }
        let dof = (self.n - P) as f64;
        let s2 = rss / dof;
        let var_b = s2 * inv[3][3];
        let se = if var_b > 0.0 { libm::sqrt(var_b) } else { 0.0 };
        let mut phimax = 0.0f32;
        for i in 0..self.n {
            let p = phi(self.t[i]).abs();
            if p > phimax {
                phimax = p;
            }
        }
        // a floor on the standard error: an encoder count is the smallest
        // motion that can be told from none, so a perfect fit to a joint
        // that did not move must not read as infinitely significant
        let b = beta[3] as f32;
        let floor = 2e-4 / (tau.abs() * phimax).max(1e-9);
        Some(FitResult {
            b,
            se: (se as f32).max(floor),
            motion: b * tau * phimax,
            n: self.n,
        })
    }
}

fn invert4(m: [[f64; 4]; 4]) -> Option<[[f64; 4]; 4]> {
    let mut a = m;
    let mut inv = [[0.0f64; 4]; 4];
    for (i, row) in inv.iter_mut().enumerate() {
        row[i] = 1.0;
    }
    for c in 0..4 {
        let mut p = c;
        for r in c + 1..4 {
            if a[r][c].abs() > a[p][c].abs() {
                p = r;
            }
        }
        if a[p][c].abs() < 1e-18 {
            return None;
        }
        a.swap(c, p);
        inv.swap(c, p);
        let d = a[c][c];
        for k in 0..4 {
            a[c][k] /= d;
            inv[c][k] /= d;
        }
        for r in 0..4 {
            if r != c {
                let f = a[r][c];
                for k in 0..4 {
                    a[r][k] -= f * a[c][k];
                    inv[r][k] -= f * inv[c][k];
                }
            }
        }
    }
    Some(inv)
}

/// Pool independent probe fits of one joint: t-statistics add as
/// Σt/√n, `b` and `motion` come from the strongest single fit, with the
/// pooled sign.
pub fn combine(fits: &[FitResult]) -> Option<FitResult> {
    if fits.is_empty() {
        return None;
    }
    let t: f32 = fits.iter().map(|f| f.t_stat()).sum::<f32>() / libm::sqrtf(fits.len() as f32);
    let best = fits.iter().max_by(|a, b| a.t_stat().abs().total_cmp(&b.t_stat().abs()))?;
    let sign = if t < 0.0 { -1.0 } else { 1.0 };
    let b = sign * best.b.abs();
    let se = if t == 0.0 { f32::INFINITY } else { b.abs() / t.abs() };
    Some(FitResult {
        b,
        se,
        motion: sign * best.motion.abs(),
        n: fits.iter().map(|f| f.n).sum(),
    })
}
