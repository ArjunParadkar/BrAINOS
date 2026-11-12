//! BFS  LEVEL    Parts and nerves - every piece of hardware as a body part  [crate brainos-part]
//! BFS  PURPOSE  PartSchema: what a part declares on acquisition (class, verbs, senses, vitals) and the digest that is its identity
//! BFS  DEFINES  Quantity, SenseSpec, VitalKind, VitalSpec, SchemaFault, PartSchema
//! BFS  USES     this crate: class | brainos: abi, body, cognition, crypto
//! BFS  USED BY  bpu/nerve, entity/entity/parts, host/nerves/hwmon, host/nerves/machine, host/nerves/mod, host/nerves/nvml, host/nerves/packfile, host/nerves/socketcan, +8 more
//!
//! What a part declares about itself.
//!
//! A `CapabilitySchema` says what a body can be *asked to do*. A part
//! has more to say than that:
//!
//! | facet | type | example |
//! |---|---|---|
//! | class | `PartClass` | `Thermal` |
//! | substrate | `Option<Substrate>` | `Gpu`, for Compute parts |
//! | verbs | `CapabilitySchema` | `fan.set(duty_pct 0..100)` |
//! | senses | `SenseSpec` | `fan.rpm`, a rate, plausible 0..8000 |
//! | vitals | `VitalSpec` | `temp.c` is degraded at 85, critical at 95 |
//!
//! A `PartSchema` is all of those. It does not replace the capability
//! schema; it *contains* one, and `lowered()` hands that to §8 unchanged
//! in shape, so the body map and the gate see an ordinary body.
//!
//! **Identity.** `facets()` is a digest of everything here that is not
//! a verb. §8 mixes it into the part's id (`Acquisition::describe_part`),
//! so a part that reconnects claiming a higher temperature limit is a
//! different part.
//!
//! **No limits, no verbs.** `lowered()` drops every verb the part gave
//! no way to bound: a parameter with no declared range (the rule §8
//! already has), and — new for parts — any verb at all on a part that
//! declared no vital. A part that cannot say when it is being harmed
//! cannot be acted on, because the gate would have nothing to refuse
//! on. It stays visible: its senses are still read.

use crate::class::PartClass;
use alloc::string::String;
use alloc::vec::Vec;
use brainos_abi::text::Text;
use brainos_abi::verb::Verb;
use brainos_body::schema::{CapabilitySchema, VerbSpec};
use brainos_body::vital::{Band, Health, SENSE_NAME_MAX};
use brainos_cognition::placement::Substrate;
use brainos_crypto::domain::Tag;
use brainos_crypto::sha256::{Hash, Sha256};

/// What physical quantity a sense reports. Closed list, like
/// `ParamKind`: a quantity nothing can interpret is noise.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Quantity {
    /// degrees Celsius
    Temperature = 0,
    /// 0..1: load, charge, wear used, budget remaining
    Fraction = 1,
    /// revolutions per minute
    Rpm = 2,
    /// watts
    Power = 3,
    /// volts
    Voltage = 4,
    /// amperes
    Current = 5,
    /// bytes
    Bytes = 6,
    /// a counter that only goes up: errors, faults, writes
    Count = 7,
    /// a position or angle in the part's own unit
    Position = 8,
    /// hertz
    Frequency = 9,
    /// 0 or 1
    Flag = 10,
}

impl Quantity {
    pub const ALL: [Quantity; 11] = [
        Quantity::Temperature,
        Quantity::Fraction,
        Quantity::Rpm,
        Quantity::Power,
        Quantity::Voltage,
        Quantity::Current,
        Quantity::Bytes,
        Quantity::Count,
        Quantity::Position,
        Quantity::Frequency,
        Quantity::Flag,
    ];

