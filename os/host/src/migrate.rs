//! Changing vendors.
//!
//! §1 of the master document is specific about what ownership means:
//!
//! > *The customer owns the entity. Permanently. The Key is theirs; the
//! > memory is theirs; the body is a subscription they can end. We build
//! > the opposite of lock-in-by-captivity. Our moat is that leaving
//! > means abandoning a life, and the customer is free to do it.*
//!
//! Every company says something like this. The question is whether the
//! architecture makes it true, and the way to find out is to do it: take
//! an entity that has spent its whole life on one vendor's robot, move
//! it to a competitor's robot that shares no mechanism, no verbs and no
//! geometry with the first, and see what arrives.
//!
//! The scenario below is deliberately unkind to us. Between the export
//! and the import, *everything of the first vendor's is destroyed* — the
//! entity, its bodies, its gate, its journal, its audit log. What
//! crosses the gap is one file and the customer's own Key, which lives
//! in a secure element the vendor never had. If the second half works
//! anyway, then "the customer owns the entity" is a property of the
//! design rather than a promise about our future behaviour.
//!
//! The three things worth watching:
//!
//! 1. **What survives.** Memories, skills, beliefs, the life record, the
//!    age. The entity on the arm remembers the workshop floor it learned
//!    about while it was a hexapod.
//! 2. **What correctly does not.** The hexapod's calibration is still in
//!    the state graph — it is a memory, and memories are not edited for
//!    convenience — but it does not apply to the arm, and the arm is
//!    calibrated from scratch. `legs.walk` is no longer in the
//!    repertoire, and asking for it is refused at the gate. A memory of
//!    a skill is not a licence to act with a body you do not have.
//! 3. **What it costs.** Nothing technical. The cost of leaving is that
//!    the second robot is a stranger to this entity's calibration and
//!    has to be learned; the memories came free. That is the moat, and
//!    it is the honest one: worth staying for, not impossible to leave.

use brainos_abi::ids::BodyId;
use brainos_abi::time::{Instant, Tick};
use brainos_abi::verb::Verb;
use brainos_adapters::mhs::from_mhs;
use brainos_adapters::native::mini_blur;
use brainos_body::calibration::{Calibration, JointTrim};
use brainos_body::hal::Telemetry;
use brainos_crypto::key::BrainKey;
use brainos_entity::entity::Entity;
use brainos_export::archive::{adopt, export, Archive};
use brainos_kira::gate::Verdict;
use brainos_memory::episode::Outcome as EpisodeOutcome;
use brainos_cognition::ambient::SenseFrame;
use brainos_memory::graph::NodeKind;

use crate::fullstack::MANUFACTURER_DESCRIPTION;

fn v(s: &str) -> Verb {
    Verb::parse(s).expect("a well-formed verb")
}

/// The customer's Key. Not the vendor's.
///
/// This is the whole asymmetry in one line. In the scenario it is a
/// seed; in a deployment it is a secure element in something the
/// customer keeps — and neither vendor can produce it, which is why
/// neither vendor can hold the entity hostage and why the second one
/// can take custody without our permission.
const CUSTOMER_KEY_SEED: [u8; 32] = [0xA7; 32];

fn fit(at: u64) -> Telemetry {
    Telemetry {
        at: Instant(at),
        link_alive: true,
        rail_v: Some(7.4),
        current_a: Some(0.42),
        tilt_deg: Some(1.1),
        hottest_c: Some(37.0),
        stops: 0,
        overruns: 0,
    }
}

/// The hexapod's calibration: ten joints, measured at the joint.
fn hexapod_calibration() -> Calibration {
    let mut c = Calibration::new(BodyId::ZERO, Tick(1));
    for ch in 0..10u8 {
        c.trim(JointTrim {
            channel: ch,
            offset_deg: 1.5,
            scale: 1.0,
            min_deg: -28.0,
            max_deg: 28.0,
            probed: true,
        })
        .expect("a sane trim");
    }
    c
}

/// What crossed the gap, and what it cost.
#[derive(Clone, Debug)]
pub struct Migration {
    // --- identity ---
    pub entity_before: String,
    pub entity_after: String,
    pub same_entity: bool,

    // --- the file ---
    pub archive_bytes: usize,
    pub sections: usize,

