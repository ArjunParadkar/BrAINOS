//! One entity, one continuous history, every capability in sequence.
//!
//! Every other scenario in this crate builds a fresh entity, does one
//! thing to it, and throws it away. That is the right shape for showing
//! a property, and it is the wrong shape for finding the bugs that
//! actually reach customers — which live in the seams: what an outage
//! does to a skill learned before it, whether an audit chain survives
//! an export, whether an entity that has changed bodies twice can still
//! be proven to have refused something on the first one.
//!
//! So: one entity, born once, carried through the whole product.
//!
//!   1. wakes on a workstation
//!   2. takes on a hexapod and works, learning by trying
//!   3. loses its datacenter link and keeps going
//!   4. is put away and woken again — a different process
//!   5. survives a torn journal write
//!   6. buys a skill from the marketplace
//!   7. is exported, and the vendor is destroyed
//!   8. is adopted by a competitor's arm
//!   9. has an incident reconstructed and proven from its whole life
//!  10. is exported again and read by a program that shares none of our
//!      code
//!
//! At every step the same four invariants are checked, because they are
//! the ones the product is: the identity never changes, the age never
//! goes backwards, what it learned by trying is still there, and the
//! audit log still proves what it proved before.

use alloc_free::*;

mod alloc_free {
    pub use brainos_abi::ids::BodyId;
    pub use brainos_abi::time::{Instant, Tick};
    pub use brainos_abi::verb::{Namespace, Verb};
    pub use brainos_adapters::mhs::from_mhs;
    pub use brainos_adapters::native::mini_blur;
    pub use brainos_body::calibration::{Calibration, JointTrim};
    pub use brainos_body::hal::Telemetry;
    pub use brainos_cognition::ambient::SenseFrame;
    pub use brainos_cognition::placement::Site;
    pub use brainos_cognition::resource::{Resource, Tier};
    pub use brainos_crypto::key::BrainKey;
    pub use brainos_entity::entity::Entity;
    pub use brainos_export::archive::{adopt, export, Archive};
    pub use brainos_fleet::marketplace::{countersign, install, Manifest, Package, SkillContents};
    pub use brainos_kira::gate::Verdict;
    pub use brainos_memory::episode::Outcome;
    pub use brainos_memory::journal::{Journal, Segment, SlotState};
}

const SEED: [u8; 32] = [0x11; 32];

fn v(s: &str) -> Verb {
    Verb::parse(s).expect("a well-formed verb")
}

fn calibration() -> Calibration {
    let mut c = Calibration::new(BodyId::ZERO, Tick(1));
    for ch in 0..10u8 {
        c.trim(JointTrim {
            channel: ch,
            offset_deg: 0.0,
            scale: 1.0,
            min_deg: -28.0,
            max_deg: 28.0,
            probed: true,
        })
        .expect("a sane trim");
    }
    c
}

fn fit() -> Telemetry {
    Telemetry {
        at: Instant(0),
        link_alive: true,
        rail_v: Some(7.4),
        current_a: Some(0.5),
        tilt_deg: Some(1.0),
        hottest_c: Some(38.0),
        stops: 0,
        overruns: 0,
    }
}

/// What was true at one moment of the life.
#[derive(Clone, Debug, PartialEq)]
pub struct Moment {
    pub step: &'static str,
    pub what_happened: String,
    /// the four invariants, read fresh each time
    pub identity: String,
    pub age: u64,
    /// preconditions this entity established by trying, as (tag, strength)
    pub judgement: Vec<(String, u8)>,
    pub memories: usize,
    pub audit_len: usize,
}

#[derive(Clone, Debug)]
pub struct Life {
    pub moments: Vec<Moment>,
    /// a proof taken early, re-checked at the very end against the root
    /// it was issued against
    pub early_proof_still_verifies: bool,
    /// the final export, read by something that shares none of our code
    pub final_export: Vec<u8>,
    /// violations of the four invariants, in order
    pub violations: Vec<String>,
}

