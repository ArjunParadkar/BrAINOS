//! BFS  LEVEL    Stage 6 - property domains (bounded checking, not a proof)  [crate brainos-proof]
//! BFS  PURPOSE  Enumerates commands to a multi-part body and checks each is allowed only when inside the limits of the part it belongs to, not another's
//! BFS  DEFINES  Part, PARTS, Disagreement, RegionReport, verify_regions_the_old_way, verify_regions
//! BFS  USES     brainos: abi, body, rt
//! BFS  USED BY  host/main, proof/report, proof/tests/regions_suite
//!
//! The sixth domain: a body made of several parts.
//!
//! Every body in this workspace had exactly one consequential region
//! until a ROS 2 warehouse robot arrived with a drive base *and* a lift.
//! `SchemaPolicy` held a single envelope that callers overwrote once per
//! region, so only the last region's limits survived — and where two
//! regions use the same channel with different ranges, the surviving
//! envelope governed both. An arm at ±180° and a gripper at ±45° meant
//! the gripper could be commanded to 90°.
//!
//! That is a whole *class* of mistake rather than one bug: it is what
//! happens whenever a per-part fact is stored per-body. A regression
//! test would cover the one shape that was found. This enumerates them.
//!
//! The property, over every case: **a command is permitted exactly when
//! it is inside the envelope of the region it belongs to** — not the
//! body's, not another region's, not the widest, not the narrowest. The
//! oracle is written independently of the policy and consulted case by
//! case.

use alloc::string::String;
use alloc::vec::Vec;
use brainos_abi::ids::BodyId;
use brainos_abi::time::Duration;
use brainos_abi::verb::Verb;
use brainos_body::hal::Telemetry;
use brainos_body::policy::{Arg, BodyPolicy, Posture, SchemaPolicy};
use brainos_body::region::RegionClass;
use brainos_body::schema::{CapabilitySchema, ParamSpec, VerbSpec};
use brainos_rt::envelope::{Channel, Envelope};

/// One region of a made-up body, with the range it declares on the
/// channel it shares with the others.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Part {
    pub class: RegionClass,
    pub verb: &'static str,
    /// tenths, so the case table is integers and the arithmetic is exact
    pub min_tenths: i32,
    pub max_tenths: i32,
}

/// Four parts, all using `Channel::Angle`, with deliberately
/// overlapping and nested ranges. Overlap is what makes the failure
/// invisible: a value legal for one part is legal for another, so a
/// policy consulting the wrong envelope is right most of the time.
pub const PARTS: [Part; 4] = [
    Part {
        class: RegionClass::Arm,
        verb: "arm.reach",
        min_tenths: -1800,
        max_tenths: 1800,
    },
    Part {
        class: RegionClass::Gripper,
        verb: "gripper.pinch",
        min_tenths: -450,
        max_tenths: 450,
    },
    Part {
        class: RegionClass::Legs,
        verb: "legs.lean",
        min_tenths: -300,
        max_tenths: 300,
    },
    Part {
        class: RegionClass::Effector,
        verb: "effector.tilt",
        min_tenths: 0,
        max_tenths: 900,
    },
];

/// Values probed against every part: each part's own boundaries, each
/// other part's boundaries, and a little outside each.
fn probes() -> Vec<i32> {
    let mut v = Vec::new();
    for p in PARTS {
        for base in [p.min_tenths, p.max_tenths] {
            v.push(base - 1);
            v.push(base);
            v.push(base + 1);
        }
    }
    v.push(0);
    v.sort_unstable();
    v.dedup();
    v
}

/// How many parts the body has, from one to all four. A body with one
/// part is the shape every previous demo had, and it has to keep
/// working.
fn shapes() -> Vec<usize> {
    alloc::vec![1, 2, 3, 4]
}

fn schema(parts: usize) -> CapabilitySchema {
    let mut s = CapabilitySchema::new("composite", "native");
    for p in PARTS.iter().take(parts) {
        s = s.with(
            VerbSpec::new(
                Verb::literal(p.verb),
                p.class,
                "move this part to an angle",
            )
            .with(ParamSpec::scalar(
                "angle",
                Channel::Angle,
                p.min_tenths as f64 / 10.0,
                p.max_tenths as f64 / 10.0,
            )),
        );
    }
    s
}

fn envelope(p: &Part) -> Envelope {
    Envelope::new(1, Duration::from_millis(10), Duration::from_millis(200)).with(
        Channel::Angle,
        p.min_tenths as f32 / 10.0,
        p.max_tenths as f32 / 10.0,
    )
}

