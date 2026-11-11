//! BFS  LEVEL    Domain 4 substrate - the BPU seam (descriptor, mock, nerve, planner)  [crate brainos-bpu]
//! BFS  PURPOSE  BpuNerve: acquires a BPU as a Compute part on the Bpu substrate, with its vitals as senses and its five verbs gated and metered
//! BFS  DEFINES  LOAD, WRITE_VERIFY, REVERIFY, SET_DIAL, INFER, BpuNerve
//! BFS  USES     this crate: device | brainos: abi, body, cognition, crypto, part
//! BFS  USED BY  bpu/tests/seam
//!
//! The BPU nerve.
//!
//! Wraps any `BpuDevice` and presents it through the same `Nerve`
//! trait as a fan or a GPU. To the entity a BPU is a `Compute` part on
//! `Substrate::Bpu`: acquired through §8, felt through its vitals,
//! used through KIRA, and offered to placement.
//!
//! # Senses and vitals
//!
//! | sense | meaning | vital |
//! |---|---|---|
//! | `budget.pulses` | write pulses left | degraded below the descriptor's `low_pulses`, critical at `reserve_pulses` |
//! | `dead.ppm` | worst tile's dead devices, ppm | critical at the descriptor's tolerance |
//! | `drift` | furthest any loaded device is from its target, 0..1 | degraded at 0.05, critical at 0.10 (see below) |
//! | `temp.c` | temperature | **none**: no limit is published |
//! | `noise.sigma` | the dial's setting | none |
//! | `tiles.loaded` | tiles holding a model | none |
//!
//! **The drift thresholds are this nerve's assumption, not the
//! paper's.** The paper gives the *write* band (0.02) and says
//! retention drifts toward G_min so re-verify must be scheduled; it
//! gives no number at which to do so. A converged write-verify leaves
//! a device within the band or half a pulse (up to 0.025) of what was
//! *measured*, plus the 8-bit readout's rounding — about 0.05 of true
//! error in the worst case. So 0.05 is "outside what a fresh write
//! guarantees" and marks the part degraded. Critical at twice that is
//! a placeholder. Both are settable (`with_drift_limits`).
//!
//! # Verbs
//!
//! | verb | arguments | notes |
//! |---|---|---|
//! | `bpu.load_model` | `image`, `first_tile`, `tiles`, `pulses`, `dead_ppm` | metered against `budget.pulses`; irreversible |
//! | `bpu.write_verify_region` | `image`, `tile`, `pulses` | metered; irreversible |
//! | `bpu.reverify` | `tile`, `pulses` | metered; irreversible; a relief verb |
//! | `bpu.set_noise_dial` | `milli` 5..=200 | a relief verb |
//! | `bpu.infer` | `tile`, `passes` 1..=2, `shift` 0..=31 | |
//!
//! `pulses` is the caller's estimate of what the write will cost (the
//! planner's). KIRA refuses, at *simulate*, any write whose estimate
//! would take the budget below the reserve. `dead_ppm` is the caller's
//! statement of how many dead devices the target tiles have; its
//! declared upper bound is the device's tolerance, so a load onto
//! worse tiles is refused at *validate*.
//!
//! Both of those are claims, so the device checks the truth as well:
//! it stops a write that actually overruns the budget, and refuses a
//! load onto tiles that actually exceed the tolerance, whatever the
//! request said. The gate is checked first; the device is checked
//! anyway — the same shape as `bodyd` refusing a fall the seat granted.

use crate::device::{BpuDevice, BpuFault, WriteReport};
use alloc::vec::Vec;
use brainos_abi::text::Text;
use brainos_abi::time::Instant;
use brainos_abi::verb::Verb;
use brainos_body::region::RegionClass;
use brainos_body::schema::{ParamSpec, VerbSpec};
use brainos_body::vital::Band;
use brainos_cognition::placement::Substrate;
use brainos_crypto::key::{BrainKey, Challenge, Signature};
use brainos_part::class::PartClass;
use brainos_part::nerve::{Found, Mandate, Nerve, NerveFault, Sensed};
use brainos_part::policy::Meter;
use brainos_part::schema::{PartSchema, Quantity, SenseSpec, VitalKind, VitalSpec};

