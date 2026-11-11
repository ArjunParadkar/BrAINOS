//! BFS  LEVEL    Stage 7 - fleet registry and skill marketplace  [crate brainos-fleet]
//! BFS  PURPOSE  Tests the registry, marketplace and hosting, checking that a fleet operator can observe every entity but cannot make any of them act
//! BFS  DEFINES  -
//! BFS  USES     brainos: abi, crypto, fleet
//! BFS  USED BY  (entry point or test)
//!
//! The Stage 7 suite.
//!
//! The property running through all of it: a fleet operator can *see*
//! everything and *make* nothing happen. Every test that looks like a
//! missing feature is that property.

use brainos_abi::ids::{BodyId, EntityId};
use brainos_abi::time::{Duration, Instant, Tick};
use brainos_abi::verb::{Namespace, Verb};
use brainos_crypto::key::{BrainKey, Challenge, ChallengePurpose};
use brainos_crypto::merkle::Root;
use brainos_fleet::hosting::{Host, HostError, HostWorkingSet, Placement, Tenancy};
use brainos_fleet::marketplace::{
    countersign, countersignature_covers, install, Manifest, MarketError, Package,
};
use brainos_fleet::registry::{Health, Registry, RegistryError, Report};

fn v(s: &str) -> Verb {
    Verb::parse(s).unwrap()
}

fn entity(n: u8) -> EntityId {
    BrainKey::from_seed(&[n; 32]).entity_id()
}

fn report(age: u64, competence: u8, stops: u32, at: u64) -> Report {
    Report {
        at: Instant(at),
        audit_root: Root::ZERO,
        bodies: 2,
        granted: 90,
        denied: 10,
        stops,
        competence,
        skills: 12,
        age,
    }
}

// ---------- the registry is a directory ----------

#[test]
fn the_registry_has_no_way_to_make_an_entity_do_anything() {
    // This test documents an absence. Every method on Registry either
    // reads or records something an entity said; there is no command,
    // no push, no configure. The only way to influence an entity is to
    // send it a proposal that faces its own gate.
    let mut r = Registry::default();
    r.register(entity(1), "workshop hexapod", Instant(0)).unwrap();
    let e = r.get(&entity(1)).unwrap();
    assert_eq!(e.label, "workshop hexapod");
    assert!(e.last_report.is_none());
    // and retiring is removal from a directory, not deactivation
    r.retire(&entity(1)).unwrap();
    assert_eq!(r.get(&entity(1)).unwrap().health(Instant(1)), Health::Retired);
}

#[test]
fn a_just_registered_entity_is_quiet_rather_than_lost() {
    let mut r = Registry::default();
    r.register(entity(1), "new", Instant(0)).unwrap();
    // a fleet that marks every fresh deployment as lost cries wolf
    assert_eq!(r.get(&entity(1)).unwrap().health(Instant(1_000)), Health::Quiet);
    // but eventually it is genuinely lost
    let far = Instant(r.default_interval.as_micros() * 4);
    assert_eq!(r.get(&entity(1)).unwrap().health(far), Health::Lost);
}

#[test]
fn health_follows_from_what_the_entity_reported() {
    let mut r = Registry::default();
    r.register(entity(1), "e", Instant(0)).unwrap();
    r.report(&entity(1), report(1_000, 80, 0, 1_000)).unwrap();
    assert_eq!(r.get(&entity(1)).unwrap().health(Instant(1_000)), Health::Well);

    r.report(&entity(1), report(1_100, 80, 3, 2_000)).unwrap();
    assert_eq!(
        r.get(&entity(1)).unwrap().health(Instant(2_000)),
        Health::Recovering
    );
    assert!(Health::Recovering.needs_attention());

    // and silence takes over
    let quiet = Instant(2_000 + r.default_interval.as_micros() + 1);
    assert_eq!(r.get(&entity(1)).unwrap().health(quiet), Health::Quiet);
}

#[test]
fn registering_twice_is_refused() {
    let mut r = Registry::default();
    r.register(entity(1), "e", Instant(0)).unwrap();
    assert_eq!(
        r.register(entity(1), "e again", Instant(1)),
        Err(RegistryError::AlreadyRegistered)
    );
}

#[test]
fn a_report_from_an_unregistered_entity_is_refused() {
    let mut r = Registry::default();
    assert_eq!(
        r.report(&entity(9), report(1, 50, 0, 1)),
        Err(RegistryError::NotFound)
    );
}

