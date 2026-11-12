//! BFS  LEVEL    Freedom principle - entity export  [crate brainos-export]
//! BFS  PURPOSE  Tests that an export is complete, carries its own spec, detects any tampering, refuses other format versions, and imports back as the same entity
//! BFS  DEFINES  -
//! BFS  USES     brainos: abi, crypto, entity, export, memory
//! BFS  USED BY  (entry point or test)
//!
//! The freedom-principle suite.
//!
//! "Export of an entity's state graph is a first-class, documented,
//! tested feature from v1.0." This file is the tested part.

use brainos_abi::ids::{BodyId, EntityId};
use brainos_abi::time::Tick;
use brainos_abi::verb::Verb;
use brainos_crypto::key::BrainKey;
use brainos_entity::entity::Entity;
use brainos_entity::life::{Life, Span};
use brainos_export::archive::{export, import, Archive, ExportError, SectionKind, FORMAT_VERSION, MAGIC};
use brainos_export::format::describe_format;
use brainos_memory::consolidate::Dream;
use brainos_memory::episode::Outcome;
use brainos_memory::graph::{NodeKind, StateGraph};
use brainos_memory::law::MemoryHandle;
use brainos_memory::skill::SkillGraph;

fn v(s: &str) -> Verb {
    Verb::parse(s).unwrap()
}

/// An entity with a life behind it.
fn lived_entity() -> Entity {
    let mut e = Entity::new(BrainKey::from_seed(&[1; 32]), Tick(0));
    e.wake(None, None, Tick(1)).unwrap();
    for i in 0..40u64 {
        e.outcome(
            v("legs.walk"),
            Some(BodyId::from_bytes([2; 32])),
            if i % 5 == 0 {
                Outcome::Stopped
            } else {
                Outcome::Succeeded
            },
            "forward on carpet",
            &["carpet", "workshop"],
        );
    }
    e.dream(Dream::deep());
    e.life.saw_body(BodyId::from_bytes([2; 32]));
    e.life.record(Span {
        woke_at: Tick(1),
        slept_at: Tick(500),
        actions_granted: 38,
        actions_denied: 2,
        clean: true,
        ..Span::default()
    });
    e
}

// ---------- completeness ----------

#[test]
fn an_export_carries_everything_that_makes_the_entity_itself() {
    let e = lived_entity();
    let a = export(
        e.key(),
        &e.graph,
        &e.life,
        e.skills.len(),
        e.bodies.body_count(),
        Tick(500),
    )
    .unwrap();

    assert_eq!(a.manifest.entity, e.id());
    assert_eq!(a.manifest.node_count as usize, e.graph.len());
    assert_eq!(a.manifest.skill_count as usize, e.skills.len());
    assert_eq!(a.manifest.wakes, e.life.wakes);
    assert!(a.manifest.age_ticks > 0);

    for kind in [SectionKind::Memory, SectionKind::Life, SectionKind::Readme] {
        assert!(a.section(kind).is_some(), "{kind:?} is missing");
        assert!(!a.section(kind).unwrap().bytes.is_empty());
    }
}

#[test]
fn the_omissions_are_stated_rather_than_silent() {
    let e = lived_entity();
    let a = export(e.key(), &e.graph, &e.life, 1, 1, Tick(500)).unwrap();
    assert_eq!(a.manifest.omissions.len(), 2);
    let all = a.manifest.omissions.join(" ");
    assert!(all.contains("private half"));
    assert!(all.contains("audit log"));
    assert!(
        all.contains("edit it in transit"),
        "the reason travels with the omission"
    );
}

// ---------- documentation ----------