    pub const fn unit(self) -> &'static str {
        match self {
            Quantity::Temperature => "C",
            Quantity::Fraction => "",
            Quantity::Rpm => "rpm",
            Quantity::Power => "W",
            Quantity::Voltage => "V",
            Quantity::Current => "A",
            Quantity::Bytes => "B",
            Quantity::Count => "",
            Quantity::Position => "",
            Quantity::Frequency => "Hz",
            Quantity::Flag => "",
        }
    }
}

/// One typed channel a part reports.
#[derive(Clone, Copy, Debug)]
pub struct SenseSpec {
    pub name: Text<SENSE_NAME_MAX>,
    pub quantity: Quantity,
    /// the range a real reading can be in. A value outside it is not a
    /// reading, it is a broken sensor, and is treated as one.
    pub plausible_min: f64,
    pub plausible_max: f64,
}

impl SenseSpec {
    pub fn new(name: &str, quantity: Quantity, plausible_min: f64, plausible_max: f64) -> SenseSpec {
        SenseSpec {
            name: Text::from_str_truncating(name),
            quantity,
            plausible_min,
            plausible_max,
        }
    }

    pub fn is_plausible(&self, v: f64) -> bool {
        v.is_finite() && v >= self.plausible_min && v <= self.plausible_max
    }
}

/// What a vital sign is a sign *of*. Used to word what is remembered
/// and to decide which vitals a homeostat may regulate.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum VitalKind {
    /// temperature
    Thermal = 0,
    /// lifetime consumed: SMART wear, write cycles, battery health
    Wear = 1,
    /// error or fault counters: ECC, CAN errors, motor fault codes
    Faults = 2,
    /// stored energy
    Charge = 3,
    /// something finite that verbs spend: a write budget
    Budget = 4,
    /// how busy the part is
    Load = 5,
}

impl VitalKind {
    pub const fn name(self) -> &'static str {
        match self {
            VitalKind::Thermal => "thermal",
            VitalKind::Wear => "wear",
            VitalKind::Faults => "faults",
            VitalKind::Charge => "charge",
            VitalKind::Budget => "budget",
            VitalKind::Load => "load",
        }
    }
}

/// One sense promoted to a vital sign: it has thresholds, and crossing
/// them matters.
#[derive(Clone, Copy, Debug)]
pub struct VitalSpec {
    /// which sense. Must name a declared `SenseSpec`.
    pub sense: Text<SENSE_NAME_MAX>,
    pub kind: VitalKind,
    pub band: Band,
}

impl VitalSpec {
    pub fn new(sense: &str, kind: VitalKind, band: Band) -> VitalSpec {
        VitalSpec {
            sense: Text::from_str_truncating(sense),
            kind,
            band,
        }
    }
}

/// Why a part's description of itself was not accepted.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SchemaFault {
    /// a sensor declared a verb
    SensorWithVerbs,
    /// a verb sits on a region its class cannot have
    VerbOnWrongRegion,
    /// a vital names a sense the part did not declare
    VitalWithoutSense,
    /// a vital's thresholds are not in order, or not numbers
    InsaneBand,
    /// two senses share a name
    DuplicateSense,
    /// a sense's plausible range is empty or not numbers
    ImplausibleRange,
    /// a Compute part did not say what tissue it is, or a non-Compute
    /// part claimed to be one
    SubstrateMismatch,
    /// nothing declared at all: no verbs and no senses
    Empty,
}

/// Everything a part declares on acquisition.
#[derive(Clone, Debug)]
pub struct PartSchema {
    pub class: PartClass,
    /// what tissue, for Compute parts; `None` for everything else
    pub substrate: Option<Substrate>,
    /// the verbs, in the form §8 already understands
    pub capability: CapabilitySchema,
    senses: Vec<SenseSpec>,
    vitals: Vec<VitalSpec>,
}

impl PartSchema {
    pub fn new(class: PartClass, label: &str, nerve: &str) -> PartSchema {
        PartSchema {
            class,
            substrate: None,
            capability: CapabilitySchema::new(label, nerve),
            senses: Vec::new(),
            vitals: Vec::new(),
        }
    }

