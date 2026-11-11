//! BFS  LEVEL    Domain 4 substrate - the BPU seam (descriptor, mock, nerve, planner)  [crate brainos-bpu]
//! BFS  PURPOSE  MockBpu: a stand-in BPU whose reads and writes are the integer contract exactly, with knobs for wear, dead devices, drift and heat
//! BFS  DEFINES  MockBpu, BAND
//! BFS  USES     this crate: contract, device, prng | brainos: cognition, part
//! BFS  USED BY  bpu/tests/seam
//!
//! A mock BPU.
//!
//! **What is faithful.** Every read is `contract::tile_read` and every
//! write is `contract::program_target`, with read noise from
//! `prng::EtaPrng` at the current dial. Those are bit-exact against
//! the reference (`tests/golden.rs`). So a number that comes out of
//! this mock is the number the contract says a tile produces.
//!
//! **What is a stand-in.** There is no device physics here. Dead
//! devices, retention drift and temperature are *set by the test*
//! through the `age_*` and `heat` methods; nothing decays on its own,
//! because the contract gives no law for it to decay by, and inventing
//! one would be inventing a device. Cycle-to-cycle write variation is
//! zero and every device has unit gain; the contract functions accept
//! both, and the golden tests exercise them, but this mock does not
//! make them up.
//!
//! **Authority.** Staging and the aging knobs are public: staging
//! changes no weight, and aging is the world acting on the device, not
//! the entity. Everything that writes a weight or moves the dial is a
//! `BpuDevice` method and takes a `Mandate`.

use crate::contract::{
    program_target, sigma_code_from_milli, tile_read, verify_current, ws_to_wq, GAIN_ONE, TOL_Q,
    VERIFY_READS, WS_ONE,
};
use crate::device::{BpuDevice, BpuFault, BpuVitals, ModelImage, TileImage, WriteReport};
use crate::prng::{eta_from_word, EtaPrng, Xoshiro128ss};
use alloc::vec;
use alloc::vec::Vec;
use brainos_cognition::bpu::BpuDescriptor;
use brainos_part::nerve::Mandate;

struct Tile {
    /// device states, Q1.15
    wp: Vec<i64>,
    wm: Vec<i64>,
    /// what was asked for, Q1.15; empty until loaded
    tp: Vec<i64>,
    tm: Vec<i64>,
    mask: Vec<bool>,
    dead_p: Vec<bool>,
    dead_m: Vec<bool>,
    loaded: bool,
}

/// The mock.
pub struct MockBpu {
    desc: BpuDescriptor,
    tiles: Vec<Tile>,
    dial_milli: u32,
    seed: u64,
    /// counts noise streams handed out, so no two reads share one
    streams: u64,
    pulses_spent: u64,
    temperature_c: f64,
    staged: Option<ModelImage>,
    input: Vec<i32>,
    output: Vec<i8>,
    /// how many tile reads the last inference clipped
    pub last_clipped: u32,
}

fn code_to_ws(w_q: u8) -> i64 {
    // the state whose read-side code is w_q: w_q * 2^15 / 255, rounded
    (w_q as i64 * WS_ONE * 2 + 255) / (2 * 255)
}

impl MockBpu {
    pub fn new(desc: BpuDescriptor, seed: u64) -> MockBpu {
        let n = desc.weights_per_tile() as usize;
        let tiles = (0..desc.tiles)
            .map(|_| Tile {
                wp: vec![0; n],
                wm: vec![0; n],
                tp: Vec::new(),
                tm: Vec::new(),
                mask: vec![false; n],
                dead_p: vec![false; n],
                dead_m: vec![false; n],
                loaded: false,
            })
            .collect();
        MockBpu {
            desc,
            tiles,
            dial_milli: 20,
            seed,
            streams: 0,
            pulses_spent: 0,
            temperature_c: 40.0,
            staged: None,
            input: Vec::new(),
            output: Vec::new(),
            last_clipped: 0,
        }
    }

    pub fn pulses_spent(&self) -> u64 {
        self.pulses_spent
    }

    pub fn dial_milli(&self) -> u32 {
        self.dial_milli
    }

    /// The read-side state codes of one tile, for checking a load.
    pub fn tile_codes(&self, tile: u32) -> Option<(Vec<u8>, Vec<u8>)> {
        let t = self.tiles.get(tile as usize)?;
        Some((
            t.wp.iter().map(|w| ws_to_wq(*w) as u8).collect(),
            t.wm.iter().map(|w| ws_to_wq(*w) as u8).collect(),
        ))
    }

    // ---- the world acting on the device (test knobs, not physics) ----

    /// Mark every `every`-th device of a tile dead.
    pub fn age_kill(&mut self, tile: u32, every: usize) {
        if let Some(t) = self.tiles.get_mut(tile as usize) {
            for i in (0..t.dead_p.len()).step_by(every.max(1)) {
                t.dead_p[i] = true;
            }
        }
    }