#[test]
fn the_operator_sees_what_needs_attention_and_nothing_else_is_noise() {
    let mut r = Registry::default();
    for i in 1..=10u8 {
        r.register(entity(i), "e", Instant(0)).unwrap();
        r.report(&entity(i), report(1_000, 80, if i <= 2 { 1 } else { 0 }, 1_000))
            .unwrap();
    }
    let attention = r.needs_attention(Instant(1_000));
    assert_eq!(attention.len(), 2);
    assert_eq!(r.count_by_health(Instant(1_000), Health::Well), 8);
}

#[test]
fn fleet_numbers_add_up() {
    let mut r = Registry::default();
    for i in 1..=5u8 {
        r.register(entity(i), "e", Instant(0)).unwrap();
        r.report(&entity(i), report(1_000, 60 + i * 4, 0, 1_000)).unwrap();
        r.note_body(&entity(i), BodyId::from_bytes([i; 32])).unwrap();
    }
    let f = r.fleet(Instant(1_000));
    assert_eq!(f.entities, 5);
    assert_eq!(f.reporting, 5);
    assert_eq!(f.bodies, 10, "two bodies each");
    assert_eq!(f.total_skills, 60);
    // mean of 64,68,72,76,80
    assert_eq!(f.mean_competence, 72);
    assert!(f.describe().as_str().contains("5 entities"));
}

#[test]
fn an_entity_that_has_never_reported_does_not_drag_the_fleet_average_down() {
    let mut r = Registry::default();
    r.register(entity(1), "reporting", Instant(0)).unwrap();
    r.report(&entity(1), report(1_000, 90, 0, 1_000)).unwrap();
    r.register(entity(2), "silent", Instant(0)).unwrap();

    let f = r.fleet(Instant(1_000));
    assert_eq!(f.entities, 2);
    assert_eq!(f.reporting, 1);
    assert_eq!(
        f.mean_competence, 90,
        "averaging a zero for an entity that said nothing would be inventing data"
    );
}

// ---------- the moat, at fleet scale ----------

#[test]
fn a_fleet_with_no_spread_in_age_says_nothing_about_age() {
    let mut r = Registry::default();
    for i in 1..=9u8 {
        r.register(entity(i), "e", Instant(0)).unwrap();
        // every entity the same age
        r.report(&entity(i), report(1_000, 50 + i, 0, 1_000)).unwrap();
    }
    assert_eq!(
        r.age_vs_competence(),
        None,
        "the honest answer is None more often than a dashboard would like"
    );
}

#[test]
fn older_entities_being_better_is_visible_when_it_is_true() {
    let mut r = Registry::default();
    // three young, three middle, three old — competence rising with age
    for (i, (age, comp)) in [
        (100u64, 30u8),
        (120, 32),
        (140, 34),
        (5_000, 60),
        (5_200, 62),
        (5_400, 64),
        (50_000, 88),
        (52_000, 90),
        (54_000, 92),
    ]
    .into_iter()
    .enumerate()
    {
        let id = entity(i as u8 + 1);
        r.register(id, "e", Instant(0)).unwrap();
        r.report(&id, report(age, comp, 0, 1_000)).unwrap();
    }
    let (young, old) = r.age_vs_competence().expect("enough spread to say");
    assert_eq!(young, 32);
    assert_eq!(old, 90);
    assert!(old > young, "this is the curve Phase 2 publishes");
}

#[test]
fn too_few_entities_is_also_a_none() {
    let mut r = Registry::default();
    for i in 1..=5u8 {
        r.register(entity(i), "e", Instant(0)).unwrap();
        r.report(&entity(i), report(i as u64 * 10_000, i * 15, 0, 1_000))
            .unwrap();
    }
    assert_eq!(r.age_vs_competence(), None);
}

// ---------- the marketplace ----------

fn manifest(scope: Namespace, verbs: &[&str]) -> Manifest {
    Manifest {
        name: "stair climbing".to_string(),
        version: 3,
        scope,
        verbs: verbs.iter().map(|s| v(s)).collect(),
        claimed_consequence: 60,
        irreversible: true,
        summary: "climb a domestic stair".to_string(),
        learned_in: "domestic stairs, carpeted".to_string(),
        tests_behind_it: 140,
    }
}

