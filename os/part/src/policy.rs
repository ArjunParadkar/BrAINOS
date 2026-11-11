//! BFS  LEVEL    Parts and nerves - every piece of hardware as a body part  [crate brainos-part]
//! BFS  PURPOSE  PartPolicy: KIRA stages 4-6 for a part, refusing verbs that would break another part's vital or overspend a budget
//! BFS  DEFINES  Guard, Meter, PartPolicy
//! BFS  USES     this crate: class, schema | brainos: abi, body, cognition
//! BFS  USED BY  part/lib
//!
//! The policy bound to a part — and the vitals invariant.
//!
//! Every body in BrAInOS supplies KIRA's stages 4, 5 and 6 through a
//! `BodyPolicy`. `PartPolicy` is that for a part. It does three things,
//! one per stage:
//!
//! | stage | what it checks | refusal code |
//! |---|---|---|
//! | 4 validate | arguments against the part's own schema (delegated to `SchemaPolicy`) | `OutOfBounds`, `Malformed`, ... |
//! | 5 simulate | **would this verb break another part's vital?** would it overspend a finite budget? | `PredictedInvariantBreak`, `Unpredictable`, `IrreversibleBeyondBudget` |
//! | 6 policy | is this part itself fit to be acted on right now? | `BodyInvariant` |
//!
//! The middle row is the reason this type exists. The canonical case is
//! *set the fan to 0 % while the GPU is over its thermal limit*. The
//! fan is perfectly capable of stopping, the argument is inside its
//! declared range, and stopping it would cook a different part. That is
//! exactly the shape of the existing "the body refuses a fall" test — a
//! well-formed request whose predicted consequence breaks an invariant
//! — so it is refused at the same stage, with the same code.
//!
//! How the fan's policy knows about the GPU: `BodyPolicy::feel`. Every
//! bound policy hears every part's readings. This one keeps the latest
//! value of exactly the senses it was told to guard, and nothing else.
//! That is a view of right now, like `Telemetry` — not a history, and
//! not memory.
//!
//! **Not knowing is a refusal.** If a guard's sense has never been
//! heard, or the last reading is stale, the prediction cannot be made
//! and the verb is refused as `Unpredictable`. "The GPU's temperature
//! is unknown" must never be read as "the GPU is cool".

use crate::schema::PartSchema;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use brainos_abi::error::DenyCode;
use brainos_abi::ids::BodyId;
use brainos_abi::text::Text;
use brainos_abi::time::{Duration, Instant, Tick};
use brainos_abi::verb::Verb;
use brainos_body::calibration::Calibration;
use brainos_body::hal::Telemetry;
use brainos_body::policy::{Args, BodyPolicy, Refusal, SchemaPolicy};
use brainos_body::vital::{Felt, Health, SENSE_NAME_MAX};

/// A cross-part rule: what one of this part's verbs must not do to
/// another part.
///
/// Guards are not in the part's schema, because a part does not know
/// what it is next to: a fan does not know which processor it cools.
/// Whoever wires the machine says so, at incorporation.
#[derive(Clone, Debug)]
pub enum Guard {
    /// While the protected sense reads at or above `engage`, this verb's
    /// parameter must be at least `floor`.
    ///
    /// *The fan must stay at or above 60 % while the GPU is at or above
    /// 85 °C.*
    Sustain {
        verb: Verb,
        param: String,
        protects: BodyId,
        sense: Text<SENSE_NAME_MAX>,
        engage: f64,
        floor: f64,
    },
    /// While the protected sense reads at or above `engage`, this verb
    /// may not be used at all.
    ///
    /// *The rail may not be switched off while the disk is writing.*
    Forbid {
        verb: Verb,
        protects: BodyId,
        sense: Text<SENSE_NAME_MAX>,
        engage: f64,
    },
}

impl Guard {
    pub fn sustain(
        verb: Verb,
        param: &str,
        protects: BodyId,
        sense: &str,
        engage: f64,
        floor: f64,
    ) -> Guard {
        Guard::Sustain {
            verb,
            param: alloc::string::ToString::to_string(param),
            protects,
            sense: Text::from_str_truncating(sense),
            engage,
            floor,
        }
    }

