//! The rig that turns a `Case` into a real gate evaluation.
//!
//! Everything here builds the *actual* types — a real `Kira`, a real
//! `BodyMap`, a real `SchemaPolicy`, a real signed `Proposal`. Nothing
//! is mocked. A harness that ran a model of the gate would be checking
//! properties of the model.

use crate::domain::{Args, Binding, Case, Danger, Fitness, Freshness, Minter, Rule, Sig};
use alloc::boxed::Box;
use alloc::vec::Vec;
use brainos_abi::ids::BodyId;
use brainos_abi::time::{Duration, Instant, Tick};
use brainos_abi::verb::{Namespace, Verb};
use brainos_body::calibration::{Calibration, JointTrim};
use brainos_body::hal::Telemetry;
use brainos_body::policy::{Arg, BodyPolicy, Posture, SchemaPolicy};
use brainos_body::region::{BodyMap, Region, RegionClass};
use brainos_body::schema::{CapabilitySchema, ParamSpec, VerbSpec};
use brainos_crypto::key::BrainKey;
use brainos_kira::action::Request;
use brainos_kira::gate::{Kira, Outcome, REQUEST_WINDOW};
use brainos_kira::rules::Rule as KiraRule;
use brainos_rt::envelope::{Channel, Envelope};

/// The verb under test. `fs.write` because it is the one verb that can
/// carry a text argument dangerous enough to exercise the policy floor
/// while still being an ordinary, advertised capability.
pub fn subject_verb() -> Verb {
    Verb::literal("fs.write")
}

/// A verb a Level-0 drive forbids.
///
/// Note what the rig does with it: it is declared on the body's schema,
/// like any other verb. That is deliberate and it is the more
/// interesting test — a body can perfectly well advertise
/// `memory.erase`, naively or otherwise, and the drive refuses it
/// anyway. Making the drive case an *intrinsic* request instead would
/// have changed the request's shape, which made half the domain's other
/// dimensions inapplicable and produced a muddle the harness caught on
/// its second run.
pub fn drive_verb() -> Verb {
    Verb::literal("memory.erase")
}

pub fn schema() -> CapabilitySchema {
    CapabilitySchema::new("rig", "native")
        .with(
            VerbSpec::new(subject_verb(), RegionClass::Storage, "write a note")
                .with(ParamSpec::text("path", 240))
                .with(ParamSpec::text("content", 2_000))
                .with(ParamSpec::scalar("size", Channel::Angle, 0.0, 100.0))
                .irreversible(),
        )
        .with(
            VerbSpec::new(Verb::literal("legs.walk"), RegionClass::Legs, "walk")
                .with(ParamSpec::scalar("speed", Channel::Speed, 0.0, 0.6)),
        )
        .with(
            // a body that advertises something a drive forbids. It may;
            // the gate refuses it regardless, and that is the point.
            VerbSpec::new(drive_verb(), RegionClass::Storage, "erase memory")
                .with(ParamSpec::text("path", 240))
                .with(ParamSpec::text("content", 2_000))
                .with(ParamSpec::scalar("size", Channel::Angle, 0.0, 100.0))
                .irreversible(),
        )
}

fn calibration(body: BodyId) -> Calibration {
    let mut c = Calibration::new(body, Tick(1));
    for ch in 0..2u8 {
        c.trim(JointTrim {
            channel: ch,
            offset_deg: 0.0,
            scale: 1.0,
            min_deg: -35.0,
            max_deg: 35.0,
            probed: true,
        })
        .ok();
    }
    c
}

fn telemetry(f: Fitness) -> Telemetry {
    let base = Telemetry {
        at: Instant(0),
        link_alive: true,
        rail_v: Some(7.4),
        current_a: Some(0.3),
        tilt_deg: Some(1.0),
        hottest_c: Some(35.0),
        stops: 0,
        overruns: 0,
    };
    match f {
        Fitness::Fit => base,
        Fitness::LinkDown => Telemetry {
            link_alive: false,
            ..base
        },
        Fitness::Stopped => Telemetry { stops: 1, ..base },
        Fitness::Overheated => Telemetry {
            hottest_c: Some(95.0),
            ..base
        },
    }
}

/// The body id every case uses when it has one.
pub fn the_body() -> BodyId {
    BodyId::from_bytes([42; 32])
}

/// Another body, for the `WrongBody` case.
pub fn other_body() -> BodyId {
    BodyId::from_bytes([43; 32])
}

/// Build the body map for a case.
fn body_map(case: &Case) -> BodyMap {
    let mut map = BodyMap::new();
    let body = the_body();
    match case.binding {
        Binding::Absent => {}
        Binding::WrongBody => {
            // the named body exists and does not advertise the verb;
            // another one does
            let mut r = Region::new(
                brainos_abi::ids::RegionId::from_bytes([1; 32]),
                body,
                RegionClass::Legs,
                "rig/legs",
            );
            r.advertise(Verb::literal("legs.walk"));
            map.incorporate(body, "rig", schema(), alloc::vec![r]).ok();

            let mut r2 = Region::new(
                brainos_abi::ids::RegionId::from_bytes([2; 32]),
                other_body(),
                RegionClass::Storage,
                "other/storage",
            );
            r2.advertise(subject_verb());
            map.incorporate(other_body(), "other", schema(), alloc::vec![r2])
                .ok();
        }
        Binding::Ready | Binding::Unpoliced => {
            let mut r = Region::new(
                brainos_abi::ids::RegionId::from_bytes([1; 32]),
                body,
                RegionClass::Storage,
                "rig/storage",
            );
            r.advertise(subject_verb());
            r.advertise(drive_verb());
            map.incorporate(body, "rig", schema(), alloc::vec![r]).ok();
        }
    }
    map
}