/// A package that will not say where it was learned cannot be
/// published.
///
/// The fleet-transfer experiment measured what a marketplace without
/// that field costs: a skill adopted from a deployment unlike your own
/// left the entity permanently *worse* than one that started from
/// nothing, because what an entity learns is never "walking needs
/// carpet", it is "walking needs carpet here". A package the buyer
/// cannot judge is worse than no package, so it is refused at
/// publication — the same place a package that contradicts itself is
/// refused.
#[test]
fn a_package_that_will_not_say_where_it_was_learned_cannot_be_published() {
    let publisher = BrainKey::from_seed(&[11; 32]);
    let mut anonymous = manifest(Namespace::only("legs").unwrap(), &["legs.climb"]);
    anonymous.learned_in = String::new();
    assert!(!anonymous.is_consistent());
    assert!(Package::publish(&publisher, anonymous, Tick(1)).is_none());
}

/// And the buyer, not the marketplace, decides whether somewhere is
/// like here. There is no authority that knows whether two workshops
/// are alike, and one that pretended to would be confidently wrong in
/// exactly the cases that matter.
#[test]
fn whether_a_skill_comes_from_somewhere_like_here_is_the_buyers_call() {
    let m = manifest(Namespace::only("legs").unwrap(), &["legs.climb"]);
    assert!(m.learned_somewhere_like("domestic stairs, carpeted"));
    assert!(m.learned_somewhere_like("Domestic Stairs, Carpeted"));
    assert!(!m.learned_somewhere_like("warehouse, steel mesh"));
}

/// Where it was learned is part of what the publisher signed, so it
/// cannot be edited after the fact by whoever is reselling it.
#[test]
fn the_provenance_is_covered_by_the_publishers_signature() {
    let publisher = BrainKey::from_seed(&[12; 32]);
    let honest = manifest(Namespace::only("legs").unwrap(), &["legs.climb"]);
    let package = Package::publish(&publisher, honest.clone(), Tick(1)).unwrap();
    assert!(package.verify());

    let mut relabelled = package.clone();
    relabelled.manifest.learned_in = "anywhere at all".to_string();
    assert!(
        !relabelled.verify(),
        "a reseller relabelled where a skill was learned and the package still verified"
    );
}

#[test]
fn a_package_that_contradicts_itself_cannot_even_be_published() {
    let publisher = BrainKey::from_seed(&[10; 32]);
    // declares legs.* but includes an arm verb
    let bad = manifest(Namespace::only("legs").unwrap(), &["legs.climb", "arm.grasp"]);
    assert!(!bad.is_consistent());
    assert!(Package::publish(&publisher, bad, Tick(1)).is_none());
}

#[test]
fn a_package_is_authentic_or_it_is_nothing() {
    let publisher = BrainKey::from_seed(&[10; 32]);
    let p = Package::publish(
        &publisher,
        manifest(Namespace::only("legs").unwrap(), &["legs.climb"]),
        Tick(1),
    )
    .unwrap();
    assert!(p.verify());

    // tamper with the manifest after signing
    let mut tampered = p.clone();
    tampered.manifest.claimed_consequence = 0;
    assert!(!tampered.verify());
}

#[test]
fn the_vendor_cannot_install_anything() {
    // This is the test the whole module exists for.
    let publisher = BrainKey::from_seed(&[10; 32]);
    let owner = BrainKey::from_seed(&[11; 32]);
    let robot = entity(20);
    let p = Package::publish(
        &publisher,
        manifest(Namespace::only("legs").unwrap(), &["legs.climb"]),
        Tick(1),
    )
    .unwrap();

    // the publisher signs it; that is not enough
    let publisher_sig = countersign(&publisher, &p, robot);
    assert_eq!(
        install(
            &p,
            robot,
            &owner.public(),
            &publisher_sig,
            &Namespace::Any,
            &[],
            Tick(2)
        ),
        Err(MarketError::NotCountersigned),
        "'the vendor added a capability to your robot overnight' must not be a sentence that can be true"
    );

    // the owner's countersignature is what installs it
    let owner_sig = countersign(&owner, &p, robot);
    assert!(install(
        &p,
        robot,
        &owner.public(),
        &owner_sig,
        &Namespace::Any,
        &[],
        Tick(2)
    )
    .is_ok());
}

#[test]
fn a_countersignature_for_one_robot_does_not_install_on_another() {
    let publisher = BrainKey::from_seed(&[10; 32]);
    let owner = BrainKey::from_seed(&[11; 32]);
    let robot_a = entity(20);
    let robot_b = entity(21);
    let p = Package::publish(
        &publisher,
        manifest(Namespace::only("legs").unwrap(), &["legs.climb"]),
        Tick(1),
    )
    .unwrap();

    let sig = countersign(&owner, &p, robot_a);
    assert!(countersignature_covers(&sig, &p.id, &robot_a, &owner.public()));
    assert!(!countersignature_covers(&sig, &p.id, &robot_b, &owner.public()));
    assert_eq!(
        install(&p, robot_b, &owner.public(), &sig, &Namespace::Any, &[], Tick(2)),
        Err(MarketError::NotCountersigned)
    );
}

