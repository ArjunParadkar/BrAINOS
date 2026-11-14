//! BFS  LEVEL    Stage 4 Entity - one Key, one state graph, every body at once  [crate brainos-entity]
//! BFS  PURPOSE  Tests that motor primitives are kept in the state graph and read back intact, and that a recipe becomes a belief only when its trial passed
//! BFS  DEFINES  -
//! BFS  USES     brainos: abi, crypto, entity, memory, motor
//! BFS  USED BY  (entry point or test)
//!

use brainos_abi::time::Tick;
use brainos_crypto::key::BrainKey;
use brainos_entity::entity::{Entity, Trial};
use brainos_memory::graph::NodeKind;
use brainos_motor::primitive::desk_arm_library;
use brainos_motor::recipe::Recipe;

fn entity() -> Entity {
    let mut e = Entity::new(BrainKey::from_seed(&[0x31; 32]), Tick(0));
    e.wake(None, None, Tick(1)).unwrap();
    e
}

const MUG: &str = "grasp frame=brush.grip it=brush to= width_m=width*1+0\n\
wipe frame=mug.mouth it=mug to= radius_m=radius*1+-0.0074 depth_m=depth*1+0.0014 turns=1 tool_m=0.075\n\
place frame=holder.rest it=brush to=holder width_m=width*1+0 tool_m=0.0757\n";

#[test]
fn primitives_live_in_the_graph_and_come_back_as_they_went_in() {
    let mut e = entity();
    let before = e.graph.count(NodeKind::Skill);
    let lib = desk_arm_library();
    for p in &lib {
        assert!(e.learn_primitive(p).is_some(), "{} did not fit in a skill node", p.verb.as_str());
    }
    assert_eq!(e.graph.count(NodeKind::Skill), before + 6);
    let back = e.primitives_for("desk_arm");
    assert_eq!(back.len(), 6);
    for p in &lib {
        assert!(back.contains(p), "{} changed in memory", p.verb.as_str());
    }
    assert!(e.primitives_for("hexapod").is_empty(), "primitives are per body class");

    // learning one again replaces it; it does not pile up
    e.learn_primitive(&lib[0]).unwrap();
    assert_eq!(e.graph.count(NodeKind::Skill), before + 6);
}

#[test]
fn primitives_survive_the_entity_being_put_away_and_woken() {
    let mut e = entity();
    for p in &desk_arm_library() {
        e.learn_primitive(p).unwrap();
    }
    let durable = e.graph.encode_durable().unwrap();
    let mut again = Entity::new(BrainKey::from_seed(&[0x31; 32]), Tick(0));
    again.wake(Some(&durable), None, Tick(2)).unwrap();
    assert_eq!(again.primitives_for("desk_arm").len(), 6);
}

#[test]
fn a_recipe_that_failed_its_trial_leaves_a_record_and_no_belief() {
    let mut e = entity();
    let recipe = Recipe::from_text(MUG).unwrap();
    let beliefs = e.graph.count(NodeKind::Belief);
    let episodes = e.graph.count(NodeKind::Episode);
    let out = e.consider_recipe(
        "wash-mug",
        &recipe,
        &Trial { passed: false, detail: "covered 0.41, brush not returned".into() },
    );
    assert!(out.is_none());
    assert_eq!(e.graph.count(NodeKind::Belief), beliefs, "a failed recipe became a belief");
    assert_eq!(e.graph.count(NodeKind::Episode), episodes + 1, "that it was tried is remembered");
    assert!(e.believed_recipe("wash-mug").is_none());
}

#[test]
fn a_recipe_that_passed_is_believed_and_can_be_read_back() {
    let mut e = entity();
    let recipe = Recipe::from_text(MUG).unwrap();
    let beliefs = e.graph.count(NodeKind::Belief);
    // first a failure, then a pass: only the pass binds
    e.consider_recipe("wash-mug", &recipe, &Trial { passed: false, detail: "timeout".into() });
    assert!(e.believed_recipe("wash-mug").is_none());
    let id = e
        .consider_recipe("wash-mug", &recipe, &Trial { passed: true, detail: "covered 1.00".into() })
        .expect("a passed recipe is believed");
    assert_eq!(e.graph.count(NodeKind::Belief), beliefs + 1);
    let back = e.believed_recipe("wash-mug").unwrap();
    assert_eq!(back.to_text(), recipe.to_text());
    assert!(e.believed_recipe("wash-plate").is_none());
    // the belief rests on the episode that tested it
    let why = e.graph.backlinks(id, brainos_memory::graph::EdgeKind::Supports);
    assert!(why.iter().any(|n| n.content.contains("passed")), "nothing supports the belief");
    assert!(!why.iter().any(|n| n.content.contains("failed")), "the failed trial is not evidence for it");
}
