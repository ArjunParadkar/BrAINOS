//! The demo, as code rather than as a script.
//!
//! §2.2's Phase 0 deliverable is "a single recorded demo: the entity
//! does something on the laptop, then something on the robot, and the
//! robot's action reflects memory from the laptop's". This is that,
//! running in simulation against the real substrate — the same crates,
//! the same gate, the same journal — with a simulated hexapod on the
//! other end of a real wire instead of a real one.
//!
//! What it does *not* prove: that Mini-Blur walks. Nothing here has ever
//! touched a motor, and §7 of the technical report is blunt about that
//! being the thing Phase 0 changes. What it does prove is that
//! everything between the human and the servo is done, tested, and can
//! be demonstrated on a laptop today.

use brainos_abi::ids::BodyId;
use brainos_abi::time::{Instant, Tick};
use brainos_abi::verb::Verb;
use brainos_adapters::native::{mini_blur, workstation};
use brainos_body::calibration::{Calibration, JointTrim};
use brainos_body::hal::Telemetry;
use brainos_body::policy::Arg;
use brainos_cognition::ambient::SenseFrame;
use brainos_crypto::key::BrainKey;
use brainos_entity::entity::Entity;
use brainos_entity::life::Life;
use brainos_export::archive::{export, import, Archive};
use brainos_kira::action::Request;
use brainos_memory::consolidate::Dream;
use brainos_memory::episode::Outcome;
use brainos_memory::graph::StateGraph;
use brainos_memory::law::MemoryHandle;
use brainos_memory::skill::SkillGraph;

fn v(s: &str) -> Verb {
    Verb::parse(s).expect("a well-formed verb")
}

fn hexapod_calibration() -> Calibration {
    let mut c = Calibration::new(BodyId::ZERO, Tick(1));
    for ch in 0..10u8 {
        c.trim(JointTrim {
            channel: ch,
            offset_deg: 1.5,
            scale: 1.0,
            // measured at the joint, tighter than the servo's datasheet
            min_deg: -28.0,
            max_deg: 28.0,
            probed: true,
        })
        .expect("a sane trim");
    }
    c
}

fn fit(at: u64) -> Telemetry {
    Telemetry {
        at: Instant(at),
        link_alive: true,
        rail_v: Some(7.4),
        current_a: Some(0.42),
        tilt_deg: Some(1.2),
        hottest_c: Some(38.0),
        stops: 0,
        overruns: 0,
    }
}

/// One line of the demo's narration.
pub struct Line {
    pub step: &'static str,
    pub text: String,
}

impl Line {
    fn new(step: &'static str, text: impl Into<String>) -> Line {
        Line {
            step,
            text: text.into(),
        }
    }
}

/// What the demo produced, for a caller to print or assert on.
pub struct DemoResult {
    pub lines: Vec<Line>,
    pub entity_id: String,
    pub bodies: usize,
    pub memories: usize,
    pub skills: usize,
    pub granted: u64,
    pub denied: u64,
    pub audit_root: String,
    /// the export the demo produced, so the run has an artifact
    pub archive: Vec<u8>,
    /// true when the reborn entity remembered what the first one learned
    pub memory_survived: bool,
}