#[test]
fn a_package_asking_for_more_than_the_owner_granted_is_refused_before_it_runs() {
    let publisher = BrainKey::from_seed(&[10; 32]);
    let owner = BrainKey::from_seed(&[11; 32]);
    let robot = entity(20);
    let greedy = Package::publish(&publisher, manifest(Namespace::Any, &["legs.climb"]), Tick(1))
        .unwrap();
    let sig = countersign(&owner, &greedy, robot);

    assert_eq!(
        install(
            &greedy,
            robot,
            &owner.public(),
            &sig,
            &Namespace::only("legs").unwrap(),
            &[],
            Tick(2)
        ),
        Err(MarketError::AsksTooMuch),
        "refusing at install is the only cheap moment"
    );
}

#[test]
fn a_withdrawn_package_stops_installing() {
    let publisher = BrainKey::from_seed(&[10; 32]);
    let owner = BrainKey::from_seed(&[11; 32]);
    let robot = entity(20);
    let mut p = Package::publish(
        &publisher,
        manifest(Namespace::only("legs").unwrap(), &["legs.climb"]),
        Tick(1),
    )
    .unwrap();
    let sig = countersign(&owner, &p, robot);
    assert!(install(&p, robot, &owner.public(), &sig, &Namespace::Any, &[], Tick(2)).is_ok());
    p.withdrawn = true;
    assert_eq!(
        install(&p, robot, &owner.public(), &sig, &Namespace::Any, &[], Tick(3)),
        Err(MarketError::Withdrawn)
    );
}

#[test]
fn installing_the_same_package_twice_is_refused() {
    let publisher = BrainKey::from_seed(&[10; 32]);
    let owner = BrainKey::from_seed(&[11; 32]);
    let robot = entity(20);
    let p = Package::publish(
        &publisher,
        manifest(Namespace::only("legs").unwrap(), &["legs.climb"]),
        Tick(1),
    )
    .unwrap();
    let sig = countersign(&owner, &p, robot);
    let a = install(&p, robot, &owner.public(), &sig, &Namespace::Any, &[], Tick(2)).unwrap();
    assert_eq!(
        install(&p, robot, &owner.public(), &sig, &Namespace::Any, &[a.package], Tick(3)),
        Err(MarketError::AlreadyInstalled)
    );
}

#[test]
fn a_package_digest_does_not_depend_on_declaration_order() {
    let a = manifest(Namespace::only("legs").unwrap(), &["legs.climb", "legs.descend"]);
    let b = manifest(Namespace::only("legs").unwrap(), &["legs.descend", "legs.climb"]);
    assert_eq!(a.digest(), b.digest());
}

#[test]
fn a_new_version_is_a_different_package() {
    let mut a = manifest(Namespace::only("legs").unwrap(), &["legs.climb"]);
    let mut b = a.clone();
    b.version = 4;
    assert_ne!(a.digest(), b.digest());
    a.version = 4;
    assert_eq!(a.digest(), b.digest());
}

// ---------- hosted placement ----------

fn attested_host(capacity: usize) -> (Host, BrainKey) {
    let host_key = BrainKey::from_seed(&[30; 32]);
    let customer = entity(1);
    let mut h = Host::new(host_key.public(), "eu-west-1a", capacity);
    let c = Challenge::new([1; 32], ChallengePurpose::BrainPlacement, customer);
    h.attest(&c, &host_key.answer(&c), Instant(0)).unwrap();
    (h, host_key)
}

fn placement(e: EntityId, tier: u8, until: u64) -> Placement {
    Placement {
        entity: e,
        lowest_tier: tier,
        leased_until: Tick(until),
        tenancy: Tenancy::Sole,
        promised: Duration::from_millis(120),
    }
}

#[test]
fn an_unattested_host_runs_nothing() {
    let host_key = BrainKey::from_seed(&[30; 32]);
    let mut h = Host::new(host_key.public(), "somewhere", 10);
    assert_eq!(
        h.place(placement(entity(1), 2, 1_000), Instant(0)),
        Err(HostError::Unattested)
    );
}