    pub fn made_of(mut self, substrate: Substrate) -> PartSchema {
        self.substrate = Some(substrate);
        self
    }

    pub fn verb(mut self, spec: VerbSpec) -> PartSchema {
        self.capability.declare(spec);
        self
    }

    /// Declare a sense. Kept sorted by name so the digest does not
    /// depend on the order a nerve discovered things in. A second sense
    /// with the same name is kept, so that `check` can report it.
    pub fn sense(mut self, s: SenseSpec) -> PartSchema {
        self.senses.push(s);
        self.senses.sort_by(|a, b| a.name.as_str().cmp(b.name.as_str()));
        self
    }

    /// Declare a vital. One per sense: a second replaces the first.
    pub fn vital(mut self, v: VitalSpec) -> PartSchema {
        self.vitals.retain(|x| x.sense.as_str() != v.sense.as_str());
        self.vitals.push(v);
        self.vitals.sort_by(|a, b| a.sense.as_str().cmp(b.sense.as_str()));
        self
    }

    pub fn label(&self) -> &str {
        &self.capability.label
    }

    pub fn senses(&self) -> &[SenseSpec] {
        &self.senses
    }

    pub fn vitals(&self) -> &[VitalSpec] {
        &self.vitals
    }

    pub fn sense_named(&self, name: &str) -> Option<&SenseSpec> {
        self.senses.iter().find(|s| s.name.as_str() == name)
    }

    pub fn vital_named(&self, sense: &str) -> Option<&VitalSpec> {
        self.vitals.iter().find(|v| v.sense.as_str() == sense)
    }

    /// Judge one reading of one of this part's senses. `None` when the
    /// sense is not a vital. An implausible value is `Failed`: the
    /// sensor is broken, and a broken sensor on a vital is a failed
    /// vital.
    pub fn judge(&self, sense: &str, value: f64) -> Option<Health> {
        let v = self.vital_named(sense)?;
        if let Some(s) = self.sense_named(sense) {
            if !s.is_plausible(value) {
                return Some(Health::Failed);
            }
        }
        Some(v.band.judge(value))
    }

    /// Is this description internally consistent?
    pub fn check(&self) -> Result<(), SchemaFault> {
        if self.capability.is_empty() && self.senses.is_empty() {
            return Err(SchemaFault::Empty);
        }
        if !self.class.may_act() && !self.capability.is_empty() {
            return Err(SchemaFault::SensorWithVerbs);
        }
        let allowed = self.class.regions();
        if self
            .capability
            .verbs()
            .iter()
            .any(|v| !allowed.contains(&v.region))
        {
            return Err(SchemaFault::VerbOnWrongRegion);
        }
        for w in self.senses.windows(2) {
            if w[0].name.as_str() == w[1].name.as_str() {
                return Err(SchemaFault::DuplicateSense);
            }
        }
        for s in &self.senses {
            if !(s.plausible_min.is_finite()
                && s.plausible_max.is_finite()
                && s.plausible_min <= s.plausible_max)
            {
                return Err(SchemaFault::ImplausibleRange);
            }
        }
        for v in &self.vitals {
            if self.sense_named(v.sense.as_str()).is_none() {
                return Err(SchemaFault::VitalWithoutSense);
            }
            if !v.band.is_sane() {
                return Err(SchemaFault::InsaneBand);
            }
        }
        if self.class.is_placement_target() != self.substrate.is_some() {
            return Err(SchemaFault::SubstrateMismatch);
        }
        Ok(())
    }

    /// The verbs this part can actually be asked to do.
    ///
    /// Every parameter must be bounded (§8's rule), and the part must
    /// have declared at least one vital (the rule for parts). Anything
    /// else is left out — not refused later, *absent*, so the entity
    /// never believes it has a capability it could not safely use.
    pub fn actionable(&self) -> Vec<Verb> {
        if self.vitals.is_empty() {
            return Vec::new();
        }
        self.capability
            .verbs()
            .iter()
            .filter(|v| v.is_actionable())
            .map(|v| v.verb)
            .collect()
    }