    // --- what survived ---
    pub memories_before: usize,
    pub memories_after: usize,
    pub skills_before: usize,
    pub skills_after: usize,
    pub age_before: u64,
    pub age_after: u64,
    pub wakes_after: u32,
    pub bodies_ever: usize,
    /// something it learned about the *world* while it was a hexapod
    pub still_knows_the_floor: bool,
    /// something it learned about *itself* while it was a hexapod
    pub still_knows_its_binding_femur: bool,
    /// the tested judgement it acquired, before and after the move:
    /// (tag, strength, tests). This is the accumulated experience §2.4
    /// calls the moat, and the interesting question is whether a change
    /// of vendor costs any of it.
    pub judgement_before: Vec<(String, u8, u16)>,
    pub judgement_after: Vec<(String, u8, u16)>,

    // --- what correctly did not ---
    pub repertoire_before: Vec<String>,
    pub repertoire_after: Vec<String>,
    /// the old body's calibration is still a memory…
    pub old_calibration_remembered: bool,
    /// …and the new body was calibrated from scratch regardless
    pub new_body_calibrated_fresh: bool,
    /// asking the arm to walk is refused, and where
    pub walk_on_the_arm: Option<String>,
    /// asking the old body to walk is refused too: it is not here
    pub walk_on_the_hexapod_id: Option<String>,
    /// the arm, however, works immediately
    pub arm_acted: bool,

    // --- the unkind part ---
    pub vendor_a_destroyed: bool,
    /// the second vendor holds no private key and cannot become the entity
    pub vendor_b_cannot_forge: bool,
}