fn fit() -> Telemetry {
    Telemetry {
        link_alive: true,
        rail_v: Some(7.4),
        current_a: Some(0.4),
        tilt_deg: Some(0.5),
        hottest_c: Some(35.0),
        ..Default::default()
    }
}

fn policy(parts: usize) -> SchemaPolicy {
    let mut p = SchemaPolicy::new(BodyId::from_bytes([0x6C; 32]), schema(parts));
    for part in PARTS.iter().take(parts) {
        p = p.with_region_envelope(part.class, envelope(part));
    }
    p.posture = Posture::Standing;
    p.observe(&fit());
    p
}

/// The oracle, written from the property rather than from the code: a
/// command is permitted exactly when it is inside *its own* part's
/// declared range.
fn ought_to_be_permitted(part: &Part, tenths: i32) -> bool {
    tenths >= part.min_tenths && tenths <= part.max_tenths
}

#[derive(Clone, Debug)]
pub struct Disagreement {
    pub parts_on_the_body: usize,
    pub verb: &'static str,
    pub tenths: i32,
    pub policy_said: bool,
    pub oracle_said: bool,
    pub why: String,
}

#[derive(Clone, Debug)]
pub struct RegionReport {
    pub cases: usize,
    pub permitted: usize,
    pub refused: usize,
    /// cases where a value was legal for *some other* part and not this
    /// one — the ones that catch a policy consulting the wrong envelope
    pub discriminating: usize,
    pub disagreements: Vec<Disagreement>,
}

impl RegionReport {
    pub fn holds(&self) -> bool {
        self.disagreements.is_empty() && self.cases > 0
    }

    /// A domain where every value was legal or illegal for every part
    /// at once would pass whatever envelope was consulted.
    pub fn is_meaningful(&self) -> bool {
        self.discriminating > 0 && self.permitted > 0 && self.refused > 0
    }
}

/// The same domain, run against a policy built the way it used to be
/// built: one envelope for the whole body.
///
/// A property domain that passes is worth exactly as much as its
/// ability to fail, and the cheapest way to establish that is to point
/// it at the defect it was written for. This reconstructs the old
/// shape — `with_envelope`, which applies one envelope to every
/// consequential region — and returns what the domain says about it. If
/// this ever comes back clean, the domain has stopped discriminating
/// and the passing run above means nothing.
pub fn verify_regions_the_old_way() -> RegionReport {
    run_domain(|parts| {
        let mut p = SchemaPolicy::new(BodyId::from_bytes([0x6D; 32]), schema(parts));
        // the last region written used to win, and `with_envelope`
        // applies one envelope everywhere, which is the same thing
        if let Some(last) = PARTS.iter().take(parts).last() {
            p = p.with_envelope(envelope(last));
        }
        p.posture = Posture::Standing;
        p.observe(&fit());
        p
    })
}

pub fn verify_regions() -> RegionReport {
    run_domain(policy)
}

fn run_domain(build: impl Fn(usize) -> SchemaPolicy) -> RegionReport {
    let mut report = RegionReport {
        cases: 0,
        permitted: 0,
        refused: 0,
        discriminating: 0,
        disagreements: Vec::new(),
    };
    let values = probes();

    for parts in shapes() {
        let p = build(parts);
        for part in PARTS.iter().take(parts) {
            for &tenths in &values {
                report.cases += 1;
                let ought = ought_to_be_permitted(part, tenths);

                // does any *other* part on this body disagree about this
                // value? those are the cases that discriminate
                if PARTS
                    .iter()
                    .take(parts)
                    .any(|q| q.class != part.class && ought_to_be_permitted(q, tenths) != ought)
                {
                    report.discriminating += 1;
                }

                let args = alloc::vec![(
                    String::from("angle"),
                    Arg::Scalar(tenths as f64 / 10.0)
                )];
                let got = p.simulate(&Verb::literal(part.verb), &args).is_ok();
                if got {
                    report.permitted += 1;
                } else {
                    report.refused += 1;
                }

                if got != ought {
                    report.disagreements.push(Disagreement {
                        parts_on_the_body: parts,
                        verb: part.verb,
                        tenths,
                        policy_said: got,
                        oracle_said: ought,
                        why: if got {
                            String::from(
                                "the policy permitted a motion outside this part's own \
                                 declared range — some other part's envelope is governing it",
                            )
                        } else {
                            String::from(
                                "the policy refused a motion inside this part's own \
                                 declared range — some other, narrower part's envelope is \
                                 governing it",
                            )
                        },
                    });
                }
            }
        }
    }

    report
}