pub const LOAD: &str = "bpu.load_model";
pub const WRITE_VERIFY: &str = "bpu.write_verify_region";
pub const REVERIFY: &str = "bpu.reverify";
pub const SET_DIAL: &str = "bpu.set_noise_dial";
pub const INFER: &str = "bpu.infer";

/// The largest image digest a request can carry exactly.
const IMAGE_MAX: i64 = (1 << 48) - 1;

/// The nerve, over any device.
pub struct BpuNerve<D: BpuDevice> {
    pub dev: D,
    key: BrainKey,
    drift_warn: f64,
    drift_critical: f64,
    /// what the last write did, or why the device refused
    pub last_write: Option<Result<WriteReport, BpuFault>>,
    pub last_fault: Option<BpuFault>,
}

impl<D: BpuDevice> BpuNerve<D> {
    /// `key` is the device's identity key. A mock is given one; what
    /// holds it on real silicon is not yet specified.
    pub fn new(dev: D, key: BrainKey) -> BpuNerve<D> {
        BpuNerve {
            dev,
            key,
            drift_warn: 0.05,
            drift_critical: 0.10,
            last_write: None,
            last_fault: None,
        }
    }

    pub fn with_drift_limits(mut self, warn: f64, critical: f64) -> BpuNerve<D> {
        self.drift_warn = warn;
        self.drift_critical = critical;
        self
    }

    pub fn found(&self) -> Found {
        Found {
            handle: 0,
            key: self.key.public(),
            label: Text::from_str_truncating("bpu"),
        }
    }

    /// The meters to bind when acquiring: every verb that writes
    /// spends pulses, and never below the reserve.
    pub fn meters(&self) -> Vec<Meter> {
        let reserve = self.dev.descriptor().reserve_pulses as f64;
        [LOAD, WRITE_VERIFY, REVERIFY]
            .iter()
            .map(|v| Meter::by_arg(Verb::literal(v), "budget.pulses", "pulses", reserve))
            .collect()
    }

    /// The verbs that stay allowed when the part is critical: the ones
    /// that can make it better.
    pub fn relief(&self) -> Vec<Verb> {
        alloc::vec![Verb::literal(REVERIFY), Verb::literal(SET_DIAL)]
    }
}

