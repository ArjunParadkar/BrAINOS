//! The integration suite.
//!
//! Everything else in this workspace is tested in isolation and with no
//! operating system underneath it. This file is where the substrate
//! meets a real filesystem and a real clock, and where the claims that
//! only mean something end-to-end get checked: that an entity survives a
//! power cut, that the demo does what it says, and that the properties
//! hold when everything is wired together rather than in a harness.

use brainos_abi::codec::{Decode, Encode};
use brainos_abi::ids::BodyId;
use brainos_abi::time::{Clock, Duration, Tick};
use brainos_abi::verb::Verb;
use brainos_adapters::native::{mini_blur, workstation};
use brainos_body::calibration::{Calibration, JointTrim};
use brainos_crypto::key::BrainKey;
use brainos_entity::entity::Entity;
use brainos_entity::life::Life;
use brainos_host::clock::{StepClock, SystemClock};
use brainos_host::store::{FileStore, StoreError};
use brainos_memory::consolidate::Dream;
use brainos_memory::episode::Outcome;
use brainos_memory::journal::{OpenFailure, Segment};
use std::path::PathBuf;

fn v(s: &str) -> Verb {
    Verb::parse(s).unwrap()
}

/// A scratch directory that cleans up after itself.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "brainos-test-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        Scratch(p)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
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
        .unwrap();
    }
    c
}

// ---------- clocks ----------

#[test]
fn the_system_clock_is_monotonic_and_starts_at_zero() {
    let c = SystemClock::new();
    let a = c.now();
    let b = c.now();
    assert!(b >= a);
    assert!(a.micros() < 1_000_000, "it starts near zero, not at boot time");
}

#[test]
fn a_step_clock_makes_a_run_reproducible() {
    let c = StepClock::new();
    assert_eq!(c.now().micros(), 0);
    c.advance(Duration::from_millis(10));
    assert_eq!(c.now().micros(), 10_000);
    c.advance(Duration::from_millis(10));
    assert_eq!(c.now().micros(), 20_000);
}

// ---------- the journal on a real filesystem ----------

#[test]
fn memory_written_to_disk_comes_back() {
    let scratch = Scratch::new("journal");
    let key = BrainKey::from_seed(&[1; 32]);
    let mut store = FileStore::open(&scratch.0, key.entity_id()).unwrap();

    assert!(!store.exists(Segment::Episodes));
    store
        .commit(&key, Segment::Episodes, b"a memory".to_vec())
        .unwrap();
    assert!(store.exists(Segment::Episodes));

    let mut reader = FileStore::open(&scratch.0, key.entity_id()).unwrap();
    let back = reader
        .open_segment(&key.public(), Segment::Episodes)
        .unwrap();
    assert_eq!(back, b"a memory");
}

#[test]
fn a_power_cut_mid_write_leaves_the_previous_memory_intact() {
    let scratch = Scratch::new("powercut");
    let key = BrainKey::from_seed(&[1; 32]);
    let mut store = FileStore::open(&scratch.0, key.entity_id()).unwrap();

    store
        .commit(&key, Segment::Episodes, b"generation one".to_vec())
        .unwrap();
    store
        .commit(&key, Segment::Episodes, b"generation two".to_vec())
        .unwrap();

    // the lights go out in the middle of writing generation three
    store
        .commit(&key, Segment::Episodes, b"generation three".to_vec())
        .unwrap();
    store.tear_newest(Segment::Episodes).unwrap();

    let mut reader = FileStore::open(&scratch.0, key.entity_id()).unwrap();
    let back = reader
        .open_segment(&key.public(), Segment::Episodes)
        .unwrap();
    assert_eq!(
        back, b"generation two",
        "the entity wakes with its previous memories, not with none"
    );
}

#[test]
fn both_slots_destroyed_is_reported_as_empty_rather_than_as_amnesia() {
    let scratch = Scratch::new("destroyed");
    let key = BrainKey::from_seed(&[1; 32]);
    let mut store = FileStore::open(&scratch.0, key.entity_id()).unwrap();
    store.commit(&key, Segment::Episodes, b"one".to_vec()).unwrap();
    store.commit(&key, Segment::Episodes, b"two".to_vec()).unwrap();

    for slot in ["a", "b"] {
        std::fs::write(scratch.0.join(format!("episodes.{slot}")), b"rubbish").unwrap();
    }

    let mut reader = FileStore::open(&scratch.0, key.entity_id()).unwrap();
    match reader.open_segment(&key.public(), Segment::Episodes) {
        Err(StoreError::Open(OpenFailure::Empty)) => {}
        other => panic!("expected Empty, got {other:?}"),
    }
}