    /// The capability schema to hand to §8: the actionable verbs only.
    pub fn lowered(&self) -> CapabilitySchema {
        let keep = self.actionable();
        let mut out = CapabilitySchema::new(&self.capability.label, &self.capability.adapter);
        for v in self.capability.verbs() {
            if keep.contains(&v.verb) {
                out.declare(v.clone());
            }
        }
        out
    }

    /// The digest of everything here that is not a verb. §8 mixes this
    /// into the part's id. Canonical: senses and vitals are sorted, and
    /// floats are hashed by their exact bits.
    pub fn facets(&self) -> Hash {
        let mut h = Sha256::tagged(Tag::Schema);
        h.field(b"facets/v1");
        h.field(&[self.class as u8]);
        h.field(&[self.substrate.map(|s| s as u8 + 1).unwrap_or(0)]);
        h.field(&(self.senses.len() as u64).to_le_bytes());
        for s in &self.senses {
            h.field(s.name.as_str().as_bytes());
            h.field(&[s.quantity as u8]);
            h.field(&s.plausible_min.to_bits().to_le_bytes());
            h.field(&s.plausible_max.to_bits().to_le_bytes());
        }
        h.field(&(self.vitals.len() as u64).to_le_bytes());
        for v in &self.vitals {
            h.field(v.sense.as_str().as_bytes());
            h.field(&[v.kind as u8, v.band.rising_is_bad as u8]);
            h.field(&v.band.warn.to_bits().to_le_bytes());
            h.field(&v.band.critical.to_bits().to_le_bytes());
        }
        h.finish()
    }

    /// One line for `parts` listings and the join record.
    pub fn describe(&self) -> String {
        let mut s = String::new();
        s.push_str(self.label());
        s.push_str(" [");
        s.push_str(self.class.name());
        if let Some(t) = self.substrate {
            s.push('/');
            s.push_str(t.name());
        }
        s.push_str("] ");
        push_n(&mut s, self.senses.len());
        s.push_str(" senses, ");
        push_n(&mut s, self.vitals.len());
        s.push_str(" vitals, ");
        push_n(&mut s, self.actionable().len());
        s.push_str(" verbs");
        s
    }
}

