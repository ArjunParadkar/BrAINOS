//! BFS  LEVEL    Host - the only crate with an operating system (CLI, scenarios, OS nerves)  [crate brainos-host]
//! BFS  PURPOSE  The NVIDIA GPU nerve: each GPU as a Compute part with temperature, load, memory, power and ECC senses, read through NVML
//! BFS  DEFINES  NvmlNerve
//! BFS  USES     this crate: nvml_ffi, stable_key | brainos: abi, body, cognition, crypto, part
//! BFS  USED BY  host/tests/nerves_live
//!
//! NVIDIA GPUs, through NVML.
//!
//! Each GPU is one Compute part on the `Gpu` substrate:
//!
//! | sense | unit | vital? |
//! |---|---|---|
//! | `temp.c` | °C | yes, when the GPU states its own limits: degraded at its *slowdown* temperature, critical at its *shutdown* temperature |
//! | `util` | 0..1 | no |
//! | `mem.used`, `mem.total` | bytes | no |
//! | `power.w` | W | no |
//! | `ecc.uncorrected` | count | yes when the card has ECC: any uncorrected error is critical |
//!
//! It has no verbs. It is a placement target: acquiring it with a
//! `ComputeOffer` adds a resource to the pool, and its temperature
//! going critical withdraws that resource.
//!
//! Skipped cleanly: with the `nvml` feature off this module is not
//! compiled; with it on, `NvmlNerve::live()` is `None` when the
//! library is absent or reports no GPU. The identity key is derived
//! from the GPU's UUID.
//!
//! Loading NVML wakes a laptop's discrete GPU. That is the cost of
//! asking it how it is.

use super::nvml_ffi::{Gpu, Nvml, THRESHOLD_SHUTDOWN, THRESHOLD_SLOWDOWN};
use super::stable_key;
use brainos_abi::text::Text;
use brainos_abi::time::Instant;
use brainos_body::vital::Band;
use brainos_cognition::placement::Substrate;
use brainos_crypto::key::{BrainKey, Challenge, Signature};
use brainos_part::class::PartClass;
use brainos_part::nerve::{Found, Mandate, Nerve, NerveFault, Sensed};
use brainos_part::schema::{PartSchema, Quantity, SenseSpec, VitalKind, VitalSpec};

struct Card {
    gpu: Gpu,
    label: String,
    key: BrainKey,
    /// (slowdown, shutdown), if the card states both and they make sense
    limits: Option<(f64, f64)>,
    has_ecc: bool,
}

pub struct NvmlNerve {
    lib: Nvml,
    cards: Vec<Card>,
}

impl NvmlNerve {
    /// `None` when there is nothing to talk to.
    pub fn live() -> Option<NvmlNerve> {
        let lib = Nvml::load()?;
        if lib.gpus().is_empty() {
            return None;
        }
        Some(NvmlNerve { lib, cards: Vec::new() })
    }

    /// The limits the first GPU states, for cross-checking against
    /// `nvidia-smi`.
    pub fn stated_limits(&self) -> Option<(f64, f64)> {
        self.cards.first().and_then(|c| c.limits)
    }
}

impl Nerve for NvmlNerve {
    fn name(&self) -> &'static str {
        "nvml"
    }

    fn enumerate(&mut self) -> Vec<Found> {
        self.cards = self
            .lib
            .gpus()
            .into_iter()
            .enumerate()
            .map(|(i, gpu)| {
                let name = self.lib.name(gpu).unwrap_or_else(|| format!("gpu {i}"));
                let uuid = self.lib.uuid(gpu).unwrap_or_else(|| format!("index-{i}"));
                let slow = self.lib.threshold_c(gpu, THRESHOLD_SLOWDOWN);
                let shut = self.lib.threshold_c(gpu, THRESHOLD_SHUTDOWN);
                let limits = match (slow, shut) {
                    (Some(a), Some(b)) if (40.0..=150.0).contains(&a) && a <= b && b <= 150.0 => {
                        Some((a, b))
                    }
                    _ => None,
                };
                Card {
                    gpu,
                    label: format!("gpu {name}"),
                    key: stable_key("nvml", &uuid),
                    limits,
                    has_ecc: self.lib.ecc_uncorrected(gpu).is_some(),
                }
            })
            .collect();
        self.cards
            .iter()
            .enumerate()
            .map(|(i, c)| Found {
                handle: i as u32,
                key: c.key.public(),
                label: Text::from_str_truncating(&c.label),
            })
            .collect()
    }

    fn answer(&mut self, part: &Found, c: &Challenge) -> Option<Signature> {
        self.cards.get(part.handle as usize).map(|k| k.key.answer(c))
    }

    fn describe(&mut self, part: &Found) -> Option<PartSchema> {
        let c = self.cards.get(part.handle as usize)?;
        let mut s = PartSchema::new(PartClass::Compute, &c.label, "nvml")
            .made_of(Substrate::Gpu)
            .sense(SenseSpec::new("temp.c", Quantity::Temperature, -40.0, 200.0))
            .sense(SenseSpec::new("util", Quantity::Fraction, 0.0, 1.0))
            .sense(SenseSpec::new("mem.used", Quantity::Bytes, 0.0, 1e13))
            .sense(SenseSpec::new("mem.total", Quantity::Bytes, 0.0, 1e13))
            .sense(SenseSpec::new("power.w", Quantity::Power, 0.0, 2000.0));
        if let Some((slow, shut)) = c.limits {
            s = s.vital(VitalSpec::new("temp.c", VitalKind::Thermal, Band::rising(slow, shut)));
        }
        if c.has_ecc {
            s = s
                .sense(SenseSpec::new("ecc.uncorrected", Quantity::Count, 0.0, 1e18))
                .vital(VitalSpec::new("ecc.uncorrected", VitalKind::Faults, Band::rising(1.0, 1.0)));
        }
        Some(s)
    }

    fn read(&mut self, part: &Found, _at: Instant, out: &mut Vec<Sensed>) {
        let Some(c) = self.cards.get(part.handle as usize) else {
            return;
        };
        if let Some(t) = self.lib.temperature_c(c.gpu) {
            out.push(Sensed::new("temp.c", t));
        }
        if let Some((u, _)) = self.lib.utilization(c.gpu) {
            out.push(Sensed::new("util", u));
        }
        if let Some((used, total)) = self.lib.memory(c.gpu) {
            out.push(Sensed::new("mem.used", used));
            out.push(Sensed::new("mem.total", total));
        }
        if let Some(w) = self.lib.power_w(c.gpu) {
            out.push(Sensed::new("power.w", w));
        }
        if c.has_ecc {
            if let Some(n) = self.lib.ecc_uncorrected(c.gpu) {
                out.push(Sensed::new("ecc.uncorrected", n));
            }
        }
    }

    fn apply(&mut self, _part: &Found, _m: &Mandate) -> Result<(), NerveFault> {
        Err(NerveFault::NotSupported)
    }

    fn safe(&mut self) {}
}