    /// Relax every live device of a tile toward zero conductance by
    /// `q15` state units — the direction retention loss goes.
    pub fn age_drift(&mut self, tile: u32, q15: i64) {
        if let Some(t) = self.tiles.get_mut(tile as usize) {
            for i in 0..t.wp.len() {
                if !t.dead_p[i] {
                    t.wp[i] = (t.wp[i] - q15).max(0);
                }
                if !t.dead_m[i] {
                    t.wm[i] = (t.wm[i] - q15).max(0);
                }
            }
        }
    }

    /// Spend write pulses, as an older device would already have.
    pub fn age_wear(&mut self, pulses: u64) {
        self.pulses_spent = self.pulses_spent.saturating_add(pulses);
    }

    pub fn heat(&mut self, celsius: f64) {
        self.temperature_c = celsius;
    }

    // ---- internals ----

    fn pulses_left(&self) -> u64 {
        self.desc.endurance_pulses.saturating_sub(self.pulses_spent)
    }

    fn tile_dead_ppm(t: &Tile) -> u32 {
        let dead = t.dead_p.iter().chain(&t.dead_m).filter(|d| **d).count() as u64;
        let all = (t.dead_p.len() + t.dead_m.len()) as u64;
        if all == 0 {
            0
        } else {
            (dead * 1_000_000 / all) as u32
        }
    }

    fn next_stream(&mut self) -> Xoshiro128ss {
        self.streams += 1;
        // instances far above the per-column ones a tile read uses
        Xoshiro128ss::seeded(self.seed, 1_000_000 + self.streams)
    }

    /// Write-verify one tile toward targets, charging the budget.
    fn program(&mut self, tile: usize, tp: Vec<i64>, tm: Vec<i64>) -> Result<(u64, bool), BpuFault> {
        let sigma_q = sigma_code_from_milli(self.dial_milli);
        let budget = self.pulses_left().saturating_sub(self.desc.reserve_pulses);
        let mut spent = 0u64;
        let mut settled = true;
        for plus in [true, false] {
            let mut g = self.next_stream();
            let t = &self.tiles[tile];
            let (w, tgt, dead) = if plus {
                (&t.wp, &tp, &t.dead_p)
            } else {
                (&t.wm, &tm, &t.dead_m)
            };
            let gain = vec![GAIN_ONE; w.len()];
            let read = |w: &[i64]| {
                let mut s = vec![0i64; w.len()];
                for _ in 0..VERIFY_READS {
                    for (i, wi) in w.iter().enumerate() {
                        s[i] += verify_current(ws_to_wq(*wi), eta_from_word(g.next(), sigma_q));
                    }
                }
                s
            };
            let done = program_target(w, tgt, &gain, read, |n| vec![0; n], dead);
            spent += done.pulses;
            // the device enforces its own budget, whatever was granted
            if spent > budget {
                return Err(BpuFault::BudgetExhausted);
            }
            settled &= done.settled;
            let t = &mut self.tiles[tile];
            if plus {
                t.wp = done.w_s;
            } else {
                t.wm = done.w_s;
            }
        }
        self.pulses_spent += spent;
        let t = &mut self.tiles[tile];
        t.tp = tp;
        t.tm = tm;
        t.loaded = true;
        Ok((spent, settled))
    }

    fn write_image(&mut self, tile: usize, img: &TileImage) -> Result<(u64, bool), BpuFault> {
        let n = self.desc.weights_per_tile() as usize;
        if img.w_plus.len() != n || img.w_minus.len() != n || img.mask.len() != n {
            return Err(BpuFault::BadGeometry);
        }
        if Self::tile_dead_ppm(&self.tiles[tile]) > self.desc.dead_tolerance_ppm {
            return Err(BpuFault::TooManyDead);
        }
        self.tiles[tile].mask = img.mask.clone();
        self.program(
            tile,
            img.w_plus.iter().map(|c| code_to_ws(*c)).collect(),
            img.w_minus.iter().map(|c| code_to_ws(*c)).collect(),
        )
    }

    fn staged_for(&self, auth: &Mandate) -> Result<ModelImage, BpuFault> {
        let named = auth.scalar("image").ok_or(BpuFault::BadArguments)? as u64;
        match &self.staged {
            Some(img) if img.digest48() == named => Ok(img.clone()),
            _ => Err(BpuFault::WrongImage),
        }
    }
}

impl BpuDevice for MockBpu {
    fn descriptor(&self) -> BpuDescriptor {
        self.desc
    }

    fn vitals(&self) -> BpuVitals {
        let mut drift: i64 = 0;
        for t in self.tiles.iter().filter(|t| t.loaded) {
            for i in 0..t.wp.len() {
                if !t.dead_p[i] {
                    drift = drift.max((t.tp[i] - t.wp[i]).abs());
                }
                if !t.dead_m[i] {
                    drift = drift.max((t.tm[i] - t.wm[i]).abs());
                }
            }
        }
        BpuVitals {
            pulses_left: self.pulses_left(),
            worst_dead_ppm: self.tiles.iter().map(Self::tile_dead_ppm).max().unwrap_or(0),
            drift: drift as f64 / WS_ONE as f64,
            temperature_c: self.temperature_c,
            noise_sigma: self.dial_milli as f64 / 1000.0,
            tiles_loaded: self.tiles.iter().filter(|t| t.loaded).count() as u32,
        }
    }

