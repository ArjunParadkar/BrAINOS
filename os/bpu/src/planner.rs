//! BFS  LEVEL    Domain 4 substrate - the BPU seam (descriptor, mock, nerve, planner)  [crate brainos-bpu]
//! BFS  PURPOSE  A planner (not a compiler): from layer shapes to a tile and tier placement, a write-pulse estimate and a projected energy per token
//! BFS  DEFINES  LayerShape, LayerPlacement, TilePlan, NoPlan, mean_pulses_per_device_milli, plan
//! BFS  USES     this crate: tables_gen | brainos: cognition
//! BFS  USED BY  bpu/tests/seam, nostd/lib
//!
//! Model to tiles: a planner.
//!
//! **This is not a compiler.** A real model-to-tiles compiler would
//! quantise weights, split and route layers, schedule tile reads and
//! account for the peripheral circuits. This does arithmetic on layer
//! *shapes*: how many tiles a model needs, where they go, roughly how
//! many write pulses loading it costs, and a projected energy per
//! token. It exists so that placement and the write budget have
//! numbers to reason with before a compiler exists.
//!
//! What each number is:
//!
//! * **tiles** — exact for the stated rule: a layer of `inputs x
//!   outputs` weights takes `ceil(inputs / rows) * ceil(outputs / cols)`
//!   tiles. Layers do not share a tile.
//! * **placement** — tiles are filled in index order; tile `t` sits at
//!   site `t / tiers`, tier `t % tiers`.
//! * **write pulses** — an *estimate*: two devices per weight, each
//!   taken from empty to a level uniformly distributed over the range,
//!   using the contract's own cumulative-pulse table, plus a
//!   quarter for write-verify correction. A real load depends on the
//!   weights; the mock's actual count is what is debited.
//! * **energy per token** — a *projection* from the paper's constants:
//!   4 fJ per multiply-accumulate and 2.56 pJ per 8-bit column
//!   conversion. It leaves out the controller, the row drivers and
//!   everything off-tile. It is good for comparing placements, and
//!   should not be quoted as a measurement.

use crate::tables_gen::NUP;
use alloc::vec::Vec;
use brainos_cognition::bpu::{BpuDescriptor, E_ADC_CONVERSION_FJ, E_MAC_FJ, E_WRITE_PULSE_FJ};

/// One dense layer: `inputs` activations in, `outputs` out.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct LayerShape {
    pub inputs: u32,
    pub outputs: u32,
}

/// Where one layer goes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct LayerPlacement {
    pub layer: usize,
    pub first_tile: u32,
    pub tiles: u32,
    /// tiles along the input direction and the output direction
    pub row_blocks: u32,
    pub col_blocks: u32,
}

/// The plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TilePlan {
    pub layers: Vec<LayerPlacement>,
    pub tiles: u32,
    /// how many sites, and the highest tier, the plan touches
    pub sites: u32,
    pub tiers_used: u8,
    /// estimated write pulses to load it (see the module note)
    pub write_pulses: u64,
    /// projected energy to load it, femtojoules
    pub load_energy_fj: u64,
    /// projected energy per token, femtojoules
    pub energy_per_token_fj: u64,
    /// multiply-accumulates and column conversions per token
    pub macs_per_token: u64,
    pub conversions_per_token: u64,
}

/// Why a model has no plan.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NoPlan {
    /// no layers, or a layer with a zero dimension
    EmptyModel,
    /// it needs more tiles than the device has
    DoesNotFit { needed: u32, available: u32 },
    /// loading it would take the write budget below the reserve
    BeyondBudget { needed: u64, available: u64 },
}

/// The mean number of pulses to take a device from empty to a level
/// uniformly distributed over the range, in thousandths, from the
/// contract's cumulative-pulse table.
pub fn mean_pulses_per_device_milli() -> u64 {
    let base = NUP[0] as i64;
    let total: i64 = NUP.iter().map(|v| *v as i64 - base).sum();
    // the table is Q.8: divide by 256; average over its 1024 entries
    (total as u64 * 1000) / (256 * NUP.len() as u64)
}