#[test]
fn a_placement_attestation_cannot_be_a_replayed_peer_attestation() {
    let host_key = BrainKey::from_seed(&[30; 32]);
    let mut h = Host::new(host_key.public(), "somewhere", 10);
    // the host answered a *peer* challenge, honestly, at some point
    let peer_c = Challenge::new([1; 32], ChallengePurpose::PeerAttestation, entity(1));
    let peer_answer = host_key.answer(&peer_c);
    assert_eq!(
        h.attest(&peer_c, &peer_answer, Instant(0)),
        Err(HostError::Unattested),
        "this is why the purposes are separate"
    );
}

#[test]
fn reflexes_and_fast_reasoning_cannot_be_hosted() {
    let (mut h, _) = attested_host(10);
    // arithmetic, not policy: a 100ms budget does not survive a 120ms
    // round trip
    assert_eq!(
        h.place(placement(entity(1), 0, 1_000), Instant(0)),
        Err(HostError::WrongTier)
    );
    assert_eq!(
        h.place(placement(entity(1), 1, 1_000), Instant(0)),
        Err(HostError::WrongTier)
    );
    assert!(h.place(placement(entity(1), 2, 1_000), Instant(0)).is_ok());
}

#[test]
fn a_host_that_is_slower_than_it_promised_gets_no_new_work() {
    let (mut h, _) = attested_host(10);
    for _ in 0..10 {
        h.observe_latency(Duration::from_millis(900));
    }
    assert_eq!(
        h.place(placement(entity(1), 2, 1_000), Instant(0)),
        Err(HostError::TooSlow)
    );
}

#[test]
fn a_host_fills_up() {
    let (mut h, _) = attested_host(2);
    assert!(h.place(placement(entity(1), 2, 1_000), Instant(0)).is_ok());
    assert!(h.place(placement(entity(2), 2, 1_000), Instant(0)).is_ok());
    assert_eq!(
        h.place(placement(entity(3), 2, 1_000), Instant(0)),
        Err(HostError::AtCapacity)
    );
    assert_eq!(h.load_pct(), 100);
}

#[test]
fn a_lease_expires_rather_than_running_forever() {
    let (mut h, _) = attested_host(10);
    h.place(placement(entity(1), 2, 100), Instant(0)).unwrap();
    assert!(h.serves(&entity(1), Tick(50)));
    assert!(!h.serves(&entity(1), Tick(101)));
    assert_eq!(h.expire(Tick(101)), 1);
    assert!(h.placements().is_empty());
}

#[test]
fn revoking_a_host_is_one_operation_and_everything_carries_on() {
    let (mut h, _) = attested_host(10);
    h.place(placement(entity(1), 2, 10_000), Instant(0)).unwrap();
    h.place(placement(entity(2), 3, 10_000), Instant(0)).unwrap();
    assert_eq!(h.placements().len(), 2);

    h.revoke();
    assert!(h.is_revoked());
    assert!(h.placements().is_empty());
    assert!(!h.serves(&entity(1), Tick(1)));
    assert_eq!(
        h.place(placement(entity(3), 2, 10_000), Instant(0)),
        Err(HostError::Revoked)
    );
    // the entities themselves are untouched: nothing here holds a Key or
    // a state graph, so there is nothing to lose
}

#[test]
fn a_hosts_working_set_is_a_loan_and_expires() {
    let mut w = HostWorkingSet {
        entity: Some(entity(1)),
        context: vec!["the floor is carpet".to_string()],
        ttl_ticks: 100,
        created_at: Tick(0),
    };
    assert!(!w.is_expired(Tick(50)));
    assert!(w.is_expired(Tick(101)));
    w.discard();
    assert!(w.context.is_empty());
    assert!(w.entity.is_none());
}

#[test]
fn a_host_reports_what_it_is_rather_than_what_it_claims() {
    let (mut h, _) = attested_host(4);
    h.observe_latency(Duration::from_millis(80));
    h.place(placement(entity(1), 2, 1_000), Instant(0)).unwrap();
    let d = h.describe();
    assert!(d.as_str().contains("eu-west-1a"));
    assert!(d.as_str().contains("1/4"));
    assert!(d.as_str().contains("80ms"));
}

#[test]
fn shared_tenancy_is_a_distinct_thing_the_customer_can_be_told_about() {
    let (mut h, _) = attested_host(10);
    let mut p = placement(entity(1), 2, 1_000);
    p.tenancy = Tenancy::Shared;
    h.place(p, Instant(0)).unwrap();
    assert_eq!(h.placements()[0].tenancy, Tenancy::Shared);
    assert_ne!(Tenancy::Shared, Tenancy::Sole);
}