impl<D: BpuDevice> Nerve for BpuNerve<D> {
    fn name(&self) -> &'static str {
        "bpu"
    }

    fn enumerate(&mut self) -> Vec<Found> {
        if self.dev.descriptor().is_coherent() {
            alloc::vec![self.found()]
        } else {
            Vec::new()
        }
    }

    fn answer(&mut self, _part: &Found, c: &Challenge) -> Option<Signature> {
        Some(self.key.answer(c))
    }

    fn describe(&mut self, _part: &Found) -> Option<PartSchema> {
        let d = self.dev.descriptor();
        if !d.is_coherent() {
            return None;
        }
        let tiles = d.tiles as i64;
        let pulses = ParamSpec::count("pulses", 0, d.endurance_pulses.min(1 << 52) as i64);
        let tile = ParamSpec::count("tile", 0, tiles - 1);
        let image = ParamSpec::count("image", 0, IMAGE_MAX);
        let region = RegionClass::Compute;
        Some(
            PartSchema::new(PartClass::Compute, "bpu", "bpu")
                .made_of(Substrate::Bpu)
                .verb(
                    VerbSpec::new(Verb::literal(LOAD), region, "write a staged model into tiles")
                        .with(image.clone())
                        .with(ParamSpec::count("first_tile", 0, tiles - 1))
                        .with(ParamSpec::count("tiles", 1, tiles))
                        .with(pulses.clone())
                        .with(ParamSpec::count("dead_ppm", 0, d.dead_tolerance_ppm as i64))
                        .irreversible(),
                )
                .verb(
                    VerbSpec::new(Verb::literal(WRITE_VERIFY), region, "rewrite one tile from the staged model")
                        .with(image)
                        .with(tile.clone())
                        .with(pulses.clone())
                        .irreversible(),
                )
                .verb(
                    VerbSpec::new(Verb::literal(REVERIFY), region, "re-verify one tile against its targets")
                        .with(tile.clone())
                        .with(pulses)
                        .irreversible(),
                )
                .verb(
                    VerbSpec::new(Verb::literal(SET_DIAL), region, "set the read-noise dial").with(
                        ParamSpec::count("milli", d.noise_dial_min_milli as i64, d.noise_dial_max_milli as i64),
                    ),
                )
                .verb(
                    VerbSpec::new(Verb::literal(INFER), region, "run one tile on the staged input")
                        .with(tile)
                        .with(ParamSpec::count("passes", 1, 2))
                        .with(ParamSpec::count("shift", 0, 31)),
                )
                .sense(SenseSpec::new("budget.pulses", Quantity::Count, 0.0, d.endurance_pulses as f64))
                .sense(SenseSpec::new("dead.ppm", Quantity::Count, 0.0, 1_000_000.0))
                .sense(SenseSpec::new("drift", Quantity::Fraction, 0.0, 1.0))
                .sense(SenseSpec::new("temp.c", Quantity::Temperature, -60.0, 250.0))
                .sense(SenseSpec::new("noise.sigma", Quantity::Fraction, 0.0, 1.0))
                .sense(SenseSpec::new("tiles.loaded", Quantity::Count, 0.0, d.tiles as f64))
                .vital(VitalSpec::new(
                    "budget.pulses",
                    VitalKind::Budget,
                    Band::falling(d.low_pulses as f64, d.reserve_pulses as f64),
                ))
                .vital(VitalSpec::new(
                    "dead.ppm",
                    VitalKind::Wear,
                    Band::rising(d.dead_tolerance_ppm as f64, d.dead_tolerance_ppm as f64),
                ))
                .vital(VitalSpec::new(
                    "drift",
                    VitalKind::Wear,
                    Band::rising(self.drift_warn, self.drift_critical),
                )),
        )
    }

    fn read(&mut self, _part: &Found, _at: Instant, out: &mut Vec<Sensed>) {
        let v = self.dev.vitals();
        out.push(Sensed::new("budget.pulses", v.pulses_left as f64));
        out.push(Sensed::new("dead.ppm", v.worst_dead_ppm as f64));
        out.push(Sensed::new("drift", v.drift));
        out.push(Sensed::new("temp.c", v.temperature_c));
        out.push(Sensed::new("noise.sigma", v.noise_sigma));
        out.push(Sensed::new("tiles.loaded", v.tiles_loaded as f64));
    }

    fn apply(&mut self, _part: &Found, m: &Mandate) -> Result<(), NerveFault> {
        self.last_fault = None;
        let wrote = match m.verb().as_str() {
            LOAD => Some(self.dev.load(m)),
            WRITE_VERIFY => Some(self.dev.write_verify(m)),
            REVERIFY => Some(self.dev.reverify(m)),
            SET_DIAL => {
                return self.dev.set_noise_dial(m).map_err(|f| {
                    self.last_fault = Some(f);
                    NerveFault::Refused
                })
            }
            INFER => {
                return self.dev.infer(m).map_err(|f| {
                    self.last_fault = Some(f);
                    NerveFault::Refused
                })
            }
            _ => return Err(NerveFault::NotSupported),
        };
        let r = wrote.expect("a write verb");
        self.last_write = Some(r);
        match r {
            Ok(_) => Ok(()),
            Err(f) => {
                self.last_fault = Some(f);
                Err(NerveFault::Refused)
            }
        }
    }

    /// Nothing to do: a BPU that is left alone holds its weights and
    /// computes nothing. There is no motion to stop.
    fn safe(&mut self) {}
}
