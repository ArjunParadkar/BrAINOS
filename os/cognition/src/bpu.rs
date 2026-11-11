//! BFS  LEVEL    Domain 4 Cognition - tiers, placement, attention, escalation  [crate brainos-cognition]
//! BFS  PURPOSE  BpuDescriptor: what a BPU says about its geometry, contract, noise dial and endurance, plus the projected energy constants for placement cost
//! BFS  DEFINES  E_MAC_FJ, E_ADC_CONVERSION_FJ, E_WRITE_PULSE_FJ, NOISE_DIAL_MIN_MILLI, NOISE_DIAL_MAX_MILLI, BpuDescriptor
//! BFS  USES     nothing outside core/alloc
//! BFS  USED BY  bpu/device, bpu/mock, bpu/planner, bpu/tests/seam, cognition/lib, nostd/lib
//!
//! What a BPU says it is.
//!
//! `Substrate::Bpu` says a resource runs on a BrAIn Processing Unit.
//! `BpuDescriptor` is the part of that which placement needs to reason
//! about: how much fits, under which integer contract, how much noise
//! can be dialled, and how much writing it has left in it.
//!
//! The figures a descriptor carries come from the device. The defaults
//! in `paper_geometry` are the published design point (256 x 256
//! tiles, four tiers, 256 levels, 8-bit column ADC); the endurance
//! figures have **no published value** — on-chip endurance is unproven
//! — so they are parameters, and anything that fills them in is saying
//! what it assumed.
//!
//! The energy constants are **projections** from the paper's device
//! model, not measurements of silicon. They exist so that placement
//! has a cost to compare, and every use of them is labelled.

/// Projected energy of one multiply-accumulate, femtojoules.
pub const E_MAC_FJ: u64 = 4;
/// Projected energy of one 8-bit column conversion, femtojoules
/// (10 fJ per step, 256 steps).
pub const E_ADC_CONVERSION_FJ: u64 = 2_560;
/// Projected energy of one write pulse, femtojoules (100 pJ).
pub const E_WRITE_PULSE_FJ: u64 = 100_000;

/// The noise dial's range, in thousandths of sigma: 0.005 to 0.2.
pub const NOISE_DIAL_MIN_MILLI: u32 = 5;
pub const NOISE_DIAL_MAX_MILLI: u32 = 200;

/// A BPU's own description of itself.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BpuDescriptor {
    /// how many tiles, across all tiers
    pub tiles: u32,
    pub tile_rows: u16,
    pub tile_cols: u16,
    /// stacked device tiers; tile `t` is at site `t / tiers`, tier `t % tiers`
    pub tiers: u8,
    /// conductance levels per device
    pub levels: u16,
    /// column ADC resolution
    pub adc_bits: u8,
    /// which integer contract it implements. 1 = `bpusim/fixed.py`.
    pub contract_version: u16,
    pub noise_dial_min_milli: u32,
    pub noise_dial_max_milli: u32,
    /// write pulses the device may take in its life. Unproven on
    /// silicon; whoever builds a descriptor states the assumption.
    pub endurance_pulses: u64,
    /// below this many pulses left, the device is running low
    pub low_pulses: u64,
    /// never spend below this: kept for re-verifying what is loaded
    pub reserve_pulses: u64,
    /// the fraction of dead devices a tile may have and still be
    /// loaded, in parts per million
    pub dead_tolerance_ppm: u32,
}

impl BpuDescriptor {
    /// The published design point, with the endurance figures supplied
    /// by the caller.
    pub fn paper_geometry(tiles: u32, endurance_pulses: u64) -> BpuDescriptor {
        BpuDescriptor {
            tiles,
            tile_rows: 256,
            tile_cols: 256,
            tiers: 4,
            levels: 256,
            adc_bits: 8,
            contract_version: 1,
            noise_dial_min_milli: NOISE_DIAL_MIN_MILLI,
            noise_dial_max_milli: NOISE_DIAL_MAX_MILLI,
            endurance_pulses,
            low_pulses: endurance_pulses / 5,
            reserve_pulses: endurance_pulses / 20,
            dead_tolerance_ppm: 10_000,
        }
    }

    /// The same contract on a smaller tile — the 32 x 32 tile the FPGA
    /// rung and the reference use.
    pub fn with_tile(mut self, rows: u16, cols: u16) -> BpuDescriptor {
        self.tile_rows = rows;
        self.tile_cols = cols;
        self
    }

    pub fn weights_per_tile(&self) -> u64 {
        self.tile_rows as u64 * self.tile_cols as u64
    }

    pub fn total_weights(&self) -> u64 {
        self.weights_per_tile() * self.tiles as u64
    }

    /// Does this description hang together? A device that says it has
    /// no tiles, a dial with no range, or a reserve larger than its
    /// lifetime is not one placement should plan against.
    pub fn is_coherent(&self) -> bool {
        self.tiles > 0
            && self.tile_rows > 0
            && self.tile_cols > 0
            && self.tiers > 0
            && self.levels >= 2
            && (1..=16).contains(&self.adc_bits)
            && self.noise_dial_min_milli <= self.noise_dial_max_milli
            && self.reserve_pulses <= self.low_pulses
            && self.low_pulses <= self.endurance_pulses
            && self.dead_tolerance_ppm <= 1_000_000
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_paper_geometry_is_65536_signed_weights_a_tile() {
        let d = BpuDescriptor::paper_geometry(16, 1_000_000_000);
        assert!(d.is_coherent());
        assert_eq!(d.weights_per_tile(), 65_536);
        assert_eq!(d.total_weights(), 16 * 65_536);
        assert_eq!((d.tiers, d.levels, d.adc_bits), (4, 256, 8));
        assert_eq!((d.noise_dial_min_milli, d.noise_dial_max_milli), (5, 200));
    }

    #[test]
    fn an_incoherent_descriptor_is_recognised() {
        let good = BpuDescriptor::paper_geometry(4, 1000);
        let mut d = good;
        d.tiles = 0;
        assert!(!d.is_coherent());
        let mut d = good;
        d.reserve_pulses = d.endurance_pulses + 1;
        assert!(!d.is_coherent());
        let mut d = good;
        d.noise_dial_min_milli = 300;
        assert!(!d.is_coherent());
    }

    #[test]
    fn the_adc_constant_is_ten_femtojoules_a_step_at_eight_bits() {
        assert_eq!(E_ADC_CONVERSION_FJ, 10 * 256);
        assert_eq!(E_WRITE_PULSE_FJ, 100 * 1000);
    }
}