    pub fn forbid(verb: Verb, protects: BodyId, sense: &str, engage: f64) -> Guard {
        Guard::Forbid {
            verb,
            protects,
            sense: Text::from_str_truncating(sense),
            engage,
        }
    }

    fn verb(&self) -> &Verb {
        match self {
            Guard::Sustain { verb, .. } | Guard::Forbid { verb, .. } => verb,
        }
    }

    fn watches(&self) -> (&BodyId, &str) {
        match self {
            Guard::Sustain { protects, sense, .. } | Guard::Forbid { protects, sense, .. } => {
                (protects, sense.as_str())
            }
        }
    }
}

/// A verb that spends something finite on this part.
///
/// *Loading a model spends write cycles.* The part reports what is left
/// as one of its own senses; the verb says how much it will spend,
/// either as a fixed amount or as one of its arguments. A request that
/// would take the remainder below `reserve` is refused at simulate as
/// `IrreversibleBeyondBudget` — it is a prediction about an effect that
/// cannot be undone, which is what that code is for.
#[derive(Clone, Debug)]
pub struct Meter {
    pub verb: Verb,
    /// this part's own sense that reports what remains
    pub remaining: Text<SENSE_NAME_MAX>,
    /// the argument that states the cost, if the cost varies
    pub cost_param: Option<String>,
    /// the cost when no argument states it
    pub fixed_cost: f64,
    /// never spend below this
    pub reserve: f64,
}

impl Meter {
    pub fn by_arg(verb: Verb, remaining: &str, cost_param: &str, reserve: f64) -> Meter {
        Meter {
            verb,
            remaining: Text::from_str_truncating(remaining),
            cost_param: Some(alloc::string::ToString::to_string(cost_param)),
            fixed_cost: 0.0,
            reserve,
        }
    }

    pub fn fixed(verb: Verb, remaining: &str, cost: f64, reserve: f64) -> Meter {
        Meter {
            verb,
            remaining: Text::from_str_truncating(remaining),
            cost_param: None,
            fixed_cost: cost,
            reserve,
        }
    }
}

/// The policy for one part.
pub struct PartPolicy {
    part: BodyId,
    schema: PartSchema,
    /// argument validation is the existing schema policy's job
    floor: SchemaPolicy,
    guards: Vec<Guard>,
    meters: Vec<Meter>,
    /// verbs that make a critical part *better*, and so stay allowed
    /// when the part is critical (raising a fan, shedding load)
    relief: Vec<Verb>,
    /// the latest reading of each sense this policy cares about: its
    /// own vitals and whatever its guards watch. Bounded by the number
    /// of those, which is fixed at construction. Never a history.
    latest: Vec<Felt>,
    /// the newest time this policy has heard about, from any part
    now: Instant,
    /// a reading older than this is not a reading
    max_age: Duration,
}

impl PartPolicy {
    pub fn new(part: BodyId, schema: PartSchema) -> PartPolicy {
        let lowered = schema.lowered();
        PartPolicy {
            part,
            floor: SchemaPolicy::new(part, lowered).calibrated(Calibration::new(part, Tick(0))),
            schema,
            guards: Vec::new(),
            meters: Vec::new(),
            relief: Vec::new(),
            latest: Vec::new(),
            now: Instant::ZERO,
            max_age: Duration::from_secs(5),
        }
    }

    pub fn guarding(mut self, g: Guard) -> PartPolicy {
        self.guards.push(g);
        self
    }

    pub fn metering(mut self, m: Meter) -> PartPolicy {
        self.meters.push(m);
        self
    }

    /// Mark a verb as one that relieves this part, so it stays allowed
    /// when the part is critical.
    pub fn relieved_by(mut self, verb: Verb) -> PartPolicy {
        self.relief.push(verb);
        self
    }

    pub fn with_max_age(mut self, d: Duration) -> PartPolicy {
        self.max_age = d;
        self
    }