fn push_n(s: &mut String, n: usize) {
    let mut buf = [0u8; 20];
    let mut i = 0;
    let mut n = n;
    if n == 0 {
        s.push('0');
        return;
    }
    while n > 0 {
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        i += 1;
    }
    while i > 0 {
        i -= 1;
        s.push(buf[i] as char);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use brainos_body::region::RegionClass;
    use brainos_body::schema::{ParamKind, ParamSpec};

    fn v(s: &str) -> Verb {
        Verb::parse(s).unwrap()
    }

    fn fan() -> PartSchema {
        PartSchema::new(PartClass::Thermal, "case fan", "test")
            .verb(
                VerbSpec::new(v("fan.set"), RegionClass::Effector, "set fan duty")
                    .with(ParamSpec::count("duty_pct", 0, 100)),
            )
            .sense(SenseSpec::new("fan.rpm", Quantity::Rpm, 0.0, 9000.0))
            .vital(VitalSpec::new("fan.rpm", VitalKind::Faults, Band::falling(300.0, 0.0)))
    }

    #[test]
    fn a_well_formed_part_checks_and_keeps_its_verb() {
        let f = fan();
        assert_eq!(f.check(), Ok(()));
        assert_eq!(f.actionable(), vec![v("fan.set")]);
        assert_eq!(f.lowered().len(), 1);
    }

    #[test]
    fn a_part_that_declares_no_vital_gets_no_verbs_but_keeps_its_senses() {
        let f = PartSchema::new(PartClass::Thermal, "mute fan", "test")
            .verb(
                VerbSpec::new(v("fan.set"), RegionClass::Effector, "set fan duty")
                    .with(ParamSpec::count("duty_pct", 0, 100)),
            )
            .sense(SenseSpec::new("fan.rpm", Quantity::Rpm, 0.0, 9000.0));
        assert_eq!(f.check(), Ok(()));
        assert!(f.actionable().is_empty());
        assert!(f.lowered().is_empty());
        assert_eq!(f.senses().len(), 1);
    }

    #[test]
    fn a_verb_with_an_unbounded_parameter_is_dropped_and_the_rest_kept() {
        let f = fan().verb(
            VerbSpec::new(v("fan.boost"), RegionClass::Effector, "boost")
                .with(ParamSpec::unbounded("amount", ParamKind::Scalar)),
        );
        assert_eq!(f.actionable(), vec![v("fan.set")]);
    }

    #[test]
    fn a_sensor_with_a_verb_is_refused() {
        let s = PartSchema::new(PartClass::Sensor, "imu", "test")
            .verb(VerbSpec::new(v("imu.zero"), RegionClass::Effector, "zero"))
            .sense(SenseSpec::new("tilt", Quantity::Position, -180.0, 180.0));
        assert_eq!(s.check(), Err(SchemaFault::SensorWithVerbs));
    }

    #[test]
    fn a_verb_on_a_region_its_class_cannot_have_is_refused() {
        let s = PartSchema::new(PartClass::Storage, "disk", "test")
            .verb(VerbSpec::new(v("legs.walk"), RegionClass::Legs, "walk"))
            .sense(SenseSpec::new("wear", Quantity::Fraction, 0.0, 1.0));
        assert_eq!(s.check(), Err(SchemaFault::VerbOnWrongRegion));
    }

    #[test]
    fn a_vital_must_name_a_declared_sense_and_have_ordered_thresholds() {
        let no_sense = PartSchema::new(PartClass::Thermal, "x", "t")
            .sense(SenseSpec::new("a", Quantity::Temperature, 0.0, 120.0))
            .vital(VitalSpec::new("b", VitalKind::Thermal, Band::rising(80.0, 95.0)));
        assert_eq!(no_sense.check(), Err(SchemaFault::VitalWithoutSense));

        let insane = PartSchema::new(PartClass::Thermal, "x", "t")
            .sense(SenseSpec::new("a", Quantity::Temperature, 0.0, 120.0))
            .vital(VitalSpec::new("a", VitalKind::Thermal, Band::rising(95.0, 80.0)));
        assert_eq!(insane.check(), Err(SchemaFault::InsaneBand));
    }

    #[test]
    fn duplicate_senses_and_empty_ranges_and_empty_parts_are_refused() {
        let dup = PartSchema::new(PartClass::Sensor, "x", "t")
            .sense(SenseSpec::new("a", Quantity::Count, 0.0, 1.0))
            .sense(SenseSpec::new("a", Quantity::Count, 0.0, 2.0));
        assert_eq!(dup.check(), Err(SchemaFault::DuplicateSense));

        let range = PartSchema::new(PartClass::Sensor, "x", "t")
            .sense(SenseSpec::new("a", Quantity::Count, 5.0, 1.0));
        assert_eq!(range.check(), Err(SchemaFault::ImplausibleRange));

        assert_eq!(
            PartSchema::new(PartClass::Sensor, "x", "t").check(),
            Err(SchemaFault::Empty)
        );
    }

    #[test]
    fn a_compute_part_must_say_what_tissue_it_is_and_nothing_else_may() {
        let cpu = PartSchema::new(PartClass::Compute, "cpu", "t")
            .sense(SenseSpec::new("load", Quantity::Fraction, 0.0, 1.0));
        assert_eq!(cpu.check(), Err(SchemaFault::SubstrateMismatch));
        assert_eq!(cpu.made_of(Substrate::Cpu).check(), Ok(()));
        assert_eq!(
            fan().made_of(Substrate::Gpu).check(),
            Err(SchemaFault::SubstrateMismatch)
        );
    }

    #[test]
    fn facets_do_not_depend_on_declaration_order() {
        let a = PartSchema::new(PartClass::Thermal, "x", "t")
            .sense(SenseSpec::new("a", Quantity::Temperature, 0.0, 120.0))
            .sense(SenseSpec::new("b", Quantity::Rpm, 0.0, 9000.0))
            .vital(VitalSpec::new("a", VitalKind::Thermal, Band::rising(80.0, 95.0)))
            .vital(VitalSpec::new("b", VitalKind::Faults, Band::falling(300.0, 0.0)));
        let b = PartSchema::new(PartClass::Thermal, "renamed", "other nerve")
            .vital(VitalSpec::new("b", VitalKind::Faults, Band::falling(300.0, 0.0)))
            .vital(VitalSpec::new("a", VitalKind::Thermal, Band::rising(80.0, 95.0)))
            .sense(SenseSpec::new("b", Quantity::Rpm, 0.0, 9000.0))
            .sense(SenseSpec::new("a", Quantity::Temperature, 0.0, 120.0));
        assert_eq!(a.facets(), b.facets(), "order or label changed the identity");
    }

    #[test]
    fn every_facet_is_part_of_the_identity() {
        let base = fan();
        let mut seen = vec![base.facets()];
        let variants = [
            // a different threshold
            PartSchema::new(PartClass::Thermal, "case fan", "test")
                .sense(SenseSpec::new("fan.rpm", Quantity::Rpm, 0.0, 9000.0))
                .vital(VitalSpec::new("fan.rpm", VitalKind::Faults, Band::falling(200.0, 0.0))),
            // a different plausible range
            PartSchema::new(PartClass::Thermal, "case fan", "test")
                .sense(SenseSpec::new("fan.rpm", Quantity::Rpm, 0.0, 9001.0))
                .vital(VitalSpec::new("fan.rpm", VitalKind::Faults, Band::falling(300.0, 0.0))),
            // a different class
            PartSchema::new(PartClass::Actuator, "case fan", "test")
                .sense(SenseSpec::new("fan.rpm", Quantity::Rpm, 0.0, 9000.0))
                .vital(VitalSpec::new("fan.rpm", VitalKind::Faults, Band::falling(300.0, 0.0))),
            // an extra sense
            fan().sense(SenseSpec::new("temp.c", Quantity::Temperature, 0.0, 120.0)),
            // a vital of a different kind
            PartSchema::new(PartClass::Thermal, "case fan", "test")
                .sense(SenseSpec::new("fan.rpm", Quantity::Rpm, 0.0, 9000.0))
                .vital(VitalSpec::new("fan.rpm", VitalKind::Load, Band::falling(300.0, 0.0))),
        ];
        for v in variants {
            let f = v.facets();
            assert!(!seen.contains(&f), "two different descriptions share an identity");
            seen.push(f);
        }
    }

    #[test]
    fn an_implausible_reading_of_a_vital_is_a_failed_vital() {
        let p = PartSchema::new(PartClass::Thermal, "probe", "t")
            .sense(SenseSpec::new("temp.c", Quantity::Temperature, -40.0, 150.0))
            .vital(VitalSpec::new("temp.c", VitalKind::Thermal, Band::rising(85.0, 95.0)));
        assert_eq!(p.judge("temp.c", 50.0), Some(Health::Nominal));
        assert_eq!(p.judge("temp.c", 90.0), Some(Health::Degraded));
        assert_eq!(p.judge("temp.c", 96.0), Some(Health::Critical));
        assert_eq!(p.judge("temp.c", 900.0), Some(Health::Failed));
        assert_eq!(p.judge("temp.c", f64::NAN), Some(Health::Failed));
        assert_eq!(p.judge("not.a.vital", 1.0), None);
    }
}