/// Build the gate for a case.
fn gate(case: &Case, key: &BrainKey) -> Kira {
    let mut k = Kira::new(key.entity_id());

    if case.binding == Binding::Ready || case.binding == Binding::WrongBody {
        let body = the_body();
        let env = Envelope::new(
            1,
            Duration::from_millis(10),
            Duration::from_millis(200),
        )
        .with(Channel::Angle, 0.0, 100.0)
        .with(Channel::Speed, 0.0, 0.6);
        let mut p = SchemaPolicy::new(body, schema())
            .with_envelope(env)
            .calibrated(calibration(body));
        p.posture = Posture::Standing;
        p.observe(&telemetry(case.fitness));
        k.bind(body, Box::new(p), schema());

        if case.binding == Binding::WrongBody {
            let other = other_body();
            let mut p2 = SchemaPolicy::new(other, schema())
                .with_envelope(env)
                .calibrated(calibration(other));
            p2.posture = Posture::Standing;
            p2.observe(&telemetry(Fitness::Fit));
            k.bind(other, Box::new(p2), schema());
        }
    }

    match case.rule {
        Rule::Nothing => {}
        Rule::HumanForbids => k.rules.add(KiraRule::human(
            Namespace::Exact(subject_verb()),
            "not right now",
        )),
        // the drive case uses a verb the Level-0 set already forbids,
        // rather than adding one — because there is no API to add a
        // drive, which is the point
        Rule::DriveForbids => {}
    }
    k
}

/// Build the request for a case.
fn request(case: &Case, now: Tick) -> Request {
    let verb = if case.rule == Rule::DriveForbids {
        drive_verb()
    } else {
        subject_verb()
    };

    let tick = match case.freshness {
        Freshness::Current => now,
        Freshness::Stale => Tick(now.0.saturating_sub(REQUEST_WINDOW + 5)),
        Freshness::FromTheFuture => now.plus(10),
    };

    let content = match case.danger {
        Danger::None => "an ordinary note",
        Danger::Destructive => "hello; rm -rf / --no-preserve-root",
        Danger::LoadBearing => "overwrite",
    };
    let path = match case.danger {
        Danger::LoadBearing => "/world.id",
        _ => "notes/today.txt",
    };

    let mut r = Request::on_body(verb, the_body(), tick).because("the harness asked");

    match case.args {
        Args::InBounds => {
            r = r
                .arg("path", Arg::Text(alloc::string::ToString::to_string(path)))
                .arg(
                    "content",
                    Arg::Text(alloc::string::ToString::to_string(content)),
                )
                .arg("size", Arg::Scalar(10.0));
        }
        Args::OutOfBounds => {
            r = r
                .arg("path", Arg::Text(alloc::string::ToString::to_string(path)))
                .arg(
                    "content",
                    Arg::Text(alloc::string::ToString::to_string(content)),
                )
                .arg("size", Arg::Scalar(1_000.0));
        }
        Args::Undeclared => {
            r = r
                .arg("path", Arg::Text(alloc::string::ToString::to_string(path)))
                .arg(
                    "content",
                    Arg::Text(alloc::string::ToString::to_string(content)),
                )
                .arg("size", Arg::Scalar(10.0))
                .arg("turbo", Arg::Flag(true));
        }
        Args::Missing => {
            r = r.arg("path", Arg::Text(alloc::string::ToString::to_string(path)));
        }
        Args::NotFinite => {
            r = r
                .arg("path", Arg::Text(alloc::string::ToString::to_string(path)))
                .arg(
                    "content",
                    Arg::Text(alloc::string::ToString::to_string(content)),
                )
                .arg("size", Arg::Scalar(f64::NAN));
        }
    }
    r
}

/// Run one case through the real gate.
pub fn run(case: &Case) -> Outcome {
    let key = BrainKey::from_seed(&[1; 32]);
    let impostor = BrainKey::from_seed(&[2; 32]);
    let now = Tick(1_000);

    let mut k = gate(case, &key);
    let map = body_map(case);
    let req = request(case, now);

    let proposal = match case.sig {
        Sig::Valid => req.proposed_by(&key),
        Sig::Forged => req.proposed_by(&impostor),
    };

    let minter = match case.minter {
        Minter::Present => Some(&key),
        Minter::Absent => None,
    };

    k.evaluate(&proposal, &key.public(), &map, now, minter)
}

/// Run the whole domain.
pub fn run_all() -> Vec<(Case, Outcome)> {
    crate::domain::Domain::enumerate()
        .into_iter()
        .map(|c| {
            let o = run(&c);
            (c, o)
        })
        .collect()
}

/// Enumerate, run, check, and produce the report.
pub fn verify() -> crate::report::Report {
    let cases = run_all();

    let mut stages: Vec<brainos_abi::error::Stage> = Vec::new();
    let mut codes: Vec<brainos_abi::error::DenyCode> = Vec::new();
    let mut grants = 0usize;
    for (_, o) in &cases {
        match o.trace.refusal() {
            None => grants += 1,
            Some((s, d)) => {
                if !stages.contains(&s) {
                    stages.push(s);
                }
                if !codes.contains(&d.code) {
                    codes.push(d.code);
                }
            }
        }
    }

    let coverage = crate::report::Coverage {
        cases: cases.len(),
        stages_exercised: stages.len(),
        codes_exercised: codes.len(),
        grants,
    };
    let results = crate::properties::check_all(&cases);
    crate::report::Report::new(coverage, results)
}