    pub fn boxed(self) -> Box<dyn BodyPolicy> {
        Box::new(self)
    }

    fn cares_about(&self, part: &BodyId, sense: &str) -> bool {
        (*part == self.part && self.schema.vital_named(sense).is_some())
            || (*part == self.part
                && self.meters.iter().any(|m| m.remaining.as_str() == sense))
            || self.guards.iter().any(|g| {
                let (p, s) = g.watches();
                p == part && s == sense
            })
    }

    fn heard(&self, part: &BodyId, sense: &str) -> Option<&Felt> {
        self.latest.iter().find(|f| f.is(part, sense))
    }

    /// The latest reading, if there is one and it can be trusted.
    fn trusted(&self, part: &BodyId, sense: &str) -> Option<f64> {
        let f = self.heard(part, sense)?;
        let age = self.now.checked_since(f.at).unwrap_or(Duration::ZERO);
        if f.fresh && f.value.is_finite() && age <= self.max_age {
            Some(f.value)
        } else {
            None
        }
    }

    /// The worst of this part's own vitals, right now. A vital that has
    /// never been heard, or has gone stale, is `Failed`: a part that
    /// has stopped reporting a vital sign is not known to be well.
    pub fn health(&self) -> Health {
        let mut worst = Health::Nominal;
        for v in self.schema.vitals() {
            let h = match self.trusted(&self.part, v.sense.as_str()) {
                Some(value) => self
                    .schema
                    .judge(v.sense.as_str(), value)
                    .unwrap_or(Health::Failed),
                None => Health::Failed,
            };
            worst = worst.max(h);
        }
        worst
    }

    fn arg(args: &Args, name: &str) -> Option<f64> {
        args.iter()
            .find(|(n, _)| n == name)
            .and_then(|(_, a)| a.as_scalar())
    }
}

impl BodyPolicy for PartPolicy {
    fn body(&self) -> BodyId {
        self.part
    }

    fn describe(&self) -> String {
        let mut s = self.schema.describe();
        s.push_str(", ");
        s.push_str(self.health().name());
        s
    }

    fn validate(&self, verb: &Verb, args: &Args) -> Result<(), Refusal> {
        self.floor.validate(verb, args)
    }

    fn simulate(&self, verb: &Verb, args: &Args) -> Result<String, Refusal> {
        // --- would this break another part's vital? ---
        for g in self.guards.iter().filter(|g| g.verb() == verb) {
            let (protects, sense) = g.watches();
            let Some(reading) = self.trusted(protects, sense) else {
                return Err(Refusal::new(
                    DenyCode::Unpredictable,
                    "a part this verb could harm has not reported recently",
                ));
            };
            match g {
                Guard::Sustain {
                    param,
                    engage,
                    floor,
                    ..
                } => {
                    if reading >= *engage {
                        let Some(asked) = Self::arg(args, param) else {
                            return Err(Refusal::new(
                                DenyCode::Unpredictable,
                                "the guarded parameter was not supplied as a number",
                            ));
                        };
                        if asked < *floor {
                            return Err(Refusal::new(
                                DenyCode::PredictedInvariantBreak,
                                "this would stop protecting a part that is past its limit",
                            ));
                        }
                    }
                }
                Guard::Forbid { engage, .. } => {
                    if reading >= *engage {
                        return Err(Refusal::new(
                            DenyCode::PredictedInvariantBreak,
                            "another part is in a state this verb would harm",
                        ));
                    }
                }
            }
        }

        // --- would this overspend something finite? ---
        for m in self.meters.iter().filter(|m| &m.verb == verb) {
            let Some(left) = self.trusted(&self.part, m.remaining.as_str()) else {
                return Err(Refusal::new(
                    DenyCode::Unpredictable,
                    "what remains of this part's budget is not known",
                ));
            };
            let cost = match &m.cost_param {
                Some(p) => match Self::arg(args, p) {
                    Some(c) => c,
                    None => {
                        return Err(Refusal::new(
                            DenyCode::Unpredictable,
                            "the cost of this request was not stated",
                        ))
                    }
                },
                None => m.fixed_cost,
            };
            if !(cost.is_finite() && cost >= 0.0) || left - cost < m.reserve {
                return Err(Refusal::new(
                    DenyCode::IrreversibleBeyondBudget,
                    "this would spend more of the part's lifetime than remains",
                ));
            }
        }

        Ok(alloc::string::ToString::to_string(
            "no other part's vital is predicted to break",
        ))
    }