/// Plan a model onto a device. `pulses_left` is the device's remaining
/// write budget.
pub fn plan(desc: &BpuDescriptor, layers: &[LayerShape], pulses_left: u64) -> Result<TilePlan, NoPlan> {
    if layers.is_empty() || layers.iter().any(|l| l.inputs == 0 || l.outputs == 0) {
        return Err(NoPlan::EmptyModel);
    }
    let (rows, cols) = (desc.tile_rows as u32, desc.tile_cols as u32);
    let mut placed = Vec::new();
    let mut next = 0u32;
    let (mut macs, mut conversions, mut weights) = (0u64, 0u64, 0u64);
    for (i, l) in layers.iter().enumerate() {
        let rb = l.inputs.div_ceil(rows);
        let cb = l.outputs.div_ceil(cols);
        let tiles = rb * cb;
        placed.push(LayerPlacement { layer: i, first_tile: next, tiles, row_blocks: rb, col_blocks: cb });
        next += tiles;
        let w = l.inputs as u64 * l.outputs as u64;
        weights += w;
        macs += w;
        // each row block of a layer converts every output column once
        conversions += rb as u64 * l.outputs as u64;
    }
    if next > desc.tiles {
        return Err(NoPlan::DoesNotFit { needed: next, available: desc.tiles });
    }
    let per_device = mean_pulses_per_device_milli();
    let write_pulses = weights * 2 * per_device / 1000 * 5 / 4;
    let available = pulses_left.saturating_sub(desc.reserve_pulses);
    if write_pulses > available {
        return Err(NoPlan::BeyondBudget { needed: write_pulses, available });
    }
    let tiers = desc.tiers as u32;
    Ok(TilePlan {
        layers: placed,
        tiles: next,
        sites: next.div_ceil(tiers),
        tiers_used: next.min(tiers) as u8,
        write_pulses,
        load_energy_fj: write_pulses * E_WRITE_PULSE_FJ,
        energy_per_token_fj: macs * E_MAC_FJ + conversions * E_ADC_CONVERSION_FJ,
        macs_per_token: macs,
        conversions_per_token: conversions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn desc(tiles: u32) -> BpuDescriptor {
        BpuDescriptor::paper_geometry(tiles, u64::MAX / 4)
    }

    #[test]
    fn a_layer_that_fits_one_tile_takes_one_and_a_larger_one_takes_a_grid() {
        let p = plan(&desc(64), &[LayerShape { inputs: 256, outputs: 256 }], u64::MAX / 4).unwrap();
        assert_eq!((p.tiles, p.sites, p.tiers_used), (1, 1, 1));
        let p = plan(&desc(64), &[LayerShape { inputs: 768, outputs: 300 }], u64::MAX / 4).unwrap();
        assert_eq!(p.layers[0].row_blocks, 3);
        assert_eq!(p.layers[0].col_blocks, 2);
        assert_eq!(p.tiles, 6);
        assert_eq!((p.sites, p.tiers_used), (2, 4), "six tiles fill one site of four tiers and start a second");
    }

    #[test]
    fn layers_are_placed_one_after_another_and_never_share_a_tile() {
        let layers = [
            LayerShape { inputs: 512, outputs: 256 },
            LayerShape { inputs: 256, outputs: 10 },
        ];
        let p = plan(&desc(64), &layers, u64::MAX / 4).unwrap();
        assert_eq!(p.layers[0].first_tile, 0);
        assert_eq!(p.layers[1].first_tile, 2);
        assert_eq!(p.tiles, 3);
    }

    #[test]
    fn the_energy_projection_is_the_stated_constants_and_nothing_else() {
        let p = plan(&desc(64), &[LayerShape { inputs: 256, outputs: 256 }], u64::MAX / 4).unwrap();
        assert_eq!(p.macs_per_token, 65_536);
        assert_eq!(p.conversions_per_token, 256);
        assert_eq!(p.energy_per_token_fj, 65_536 * 4 + 256 * 2_560);
        assert_eq!(p.load_energy_fj, p.write_pulses * 100_000);
    }

    #[test]
    fn a_model_that_does_not_fit_or_cannot_be_afforded_has_no_plan() {
        let big = [LayerShape { inputs: 4096, outputs: 4096 }];
        assert_eq!(
            plan(&desc(16), &big, u64::MAX / 4),
            Err(NoPlan::DoesNotFit { needed: 256, available: 16 })
        );
        let d = BpuDescriptor::paper_geometry(16, 1_000_000);
        let one = [LayerShape { inputs: 256, outputs: 256 }];
        assert!(matches!(plan(&d, &one, 100_000), Err(NoPlan::BeyondBudget { .. })));
        assert_eq!(plan(&d, &[], 1_000_000), Err(NoPlan::EmptyModel));
        assert_eq!(
            plan(&d, &[LayerShape { inputs: 0, outputs: 4 }], 1_000_000),
            Err(NoPlan::EmptyModel)
        );
    }

    #[test]
    fn the_pulse_estimate_comes_from_the_contracts_table() {
        let m = mean_pulses_per_device_milli();
        // tens of pulses per device, not ones and not thousands
        assert!((10_000..200_000).contains(&m), "{m}");
    }
}
