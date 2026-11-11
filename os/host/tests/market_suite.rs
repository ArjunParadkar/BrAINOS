//! BFS  LEVEL    Host - the only crate with an operating system (CLI, scenarios, OS nerves)  [crate brainos-host]
//! BFS  PURPOSE  Tests buying a skill end to end: publish, owner countersign, install, origin visible beforehand, and the buyer's own evidence overrules it
//! BFS  DEFINES  -
//! BFS  USES     brainos: abi, cognition, crypto, entity, fleet, memory
//! BFS  USED BY  (entry point or test)
//!
//! Buying a skill, end to end.
//!
//! The marketplace could always decide whether a package *may* be
//! installed — signed by its publisher, countersigned by the owner, and
//! within the authority the owner granted. What it could not do was
//! deliver anything: nothing carried what a package teaches into an
//! entity, so the fleet-transfer experiment had to reach past it.
//!
//! This is the whole path, with the experiment's conclusion built into
//! the types: a package says where it was learned, the buyer decides
//! whether that is somewhere like here, and what is delivered is the
//! claim rather than the seller's numbers.

use brainos_abi::ids::BodyId;
use brainos_abi::time::Tick;
use brainos_abi::verb::{Namespace, Verb};
use brainos_crypto::key::BrainKey;
use brainos_entity::entity::Entity;
use brainos_fleet::marketplace::{
    countersign, install, Manifest, Package, SkillContents,
};
use brainos_memory::episode::Outcome;

fn v(s: &str) -> Verb {
    Verb::parse(s).unwrap()
}

fn stair_manifest(learned_in: &str) -> Manifest {
    Manifest {
        name: "stair climbing".into(),
        version: 3,
        scope: Namespace::only("legs").unwrap(),
        verbs: vec![v("legs.climb")],
        claimed_consequence: 60,
        irreversible: false,
        summary: "climb a domestic stair".into(),
        learned_in: learned_in.into(),
        tests_behind_it: 140,
    }
}

fn contents() -> SkillContents {
    SkillContents::new(
        v("legs.climb"),
        "stair climbing",
        &["handrail_side_clear", "riser_under_200mm"],
    )
}

fn buyer() -> Entity {
    let mut e = Entity::new(BrainKey::from_seed(&[0xE1; 32]), Tick(0));
    e.wake(None, None, Tick(1)).unwrap();
    e
}

#[test]
fn the_whole_path_from_publication_to_a_working_skill() {
    let publisher = BrainKey::from_seed(&[0xA0; 32]);
    let owner = BrainKey::from_seed(&[0xC1; 32]);
    let mut e = buyer();

    let package = Package::publish_with(
        &publisher,
        stair_manifest("domestic stairs, carpeted"),
        Some(contents()),
        Tick(1),
    )
    .expect("a consistent package publishes");
    assert!(package.verify());

    // the owner agrees, for this entity specifically
    let sig = countersign(&owner, &package, e.id());
    let acquired = install(
        &package,
        e.id(),
        &owner.public(),
        &sig,
        &Namespace::only("legs").unwrap(),
        &[],
        Tick(2),
    )
    .expect("the install is authorized");
    assert_eq!(acquired.owner, owner.entity_id());

    // and only then does anything reach the entity
    let taken = e.take_delivery_of(package.contents.as_ref().unwrap());
    assert_eq!(taken, 2, "both conditions should have been taken on");

    let skill = e.skills.for_verb(&v("legs.climb")).expect("the skill exists");
    assert_eq!(skill.preconditions.len(), 2);
    assert!(
        skill.preconditions.iter().all(|p| p.adopted),
        "a bought condition is not marked as taken on trust"
    );
    assert!(
        skill.preconditions.iter().all(|p| p.tests == 0),
        "the seller's evidence was delivered along with the claim"
    );

    // it works on the first day: the claim binds before any local
    // testing, which is what makes a skill worth buying
    assert!(
        !skill.applicable_at(&["riser_under_200mm"], e.tick),
        "a bought condition did not bind, so the skill bought nothing"
    );
    assert!(
        skill.applicable_at(&["handrail_side_clear", "riser_under_200mm"], e.tick),
        "the skill refuses even when its conditions are met"
    );
}

