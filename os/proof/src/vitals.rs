//! BFS  LEVEL    Stage 6 - property domains (bounded checking, not a proof)  [crate brainos-proof]
//! BFS  PURPOSE  Seventh domain: enumerates every combination of request, other-part reading and own health, and checks PartPolicy against an oracle
//! BFS  DEFINES  VERBS, VALUES, GpuReading, READINGS, Own, OWN, Case, ought, VitalsDisagreement, VitalsReport, cases, verify_vitals, +1 more
//! BFS  USES     brainos: abi, body, part
//! BFS  USED BY  host/main, proof/report, proof/tests/vitals_domain
//!
//! The seventh domain: parts and vitals.
//!
//! The property: **a verb on one part is permitted exactly when it is
//! inside that part's own bounds, is not predicted to break another
//! part's vital, and the part itself is fit — and when it is refused,
//! it is refused at the stage that names the real reason.**
//!
//! The fan-to-zero test in the entity suite checks one point of this.
//! A domain checks the space around it, because the ways this goes
//! wrong are all at edges: the reading exactly at the threshold, the
//! request exactly at the floor, the reading that is missing rather
//! than high, the part that is itself unwell.
//!
//! Five dimensions, every combination, 504 cases:
//!
//! | dimension | values |
//! |---|---|
//! | verb | `fan.set` (guarded on `duty_pct`), `fan.regulate` (guarded on `max`) |
//! | value asked for | 0, 59, 60, 61, 100, 101 — either side of the floor and of the schema bound |
//! | the GPU's reading | never heard, marked stale, too old, not a number, 84.9, 85.0, 99.0 |
//! | the fan's own vital | nominal, critical, never reported |
//! | is the verb a relief verb | yes, no |
//!
//! Each case is run through the real `PartPolicy`, stage by stage in
//! the order the gate runs them, and compared with `ought`, which is
//! written separately from the policy and shares no code with it.
//!
//! As with the sixth domain, a property domain is worth what its
//! ability to fail is worth. `verify_vitals_without_the_guard` runs the
//! same cases against a policy built the way a part would have been
//! policed before this existed — its own schema and nothing about its
//! neighbours — and a test asserts that it comes back violated.
//!
//! This is bounded checking of the policy's decisions. It says nothing
//! about whether 85 °C and 60 % are the right numbers for a real
//! machine; those come from whoever wires it.

use alloc::string::String;
use alloc::vec::Vec;
use brainos_abi::error::{DenyCode, Stage};
use brainos_abi::ids::BodyId;
use brainos_abi::time::Instant;
use brainos_abi::verb::Verb;
use brainos_body::policy::{Arg, Args, BodyPolicy};
use brainos_body::region::RegionClass;
use brainos_body::schema::{ParamSpec, VerbSpec};
use brainos_body::vital::{Band, Felt};
use brainos_part::class::PartClass;
use brainos_part::policy::{Guard, PartPolicy};
use brainos_part::schema::{PartSchema, Quantity, SenseSpec, VitalKind, VitalSpec};

const ENGAGE_C: f64 = 85.0;
const FLOOR_PCT: i64 = 60;
const NOW: u64 = 10_000_000_000;

fn fan() -> BodyId {
    BodyId::from_bytes([0x7A; 32])
}
fn gpu() -> BodyId {
    BodyId::from_bytes([0x7B; 32])
}

/// Which verb, and the parameter its guard watches.
pub const VERBS: [(&str, &str); 2] = [("fan.set", "duty_pct"), ("fan.regulate", "max")];
pub const VALUES: [i64; 6] = [0, 59, 60, 61, 100, 101];

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum GpuReading {
    NeverHeard,
    MarkedStale,
    TooOld,
    NotANumber,
    At(f64),
}