    fn dead_ppm(&self, first_tile: u32, tiles: u32) -> Option<u32> {
        let end = first_tile.checked_add(tiles)? as usize;
        self.tiles
            .get(first_tile as usize..end)?
            .iter()
            .map(Self::tile_dead_ppm)
            .max()
    }

    fn stage_image(&mut self, image: ModelImage) {
        self.staged = Some(image);
    }

    fn stage_input(&mut self, x: Vec<i32>) {
        self.input = x;
    }

    fn output(&self) -> &[i8] {
        &self.output
    }

    fn load(&mut self, auth: &Mandate) -> Result<WriteReport, BpuFault> {
        let img = self.staged_for(auth)?;
        let first = auth.scalar("first_tile").ok_or(BpuFault::BadArguments)? as u32;
        let count = auth.scalar("tiles").ok_or(BpuFault::BadArguments)? as u32;
        if first != img.first_tile || count as usize != img.tiles.len() {
            return Err(BpuFault::WrongImage);
        }
        if first.checked_add(count).map(|e| e > self.desc.tiles).unwrap_or(true) {
            return Err(BpuFault::NoSuchTile);
        }
        // the gate checked the dead fraction the request claimed; the
        // device checks the real one before anything is written
        if self.dead_ppm(first, count).unwrap_or(u32::MAX) > self.desc.dead_tolerance_ppm {
            return Err(BpuFault::TooManyDead);
        }
        let mut report = WriteReport { tiles: 0, pulses: 0, settled: true };
        for (k, t) in img.tiles.iter().enumerate() {
            let (p, c) = self.write_image(first as usize + k, t)?;
            report.tiles += 1;
            report.pulses += p;
            report.settled &= c;
        }
        Ok(report)
    }

    fn write_verify(&mut self, auth: &Mandate) -> Result<WriteReport, BpuFault> {
        let img = self.staged_for(auth)?;
        let tile = auth.scalar("tile").ok_or(BpuFault::BadArguments)? as u32;
        let k = tile.checked_sub(img.first_tile).ok_or(BpuFault::NoSuchTile)? as usize;
        let t = img.tiles.get(k).ok_or(BpuFault::NoSuchTile)?;
        if tile >= self.desc.tiles {
            return Err(BpuFault::NoSuchTile);
        }
        let (pulses, settled) = self.write_image(tile as usize, t)?;
        Ok(WriteReport { tiles: 1, pulses, settled })
    }

    fn reverify(&mut self, auth: &Mandate) -> Result<WriteReport, BpuFault> {
        let tile = auth.scalar("tile").ok_or(BpuFault::BadArguments)? as usize;
        let t = self.tiles.get(tile).ok_or(BpuFault::NoSuchTile)?;
        if !t.loaded {
            return Err(BpuFault::NotLoaded);
        }
        let (tp, tm) = (t.tp.clone(), t.tm.clone());
        let (pulses, settled) = self.program(tile, tp, tm)?;
        Ok(WriteReport { tiles: 1, pulses, settled })
    }

    fn set_noise_dial(&mut self, auth: &Mandate) -> Result<(), BpuFault> {
        let milli = auth.scalar("milli").ok_or(BpuFault::BadArguments)?;
        // checked here as well as at the gate: the device has a range
        // whether or not anything upstream enforced it
        if !(milli >= self.desc.noise_dial_min_milli as f64
            && milli <= self.desc.noise_dial_max_milli as f64)
        {
            return Err(BpuFault::DialOutOfRange);
        }
        self.dial_milli = milli as u32;
        Ok(())
    }

    fn infer(&mut self, auth: &Mandate) -> Result<(), BpuFault> {
        let tile = auth.scalar("tile").ok_or(BpuFault::BadArguments)? as usize;
        let passes = auth.scalar("passes").ok_or(BpuFault::BadArguments)? as u32;
        let shift = auth.scalar("shift").ok_or(BpuFault::BadArguments)? as u32;
        let (rows, cols) = (self.desc.tile_rows as usize, self.desc.tile_cols as usize);
        let t = self.tiles.get(tile).ok_or(BpuFault::NoSuchTile)?;
        if !t.loaded {
            return Err(BpuFault::NotLoaded);
        }
        if self.input.len() != rows || !(1..=2).contains(&passes) || shift > 31 {
            return Err(BpuFault::BadGeometry);
        }
        self.streams += 1;
        let mut noise = EtaPrng::new(
            self.seed.wrapping_add(self.streams),
            sigma_code_from_milli(self.dial_milli),
            cols,
        );
        let eta = noise.read_block(rows);
        let wp: Vec<u8> = t.wp.iter().map(|w| ws_to_wq(*w) as u8).collect();
        let wm: Vec<u8> = t.wm.iter().map(|w| ws_to_wq(*w) as u8).collect();
        let r = tile_read(rows, cols, &wp, &wm, &t.mask, &self.input, &eta, shift, passes);
        self.last_clipped = r.clipped;
        self.output = r.y;
        Ok(())
    }
}

/// The contract's band, as a fraction of full scale.
pub const BAND: f64 = TOL_Q as f64 / WS_ONE as f64;
