//! Policy inference: a small multilayer perceptron, `no_std`, no
//! allocation per call.
//!
//! The skills BLUR learns in the twin are MLPs (observation → joint-target
//! offsets). They are trained in `blur/learn/ppo.py` and exported as a
//! flat little-endian blob (`blur/learn/export.py`); this module runs that
//! blob. The same code runs on the Pi (the motor cortex, 50 Hz) and, for a
//! reflex policy, on the Teensy. A get-up policy of 63→256→256→128→19 is
//! ~102k multiply-adds: ~0.2 ms on a Cortex-M7 at 600 MHz, well inside a
//! 1 ms tick.
//!
//! Where a policy *lives* is the state graph (a skill node's state), not
//! this crate and not the body: the blob is pushed down like a
//! calibration, and a body that kept its own copy would be a body with
//! memory the entity does not control.
//!
//! Blob layout (all little-endian):
//! ```text
//!   b"BLURMLP1"  n_layers:u32  obs_dim:u32  act_dim:u32
//!   obs_clip:f32  action_scale:f32
//!   obs_mean[obs_dim]:f32  obs_std[obs_dim]:f32
//!   per layer: rows:u32 cols:u32 w[rows*cols]:f32 (row-major) b[rows]:f32
//! ```

use alloc::vec::Vec;

pub const MAGIC: &[u8; 8] = b"BLURMLP1";
pub const MAX_WIDTH: usize = 512;

#[derive(Clone, Debug)]
pub struct Layer {
    pub rows: usize,
    pub cols: usize,
    pub w: Vec<f32>,
    pub b: Vec<f32>,
}

#[derive(Clone, Debug)]
pub struct Mlp {
    pub obs_dim: usize,
    pub act_dim: usize,
    pub obs_clip: f32,
    pub action_scale: f32,
    pub mean: Vec<f32>,
    pub std: Vec<f32>,
    pub layers: Vec<Layer>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MlpError {
    BadMagic,
    Truncated,
    Shape,
    TooWide,
    NotFinite,
}

struct Rd<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> Rd<'a> {
    fn u32(&mut self) -> Result<u32, MlpError> {
        let s = self.b.get(self.p..self.p + 4).ok_or(MlpError::Truncated)?;
        self.p += 4;
        Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn f32(&mut self) -> Result<f32, MlpError> {
        let v = f32::from_bits(self.u32()?);
        if v.is_finite() {
            Ok(v)
        } else {
            Err(MlpError::NotFinite)
        }
    }
    fn vec(&mut self, n: usize) -> Result<Vec<f32>, MlpError> {
        let mut v = Vec::with_capacity(n);
        for _ in 0..n {
            v.push(self.f32()?);
        }
        Ok(v)
    }
}

impl Mlp {
    pub fn from_bytes(b: &[u8]) -> Result<Mlp, MlpError> {
        if b.len() < 8 || &b[..8] != MAGIC {
            return Err(MlpError::BadMagic);
        }
        let mut r = Rd { b, p: 8 };
        let n_layers = r.u32()? as usize;
        let obs_dim = r.u32()? as usize;
        let act_dim = r.u32()? as usize;
        let obs_clip = r.f32()?;
        let action_scale = r.f32()?;
        if n_layers == 0 || n_layers > 8 || obs_dim == 0 || obs_dim > MAX_WIDTH || act_dim == 0 || act_dim > MAX_WIDTH {
            return Err(MlpError::Shape);
        }
        let mean = r.vec(obs_dim)?;
        let std = r.vec(obs_dim)?;
        let mut layers = Vec::new();
        let mut cols_expected = obs_dim;
        for _ in 0..n_layers {
            let rows = r.u32()? as usize;
            let cols = r.u32()? as usize;
            if cols != cols_expected {
                return Err(MlpError::Shape);
            }
            if rows > MAX_WIDTH {
                return Err(MlpError::TooWide);
            }
            let w = r.vec(rows * cols)?;
            let bias = r.vec(rows)?;
            layers.push(Layer { rows, cols, w, b: bias });
            cols_expected = rows;
        }
        if cols_expected != act_dim || r.p != b.len() {
            return Err(MlpError::Shape);
        }
        Ok(Mlp { obs_dim, act_dim, obs_clip, action_scale, mean, std, layers })
    }

    /// Multiply-adds per inference, for the latency budget.
    pub fn macs(&self) -> usize {
        self.layers.iter().map(|l| l.rows * l.cols).sum()
    }

    /// Run the policy. `obs` is raw (unnormalised); `out` gets the action
    /// in [-1, 1] (the deterministic mean, clipped). Returns false if the
    /// observation has the wrong length or is not finite — the caller
    /// treats that as "no action", never as zeros.
    pub fn act(&self, obs: &[f32], out: &mut [f32]) -> bool {
        if obs.len() != self.obs_dim || out.len() != self.act_dim || obs.iter().any(|x| !x.is_finite()) {
            return false;
        }
        let mut a = [0.0f32; MAX_WIDTH];
        let mut b = [0.0f32; MAX_WIDTH];
        for i in 0..self.obs_dim {
            let z = (obs[i] - self.mean[i]) / self.std[i];
            a[i] = z.clamp(-self.obs_clip, self.obs_clip);
        }
        let n = self.layers.len();
        for (k, l) in self.layers.iter().enumerate() {
            for r in 0..l.rows {
                let row = &l.w[r * l.cols..(r + 1) * l.cols];
                let mut s = l.b[r];
                for c in 0..l.cols {
                    s += row[c] * a[c];
                }
                b[r] = if k + 1 < n {
                    // ELU
                    if s > 0.0 {
                        s
                    } else {
                        libm::expf(s) - 1.0
                    }
                } else {
                    s
                };
            }
            core::mem::swap(&mut a, &mut b);
        }
        for i in 0..self.act_dim {
            out[i] = a[i].clamp(-1.0, 1.0);
        }
        true
    }
}