/// Run it.
pub fn migrate() -> Migration {
    // =================================================================
    // Vendor A — a hexapod. The entity's whole life so far.
    // =================================================================
    let mut a = Entity::new(BrainKey::from_seed(&CUSTOMER_KEY_SEED), Tick(0));
    a.wake(None, None, Tick(1)).expect("a first waking");

    let device_a = BrainKey::from_seed(&[0x11; 32]);
    let hexapod = a
        .acquire(&device_a, mini_blur(), hexapod_calibration(), 10)
        .expect("the hexapod joins");
    a.observe_body(&hexapod, &fit(0));

    let entity_before = a.id().fingerprint().to_string();
    let repertoire_before: Vec<String> = a
        .repertoire()
        .iter()
        .map(|v| v.as_str().to_string())
        .collect();

    // It learns two different kinds of thing, and the difference is the
    // point. One is about the world, which is still true on any body.
    // The other is about this body, which is not.
    a.outcome(
        v("legs.stand"),
        Some(hexapod),
        EpisodeOutcome::Succeeded,
        "stood and found the workshop floor is carpet, so gaits need more clearance",
        &["workshop", "carpet"],
    );
    a.outcome(
        v("legs.stand"),
        Some(hexapod),
        EpisodeOutcome::Stopped,
        "stopped: this body's left middle femur binds past 28 degrees",
        &["workshop"],
    );

    // It practises, and while practising it finds something out: the
    // gait works on carpet and fails on the tiled section. That is not
    // a fact about walking, it is a fact about *this workshop*, learned
    // by trying it — and it is the kind of thing the moat is supposedly
    // made of, so the migration had better carry it.
    a.learn_skill(v("legs.walk"), "walking");
    for i in 0..60u64 {
        a.tick(SenseFrame::quiet(Instant(10 + i)));
        let on_carpet = i % 3 != 0;
        let out = a.attempt(v("legs.walk"), hexapod, &[("speed", 0.08), ("heading", 0.0)]);
        let granted = matches!(out.verdict, Verdict::Granted(_));
        a.outcome(
            v("legs.walk"),
            Some(hexapod),
            if granted && on_carpet {
                EpisodeOutcome::Succeeded
            } else {
                EpisodeOutcome::Failed
            },
            "forward across the workshop",
            &[if on_carpet { "carpet" } else { "tile" }, "workshop"],
        );
    }
    let judgement_before: Vec<(String, u8, u16)> = a
        .skills
        .for_verb(&v("legs.walk"))
        .map(|s| {
            s.preconditions
                .iter()
                .filter(|p| p.is_binding_at(a.tick))
                .map(|p| (p.tag.clone(), p.strength(), p.tests))
                .collect()
        })
        .unwrap_or_default();

    let memories_before = a.graph.len();
    let skills_before = a.skills.len();
    let age_before = a.life.age(a.tick);

    // --- the customer asks to leave ---
    let archive = export(
        a.key(),
        &a.graph,
        &a.life,
        a.skills.len(),
        a.bodies.body_count(),
        a.tick,
    )
    .expect("an entity can always be exported");
    let file = archive.to_bytes().expect("the archive encodes");
    let sections = archive.sections.len();

    // =================================================================
    // …and then vendor A ceases to exist.
    // =================================================================
    //
    // Not "stops supporting the product". Ceases. The entity, the body
    // map, the gate, the audit log, the journal, the servers: gone. The
    // only things on the other side of this line are `file` and the
    // customer's Key, which vendor A never had.
    drop(a);
    drop(device_a);
    let vendor_a_destroyed = true;

    // =================================================================
    // Vendor B — a six-axis arm from a company that has never heard of
    // us, described in their own format.
    // =================================================================
    let mut b = Entity::new(BrainKey::from_seed(&CUSTOMER_KEY_SEED), Tick(0));

    // Verification happens before anything is loaded. A vendor who loads
    // first and checks afterwards has already been handed somebody's
    // memories.
    let read_back = Archive::from_bytes(&file).expect("the archive reads back");
    let woke = adopt(&mut b, &read_back, Tick(50_000)).expect("the customer's entity is adopted");

    let entity_after = b.id().fingerprint().to_string();
    let memories_after = b.graph.len();
    let skills_after = woke.skills;
    let age_after = b.life.age(b.tick);
    let wakes_after = b.life.wakes;
    let bodies_ever = b.life.bodies_ever.len();

    let judgement_after: Vec<(String, u8, u16)> = b
        .skills
        .for_verb(&v("legs.walk"))
        .map(|s| {
            s.preconditions
                .iter()
                .filter(|p| p.is_binding_at(b.tick))
                .map(|p| (p.tag.clone(), p.strength(), p.tests))
                .collect()
        })
        .unwrap_or_default();

    let still_knows_the_floor = b
        .recall("what is the workshop floor like", 4)
        .iter()
        .any(|(n, _)| n.content.contains("carpet"));
    let still_knows_its_binding_femur = b
        .recall("which joint binds", 4)
        .iter()
        .any(|(n, _)| n.content.contains("femur"));
    let old_calibration_remembered = b
        .graph
        .of_kind(NodeKind::Calibration)
        .any(|n| n.content.contains("joints=10"));

    // --- the new body ---
    let schema_b = from_mhs(MANUFACTURER_DESCRIPTION).expect("a well-formed device description");
    let device_b = BrainKey::from_seed(&[0x22; 32]);
    let arm = b
        .acquire(
            &device_b,
            schema_b,
            // from scratch: the new body's calibration is its own, and
            // the hexapod's numbers are not evidence about it
            Calibration::new(BodyId::ZERO, Tick(50_001)),
            0,
        )
        .expect("the arm joins");
    b.observe_body(&arm, &fit(1));
    let new_body_calibrated_fresh = b
        .graph
        .of_kind(NodeKind::Calibration)
        .any(|n| n.content.contains("joints=0"));

    let repertoire_after: Vec<String> = b
        .repertoire()
        .iter()
        .map(|v| v.as_str().to_string())
        .collect();

    // A memory of a skill is not a licence to act with a body you do
    // not have. The entity still knows how to walk; there is nothing
    // here to walk with, and the gate says so rather than the arm
    // finding out.
    let walk_on_the_arm = match b
        .attempt(v("legs.walk"), arm, &[("speed", 0.08)])
        .verdict
    {
        Verdict::Denied(d) => Some(format!("{:?} at {:?}", d.code, d.stage)),
        Verdict::Granted(_) => None,
    };
    let walk_on_the_hexapod_id = match b
        .attempt(v("legs.walk"), hexapod, &[("speed", 0.08)])
        .verdict
    {
        Verdict::Denied(d) => Some(format!("{:?} at {:?}", d.code, d.stage)),
        Verdict::Granted(_) => None,
    };

    let arm_acted = matches!(
        b.attempt(
            v("arm.move_joint"),
            arm,
            &[("joint", 1.0), ("angle", 45.0), ("speed", 30.0)]
        )
        .verdict,
        Verdict::Granted(_)
    );

    // Vendor B holds the file and the public half. That is enough to
    // read an entity and not enough to be one: a forged archive signed
    // by anything other than the customer's Key does not verify.
    let vendor_b_cannot_forge = {
        let impostor = BrainKey::from_seed(&[0x99; 32]);
        let mut forged = read_back.clone();
        forged.signature = impostor.sign(
            brainos_crypto::domain::Tag::Export,
            &[0u8; 32],
        );
        forged.verify(&b.key().public()).is_err()
            && read_back.verify(&impostor.public()).is_err()
    };

    Migration {
        same_entity: entity_before == entity_after,
        entity_before,
        entity_after,
        archive_bytes: file.len(),
        sections,
        memories_before,
        memories_after,
        skills_before,
        skills_after,
        age_before,
        age_after,
        wakes_after,
        bodies_ever,
        still_knows_the_floor,
        still_knows_its_binding_femur,
        judgement_before,
        judgement_after,
        repertoire_before,
        repertoire_after,
        old_calibration_remembered,
        new_body_calibrated_fresh,
        walk_on_the_arm,
        walk_on_the_hexapod_id,
        arm_acted,
        vendor_a_destroyed,
        vendor_b_cannot_forge,
    }
}