#[test]
fn another_entitys_memory_on_our_medium_is_refused_distinctly() {
    let scratch = Scratch::new("foreign");
    let mine = BrainKey::from_seed(&[1; 32]);
    let theirs = BrainKey::from_seed(&[2; 32]);

    let mut their_store = FileStore::open(&scratch.0, theirs.entity_id()).unwrap();
    their_store
        .commit(&theirs, Segment::Episodes, b"their memories".to_vec())
        .unwrap();

    let mut my_reader = FileStore::open(&scratch.0, mine.entity_id()).unwrap();
    match my_reader.open_segment(&mine.public(), Segment::Episodes) {
        Err(StoreError::Open(OpenFailure::NotOurs)) => {}
        other => panic!("expected NotOurs, got {other:?}"),
    }
}

#[test]
fn a_rollback_of_the_whole_directory_is_detected() {
    let scratch = Scratch::new("rollback");
    let key = BrainKey::from_seed(&[1; 32]);
    let mut store = FileStore::open(&scratch.0, key.entity_id()).unwrap();

    store.commit(&key, Segment::Skills, b"one".to_vec()).unwrap();
    store.commit(&key, Segment::Skills, b"two".to_vec()).unwrap();
    let snapshot = (
        std::fs::read(scratch.0.join("skills.a")).unwrap(),
        std::fs::read(scratch.0.join("skills.b")).unwrap(),
    );
    store.commit(&key, Segment::Skills, b"three".to_vec()).unwrap();
    store.commit(&key, Segment::Skills, b"four".to_vec()).unwrap();

    // someone restores the directory from a backup taken two writes ago.
    // Every record in it is genuine and correctly signed.
    std::fs::write(scratch.0.join("skills.a"), &snapshot.0).unwrap();
    std::fs::write(scratch.0.join("skills.b"), &snapshot.1).unwrap();

    match store.open_segment_checked(&key.public(), Segment::Skills) {
        Err(StoreError::Open(OpenFailure::RolledBack { .. })) => {}
        other => panic!("a silently restored backup is a silently restored life: {other:?}"),
    }
}

#[test]
fn segments_are_independent_on_disk() {
    let scratch = Scratch::new("segments");
    let key = BrainKey::from_seed(&[1; 32]);
    let mut store = FileStore::open(&scratch.0, key.entity_id()).unwrap();

    store.commit(&key, Segment::Episodes, b"episodes".to_vec()).unwrap();
    store.commit(&key, Segment::Skills, b"skills".to_vec()).unwrap();

    // losing one does not lose the other — which is the point of
    // segmenting: a gait's worth of new episodes must not rewrite the
    // skill graph
    for slot in ["a", "b"] {
        std::fs::write(scratch.0.join(format!("episodes.{slot}")), b"").unwrap();
    }
    let mut reader = FileStore::open(&scratch.0, key.entity_id()).unwrap();
    assert!(reader
        .open_segment(&key.public(), Segment::Episodes)
        .is_err());
    assert_eq!(
        reader.open_segment(&key.public(), Segment::Skills).unwrap(),
        b"skills"
    );
}

// ---------- an entity across a power cut, end to end ----------