#[test]
fn the_specification_ships_inside_the_file() {
    let e = lived_entity();
    let a = export(e.key(), &e.graph, &e.life, 1, 1, Tick(500)).unwrap();
    let readme = a.section(SectionKind::Readme).unwrap();
    let text = core::str::from_utf8(&readme.bytes).unwrap();

    // a format documented only on a website stops being documented when
    // the website does
    assert!(text.contains("BRAINEX1"));
    assert!(text.contains("LAYOUT"));
    assert!(text.contains("MEMORY SECTION"));
    assert!(text.contains("LIFE SECTION"));
    assert!(text.contains("READING THIS WITHOUT US"));
    assert!(text.contains("You do not need us"));
    assert_eq!(text, describe_format());
}

#[test]
fn the_specification_describes_the_layout_the_encoder_actually_writes() {
    let e = lived_entity();
    let a = export(e.key(), &e.graph, &e.life, 1, 1, Tick(500)).unwrap();
    let bytes = a.to_bytes().unwrap();

    // the first ten bytes, exactly as documented
    assert_eq!(&bytes[..8], &MAGIC);
    assert_eq!(&bytes[..8], b"BRAINEX1");
    assert_eq!(u16::from_le_bytes([bytes[8], bytes[9]]), FORMAT_VERSION);

    // the last 96 bytes are the root and the signature
    let n = bytes.len();
    assert_eq!(&bytes[n - 64..], a.signature.as_bytes());
    assert_eq!(&bytes[n - 96..n - 64], a.root.as_bytes());
}

// ---------- signed and checkable ----------

#[test]
fn an_archive_verifies_against_the_entity_it_names() {
    let e = lived_entity();
    let a = export(e.key(), &e.graph, &e.life, 1, 1, Tick(500)).unwrap();
    assert!(a.verify(&e.key().public()).is_ok());

    let stranger = BrainKey::from_seed(&[99; 32]);
    assert_eq!(
        a.verify(&stranger.public()),
        Err(ExportError::WrongEntity)
    );
}

#[test]
fn altering_one_byte_of_one_section_is_caught() {
    let e = lived_entity();
    let mut a = export(e.key(), &e.graph, &e.life, 1, 1, Tick(500)).unwrap();
    let memory = a
        .sections
        .iter_mut()
        .find(|s| s.kind == SectionKind::Memory)
        .unwrap();
    memory.bytes[20] ^= 1;
    assert_eq!(a.verify(&e.key().public()), Err(ExportError::Tampered));
}

#[test]
fn rewriting_the_manifest_is_caught_even_when_the_sections_are_intact() {
    let e = lived_entity();
    let mut a = export(e.key(), &e.graph, &e.life, 1, 1, Tick(500)).unwrap();
    // claim more skills than are in the file
    a.manifest.skill_count = 9_999;
    assert_eq!(a.verify(&e.key().public()), Err(ExportError::Unsigned));
}

#[test]
fn swapping_a_section_between_two_archives_is_caught() {
    let e1 = lived_entity();
    let mut e2 = Entity::new(BrainKey::from_seed(&[7; 32]), Tick(0));
    e2.wake(None, None, Tick(1)).unwrap();
    e2.outcome(v("legs.walk"), None, Outcome::Succeeded, "x", &[]);

    let a1 = export(e1.key(), &e1.graph, &e1.life, 1, 1, Tick(500)).unwrap();
    let a2 = export(e2.key(), &e2.graph, &e2.life, 1, 1, Tick(500)).unwrap();

    let mut frankenstein = a1.clone();
    let stolen = a2.section(SectionKind::Memory).unwrap().clone();
    frankenstein
        .sections
        .retain(|s| s.kind != SectionKind::Memory);
    frankenstein.sections.push(stolen);
    assert!(frankenstein.verify(&e1.key().public()).is_err());
}

