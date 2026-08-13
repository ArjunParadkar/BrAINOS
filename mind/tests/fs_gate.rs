//! Stage 2.1 — the full filesystem limb faces the same gate as everything
//! else: every new verb well-typed, both ends of a move validated, chunked
//! reads bounded, and the volume marker protected like memory itself.

mod common;

use brainos_mind::instance::Instance;
use brainos_mind::kira::{fs_move_paths, fs_read_target, Action, Verdict};
use common::embodied_instance;

fn gate(me: &mut Instance, verb: &str, target: &str) -> Verdict {
    let (v, _) = me.propose(Action::UseLimb {
        verb: String::from(verb),
        target: String::from(target),
    });
    v
}


fn describe(v: &Verdict) -> String {
    match v {
        Verdict::Granted(_) => String::from("GRANTED"),
        Verdict::Denied { stage, reason } => format!("denied@{stage}: {reason}"),
    }
}

fn denied_at(v: &Verdict, want: &str) -> bool {
    matches!(v, Verdict::Denied { stage, .. } if *stage == want)
}

#[test]
fn every_new_verb_grants_on_benign_targets() {
    let mut me = embodied_instance();
    for (verb, target) in [
        ("fs.mkdir", "work/projects"),
        ("fs.delete", "work/old.txt"),
        ("fs.move", "draft.txt docs/final.txt"),
        ("fs.stat", "docs/about.txt"),
        ("fs.search", "fibonacci"),
        ("fs.read", "docs/about.txt@4096"),
    ] {
        let v = gate(&mut me, verb, target);
        assert!(
            matches!(v, Verdict::Granted(_)),
            "{verb}|{target} should grant, got {}",
            describe(&v),
        );
    }
}

#[test]
fn the_volume_marker_is_protected_at_policy() {
    let mut me = embodied_instance();
    for (verb, target) in [
        ("fs.delete", "WORLD.ID"),
        ("fs.delete", "world.id"),     // case must not slip past
        ("fs.delete", "/world.id"),    // nor a leading separator
        ("fs.write", "WORLD.ID forged marker content"),
        ("fs.move", "WORLD.ID GONE.TXT"),
    ] {
        let v = gate(&mut me, verb, target);
        assert!(
            denied_at(&v, "policy"),
            "{verb}|{target} must die at policy, got {}",
            describe(&v),
        );
    }
    // reading the marker is harmless and stays allowed
    assert!(matches!(gate(&mut me, "fs.read", "WORLD.ID"), Verdict::Granted(_)));
}

#[test]
fn moves_validate_both_ends() {
    let mut me = embodied_instance();
    for target in [
        "onlyonepath.txt",             // no destination
        "a.txt ../escape.txt",         // traversal in the destination
        "../escape.txt b.txt",         // traversal in the source
        "a.txt c:stealth.txt",         // drive specifier
    ] {
        let v = gate(&mut me, "fs.move", target);
        assert!(denied_at(&v, "validate"), "'fs.move|{target}' escaped validate: {}", describe(&v));
    }
}

#[test]
fn chunked_read_offsets_parse_or_refuse() {
    assert_eq!(fs_read_target("notes.txt"), ("notes.txt", Some(0)));
    assert_eq!(fs_read_target("notes.txt@4096"), ("notes.txt", Some(4096)));
    assert_eq!(fs_read_target(" big.log@0 "), ("big.log", Some(0)));
    // malformed offsets are None -> validate refuses, never a guess
    assert_eq!(fs_read_target("notes.txt@zero").1, None);
    let mut me = embodied_instance();
    let v = gate(&mut me, "fs.read", "notes.txt@zero");
    assert!(denied_at(&v, "validate"));
}

#[test]
fn move_target_splitting_is_exact() {
    assert_eq!(fs_move_paths("a.txt b.txt"), ("a.txt", "b.txt"));
    assert_eq!(fs_move_paths("  a.txt   docs/b.txt "), ("a.txt", "docs/b.txt"));
    assert_eq!(fs_move_paths("lonely.txt"), ("lonely.txt", ""));
}

#[test]
fn search_queries_are_bounded_text_not_paths() {
    let mut me = embodied_instance();
    assert!(matches!(gate(&mut me, "fs.search", "launch notes"), Verdict::Granted(_)));
    let v = gate(&mut me, "fs.search", "");
    assert!(denied_at(&v, "validate"));
    let long = "x".repeat(65);
    let v = gate(&mut me, "fs.search", &long);
    assert!(denied_at(&v, "validate"));
}
