//! BFS  LEVEL    Seed (Stages 1-2) - the portable no_std mind  [crate brainos-mind]
//! BFS  PURPOSE  Tests that a policy bound to one body region supplies KIRA's validate/simulate/policy stages for that body's verbs only, using a toy legged body
//! BFS  DEFINES  -
//! BFS  USES     this crate: common | brainos: mind
//! BFS  USED BY  (entry point or test)
//!
//! Per-body policy (Execution Plan §4.5): KIRA is one pipeline; the
//! invariants are the body's. A policy bound at §8 step 4 supplies stages
//! 4–6 for limb uses through its region — and only tightens. These tests
//! use a toy body whose envelope, forward model and live state are all
//! explicit, so each refusal lands at a named stage.

mod common;

use brainos_mind::body::{Region, RegionClass};
use brainos_mind::kira::{Action, BodyPolicy, Verdict};
use common::bare_instance;

/// A toy two-legged body: `step.walk <n>` walks n steps; more than 5 tips it
/// over in simulation; more than 20 is outside the envelope; and it refuses
/// everything while its link is down.
struct ToyLegs {
    link_alive: bool,
}

impl BodyPolicy for ToyLegs {
    fn validate(&self, verb: &str, target: &str) -> Result<(), String> {
        if verb != "step.walk" {
            return Ok(());
        }
        match target.trim().parse::<u32>() {
            Ok(n) if (1..=20).contains(&n) => Ok(()),
            Ok(n) => Err(format!("{n} steps is outside the envelope (1..=20)")),
            Err(_) => Err(String::from("step.walk wants a step count")),
        }
    }
    fn simulate(&self, verb: &str, target: &str) -> Result<String, String> {
        if verb != "step.walk" {
            return Ok(String::from("no motion"));
        }
        let n: u32 = target.trim().parse().unwrap_or(0);
        if n > 5 {
            Err(format!("forward model: after {n} steps the CoM leaves the support polygon"))
        } else {
            Ok(format!("{n} steps, CoM stays inside the support polygon"))
        }
    }
    fn policy(&self, _verb: &str, _target: &str) -> Result<(), String> {
        if self.link_alive {
            Ok(())
        } else {
            Err(String::from("link to the body is down; standing still"))
        }
    }
    fn observe(&mut self, report: &str) {
        self.link_alive = report != "link-lost";
    }
    fn describe(&self) -> String {
        String::from("toy legs: 1..=20 steps, tips over past 5, needs a live link")
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

fn legged_instance() -> brainos_mind::instance::Instance {
    let mut me = bare_instance();
    me.body.incorporate(Region {
        id: String::from("reach/toy"),
        class: RegionClass::Legs,
        capabilities: vec![String::from("step.walk"), String::from("step.stop")],
        proprioception: String::from("toy legs"),
    });
    me.kira
        .bind_body_policy("reach/toy", Box::new(ToyLegs { link_alive: true }));
    me
}

fn walk(me: &mut brainos_mind::instance::Instance, target: &str) -> (Verdict, Vec<brainos_mind::kira::StageResult>) {
    me.propose(Action::UseLimb { verb: String::from("step.walk"), target: String::from(target) })
}

fn denied_at(v: &Verdict, stage: &str) -> bool {
    matches!(v, Verdict::Denied { stage: s, .. } if *s == stage)
}

#[test]
fn a_good_step_is_granted_and_the_body_policy_is_in_the_trace() {
    let mut me = legged_instance();
    let (v, trace) = walk(&mut me, "3");
    assert!(matches!(v, Verdict::Granted(_)), "3 steps should be granted");
    assert!(trace[3].note.contains("envelope"), "validate note: {}", trace[3].note);
    assert!(trace[4].note.contains("support polygon"), "simulate note: {}", trace[4].note);
}

#[test]
fn outside_the_envelope_dies_at_validate() {
    let mut me = legged_instance();
    let (v, trace) = walk(&mut me, "40");
    assert!(denied_at(&v, "validate"), "expected DENY at validate");
    assert!(!trace[3].ok);
    // simulate and policy still ran (all eight stages always run) but no token
    assert_eq!(trace.len(), 8);
    assert!(!trace[6].ok);
}

#[test]
fn a_step_that_tips_over_dies_at_simulate() {
    let mut me = legged_instance();
    let (v, trace) = walk(&mut me, "9");
    assert!(denied_at(&v, "simulate"), "expected DENY at simulate, got {:?}", trace.iter().map(|s| (s.stage, s.ok)).collect::<Vec<_>>());
    assert!(trace[3].ok, "validate passed (9 is inside 1..=20)");
    assert!(!trace[4].ok);
}

#[test]
fn a_lost_link_dies_at_policy_and_recovers_when_the_link_returns() {
    let mut me = legged_instance();
    me.kira.body_policy_mut("reach/toy").unwrap().observe("link-lost");
    let (v, trace) = walk(&mut me, "2");
    assert!(denied_at(&v, "policy"), "expected DENY at policy");
    assert!(trace[3].ok && trace[4].ok, "validate and simulate were fine");
    assert!(trace[5].note.contains("link"), "{}", trace[5].note);

    me.kira.body_policy_mut("reach/toy").unwrap().observe("link-ok");
    let (v, _) = walk(&mut me, "2");
    assert!(matches!(v, Verdict::Granted(_)));
}

#[test]
fn the_policy_only_applies_to_its_own_region() {
    let mut me = legged_instance();
    // a second body offering the same verb through a different region has no policy bound
    me.body.incorporate(Region {
        id: String::from("reach/other"),
        class: RegionClass::Legs,
        capabilities: vec![String::from("hop.once")],
        proprioception: String::from("other"),
    });
    let (v, trace) = me.propose(Action::UseLimb { verb: String::from("hop.once"), target: String::from("99") });
    assert!(matches!(v, Verdict::Granted(_)));
    assert_eq!(trace[3].note, "within bounds");
}

#[test]
fn releasing_the_region_unbinds_the_policy_and_the_verb() {
    let mut me = legged_instance();
    assert!(me.body.release("reach/toy"));
    assert!(me.kira.unbind_body_policy("reach/toy"));
    assert!(!me.kira.unbind_body_policy("reach/toy"));
    let (v, _) = walk(&mut me, "3");
    // no limb advertises the verb any more → authz, before any policy could speak
    assert!(denied_at(&v, "authz"));
}

#[test]
fn generic_floors_run_before_the_body_policy() {
    // the danger floor is a generic policy check; a body policy cannot loosen it
    let mut me = legged_instance();
    me.body.incorporate(Region {
        id: String::from("reach/toy-shell"),
        class: RegionClass::Compute,
        capabilities: vec![String::from("shell.run")],
        proprioception: String::from("toy shell"),
    });
    me.kira.bind_body_policy("reach/toy-shell", Box::new(ToyLegs { link_alive: true }));
    let (v, trace) = me.propose(Action::UseLimb { verb: String::from("shell.run"), target: String::from("rm -rf /") });
    assert!(denied_at(&v, "policy"));
    assert!(trace[5].note.contains("destructive"), "{}", trace[5].note);
}

#[test]
fn two_bodies_with_the_same_verb_are_gated_by_the_named_one() {
    let mut me = legged_instance();
    // a second, stricter body offering the same verb
    struct NeverWalk;
    impl BodyPolicy for NeverWalk {
        fn validate(&self, _: &str, _: &str) -> Result<(), String> { Ok(()) }
        fn simulate(&self, _: &str, _: &str) -> Result<String, String> { Err(String::from("this body tips on any step")) }
        fn policy(&self, _: &str, _: &str) -> Result<(), String> { Ok(()) }
        fn describe(&self) -> String { String::from("never walks") }
        fn as_any(&self) -> &dyn std::any::Any { self }
    }
    me.body.incorporate(Region {
        id: String::from("reach/tippy"),
        class: RegionClass::Legs,
        capabilities: vec![String::from("step.walk")],
        proprioception: String::from("tippy"),
    });
    me.kira.bind_body_policy("reach/tippy", Box::new(NeverWalk));
    let action = || Action::UseLimb { verb: String::from("step.walk"), target: String::from("2") };
    // first-match (no body named) reaches the first region: toy, which grants 2 steps
    assert!(matches!(me.propose(action()).0, Verdict::Granted(_)));
    // named: tippy's own policy refuses at simulate
    let (v, trace) = me.propose_via(action(), Some("reach/tippy"));
    assert!(denied_at(&v, "simulate"), "{:?}", trace.iter().map(|s| (s.stage, s.ok, s.note.clone())).collect::<Vec<_>>());
    // named: toy still grants
    assert!(matches!(me.propose_via(action(), Some("reach/toy")).0, Verdict::Granted(_)));
    // a body that does not advertise the verb → authz names the body
    let (v, trace) = me.propose_via(Action::UseLimb { verb: String::from("step.stop"), target: String::new() }, Some("reach/tippy"));
    assert!(denied_at(&v, "authz"));
    assert!(trace[2].note.contains("reach/tippy"), "{}", trace[2].note);
    // an unknown body → authz
    let (v, _) = me.propose_via(action(), Some("reach/ghost"));
    assert!(denied_at(&v, "authz"));
}

#[test]
fn bound_regions_are_visible_as_proprioception() {
    let me = legged_instance();
    let b = me.kira.bound_regions();
    assert_eq!(b.len(), 1);
    assert_eq!(b[0].0, "reach/toy");
    assert!(b[0].1.contains("toy legs"));
}