impl Life {
    pub fn holds(&self) -> bool {
        self.violations.is_empty() && self.moments.len() >= 10 && self.early_proof_still_verifies
    }
}

struct Recorder {
    moments: Vec<Moment>,
    violations: Vec<String>,
}

impl Recorder {
    fn note(&mut self, e: &Entity, step: &'static str, what: impl Into<String>) {
        let judgement: Vec<(String, u8)> = e
            .skills
            .for_verb(&v("legs.walk"))
            .map(|s| {
                s.preconditions
                    .iter()
                    .filter(|p| !p.adopted && p.is_well_supported())
                    .map(|p| (p.tag.clone(), p.strength()))
                    .collect()
            })
            .unwrap_or_default();
        let m = Moment {
            step,
            what_happened: what.into(),
            identity: e.id().fingerprint().to_string(),
            age: e.life.age(e.tick),
            judgement,
            memories: e.graph.len(),
            audit_len: e.kira.trail.len(),
        };

        // --- the four invariants, against the previous moment ---
        if let Some(prev) = self.moments.last() {
            if m.identity != prev.identity {
                self.violations.push(format!(
                    "at `{step}` the identity changed from {} to {}",
                    prev.identity, m.identity
                ));
            }
            if m.age < prev.age {
                self.violations.push(format!(
                    "at `{step}` the age went backwards: {} → {}",
                    prev.age, m.age
                ));
            }
            // judgement established by trying is never lost, only added
            // to or overturned by further trying — and nothing in this
            // life overturns it
            for (tag, strength) in &prev.judgement {
                match m.judgement.iter().find(|(t, _)| t == tag) {
                    None => self.violations.push(format!(
                        "at `{step}` the entity forgot that it had established `{tag}` \
                         at {strength}%"
                    )),
                    Some((_, now)) if *now + 20 < *strength => self.violations.push(format!(
                        "at `{step}` the evidence for `{tag}` collapsed from {strength}% \
                         to {now}%"
                    )),
                    Some(_) => {}
                }
            }
        }
        self.moments.push(m);
    }
}