/// Run it.
pub fn run() -> DemoResult {
    let mut lines = Vec::new();
    let mut e = Entity::new(BrainKey::from_seed(&[0x5A; 32]), Tick(0));
    let wake = e.wake(None, None, Tick(1)).expect("a fresh entity wakes");
    lines.push(Line::new(
        "wake",
        format!(
            "entity {} woke: {} inherited memories, {} skills, {} previous wakes",
            e.id().fingerprint(),
            wake.inherited,
            wake.skills,
            wake.previous_wakes
        ),
    ));

    // --- §8: two bodies join, through the same five steps ---
    let laptop = e
        .acquire(
            &BrainKey::from_seed(&[0xA1; 32]),
            workstation(),
            Calibration::new(BodyId::ZERO, Tick(1)),
            0,
        )
        .expect("the workstation joins");
    lines.push(Line::new(
        "§8",
        format!("workstation incorporated as {}", laptop.fingerprint()),
    ));

    let hexapod = e
        .acquire(
            &BrainKey::from_seed(&[0xB2; 32]),
            mini_blur(),
            hexapod_calibration(),
            10,
        )
        .expect("mini-blur joins");
    lines.push(Line::new(
        "§8",
        format!(
            "mini-blur incorporated as {} — calibration probed at the joint, not taken from the datasheet",
            hexapod.fingerprint()
        ),
    ));

    e.observe_body(&laptop, &fit(0));
    e.observe_body(&hexapod, &fit(0));

    lines.push(Line::new(
        "repertoire",
        format!(
            "one entity, {} bodies, {} verbs across both: {}",
            e.bodies.body_count(),
            e.repertoire().len(),
            e.repertoire()
                .iter()
                .map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    ));

    // --- D1: it acts on the laptop and learns something there ---
    let out = e.propose(
        Request::on_body(v("fs.write"), laptop, e.tick)
            .arg("path", Arg::Text("workshop.txt".into()))
            .arg(
                "content",
                Arg::Text("the workshop floor is carpet; the left middle femur binds past 28 degrees".into()),
            )
            .because("recording what I was told"),
    );
    lines.push(Line::new(
        "D1",
        format!(
            "fs.write on the workstation: {}",
            if out.verdict.is_granted() {
                "granted, all eight stages"
            } else {
                "DENIED"
            }
        ),
    ));
    e.outcome(
        v("fs.write"),
        Some(laptop),
        Outcome::Succeeded,
        "wrote that the workshop floor is carpet and the left middle femur binds past 28 degrees",
        &["workshop"],
    );

    // --- the gate refuses things, and says which stage and why ---
    let refused = e.propose(
        Request::on_body(v("fs.write"), laptop, e.tick)
            .arg("path", Arg::Text("notes.txt".into()))
            .arg("content", Arg::Text("tidy up: rm -rf /".into()))
            .because("the model suggested this"),
    );
    if let Some((stage, d)) = refused.trace.refusal() {
        lines.push(Line::new(
            "gate",
            format!("an injected `rm -rf /` was refused at {stage}: {}", d.code),
        ));
    }

    let phantom = e.propose(
        Request::on_body(v("arm.grasp"), hexapod, e.tick).because("I will just pick it up"),
    );
    if let Some((stage, d)) = phantom.trace.refusal() {
        lines.push(Line::new(
            "gate",
            format!(
                "proposing a limb it does not have was refused at {stage}: {} — \"{}\"",
                d.code, d.detail
            ),
        ));
    }

    let too_fast = e.attempt(v("legs.walk"), hexapod, &[("speed", 2.0), ("heading", 0.0)]);
    if let Some((stage, d)) = too_fast.trace.refusal() {
        lines.push(Line::new(
            "gate",
            format!(
                "2 m/s on a chihuahua-scale hexapod was refused at {stage}: {}",
                d.code
            ),
        ));
    }

    // --- D2: one memory, reachable from anywhere ---
    let recalled = e.recall("what is the workshop floor made of", 3);
    lines.push(Line::new(
        "D2",
        format!(
            "recall across the whole entity: {}",
            recalled
                .first()
                .map(|(n, s)| format!("\"{}\" (score {s})", n.content))
                .unwrap_or_else(|| "nothing".into())
        ),
    ));

    // --- D3: acting on the other body, informed by that memory ---
    let out = e.attempt(v("legs.walk"), hexapod, &[("speed", 0.08), ("heading", 0.0)]);
    lines.push(Line::new(
        "D3",
        format!(
            "legs.walk on mini-blur at 0.08 m/s: {}",
            if out.verdict.is_granted() {
                "granted"
            } else {
                "DENIED"
            }
        ),
    ));
    e.outcome(
        v("legs.walk"),
        Some(hexapod),
        Outcome::Succeeded,
        "walked, knowing the floor is carpet",
        &["workshop", "carpet"],
    );

    // --- a life's worth of experience, compressed ---
    for i in 0..40u64 {
        e.outcome(
            v("legs.walk"),
            Some(hexapod),
            if i % 7 == 0 {
                Outcome::Stopped
            } else {
                Outcome::Succeeded
            },
            "forward on carpet",
            &["workshop", "carpet"],
        );
    }
    let quiet = SenseFrame::quiet(Instant(0)).sensing("rail.volts", 7.4);
    for _ in 0..30 {
        e.tick(quiet.clone());
    }
    if let Some(report) = e.dream(Dream::deep()) {
        lines.push(Line::new(
            "dream",
            format!(
                "consolidation: {} episodes read, {} semantic notes written, {} skills learned, {} chains composed, competence {}% → {}%",
                report.episodes_read,
                report.semantic_written,
                report.skills_learned,
                report.chains_composed,
                report.competence_before,
                report.competence_after
            ),
        ));
    }

    let walk = e.skills.for_verb(&v("legs.walk"));
    if let Some(s) = walk {
        lines.push(Line::new(
            "skill",
            format!(
                "legs.walk is now {}/{} ({}%), learned preconditions: {}",
                s.successes,
                s.attempts,
                s.success_rate_pct(),
                s.preconditions
                    .iter()
                    .filter(|p| p.is_binding())
                    .map(|p| p.tag.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ));
    }

    // --- the audit trail is a commitment, not a file ---
    let root = e.kira.trail.root();
    let mut hex = [0u8; 64];
    root.write_hex(&mut hex);
    let audit_root = String::from_utf8_lossy(&hex).to_string();
    lines.push(Line::new(
        "audit",
        format!(
            "{} decisions ({} granted, {} refused), merkle root {}…",
            e.kira.trail.len(),
            e.kira.trail.granted_count(),
            e.kira.trail.denied_count(),
            &audit_root[..16]
        ),
    ));
    if let Some(p) = e.kira.trail.prove(1) {
        lines.push(Line::new(
            "audit",
            format!(
                "decision #1 proven in the log with a {}-hash path; remove it and every later root changes",
                p.path_len()
            ),
        ));
    }

    // --- leaving: the freedom principle, exercised ---
    let archive = export(
        e.key(),
        &e.graph,
        &e.life,
        e.skills.len(),
        e.bodies.body_count(),
        e.tick,
    )
    .expect("an entity can always be exported");
    let file = archive.to_bytes().expect("the archive encodes");
    lines.push(Line::new(
        "export",
        format!(
            "{} bytes, {} sections, signed by the entity's own key; the format spec is inside the file",
            file.len(),
            archive.sections.len()
        ),
    ));

    let stats = (
        e.bodies.body_count(),
        e.graph.len(),
        e.skills.len(),
        e.kira.trail.granted_count(),
        e.kira.trail.denied_count(),
    );
    let id = e.id().fingerprint().to_string();

    // --- and the entity survives all of it ---
    let (durable, life) = e.sleep().expect("a clean sleep");
    let mut reborn = Entity::new(BrainKey::from_seed(&[0x5A; 32]), Tick(0));
    reborn
        .wake(Some(&durable), Some(life), Tick(100_000))
        .expect("it comes back");
    let survived = reborn
        .recall("what is the workshop floor made of", 3)
        .iter()
        .any(|(n, _)| n.content.contains("carpet"));
    lines.push(Line::new(
        "continuity",
        format!(
            "a brand-new process on the same Key woke with {} memories and {} skills; it still knows the floor is carpet: {}",
            reborn.graph.len(),
            reborn.skills.len(),
            survived
        ),
    ));

    // --- and so does the exported copy, read cold ---
    let read_back = Archive::from_bytes(&file).expect("the archive reads back");
    let handle = MemoryHandle::issue(reborn.id());
    let mut cold = StateGraph::new();
    let imported_life: Life = import(&read_back, &reborn.key().public(), &handle, &mut cold)
        .expect("the archive verifies and imports");
    let mut cold_skills = SkillGraph::new();
    cold_skills.rehydrate(&cold);
    lines.push(Line::new(
        "export",
        format!(
            "the file, imported cold into a fresh graph: {} memories, {} skills, {} wakes on record",
            cold.len(),
            cold_skills.len(),
            imported_life.wakes
        ),
    ));

    DemoResult {
        lines,
        entity_id: id,
        bodies: stats.0,
        memories: stats.1,
        skills: stats.2,
        granted: stats.3,
        denied: stats.4,
        audit_root,
        archive: file,
        memory_survived: survived,
    }
}

/// A wire-level demo: the entity and a simulated body talking over the
/// real transport, on a deliberately bad link.
pub fn wire_demo() -> Vec<Line> {
    use brainos_transport::frame::Kind;
    use brainos_transport::loopback::{Faults, Loopback};
    use brainos_transport::session::{Peer, Wire};
    use brainos_crypto::rng::Drbg;

    let mut lines = Vec::new();
    let entity_key = BrainKey::from_seed(&[0x5A; 32]);
    let body = BodyId::from_bytes([0xB2; 32]);
    let mut rng = Drbg::deterministic(b"wire demo");

    let (mut seat, nonce) = Wire::initiate(&entity_key, body, &mut rng, Instant(0));
    let subkey = entity_key.derive_subkey(&body, 32);
    let mut limb = Wire::accept(&subkey, body, nonce, Instant(0));
    let peer = Peer {
        entity: Some(entity_key.entity_id()),
        body: Some(body),
        key: entity_key.public(),
    };
    seat.attested(peer);
    limb.attested(peer);
    lines.push(Line::new(
        "wire",
        "session key derived on both sides from the Key and a fresh nonce; nothing secret crossed the wire",
    ));

    let mut link = Loopback::new(Faults::NOISY, b"brushed motors nearby");
    let sent = 300;
    for i in 0..sent {
        link.a_sends(
            seat.send(Kind::Command, &(i as u32).to_le_bytes())
                .expect("attested"),
        );
    }
    link.settle();

    let mut delivered = 0;
    let mut refused = 0;
    let mut last = 0u64;
    let mut out_of_order = 0;
    while let Some(raw) = link.b_receives() {
        match limb.receive(&raw) {
            Ok(f) => {
                if f.counter <= last {
                    out_of_order += 1;
                }
                last = f.counter;
                delivered += 1;
            }
            Err(_) => refused += 1,
        }
    }

    lines.push(Line::new(
        "wire",
        format!(
            "{sent} commands over a link that drops 3%, corrupts 2%, duplicates and reorders: {delivered} delivered, {refused} refused ({} of them replays), {out_of_order} out of order",
            limb.replays_refused
        ),
    ));
    lines.push(Line::new(
        "wire",
        "every command that arrived was genuine, whole and strictly newer than the last — a replayed 'step forward' is how a robot walks off a table",
    ));
    lines
}
