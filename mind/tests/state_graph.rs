//! Stage 1.1 — memory: persistence roundtrip, the episodic ring, the
//! skill graph, and survival of corrupted input without panic.

use brainos_mind::state::{NodeKind, StateGraph};

#[test]
fn serialize_load_roundtrip_preserves_durable_memory() {
    let mut g = StateGraph::new();
    g.add(NodeKind::Episode, String::from("met the human"), 90, 1);
    g.add(NodeKind::Semantic, String::from("the human is called soapai"), 70, 2);
    g.add(NodeKind::Belief, String::from("i live on a key"), 80, 3);
    g.add(NodeKind::Audit, String::from("GRANT screen.speak"), 100, 4);
    g.add(NodeKind::Skill, String::from("sh :: ran a command"), 60, 5);

    let mut back = StateGraph::new();
    back.load(g.serialize().as_bytes());

    assert_eq!(back.count(NodeKind::Episode), 1);
    assert_eq!(back.count(NodeKind::Semantic), 1);
    assert_eq!(back.count(NodeKind::Belief), 1);
    // audit stays on the machine of record; skills are re-learned in body
    assert_eq!(back.count(NodeKind::Audit), 0);
    assert_eq!(back.count(NodeKind::Skill), 0);
    assert_eq!(back.inherited_episodes, 1);
    let ep = back.last(NodeKind::Episode).unwrap();
    assert_eq!(ep.content, "met the human");
    assert_eq!(ep.confidence, 90);
    assert_eq!(ep.tick, 1);
}

#[test]
fn episodic_memory_is_a_ring_buffer() {
    let mut g = StateGraph::new();
    for i in 0..60 {
        g.add(NodeKind::Episode, format!("episode {i}"), 90, i);
    }
    assert_eq!(g.count(NodeKind::Episode), 48, "ring cap is 48");
    // the oldest fell away; the newest survived
    let s = g.serialize();
    assert!(!s.contains("episode 0\n"));
    assert!(s.contains("episode 59"));
}

#[test]
fn serialized_semantic_memory_stays_bounded() {
    let mut g = StateGraph::new();
    for i in 0..80 {
        g.add(NodeKind::Semantic, format!("fact {i}"), 70, i);
    }
    let s = g.serialize();
    assert!(!s.contains("|fact 0\n"), "oldest semantic must fall away");
    assert!(!s.contains("|fact 15\n"));
    assert!(s.contains("|fact 16\n"), "the newest 64 must survive");
    assert!(s.contains("|fact 79\n"));
}

#[test]
fn load_survives_corruption_without_panic() {
    // garbage bytes, truncated lines, wrong field counts, bad numbers,
    // unknown kinds, raw binary — none of it may panic or half-load
    let cases: Vec<Vec<u8>> = vec![
        b"complete garbage".to_vec(),
        b"episode|notanumber|90|content\n".to_vec(),
        b"episode|1\n".to_vec(),
        b"unknownkind|1|50|x\n".to_vec(),
        vec![0xFF, 0xFE, 0x00, 0x01, 0x80],
        b"|||\n||\n|\n".to_vec(),
    ];
    for c in &cases {
        let mut g = StateGraph::new();
        g.load(c);
        // a bad-number line still parses defensively (tick/conf default),
        // but nothing here may produce an unknown kind or a crash
        assert_eq!(g.count(NodeKind::Audit), 0);
    }

    // a corrupt line between two good ones costs only itself
    let mut g = StateGraph::new();
    g.load(b"episode|1|90|first\ngarbage line here\nepisode|2|90|second\n");
    assert_eq!(g.count(NodeKind::Episode), 2);
}

#[test]
fn skill_graph_learns_then_reinforces() {
    let mut g = StateGraph::new();
    assert!(!g.knows_skill("web.fetch"));
    assert!(g.learn_skill("web.fetch", "fetched a page", 1), "first use is new");
    assert!(g.knows_skill("web.fetch"));
    assert!(!g.learn_skill("web.fetch", "fetched again", 2), "second use reinforces");
    assert_eq!(g.skill_count(), 1, "reinforcement must not duplicate");
    // confidence climbed
    let skill = g.last(NodeKind::Skill).unwrap();
    assert_eq!(skill.confidence, 65);
}

#[test]
fn knows_skill_matches_whole_verbs_only() {
    let mut g = StateGraph::new();
    g.learn_skill("web.fetch", "fetched", 1);
    assert!(!g.knows_skill("web"), "a prefix is not the skill");
    assert!(!g.knows_skill("web.fetch.extra"));
}

#[test]
fn retrieval_finds_related_memories() {
    let mut g = StateGraph::new();
    g.add(NodeKind::Episode, String::from("talked about quadcopters"), 90, 1);
    g.add(NodeKind::Episode, String::from("the weather was fine"), 90, 2);
    g.add(NodeKind::Semantic, String::from("quadcopters have four rotors"), 70, 3);
    let hits = g.relevant("do you remember the quadcopters?", 5);
    assert_eq!(hits.len(), 2);
    assert!(hits.iter().all(|n| n.content.contains("quadcopter")));
    // short words never match on their own
    assert!(g.relevant("the was a of", 5).is_empty());
}

#[test]
fn durable_digest_prefers_knowledge_then_fresh_episodes() {
    let mut g = StateGraph::new();
    g.add(NodeKind::Episode, String::from("old episode"), 90, 1);
    g.add(NodeKind::Semantic, String::from("a durable fact"), 70, 2);
    g.add(NodeKind::Episode, String::from("fresh episode"), 90, 3);
    let d = g.durable_digest(2, 1);
    assert!(d.contains(&String::from("a durable fact")));
    assert!(d.contains(&String::from("fresh episode")));
    assert!(!d.contains(&String::from("old episode")));
}