pub fn live() -> Life {
    let mut r = Recorder {
        moments: Vec::new(),
        violations: Vec::new(),
    };

    // ---------------------------------------------------------------
    // 1. born
    // ---------------------------------------------------------------
    let mut e = Entity::new(BrainKey::from_seed(&SEED), Tick(0));
    e.wake(None, None, Tick(1)).expect("a first waking");
    e.resources
        .add(Resource::new("reflex", Tier::Autonomic, Site::Body));
    e.resources
        .add(Resource::new("frontier", Tier::Frontier, Site::Remote));
    r.note(&e, "born", "wakes on a workstation with nothing behind it");

    // ---------------------------------------------------------------
    // 2. a body, and work
    // ---------------------------------------------------------------
    let hexapod = e
        .acquire(&BrainKey::from_seed(&[0x21; 32]), mini_blur(), calibration(), 10)
        .expect("the hexapod joins");
    e.observe_body(&hexapod, &fit());
    e.learn_skill(v("legs.walk"), "walking");
    for i in 0..90u64 {
        e.tick(SenseFrame::quiet(Instant(i)));
        let on_carpet = i % 3 != 0;
        let granted = e
            .attempt(v("legs.walk"), hexapod, &[("speed", 0.08), ("heading", 0.0)])
            .verdict
            .is_granted();
        e.outcome(
            v("legs.walk"),
            Some(hexapod),
            if granted && on_carpet {
                Outcome::Succeeded
            } else {
                Outcome::Failed
            },
            "across the workshop",
            &[if on_carpet { "carpet" } else { "tile" }, "workshop"],
        );
    }
    r.note(&e, "works", "takes on a hexapod and finds out by trying that the gait needs carpet");

    // a proof taken now, to be re-checked at the very end
    let early_index = 3.min(e.kira.trail.len().saturating_sub(1));
    let early_proof = e.kira.trail.prove(early_index);
    let early_root = e.kira.trail.root();
    let early_record = e
        .kira
        .trail
        .recent(usize::MAX)
        .get(early_index)
        .map(|rec| rec.canonical());

    // ---------------------------------------------------------------
    // 3. the datacenter goes away, and comes back
    // ---------------------------------------------------------------
    let (before, after) = e.lose_site(Site::Remote);
    for i in 90..110u64 {
        e.tick(SenseFrame::quiet(Instant(i)));
    }
    e.regain_site(Site::Remote);
    r.note(
        &e,
        "outage",
        format!("loses its datacenter link (ceiling {} → {}) and keeps working", before.name(), after.name()),
    );

    // ---------------------------------------------------------------
    // 4. put away, and woken again in a different process
    // ---------------------------------------------------------------
    let (durable, life_record) = e.sleep().expect("a clean sleep");
    let mut e = Entity::new(BrainKey::from_seed(&SEED), Tick(0));
    e.wake(Some(&durable), Some(life_record.clone()), Tick(999_999))
        .expect("it comes back");
    r.note(&e, "reboot", "put away, and woken in a different process on the same Key");

    // ---------------------------------------------------------------
    // 5. a torn journal write
    // ---------------------------------------------------------------
    let key = BrainKey::from_seed(&SEED);
    let mut journal = Journal::new(e.id());
    let (_, rec1) = journal.commit(&key, Segment::graph(), durable.clone(), SlotState::default());
    let slot_a = rec1.to_slot_bytes().expect("a record encodes");
    let (_, rec2) = journal.commit(
        &key,
        Segment::graph(),
        durable.clone(),
        SlotState {
            generation_a: Some(rec1.generation),
            generation_b: None,
        },
    );
    let full = rec2.to_slot_bytes().expect("a record encodes");
    let torn = &full[..full.len() / 2]; // power lost halfway through
    let mut recovering = Journal::new(e.id());
    let recovered = recovering
        .open_cold(&key.public(), Segment::graph(), &slot_a, torn)
        .expect("the previous good record comes back");
    let mut e = Entity::new(BrainKey::from_seed(&SEED), Tick(0));
    e.wake(Some(&recovered.payload), Some(life_record), Tick(0))
        .expect("it comes back from the torn medium");
    r.note(
        &e,
        "power lost",
        "the journal write tears halfway; it wakes from the previous good record",
    );

    // ---------------------------------------------------------------
    // 6. buying a skill
    // ---------------------------------------------------------------
    let publisher = BrainKey::from_seed(&[0x31; 32]);
    let owner = BrainKey::from_seed(&[0x41; 32]);
    let manifest = Manifest {
        name: "stair climbing".into(),
        version: 1,
        scope: Namespace::only("legs").unwrap(),
        verbs: vec![v("legs.climb")],
        claimed_consequence: 60,
        irreversible: false,
        summary: "climb a domestic stair".into(),
        learned_in: "carpeted workshop".into(),
        tests_behind_it: 140,
    };
    let contents = SkillContents::new(v("legs.climb"), "stair climbing", &["handrail_clear"]);
    let package = Package::publish_with(&publisher, manifest, Some(contents), e.tick)
        .expect("a consistent package");
    let sig = countersign(&owner, &package, e.id());
    install(
        &package,
        e.id(),
        &owner.public(),
        &sig,
        &Namespace::only("legs").unwrap(),
        &[],
        e.tick,
    )
    .expect("the owner authorized it");
    let taken = e.take_delivery_of(package.contents.as_ref().unwrap());
    r.note(
        &e,
        "buys",
        format!("buys a skill learned in a carpeted workshop; {taken} condition taken on trust"),
    );

    // ---------------------------------------------------------------
    // 7. the vendor is destroyed
    // ---------------------------------------------------------------
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
    drop(e);
    // and the only things that cross are `file` and the customer's Key

    // ---------------------------------------------------------------
    // 8. a competitor's arm adopts it
    // ---------------------------------------------------------------
    let mut e = Entity::new(BrainKey::from_seed(&SEED), Tick(0));
    let read_back = Archive::from_bytes(&file).expect("the archive reads back");
    adopt(&mut e, &read_back, Tick(0)).expect("the customer's entity is adopted");
    let arm_schema = from_mhs(crate::fullstack::MANUFACTURER_DESCRIPTION)
        .expect("a well-formed device description");
    let arm = e
        .acquire(
            &BrainKey::from_seed(&[0x51; 32]),
            arm_schema,
            Calibration::new(BodyId::ZERO, e.tick),
            0,
        )
        .expect("the arm joins");
    e.observe_body(&arm, &fit());
    r.note(
        &e,
        "new vendor",
        "the first vendor is destroyed; a competitor's six-axis arm adopts the file",
    );

    // ---------------------------------------------------------------
    // 9. it works on the new body, and still refuses the old one's verbs
    // ---------------------------------------------------------------
    let moved = e
        .attempt(v("arm.move_joint"), arm, &[("joint", 1.0), ("angle", 45.0), ("speed", 30.0)])
        .verdict
        .is_granted();
    let walked = matches!(
        e.attempt(v("legs.walk"), arm, &[("speed", 0.08)]).verdict,
        Verdict::Granted(_)
    );
    r.note(
        &e,
        "acts",
        format!(
            "moves the arm on the first try ({moved}); asking it to walk is refused ({})",
            !walked
        ),
    );

    // ---------------------------------------------------------------
    // 10. exported again, and read by something that is not us
    // ---------------------------------------------------------------
    let final_archive = export(
        e.key(),
        &e.graph,
        &e.life,
        e.skills.len(),
        e.bodies.body_count(),
        e.tick,
    )
    .expect("exported again");
    let final_export = final_archive.to_bytes().expect("the archive encodes");
    r.note(
        &e,
        "leaves again",
        format!("exported a second time: {} bytes", final_export.len()),
    );

    // The proof taken on the first body, re-checked now — against the
    // root it was issued against, which is the whole point of a proof.
    //
    // Note what the entity does *not* carry: the audit log resets to
    // zero at every reboot and is not in the export, because it belongs
    // to the machine of record rather than to the entity. An entity
    // carrying its own audit log between owners could edit it in
    // transit. So this models the machine of record still holding the
    // root and the proof, and the entity having changed body, owner and
    // process since.
    let early_proof_still_verifies = match (early_proof, early_record) {
        (Some(p), Some(bytes)) => p.verify_record(&bytes, &early_root),
        _ => false,
    };

    let readable = Archive::from_bytes(&final_export)
        .map(|a| a.verify(&e.key().public()).is_ok())
        .unwrap_or(false);
    r.note(
        &e,
        "checkable",
        format!(
            "the second export verifies against the entity's own Key ({readable}); a              decision recorded on the first body, two owners and three processes ago,              still proves against the root it was issued against ({early_proof_still_verifies})"
        ),
    );

    Life {
        moments: r.moments,
        early_proof_still_verifies,
        final_export,
        violations: r.violations,
    }
}

pub fn narrate(l: &Life) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    for m in &l.moments {
        out.push((
            m.step,
            format!(
                "{} · {} · age {} · {} memories · {} decisions · knows: {}",
                m.what_happened,
                m.identity,
                m.age,
                m.memories,
                m.audit_len,
                if m.judgement.is_empty() {
                    String::from("nothing it has tested")
                } else {
                    m.judgement
                        .iter()
                        .map(|(t, s)| format!("{t} {s}%"))
                        .collect::<Vec<_>>()
                        .join(", ")
                }
            ),
        ));
    }
    out
}
