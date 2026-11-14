//! BFS  LEVEL    Domain 2 Autonomic - motor memory (self-model, mandates, primitives, reflexes)  [crate brainos-motor]
//! BFS  PURPOSE  Runs a trained policy shipped as int8 weights using integer arithmetic only between input and output, so the body's result matches the training tool's bit for bit
//! BFS  DEFINES  MAGIC, MAX_WIDTH, PolicyError, Int8Mlp
//! BFS  USES     nothing outside core/alloc
//! BFS  USED BY  motor/tests/policy_golden
//!
//! A learned policy, as the body runs it: int8 weights, integer sums.
//!
//! Training happens elsewhere, in floating point, on a workstation. What
//! ships is a small blob of 8-bit weights. Between the first rounding of
//! the inputs and the last scaling of the outputs everything is integer
//! arithmetic, so there is exactly one right answer for a given input
//! and blob, and a microcontroller, a laptop and the Python reference
//! all get it. `motor/gait/int8_ref.py` is that reference; the gate compares
//! the two on recorded observations.
//!
//! Blob, little-endian:
//!
//! ```text
//! "BMOTI8\x01\x00"            8 bytes
//! n_in  u16, n_layers u16
//! in_scale   f32 * n_in        input i -> round(x * in_scale[i]), clipped to +-127
//! per layer:
//!   rows u16, cols u16, mult i32, shift u8
//!   weights i8 * rows*cols     row-major
//!   bias    i32 * rows
//! out_scale  f32               output = y / 127 * out_scale
//! ```
//!
//! A hidden layer computes `clamp((acc * mult + half) >> shift, 0, 127)`;
//! the last layer clamps to `[-127, 127]` instead.

use alloc::vec::Vec;

pub const MAGIC: &[u8; 8] = b"BMOTI8\x01\x00";
pub const MAX_WIDTH: usize = 256;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PolicyError {
    BadMagic,
    Truncated,
    /// a layer's input width is not the previous layer's output width
    Mismatch,
    TooWide,
    BadShift,
    TrailingBytes,
}

#[derive(Clone, Debug)]
struct Layer {
    rows: usize,
    cols: usize,
    mult: i32,
    shift: u8,
    w: Vec<i8>,
    b: Vec<i32>,
}

/// An int8 multilayer perceptron.
#[derive(Clone, Debug)]
pub struct Int8Mlp {
    in_scale: Vec<f32>,
    layers: Vec<Layer>,
    out_scale: f32,
}

struct Cursor<'a> {
    b: &'a [u8],
    at: usize,
}

impl Cursor<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], PolicyError> {
        let end = self.at.checked_add(n).ok_or(PolicyError::Truncated)?;
        let s = self.b.get(self.at..end).ok_or(PolicyError::Truncated)?;
        self.at = end;
        Ok(s)
    }
    fn u16(&mut self) -> Result<usize, PolicyError> {
        let s = self.take(2)?;
        Ok(u16::from_le_bytes([s[0], s[1]]) as usize)
    }
    fn i32(&mut self) -> Result<i32, PolicyError> {
        let s = self.take(4)?;
        Ok(i32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn f32(&mut self) -> Result<f32, PolicyError> {
        let s = self.take(4)?;
        Ok(f32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }
}

impl Int8Mlp {
    pub fn from_bytes(blob: &[u8]) -> Result<Int8Mlp, PolicyError> {
        let mut c = Cursor { b: blob, at: 0 };
        if c.take(8)? != MAGIC {
            return Err(PolicyError::BadMagic);
        }
        let n_in = c.u16()?;
        let n_layers = c.u16()?;
        if n_in == 0 || n_in > MAX_WIDTH || n_layers == 0 {
            return Err(PolicyError::TooWide);
        }
        let mut in_scale = Vec::with_capacity(n_in);
        for _ in 0..n_in {
            in_scale.push(c.f32()?);
        }
        let mut layers = Vec::with_capacity(n_layers);
        let mut width = n_in;
        for _ in 0..n_layers {
            let rows = c.u16()?;
            let cols = c.u16()?;
            let mult = c.i32()?;
            let shift = c.take(1)?[0];
            if cols != width {
                return Err(PolicyError::Mismatch);
            }
            if rows == 0 || rows > MAX_WIDTH {
                return Err(PolicyError::TooWide);
            }
            // with at most 256 inputs of magnitude 127, weights of 127 and
            // a 32-bit bias, a sum is below 2^32; times a positive 32-bit
            // multiplier it is below 2^63, so the product cannot overflow
            if shift == 0 || shift > 40 || mult <= 0 {
                return Err(PolicyError::BadShift);
            }
            let w: Vec<i8> = c.take(rows * cols)?.iter().map(|b| *b as i8).collect();
            let mut b = Vec::with_capacity(rows);
            for _ in 0..rows {
                b.push(c.i32()?);
            }
            layers.push(Layer { rows, cols, mult, shift, w, b });
            width = rows;
        }
        let out_scale = c.f32()?;
        if c.at != blob.len() {
            return Err(PolicyError::TrailingBytes);
        }
        Ok(Int8Mlp { in_scale, layers, out_scale })
    }

    pub fn inputs(&self) -> usize {
        self.in_scale.len()
    }

    pub fn outputs(&self) -> usize {
        self.layers.last().map(|l| l.rows).unwrap_or(0)
    }

    /// The integer part: quantized inputs in, quantized outputs out.
    /// `out` receives values in `[-127, 127]`.
    pub fn act_int(&self, obs: &[f32], out: &mut [i32]) -> bool {
        if obs.len() != self.inputs() || out.len() != self.outputs() {
            return false;
        }
        let mut a = [0i32; MAX_WIDTH];
        let mut b = [0i32; MAX_WIDTH];
        for (i, x) in obs.iter().enumerate() {
            // ties to even, as numpy's rint does
            let q = libm::rintf(*x * self.in_scale[i]);
            a[i] = if q.is_nan() { 0 } else { q.clamp(-127.0, 127.0) as i32 };
        }
        let last = self.layers.len() - 1;
        for (li, l) in self.layers.iter().enumerate() {
            for r in 0..l.rows {
                let mut acc = l.b[r] as i64;
                let row = &l.w[r * l.cols..(r + 1) * l.cols];
                for (k, w) in row.iter().enumerate() {
                    acc += (*w as i64) * (a[k] as i64);
                }
                let y = (acc * l.mult as i64 + (1i64 << (l.shift - 1))) >> l.shift;
                b[r] = if li == last {
                    y.clamp(-127, 127) as i32
                } else {
                    y.clamp(0, 127) as i32
                };
            }
            a[..l.rows].copy_from_slice(&b[..l.rows]);
        }
        out.copy_from_slice(&a[..out.len()]);
        true
    }

    /// The whole policy: observations in, actions out.
    pub fn act(&self, obs: &[f32], out: &mut [f32]) -> bool {
        let mut q = [0i32; MAX_WIDTH];
        let n = self.outputs();
        if out.len() != n || !self.act_int(obs, &mut q[..n]) {
            return false;
        }
        for i in 0..n {
            out[i] = q[i] as f32 / 127.0 * self.out_scale;
        }
        true
    }
}