#[test]
fn a_customer_can_prove_one_section_without_handing_over_everything() {
    // "here is my life record, and here is a proof it came from that
    // archive" — without also handing over every memory in it
    let e = lived_entity();
    let a = export(e.key(), &e.graph, &e.life, 1, 1, Tick(500)).unwrap();

    let (proof, root) = a.prove(SectionKind::Life).unwrap();
    assert_eq!(root, a.root);
    assert!(proof.verify(&root));

    // the leaf is the section's own digest, so a verifier who was given
    // only the bytes of that section can check them against the root
    let section = a.section(SectionKind::Life).unwrap();
    assert_eq!(proof.leaf, section.digest());

    // and a proof for one section does not verify a different one
    let memory = a.section(SectionKind::Memory).unwrap();
    assert_ne!(proof.leaf, memory.digest());
}

#[test]
fn a_newer_format_is_refused_rather_than_partly_read() {
    let e = lived_entity();
    let a = export(e.key(), &e.graph, &e.life, 1, 1, Tick(500)).unwrap();
    let mut bytes = a.to_bytes().unwrap();
    bytes[8] = 99;
    assert_eq!(
        Archive::from_bytes(&bytes),
        Err(ExportError::TooNew {
            theirs: 99,
            ours: FORMAT_VERSION
        })
    );
}

#[test]
fn something_that_is_not_an_archive_is_recognized_as_such() {
    assert_eq!(Archive::from_bytes(b""), Err(ExportError::NotAnArchive));
    assert_eq!(
        Archive::from_bytes(b"not an archive at all"),
        Err(ExportError::NotAnArchive)
    );
    let mut truncated = b"BRAINEX1".to_vec();
    truncated.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    assert!(matches!(
        Archive::from_bytes(&truncated),
        Err(ExportError::Corrupt(_))
    ));
}

/// A version this build is too new to read is refused *by name*.
///
/// v1 predates nodes carrying their structured form, so its memory
/// section decodes into skills that have forgotten every precondition
/// they ever learned. Reading one would hand the customer back an
/// entity quietly missing the part of itself that took the longest to
/// acquire, and `Corrupt("memory")` would be a misleading way to say
/// so. It is a different situation and it gets a different error.
#[test]
fn an_older_format_is_refused_by_name_rather_than_read_lossily() {
    let mut old = b"BRAINEX1".to_vec();
    old.extend_from_slice(&1u16.to_le_bytes());
    old.extend_from_slice(&[0u8; 64]);
    assert_eq!(
        Archive::from_bytes(&old),
        Err(ExportError::TooOld {
            theirs: 1,
            oldest: brainos_export::archive::OLDEST_READABLE
        })
    );
}

#[test]
fn an_archive_missing_a_required_section_is_incomplete_not_empty() {
    let e = lived_entity();
    let mut a = export(e.key(), &e.graph, &e.life, 1, 1, Tick(500)).unwrap();
    a.sections.retain(|s| s.kind != SectionKind::Life);
    // the root no longer matches, which is caught first
    assert_eq!(a.verify(&e.key().public()), Err(ExportError::Tampered));
    assert!(SectionKind::Life.required());
    assert!(!SectionKind::Readme.required());
}

// ---------- round trip: the test that makes the others mean something ----------

#[test]
fn an_exported_entity_imports_into_a_fresh_process_as_itself() {
    let original = lived_entity();
    let key = BrainKey::from_seed(&[1; 32]);

    let memories_before = original.graph.len();
    let skills_before = original.skills.len();
    let episodes_before = original.graph.count(NodeKind::Episode);
    let semantic_before = original.graph.count(NodeKind::Semantic);
    let walk_before = original
        .skills
        .for_verb(&v("legs.walk"))
        .map(|s| (s.attempts, s.successes));
    let life_before = original.life.clone();

    // the file, as it would exist on a customer's disk
    let archive = export(
        original.key(),
        &original.graph,
        &original.life,
        original.skills.len(),
        original.bodies.body_count(),
        Tick(500),
    )
    .unwrap();
    let file = archive.to_bytes().unwrap();

    // ---- everything above this line is thrown away ----

    let read_back = Archive::from_bytes(&file).unwrap();
    let handle = MemoryHandle::issue(&key);
    let mut graph = StateGraph::new();
    let life = import(&read_back, &key.public(), &handle, &mut graph).unwrap();

    let mut skills = SkillGraph::new();
    skills.rehydrate(&graph);

    assert_eq!(graph.len(), memories_before);
    assert_eq!(graph.count(NodeKind::Episode), episodes_before);
    assert_eq!(graph.count(NodeKind::Semantic), semantic_before);
    assert_eq!(skills.len(), skills_before);
    assert_eq!(
        skills.for_verb(&v("legs.walk")).map(|s| (s.attempts, s.successes)),
        walk_before
    );
    assert_eq!(life, life_before);
    assert_eq!(life.bodies_ever, life_before.bodies_ever);
}