#[test]
fn an_entity_survives_a_power_cut_with_its_skills() {
    let scratch = Scratch::new("entity-powercut");
    let seed = [0x5A; 32];

    // --- a life ---
    let (durable, life, skills_before, walk_before) = {
        let mut e = Entity::new(BrainKey::from_seed(&seed), Tick(0));
        e.wake(None, None, Tick(1)).unwrap();
        let hexapod = e
            .acquire(
                &BrainKey::from_seed(&[0xB2; 32]),
                mini_blur(),
                calibration(),
                10,
            )
            .unwrap();
        for i in 0..30u64 {
            e.outcome(
                v("legs.walk"),
                Some(hexapod),
                if i % 6 == 0 {
                    Outcome::Stopped
                } else {
                    Outcome::Succeeded
                },
                "forward",
                &["carpet"],
            );
        }
        e.dream(Dream::deep());
        let walk = e
            .skills
            .for_verb(&v("legs.walk"))
            .map(|s| (s.attempts, s.successes));
        let (d, l) = e.sleep().unwrap();
        (d, l, e.skills.len(), walk)
    };

    // --- written to disk, then the lights go out mid-write ---
    let key = BrainKey::from_seed(&seed);
    let mut store = FileStore::open(&scratch.0, key.entity_id()).unwrap();
    store.commit(&key, Segment::graph(), durable.clone()).unwrap();
    store
        .commit(&key, Segment::Self_, life.to_bytes().unwrap())
        .unwrap();
    // a second commit that is interrupted
    store.commit(&key, Segment::graph(), durable.clone()).unwrap();
    store.tear_newest(Segment::graph()).unwrap();

    // --- it comes back ---
    let mut reader = FileStore::open(&scratch.0, key.entity_id()).unwrap();
    let memory = reader
        .open_segment(&key.public(), Segment::graph())
        .expect("the intact slot is used");
    let life_bytes = reader
        .open_segment(&key.public(), Segment::Self_)
        .unwrap();
    let recovered_life = Life::from_bytes(&life_bytes).unwrap();

    let mut reborn = Entity::new(BrainKey::from_seed(&seed), Tick(0));
    reborn
        .wake(Some(&memory), Some(recovered_life), Tick(50_000))
        .unwrap();

    assert_eq!(reborn.skills.len(), skills_before);
    assert_eq!(
        reborn
            .skills
            .for_verb(&v("legs.walk"))
            .map(|s| (s.attempts, s.successes)),
        walk_before,
        "a power cut must not cost the entity what it learned"
    );
    assert!(reborn.graph.inherited > 0);
}

// ---------- the demo ----------

#[test]
fn the_demo_does_what_it_says() {
    let r = brainos_host::demo::run();
    assert!(r.memory_survived, "continuity is the whole demo");
    assert_eq!(r.bodies, 2, "two bodies");
    assert!(r.memories > 10);
    assert!(r.skills >= 1);
    assert!(r.granted >= 2);
    assert!(r.denied >= 3, "the gate refused things, and said why");
    assert!(!r.archive.is_empty());
    assert_eq!(r.audit_root.len(), 64);

    // every step of the Phase 0 demo is present
    let steps: Vec<&str> = r.lines.iter().map(|l| l.step).collect();
    for expected in ["wake", "§8", "D1", "D2", "D3", "dream", "audit", "export", "continuity"] {
        assert!(steps.contains(&expected), "{expected} is missing");
    }

    // the narration is honest about what it did not show
    let all: String = r.lines.iter().map(|l| l.text.clone()).collect::<Vec<_>>().join(" ");
    assert!(all.contains("refused at policy"));
    assert!(all.contains("refused at authorize"));
    assert!(all.contains("refused at validate"));
}

#[test]
fn the_demo_is_reproducible() {
    let a = brainos_host::demo::run();
    let b = brainos_host::demo::run();
    assert_eq!(a.entity_id, b.entity_id);
    assert_eq!(a.audit_root, b.audit_root);
    assert_eq!(a.memories, b.memories);
    assert_eq!(a.archive.len(), b.archive.len());
}

#[test]
fn the_wire_demo_delivers_most_of_a_bad_link_and_no_wrong_frames() {
    let lines = brainos_host::demo::wire_demo();
    assert!(lines.len() >= 3);
    let text: String = lines.iter().map(|l| l.text.clone()).collect::<Vec<_>>().join(" ");
    assert!(text.contains("nothing secret crossed the wire"));
    assert!(text.contains("0 out of order"));
}

// ---------- the properties, wired together ----------

#[test]
fn the_property_report_holds_in_this_build() {
    // The same verification the binary runs, so a release of this crate
    // cannot ship with a violated property.
    //
    // Note for anyone watching the clock: this is ~26,000 ed25519
    // operations and takes minutes in a debug build. `cargo test
    // --release -p brainos-host` is the intended way to run it.
    let report = brainos_proof::verify();
    assert!(report.holds(), "\n{}", report.render());
    assert!(report.coverage.is_meaningful());
}