impl Migration {
    /// Did the customer actually get to leave?
    pub fn is_free(&self) -> bool {
        self.same_entity
            && self.memories_after >= self.memories_before
            && self.skills_after == self.skills_before
            && self.age_after >= self.age_before
            && self.still_knows_the_floor
            && !self.judgement_before.is_empty()
            && self.judgement_after == self.judgement_before
            && self.walk_on_the_arm.is_some()
            && self.arm_acted
            && self.vendor_b_cannot_forge
    }
}

fn describe(j: &[(String, u8, u16)]) -> String {
    if j.is_empty() {
        return String::from("nothing");
    }
    j.iter()
        .map(|(tag, strength, tests)| format!("{tag} {strength}% over {tests} tests"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The narrated run.
pub fn narrate(m: &Migration) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();

    out.push((
        "vendor A",
        format!(
            "entity {} lived on a hexapod: {} memories, {} skills, {} ticks old, verbs {}",
            m.entity_before,
            m.memories_before,
            m.skills_before,
            m.age_before,
            m.repertoire_before.join(", ")
        ),
    ));
    out.push((
        "leave",
        format!(
            "the customer exports: {} bytes in {} sections, signed by their own Key",
            m.archive_bytes, m.sections
        ),
    ));
    out.push((
        "gone",
        String::from(
            "vendor A ceases to exist — entity, bodies, gate, audit log, journal, all destroyed. \
             One file crosses the gap, plus a Key that vendor A never held",
        ),
    ));
    out.push((
        "vendor B",
        format!(
            "a six-axis arm described in the manufacturer's own format adopts the file: same entity ({}), \
             {} memories, {} skills, {} ticks old, {} completed embodiments on record, {} bodies ever",
            if m.same_entity { "yes" } else { "NO" },
            m.memories_after,
            m.skills_after,
            m.age_after,
            m.wakes_after,
            m.bodies_ever
        ),
    ));
    out.push((
        "judgement",
        format!(
            "what it worked out by trying — {} — arrives {}. That is the moat: not the \
             file, the {} attempts behind it",
            describe(&m.judgement_after),
            if m.judgement_after == m.judgement_before {
                "intact"
            } else {
                "CHANGED — it forgot something in transit"
            },
            m.judgement_before
                .iter()
                .map(|(_, _, t)| *t as u32)
                .sum::<u32>(),
        ),
    ));
    out.push((
        "world",
        format!(
            "it still knows the workshop floor is carpet: {} — a fact about the world, still true without the body that learned it",
            m.still_knows_the_floor
        ),
    ));
    out.push((
        "body",
        format!(
            "it still remembers the old body's binding femur ({}) and the old calibration ({}), \
             and neither applies here: the arm was calibrated from scratch ({})",
            m.still_knows_its_binding_femur,
            m.old_calibration_remembered,
            m.new_body_calibrated_fresh
        ),
    ));
    out.push((
        "repertoire",
        format!(
            "what it can do is now {} — walking is not on the list, because there are no legs",
            m.repertoire_after.join(", ")
        ),
    ));
    out.push((
        "refused",
        format!(
            "asking the arm to walk: {} · asking the hexapod that no longer exists: {} — \
             a memory of a skill is not a licence to act with a body you do not have",
            m.walk_on_the_arm.clone().unwrap_or_else(|| "GRANTED".into()),
            m.walk_on_the_hexapod_id
                .clone()
                .unwrap_or_else(|| "GRANTED".into())
        ),
    ));
    out.push((
        "acts",
        format!(
            "and the arm moves on the first try: {} — no relearning, no migration window, no support ticket",
            m.arm_acted
        ),
    ));
    out.push((
        "forgery",
        format!(
            "vendor B can read the entity and cannot be it: a re-signed archive does not verify ({})",
            m.vendor_b_cannot_forge
        ),
    ));

    out
}