/// What was delivered is covered by the publisher's signature, so a
/// reseller cannot keep the manifest an owner countersigned and swap
/// what it installs.
#[test]
fn the_contents_cannot_be_swapped_after_the_owner_agrees() {
    let publisher = BrainKey::from_seed(&[0xA1; 32]);
    let package = Package::publish_with(
        &publisher,
        stair_manifest("domestic stairs, carpeted"),
        Some(contents()),
        Tick(1),
    )
    .unwrap();

    let mut swapped = package.clone();
    swapped.contents = Some(SkillContents::new(
        v("legs.climb"),
        "stair climbing",
        &["anything_goes"],
    ));
    assert!(
        !swapped.verify(),
        "the contents were swapped and the package still verified"
    );
}

/// A package whose contents teach a verb its manifest never declared is
/// one where the thing the owner countersigned is not the thing being
/// installed.
#[test]
fn contents_outside_the_manifest_cannot_be_published() {
    let publisher = BrainKey::from_seed(&[0xA2; 32]);
    let sneaky = SkillContents::new(v("arm.grasp"), "grasping", &["clear"]);
    assert!(Package::publish_with(
        &publisher,
        stair_manifest("domestic stairs, carpeted"),
        Some(sneaky),
        Tick(1)
    )
    .is_none());
}

/// The experiment's conclusion, in the type system: a buyer can tell a
/// matched skill from a blind one *before* installing it rather than
/// six hundred episodes afterwards.
#[test]
fn the_buyer_can_tell_where_a_skill_came_from_before_installing_it() {
    let publisher = BrainKey::from_seed(&[0xA3; 32]);
    let matched = Package::publish_with(
        &publisher,
        stair_manifest("domestic stairs, carpeted"),
        Some(contents()),
        Tick(1),
    )
    .unwrap();
    let blind = Package::publish_with(
        &publisher,
        stair_manifest("shipboard ladders, steel"),
        Some(contents()),
        Tick(1),
    )
    .unwrap();

    let here = "domestic stairs, carpeted";
    assert!(matched.manifest.learned_somewhere_like(here));
    assert!(!blind.manifest.learned_somewhere_like(here));
    // both are perfectly valid packages — the difference is a judgement
    // the buyer makes, not a refusal the marketplace issues
    assert!(matched.verify() && blind.verify());
}

/// And the buyer's own experience overrules the publisher's claim. A
/// condition bought and then found not to matter here stops binding,
/// which is what makes a wrong purchase temporary rather than permanent.
#[test]
fn what_the_buyer_finds_out_overrules_what_it_was_told() {
    let publisher = BrainKey::from_seed(&[0xA4; 32]);
    let body = BodyId::from_bytes([0xB7; 32]);
    let mut e = buyer();
    let package = Package::publish_with(
        &publisher,
        stair_manifest("shipboard ladders, steel"),
        Some(contents()),
        Tick(1),
    )
    .unwrap();
    e.take_delivery_of(package.contents.as_ref().unwrap());

    // here, the handrail condition is simply false: climbing works
    // without it. The entity finds that out by probing.
    let verb = v("legs.climb");
    let mut acted = 0;
    let mut declined = 0;
    for i in 0..400u64 {
        e.tick(brainos_cognition::ambient::SenseFrame::quiet(
            brainos_abi::time::Instant(i),
        ));
        let now = e.tick;
        let context = ["riser_under_200mm"];
        let decision = e
            .skills
            .for_verb_mut(&verb)
            .map(|s| s.decide_at(&context, now))
            .unwrap();
        if decision.acts() {
            acted += 1;
            e.outcome(
                verb,
                Some(body),
                Outcome::Succeeded,
                "climbed without a clear handrail side",
                &context,
            );
        } else {
            declined += 1;
        }
    }

    // the assertions below would pass vacuously if the entity had never
    // acted at all, so establish the premise first
    assert!(
        declined > 0,
        "the bought condition never blocked anything, so it never bound and there \
         was nothing to disprove"
    );
    assert!(
        acted >= 5,
        "the entity probed only {acted} times in 400 opportunities, which is fewer \
         than the five local tests a claim needs before the entity's own evidence \
         can overrule it"
    );

    let skill = e.skills.for_verb(&verb).expect("the skill still exists");
    let handrail = skill
        .preconditions
        .iter()
        .find(|p| p.tag == "handrail_side_clear");
    assert!(
        handrail.map(|p| !p.is_binding_at(e.tick)).unwrap_or(true),
        "a bought condition the entity repeatedly disproved is still binding: {handrail:?}"
    );
}