#[test]
fn an_imported_entity_still_remembers_what_it_learned() {
    let original = lived_entity();
    let key = BrainKey::from_seed(&[1; 32]);
    let archive = export(original.key(), &original.graph, &original.life, 1, 1, Tick(500)).unwrap();

    let handle = MemoryHandle::issue(&key);
    let mut graph = StateGraph::new();
    import(&archive, &key.public(), &handle, &mut graph).unwrap();

    let index = brainos_memory::index::Index::build(&graph);
    let hits = index.recall(&graph, "walking on carpet in the workshop", Tick(600), 5);
    assert!(
        hits.iter().any(|(n, _)| n.content.contains("carpet")),
        "an export that loses the memories is not an export"
    );
}

#[test]
fn importing_with_the_wrong_handle_is_refused() {
    let original = lived_entity();
    let key = BrainKey::from_seed(&[1; 32]);
    let archive = export(original.key(), &original.graph, &original.life, 1, 1, Tick(500)).unwrap();

    let wrong = MemoryHandle::issue(&BrainKey::from_seed(&[77; 32]));
    let mut graph = StateGraph::new();
    assert_eq!(
        import(&archive, &key.public(), &wrong, &mut graph),
        Err(ExportError::WrongEntity)
    );
    assert!(graph.is_empty(), "nothing was partially imported");
}

#[test]
fn a_tampered_archive_imports_nothing_at_all() {
    let original = lived_entity();
    let key = BrainKey::from_seed(&[1; 32]);
    let mut archive =
        export(original.key(), &original.graph, &original.life, 1, 1, Tick(500)).unwrap();
    archive.sections[0].bytes[10] ^= 0xFF;

    let handle = MemoryHandle::issue(&key);
    let mut graph = StateGraph::new();
    assert!(import(&archive, &key.public(), &handle, &mut graph).is_err());
    assert!(
        graph.is_empty(),
        "a partially-imported entity is a corrupted entity"
    );
}

#[test]
fn the_file_survives_a_full_byte_level_round_trip() {
    let e = lived_entity();
    let a = export(e.key(), &e.graph, &e.life, 3, 2, Tick(500)).unwrap();
    let bytes = a.to_bytes().unwrap();
    let back = Archive::from_bytes(&bytes).unwrap();
    assert_eq!(back.manifest, a.manifest);
    assert_eq!(back.sections, a.sections);
    assert_eq!(back.root, a.root);
    assert_eq!(back.signature.as_bytes(), a.signature.as_bytes());
    assert_eq!(back.to_bytes().unwrap(), bytes);
}

#[test]
fn an_entity_with_no_life_yet_still_exports_cleanly() {
    let key = BrainKey::from_seed(&[5; 32]);
    let graph = StateGraph::new();
    let life = Life::begin(Tick(0));
    let a = export(&key, &graph, &life, 0, 0, Tick(1)).unwrap();
    assert!(a.verify(&key.public()).is_ok());

    let handle = MemoryHandle::issue(&key);
    let mut fresh = StateGraph::new();
    let back = import(&a, &key.public(), &handle, &mut fresh).unwrap();
    assert_eq!(back.wakes, 0);
    assert!(fresh.is_empty());
}
