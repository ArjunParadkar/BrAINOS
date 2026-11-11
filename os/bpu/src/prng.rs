//! BFS  LEVEL    Domain 4 substrate - the BPU seam (descriptor, mock, nerve, planner)  [crate brainos-bpu]
//! BFS  PURPOSE  The contract's read-noise generator: splitmix64-seeded xoshiro128** per column and device, Gaussian by a 1024-entry table, scaled by the dial
//! BFS  DEFINES  Xoshiro128ss, eta_from_word, EtaPrng
//! BFS  USES     this crate: contract, tables_gen
//! BFS  USED BY  bpu/mock, bpu/tests/golden, nostd/lib
//!
//! The read-noise generator (contract mode B).
//!
//! One xoshiro128** generator per (column, device), each seeded from a
//! 64-bit seed by splitmix64 so that no two overlap. A sample is the
//! top 10 bits of a generator word, looked up in a 1,024-entry inverse
//! Gaussian table (Q3.13, tails at about +-3.29 sigma), scaled by the
//! noise dial `sigma_q` (Q2.14), and clamped to the Q1.14 range:
//!
//! ```text
//!   eta_q = clamp( (sigma_q * ZLUT[word >> 22] + 2^12) >> 13 )
//! ```
//!
//! This is the same generator as `fpga/hdl/eta_prng.sv` and
//! `bpusim.fixed.EtaPRNG`; `tests/golden.rs` checks it against the
//! latter's output.

use crate::contract::ETA_MAX;
use crate::tables_gen::ZLUT;
use alloc::vec::Vec;

const M32: u64 = 0xFFFF_FFFF;

fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// xoshiro128** (Blackman and Vigna), 32-bit words.
#[derive(Clone, Debug)]
pub struct Xoshiro128ss {
    s: [u32; 4],
}

impl Xoshiro128ss {
    /// Generator number `instance` of a seed: four successive splitmix64
    /// outputs (low 32 bits each) from a state offset per instance.
    pub fn seeded(seed64: u64, instance: u64) -> Xoshiro128ss {
        let mut st = seed64.wrapping_add(instance.wrapping_mul(4).wrapping_mul(0x9E37_79B9_7F4A_7C15));
        let mut s = [0u32; 4];
        for w in &mut s {
            *w = (splitmix64(&mut st) & M32) as u32;
        }
        if s.iter().all(|w| *w == 0) {
            s[0] = 1;
        }
        Xoshiro128ss { s }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> u32 {
        let s = &mut self.s;
        let result = s[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = s[1] << 9;
        s[2] ^= s[0];
        s[3] ^= s[1];
        s[1] ^= s[2];
        s[0] ^= s[3];
        s[2] ^= t;
        s[3] = s[3].rotate_left(11);
        result
    }
}

/// One Gaussian sample scaled by the dial.
pub fn eta_from_word(word: u32, sigma_q: i64) -> i64 {
    let z = ZLUT[(word >> 22) as usize] as i64;
    ((sigma_q * z + (1 << 12)) >> 13).clamp(-ETA_MAX, ETA_MAX)
}

/// The noise source for a tile: `2 * cols` generators.
pub struct EtaPrng {
    gens: Vec<Xoshiro128ss>,
    pub sigma_q: i64,
    cols: usize,
}

impl EtaPrng {
    pub fn new(seed64: u64, sigma_q: i64, cols: usize) -> EtaPrng {
        EtaPrng {
            gens: (0..2 * cols as u64).map(|m| Xoshiro128ss::seeded(seed64, m)).collect(),
            sigma_q,
            cols,
        }
    }

    pub fn eta(&mut self, col: usize, device: usize) -> i64 {
        let word = self.gens[2 * col + device].next();
        eta_from_word(word, self.sigma_q)
    }

    /// The samples of one read of `rows` rows, in stream order
    /// `(row * cols + col) * 2 + device`.
    pub fn read_block(&mut self, rows: usize) -> Vec<i16> {
        let mut out = Vec::with_capacity(rows * self.cols * 2);
        for _ in 0..rows {
            for c in 0..self.cols {
                out.push(self.eta(c, 0) as i16);
                out.push(self.eta(c, 1) as i16);
            }
        }
        out
    }
}