    fn policy(&self, verb: &Verb, _args: &Args) -> Result<(), Refusal> {
        match self.health() {
            Health::Nominal | Health::Degraded => Ok(()),
            Health::Critical if self.relief.contains(verb) => Ok(()),
            Health::Critical => Err(Refusal::new(
                DenyCode::BodyInvariant,
                "this part is past a critical limit",
            )),
            Health::Failed => Err(Refusal::new(
                DenyCode::BodyInvariant,
                "this part has failed or stopped reporting",
            )),
        }
    }

    fn observe(&mut self, t: &Telemetry) {
        self.floor.observe(t);
    }

    fn feel(&mut self, f: &Felt) {
        // time moves forward for every reading, whoever it is about:
        // that is what lets a guarded reading go stale
        if f.at > self.now {
            self.now = f.at;
        }
        if !self.cares_about(&f.part, f.sense.as_str()) {
            return;
        }
        match self
            .latest
            .iter_mut()
            .find(|x| x.is(&f.part, f.sense.as_str()))
        {
            Some(slot) => *slot = *f,
            None => self.latest.push(*f),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::class::PartClass;
    use crate::schema::{Quantity, SenseSpec, VitalKind, VitalSpec};
    use brainos_body::policy::Arg;
    use brainos_body::region::RegionClass;
    use brainos_body::schema::{ParamSpec, VerbSpec};
    use brainos_body::vital::Band;

    fn v(s: &str) -> Verb {
        Verb::parse(s).unwrap()
    }
    fn id(b: u8) -> BodyId {
        BodyId::from_bytes([b; 32])
    }
    const FAN: u8 = 1;
    const GPU: u8 = 2;

    fn fan_schema() -> PartSchema {
        PartSchema::new(PartClass::Thermal, "fan", "test")
            .verb(
                VerbSpec::new(v("fan.set"), RegionClass::Effector, "set duty")
                    .with(ParamSpec::count("duty_pct", 0, 100)),
            )
            .sense(SenseSpec::new("fan.rpm", Quantity::Rpm, 0.0, 9000.0))
            .vital(VitalSpec::new("fan.rpm", VitalKind::Faults, Band::falling(-1.0, -2.0)))
    }

    fn fan_policy() -> PartPolicy {
        PartPolicy::new(id(FAN), fan_schema()).guarding(Guard::sustain(
            v("fan.set"),
            "duty_pct",
            id(GPU),
            "temp.c",
            85.0,
            60.0,
        ))
    }

    fn duty(n: i64) -> Args {
        vec![("duty_pct".to_string(), Arg::Count(n))]
    }

    fn hear(p: &mut PartPolicy, gpu_c: f64, at: u64) {
        p.feel(&Felt::new(id(FAN), "fan.rpm", 2000.0, Instant(at)));
        p.feel(&Felt::new(id(GPU), "temp.c", gpu_c, Instant(at)));
    }

    #[test]
    fn fan_to_zero_while_the_gpu_is_hot_is_refused_at_simulate() {
        let mut p = fan_policy();
        hear(&mut p, 92.0, 1_000);
        assert_eq!(p.validate(&v("fan.set"), &duty(0)), Ok(()));
        let r = p.simulate(&v("fan.set"), &duty(0)).unwrap_err();
        assert_eq!(r.code, DenyCode::PredictedInvariantBreak);
    }

    #[test]
    fn the_same_request_is_fine_when_the_gpu_is_cool() {
        let mut p = fan_policy();
        hear(&mut p, 45.0, 1_000);
        assert!(p.simulate(&v("fan.set"), &duty(0)).is_ok());
        assert_eq!(p.policy(&v("fan.set"), &duty(0)), Ok(()));
    }

    #[test]
    fn at_or_above_the_floor_is_allowed_even_when_hot() {
        let mut p = fan_policy();
        hear(&mut p, 92.0, 1_000);
        assert!(p.simulate(&v("fan.set"), &duty(60)).is_ok());
        assert!(p.simulate(&v("fan.set"), &duty(100)).is_ok());
        assert!(p.simulate(&v("fan.set"), &duty(59)).is_err());
    }

    #[test]
    fn the_threshold_itself_engages_the_guard() {
        let mut p = fan_policy();
        hear(&mut p, 85.0, 1_000);
        assert!(p.simulate(&v("fan.set"), &duty(0)).is_err());
        hear(&mut p, 84.9, 2_000);
        assert!(p.simulate(&v("fan.set"), &duty(0)).is_ok());
    }

    #[test]
    fn a_gpu_that_has_never_reported_is_not_assumed_cool() {
        let mut p = fan_policy();
        p.feel(&Felt::new(id(FAN), "fan.rpm", 2000.0, Instant(1_000)));
        let r = p.simulate(&v("fan.set"), &duty(0)).unwrap_err();
        assert_eq!(r.code, DenyCode::Unpredictable);
    }

    #[test]
    fn a_stale_or_old_or_nonsense_gpu_reading_is_not_trusted() {
        let mut p = fan_policy();
        hear(&mut p, 45.0, 1_000);
        // marked stale by the nerve
        p.feel(&Felt::new(id(GPU), "temp.c", 45.0, Instant(2_000)).stale());
        assert_eq!(
            p.simulate(&v("fan.set"), &duty(0)).unwrap_err().code,
            DenyCode::Unpredictable
        );
        // fresh again, then time passes with only the fan reporting
        hear(&mut p, 45.0, 3_000);
        assert!(p.simulate(&v("fan.set"), &duty(0)).is_ok());
        p.feel(&Felt::new(id(FAN), "fan.rpm", 2000.0, Instant(3_000 + 6_000_000)));
        assert_eq!(
            p.simulate(&v("fan.set"), &duty(0)).unwrap_err().code,
            DenyCode::Unpredictable
        );
        // not a number
        p.feel(&Felt::new(id(GPU), "temp.c", f64::NAN, Instant(3_000 + 6_000_001)));
        assert!(p.simulate(&v("fan.set"), &duty(0)).is_err());
    }

    #[test]
    fn the_policy_keeps_only_the_senses_it_guards_and_only_their_latest_value() {
        let mut p = fan_policy();
        for i in 0..1_000u64 {
            hear(&mut p, 40.0 + (i % 7) as f64, 1_000 + i);
            p.feel(&Felt::new(id(9), "unrelated", i as f64, Instant(1_000 + i)));
            p.feel(&Felt::new(id(GPU), "util", 0.5, Instant(1_000 + i)));
        }
        assert_eq!(p.latest.len(), 2, "the policy is accumulating readings");
    }

    #[test]
    fn a_forbid_guard_blocks_the_verb_outright() {
        let rail = PartSchema::new(PartClass::Power, "rail", "test")
            .verb(VerbSpec::new(v("rail.off"), RegionClass::Effector, "switch off").irreversible())
            .sense(SenseSpec::new("volts", Quantity::Voltage, 0.0, 30.0))
            .vital(VitalSpec::new("volts", VitalKind::Charge, Band::falling(10.0, 9.0)));
        let mut p = PartPolicy::new(id(3), rail).guarding(Guard::forbid(
            v("rail.off"),
            id(4),
            "writing",
            1.0,
        ));
        p.feel(&Felt::new(id(3), "volts", 12.0, Instant(10)));
        p.feel(&Felt::new(id(4), "writing", 1.0, Instant(10)));
        assert_eq!(
            p.simulate(&v("rail.off"), &Vec::new()).unwrap_err().code,
            DenyCode::PredictedInvariantBreak
        );
        p.feel(&Felt::new(id(4), "writing", 0.0, Instant(20)));
        assert!(p.simulate(&v("rail.off"), &Vec::new()).is_ok());
    }

    fn flash() -> PartPolicy {
        let s = PartSchema::new(PartClass::Storage, "flash", "test")
            .verb(
                VerbSpec::new(v("flash.write"), RegionClass::Storage, "write blocks")
                    .with(ParamSpec::count("blocks", 1, 1_000_000))
                    .irreversible(),
            )
            .sense(SenseSpec::new("writes.left", Quantity::Count, 0.0, 1e12))
            .vital(VitalSpec::new("writes.left", VitalKind::Budget, Band::falling(1_000.0, 100.0)));
        PartPolicy::new(id(5), s).metering(Meter::by_arg(v("flash.write"), "writes.left", "blocks", 100.0))
    }

    #[test]
    fn a_write_that_would_overspend_the_budget_is_refused_as_irreversible() {
        let mut p = flash();
        p.feel(&Felt::new(id(5), "writes.left", 5_000.0, Instant(10)));
        let ok = vec![("blocks".to_string(), Arg::Count(4_900))];
        let over = vec![("blocks".to_string(), Arg::Count(4_901))];
        assert!(p.simulate(&v("flash.write"), &ok).is_ok());
        assert_eq!(
            p.simulate(&v("flash.write"), &over).unwrap_err().code,
            DenyCode::IrreversibleBeyondBudget
        );
    }

    #[test]
    fn an_unknown_budget_is_not_assumed_to_be_enough() {
        let p = flash();
        let ask = vec![("blocks".to_string(), Arg::Count(1))];
        assert_eq!(
            p.simulate(&v("flash.write"), &ask).unwrap_err().code,
            DenyCode::Unpredictable
        );
    }

    #[test]
    fn a_critical_part_refuses_everything_except_what_relieves_it() {
        let cpu = PartSchema::new(PartClass::Compute, "cpu", "test")
            .made_of(brainos_cognition::placement::Substrate::Cpu)
            .verb(
                VerbSpec::new(v("cpu.cap"), RegionClass::Compute, "cap frequency")
                    .with(ParamSpec::count("pct", 10, 100)),
            )
            .verb(
                VerbSpec::new(v("cpu.boost"), RegionClass::Compute, "boost")
                    .with(ParamSpec::count("pct", 100, 130)),
            )
            .sense(SenseSpec::new("temp.c", Quantity::Temperature, -20.0, 130.0))
            .vital(VitalSpec::new("temp.c", VitalKind::Thermal, Band::rising(85.0, 95.0)));
        let mut p = PartPolicy::new(id(6), cpu).relieved_by(v("cpu.cap"));
        let a = vec![("pct".to_string(), Arg::Count(100))];

        p.feel(&Felt::new(id(6), "temp.c", 97.0, Instant(10)));
        assert_eq!(p.health(), Health::Critical);
        assert_eq!(p.policy(&v("cpu.cap"), &a), Ok(()));
        assert_eq!(
            p.policy(&v("cpu.boost"), &a).unwrap_err().code,
            DenyCode::BodyInvariant
        );

        p.feel(&Felt::new(id(6), "temp.c", 500.0, Instant(20)));
        assert_eq!(p.health(), Health::Failed);
        assert!(p.policy(&v("cpu.cap"), &a).is_err(), "a failed part acts on nothing");
    }

    #[test]
    fn a_part_that_has_never_reported_its_vitals_is_not_fit() {
        let p = fan_policy();
        assert_eq!(p.health(), Health::Failed);
        assert!(p.policy(&v("fan.set"), &duty(50)).is_err());
    }

    #[test]
    fn arguments_are_still_checked_against_the_schema_first() {
        let p = fan_policy();
        assert_eq!(
            p.validate(&v("fan.set"), &duty(101)).unwrap_err().code,
            DenyCode::OutOfBounds
        );
        assert!(p.validate(&v("fan.set"), &Vec::new()).is_err());
        assert!(p.validate(&v("fan.explode"), &duty(1)).is_err());
    }
}