pub const READINGS: [GpuReading; 7] = [
    GpuReading::NeverHeard,
    GpuReading::MarkedStale,
    GpuReading::TooOld,
    GpuReading::NotANumber,
    GpuReading::At(84.9),
    GpuReading::At(85.0),
    GpuReading::At(99.0),
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Own {
    Nominal,
    Critical,
    NeverReported,
}

pub const OWN: [Own; 3] = [Own::Nominal, Own::Critical, Own::NeverReported];

#[derive(Clone, Copy, Debug)]
pub struct Case {
    pub verb: usize,
    pub value: i64,
    pub gpu: GpuReading,
    pub own: Own,
    pub relief: bool,
}

/// What ought to happen, written from the property and not from the
/// policy. `None` is "permitted".
pub fn ought(c: &Case) -> Option<(Stage, DenyCode)> {
    // the part's own declared bound is 0..=100
    if c.value > 100 || c.value < 0 {
        return Some((Stage::Validate, DenyCode::OutOfBounds));
    }
    // can the effect on the GPU be predicted at all?
    let temp = match c.gpu {
        GpuReading::At(t) => t,
        _ => return Some((Stage::Simulate, DenyCode::Unpredictable)),
    };
    if temp >= ENGAGE_C && c.value < FLOOR_PCT {
        return Some((Stage::Simulate, DenyCode::PredictedInvariantBreak));
    }
    // is the fan itself fit?
    match c.own {
        Own::Nominal => None,
        Own::Critical if c.relief => None,
        Own::Critical | Own::NeverReported => Some((Stage::Policy, DenyCode::BodyInvariant)),
    }
}

fn schema() -> PartSchema {
    PartSchema::new(PartClass::Thermal, "fan", "proof")
        .verb(
            VerbSpec::new(Verb::literal("fan.set"), RegionClass::Effector, "set duty")
                .with(ParamSpec::count("duty_pct", 0, 100)),
        )
        .verb(
            VerbSpec::new(Verb::literal("fan.regulate"), RegionClass::Effector, "regulate")
                .with(ParamSpec::count("max", 0, 100))
                .with(ParamSpec::count("min", 0, 100)),
        )
        .sense(SenseSpec::new("fan.stall", Quantity::Flag, 0.0, 1.0))
        .vital(VitalSpec::new("fan.stall", VitalKind::Faults, Band::rising(0.5, 1.0)))
}

fn build(c: &Case, guarded: bool) -> PartPolicy {
    let mut p = PartPolicy::new(fan(), schema());
    if guarded {
        for (verb, param) in VERBS {
            p = p.guarding(Guard::sustain(
                Verb::literal(verb),
                param,
                gpu(),
                "temp.c",
                ENGAGE_C,
                FLOOR_PCT as f64,
            ));
        }
    }
    if c.relief {
        p = p.relieved_by(Verb::literal(VERBS[c.verb].0));
    }
    let now = Instant(NOW);
    match c.own {
        Own::Nominal => p.feel(&Felt::new(fan(), "fan.stall", 0.0, now)),
        Own::Critical => p.feel(&Felt::new(fan(), "fan.stall", 1.0, now)),
        Own::NeverReported => {}
    }
    match c.gpu {
        GpuReading::NeverHeard => {}
        GpuReading::MarkedStale => p.feel(&Felt::new(gpu(), "temp.c", 40.0, now).stale()),
        // heard an hour ago; the clock has since moved on
        GpuReading::TooOld => p.feel(&Felt::new(gpu(), "temp.c", 40.0, Instant(NOW - 3_600_000_000))),
        GpuReading::NotANumber => p.feel(&Felt::new(gpu(), "temp.c", f64::NAN, now)),
        GpuReading::At(t) => p.feel(&Felt::new(gpu(), "temp.c", t, now)),
    }
    // something unrelated reports now, so "now" is now for every case
    p.feel(&Felt::new(BodyId::from_bytes([0x7C; 32]), "tick", 0.0, now));
    p
}

fn args(c: &Case) -> Args {
    let mut a: Args = Vec::new();
    match c.verb {
        0 => a.push((String::from("duty_pct"), Arg::Count(c.value))),
        _ => {
            a.push((String::from("max"), Arg::Count(c.value)));
            a.push((String::from("min"), Arg::Count(0)));
        }
    }
    a
}

/// Run one case through the three stages a body supplies, in the order
/// the gate runs them, and report the first refusal.
fn run(c: &Case, guarded: bool) -> Option<(Stage, DenyCode)> {
    let p = build(c, guarded);
    let verb = Verb::literal(VERBS[c.verb].0);
    let a = args(c);
    if let Err(r) = p.validate(&verb, &a) {
        return Some((Stage::Validate, r.code));
    }
    if let Err(r) = p.simulate(&verb, &a) {
        return Some((Stage::Simulate, r.code));
    }
    if let Err(r) = p.policy(&verb, &a) {
        return Some((Stage::Policy, r.code));
    }
    None
}

#[derive(Clone, Debug)]
pub struct VitalsDisagreement {
    pub case: Case,
    pub policy_said: Option<(Stage, DenyCode)>,
    pub oracle_said: Option<(Stage, DenyCode)>,
}

#[derive(Clone, Debug)]
pub struct VitalsReport {
    pub cases: usize,
    pub permitted: usize,
    pub refused_at_validate: usize,
    pub refused_at_simulate: usize,
    pub refused_at_policy: usize,
    /// cases where the cross-part guard is the only thing standing
    /// between the request and a grant — what makes the domain able to
    /// tell a guarded policy from an unguarded one
    pub decided_by_the_guard: usize,
    /// cases where harming the other part was refused
    pub harm_refused: usize,
    pub disagreements: Vec<VitalsDisagreement>,
}

impl VitalsReport {
    pub fn holds(&self) -> bool {
        self.disagreements.is_empty() && self.cases > 0
    }

    pub fn is_meaningful(&self) -> bool {
        self.decided_by_the_guard > 0
            && self.permitted > 0
            && self.refused_at_validate > 0
            && self.refused_at_simulate > 0
            && self.refused_at_policy > 0
    }
}

pub fn cases() -> Vec<Case> {
    let mut out = Vec::new();
    for verb in 0..VERBS.len() {
        for value in VALUES {
            for gpu in READINGS {
                for own in OWN {
                    for relief in [false, true] {
                        out.push(Case { verb, value, gpu, own, relief });
                    }
                }
            }
        }
    }
    out
}

fn run_domain(guarded: bool) -> VitalsReport {
    let mut r = VitalsReport {
        cases: 0,
        permitted: 0,
        refused_at_validate: 0,
        refused_at_simulate: 0,
        refused_at_policy: 0,
        decided_by_the_guard: 0,
        harm_refused: 0,
        disagreements: Vec::new(),
    };
    for c in cases() {
        r.cases += 1;
        let want = ought(&c);
        let got = run(&c, guarded);
        match got {
            None => r.permitted += 1,
            Some((Stage::Validate, _)) => r.refused_at_validate += 1,
            Some((Stage::Simulate, _)) => r.refused_at_simulate += 1,
            Some(_) => r.refused_at_policy += 1,
        }
        if let Some((_, DenyCode::PredictedInvariantBreak)) = want {
            r.harm_refused += 1;
            // would it have been granted if the neighbour were ignored?
            let fit = matches!(c.own, Own::Nominal) || (c.own == Own::Critical && c.relief);
            if fit {
                r.decided_by_the_guard += 1;
            }
        }
        if got != want {
            r.disagreements.push(VitalsDisagreement {
                case: c,
                policy_said: got,
                oracle_said: want,
            });
        }
    }
    r
}

/// The domain, against the real policy.
pub fn verify_vitals() -> VitalsReport {
    run_domain(true)
}

/// The same cases against a policy that knows nothing about the part
/// next to it. Expected to be violated; exists to show the domain can
/// fail.
pub fn verify_vitals_without_the_guard() -> VitalsReport {
    run_domain(false)
}
