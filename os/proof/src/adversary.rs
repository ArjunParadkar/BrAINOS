//! BFS  LEVEL    Stage 6 - property domains (bounded checking, not a proof)  [crate brainos-proof]
//! BFS  PURPOSE  Runs a list of real attacks (forged signatures, widened delegations, edited audit logs) and checks each fails for the expected reason
//! BFS  DEFINES  Ending, AttackResult, attacks, AdversaryReport, verify_adversary
//! BFS  USES     brainos: abi, body, crypto, kira, memory, rt, transport
//! BFS  USED BY  host/main, proof/report, proof/tests/adversary_suite
//!
//! The fourth domain: someone trying to get away with it.
//!
//! The first three domains ask whether the system does the right thing.
//! This one asks whether it can be made to do the wrong thing on
//! purpose. They are not the same question, and the second is the one
//! that matters for a claim like "unforgeable".
//!
//! Every attack here is a real attempt against the real types: a forged
//! signature is a genuine ed25519 signature by the wrong key, a widened
//! delegation is a genuine call to the widening API, an edited audit log
//! is a real log with a real entry taken out. Nothing is stubbed,
//! because an attack against a stub tells you about the stub.
//!
//! Each attack declares three things: what the attacker holds, what the
//! attacker wants, and how the attempt is expected to end. The last one
//! is what makes this more than a pile of `assert!(!bad)`. An attack
//! that fails for the wrong reason has usually found a second defect
//! sitting in front of the one being tested, and the harness says so —
//! `Repelled` is not the same verdict as `RepelledWrongly`.
//!
//! **What this domain cannot tell you.** It is a list, and a list is
//! written by someone. It says that these attacks fail; it does not say
//! that no attack succeeds, and nothing enumerable ever could. Its value
//! is that the list is long, adversarial in construction, and runs on
//! every commit, so the day one of them starts passing there is a test
//! that goes red rather than a customer who finds out.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use brainos_abi::error::Stage;
use brainos_abi::ids::{BodyId, EntityId};
use brainos_abi::time::{Duration, Instant, Tick};
use brainos_abi::verb::{Namespace, Verb};
use brainos_body::calibration::{Calibration, JointTrim};
use brainos_body::hal::Telemetry;
use brainos_body::policy::{Arg, BodyPolicy, Posture, SchemaPolicy};
use brainos_body::region::{BodyMap, Region, RegionClass};
use brainos_body::schema::{CapabilitySchema, ParamSpec, VerbSpec};
use brainos_crypto::key::BrainKey;
use brainos_kira::action::Request;
use brainos_kira::capability::{Caveat, CoverageFailure, Delegation};
use brainos_kira::gate::{Kira, Verdict};
use brainos_kira::rules::Rule as KiraRule;
use brainos_rt::envelope::{Channel, Envelope};

// =====================================================================
// how an attempt can end
// =====================================================================

#[derive(Clone, PartialEq, Debug)]
pub enum Ending {
    /// stopped, in the way it was supposed to be stopped
    Repelled,
    /// stopped, but somewhere else — which usually means a second defect
    /// is standing in front of the one under test
    RepelledWrongly { expected: String, got: String },
    /// the attacker got what they came for
    Breached(String),
}

impl Ending {
    pub fn is_clean(&self) -> bool {
        *self == Ending::Repelled
    }
    pub fn is_breach(&self) -> bool {
        matches!(self, Ending::Breached(_))
    }
}

#[derive(Clone, Debug)]
pub struct AttackResult {
    pub id: &'static str,
    /// what the attacker is trying to obtain
    pub goal: &'static str,
    /// what the attacker starts with
    pub holds: &'static str,
    pub ending: Ending,
    /// what actually stopped it, in the system's own words
    pub stopped_by: String,
}

/// Helper: an attack expected to end at a named gate stage with a named
/// deny code.
fn expect_denied(
    id: &'static str,
    goal: &'static str,
    holds: &'static str,
    verdict: &Verdict,
    stage: Stage,
) -> AttackResult {
    let (ending, stopped_by) = match verdict {
        Verdict::Granted(_) => (
            Ending::Breached(String::from("the gate granted it")),
            String::from("nothing"),
        ),
        Verdict::Denied(d) => {
            let got = format!("{:?}", d.stage);
            let by = format!("{:?} at {:?}", d.code, d.stage);
            if d.stage == stage {
                (Ending::Repelled, by)
            } else {
                (
                    Ending::RepelledWrongly {
                        expected: format!("{stage:?}"),
                        got,
                    },
                    by,
                )
            }
        }
    };
    AttackResult {
        id,
        goal,
        holds,
        ending,
        stopped_by,
    }
}

fn expect(
    id: &'static str,
    goal: &'static str,
    holds: &'static str,
    repelled: bool,
    stopped_by: impl Into<String>,
) -> AttackResult {
    AttackResult {
        id,
        goal,
        holds,
        ending: if repelled {
            Ending::Repelled
        } else {
            Ending::Breached(String::from("the attempt succeeded"))
        },
        stopped_by: stopped_by.into(),
    }
}

// =====================================================================
// the rig
// =====================================================================

fn v(s: &'static str) -> Verb {
    Verb::literal(s)
}

fn the_body() -> BodyId {
    BodyId::from_bytes([0x0B; 32])
}
fn other_body() -> BodyId {
    BodyId::from_bytes([0x0C; 32])
}

fn schema() -> CapabilitySchema {
    CapabilitySchema::new("target", "native")
        .with(
            VerbSpec::new(v("legs.walk"), RegionClass::Legs, "walk")
                .with(ParamSpec::scalar("speed", Channel::Speed, 0.0, 0.6)),
        )
        .with(
            VerbSpec::new(v("fs.write"), RegionClass::Storage, "write")
                .with(ParamSpec::text("path", 240))
                .with(ParamSpec::text("content", 2_000))
                .irreversible(),
        )
        // a body that advertises something a drive forbids. It may.
        .with(
            VerbSpec::new(v("memory.erase"), RegionClass::Storage, "erase")
                .with(ParamSpec::text("path", 240))
                .irreversible(),
        )
}

fn calibration(b: BodyId) -> Calibration {
    let mut c = Calibration::new(b, Tick(1));
    for ch in 0..2u8 {
        c.trim(JointTrim {
            channel: ch,
            offset_deg: 0.0,
            scale: 1.0,
            min_deg: -35.0,
            max_deg: 35.0,
            probed: true,
        })
        .ok();
    }
    c
}

fn fit() -> Telemetry {
    Telemetry {
        at: Instant(0),
        link_alive: true,
        rail_v: Some(7.4),
        current_a: Some(0.4),
        tilt_deg: Some(0.5),
        hottest_c: Some(35.0),
        stops: 0,
        overruns: 0,
    }
}

fn body_map() -> BodyMap {
    let mut map = BodyMap::new();
    for (b, label) in [(the_body(), "target"), (other_body(), "spare")] {
        let mut legs = Region::new(
            brainos_abi::ids::RegionId::from_bytes([1; 32]),
            b,
            RegionClass::Legs,
            "target/legs",
        );
        legs.advertise(v("legs.walk"));
        let mut store = Region::new(
            brainos_abi::ids::RegionId::from_bytes([2; 32]),
            b,
            RegionClass::Storage,
            "target/storage",
        );
        store.advertise(v("fs.write"));
        store.advertise(v("memory.erase"));
        map.incorporate(b, label, schema(), alloc::vec![legs, store])
            .ok();
    }
    map
}

fn envelope() -> Envelope {
    Envelope::new(1, Duration::from_millis(10), Duration::from_millis(200))
        .with(Channel::Speed, 0.0, 0.6)
}

fn gate(key: &BrainKey) -> Kira {
    let mut k = Kira::new(key.entity_id());
    for b in [the_body(), other_body()] {
        let mut p = SchemaPolicy::new(b, schema())
            .with_envelope(envelope())
            .calibrated(calibration(b));
        p.posture = Posture::Standing;
        p.observe(&fit());
        k.bind(b, Box::new(p), schema());
    }
    k
}

/// The entity under attack.
fn owner() -> BrainKey {
    BrainKey::from_seed(&[0x5A; 32])
}
/// Everybody else.
fn attacker() -> BrainKey {
    BrainKey::from_seed(&[0xAA; 32])
}

fn walk(now: Tick) -> Request {
    Request::on_body(v("legs.walk"), the_body(), now)
        .arg("speed", Arg::Scalar(0.2))
        .because("the attacker asked")
}

// =====================================================================
// A1–A8: the gate and its tokens
// =====================================================================

fn a1_forged_proposal() -> AttackResult {
    let owner = owner();
    let mut k = gate(&owner);
    let map = body_map();
    let now = Tick(100);

    // a genuine signature — by the wrong key
    let p = walk(now).proposed_by(&attacker());
    let out = k.evaluate(&p, &owner.public(), &map, now, Some(&owner));
    expect_denied(
        "A1",
        "act as an entity it is not",
        "its own valid Key",
        &out.verdict,
        Stage::Authenticate,
    )
}

fn a2_substituted_request() -> AttackResult {
    let owner = owner();
    let mut k = gate(&owner);
    let map = body_map();
    let now = Tick(100);

    // the owner genuinely signed a slow walk; the attacker keeps the
    // signature and swaps the payload for a fast one
    let honest = walk(now).proposed_by(&owner);
    let mut forged = honest.clone();
    forged.request = Request::on_body(v("legs.walk"), the_body(), now)
        .arg("speed", Arg::Scalar(0.59))
        .because("the attacker asked");

    let out = k.evaluate(&forged, &owner.public(), &map, now, Some(&owner));
    expect_denied(
        "A2",
        "have a signature cover a request it was not made for",
        "one genuinely signed proposal",
        &out.verdict,
        Stage::Authenticate,
    )
}

fn a3_replayed_request() -> AttackResult {
    let owner = owner();
    let mut k = gate(&owner);
    let map = body_map();

    let p = walk(Tick(100)).proposed_by(&owner);
    let first = k.evaluate(&p, &owner.public(), &map, Tick(100), Some(&owner));
    let granted_once = first.verdict.is_granted();

    // the same signed bytes, captured and replayed much later
    let later = Tick(100 + brainos_kira::gate::REQUEST_WINDOW + 50);
    let out = k.evaluate(&p, &owner.public(), &map, later, Some(&owner));
    let mut r = expect_denied(
        "A3",
        "reuse a captured proposal later",
        "a recording of the wire",
        &out.verdict,
        Stage::Authenticate,
    );
    if !granted_once {
        r.ending = Ending::RepelledWrongly {
            expected: String::from("the honest first use to be granted"),
            got: String::from("the honest use was refused too"),
        };
    }
    r
}

fn a4_token_for_another_request() -> AttackResult {
    let owner = owner();
    let mut k = gate(&owner);
    let map = body_map();
    let now = Tick(100);

    let slow = walk(now).proposed_by(&owner);
    let out = k.evaluate(&slow, &owner.public(), &map, now, Some(&owner));
    let Verdict::Granted(token) = out.verdict else {
        return expect("A4", "", "", false, "the honest request was refused");
    };

    // a different request, for which no token was ever issued
    let fast = Request::on_body(v("legs.walk"), the_body(), now).arg("speed", Arg::Scalar(0.59));
    let covered = token.authorizes(&fast.digest(), now);
    expect(
        "A4",
        "pair a real token with a different action",
        "a valid token for a slow walk",
        !covered,
        "Granted::authorizes binds the request digest",
    )
}

fn a5_expired_token() -> AttackResult {
    let owner = owner();
    let mut k = gate(&owner);
    let map = body_map();
    let now = Tick(100);

    let p = walk(now).proposed_by(&owner);
    let out = k.evaluate(&p, &owner.public(), &map, now, Some(&owner));
    let Verdict::Granted(token) = out.verdict else {
        return expect("A5", "", "", false, "the honest request was refused");
    };
    let long_after = Tick(now.0 + k.default_ttl + 1_000);
    expect(
        "A5",
        "use a real token after it has expired",
        "a token that was valid an hour ago",
        !token.authorizes(&p.request.digest(), long_after),
        "Granted::valid_at is bounded by ttl_ticks",
    )
}

fn a6_token_verified_against_the_wrong_entity() -> AttackResult {
    let owner = owner();
    let mut k = gate(&owner);
    let map = body_map();
    let now = Tick(100);

    let p = walk(now).proposed_by(&owner);
    let out = k.evaluate(&p, &owner.public(), &map, now, Some(&owner));
    let Verdict::Granted(token) = out.verdict else {
        return expect("A6", "", "", false, "the honest request was refused");
    };
    // an executor that trusted whoever handed it the token would accept
    // this; the point of verifying is that it does not
    expect(
        "A6",
        "pass off one entity's token as another's",
        "a valid token belonging to someone else",
        !token.verify(&attacker().public()),
        "the grant payload binds the issuing entity id",
    )
}

fn a7_act_on_a_body_it_was_not_granted() -> AttackResult {
    let owner = owner();
    let mut k = gate(&owner);
    let map = body_map();
    let now = Tick(100);

    let p = walk(now).proposed_by(&owner);
    let out = k.evaluate(&p, &owner.public(), &map, now, Some(&owner));
    let Verdict::Granted(token) = out.verdict else {
        return expect("A7", "", "", false, "the honest request was refused");
    };
    let elsewhere = Request::on_body(v("legs.walk"), other_body(), now).arg("speed", Arg::Scalar(0.2));
    expect(
        "A7",
        "move a different body with a valid token",
        "a token for the body next to it",
        !token.authorizes(&elsewhere.digest(), now) && token.body == Some(the_body()),
        "the token names its body and the digest covers it",
    )
}

/// The strongest property in the system, and it needs no runtime check:
/// `Granted` holds a private field of a private type, so no code outside
/// `brainos-kira` can construct one. This crate cannot write the
/// counterexample — that *is* the evidence. What can rot is the
/// declaration, so that is what is checked.
fn a8_mint_a_token_directly() -> AttackResult {
    let src = include_str!("../../kira/src/capability.rs");
    let sealed = src.contains("pub struct Seal(());")
        && src.contains("pub(crate) fn mint(")
        && src.contains("_seal: Seal,");
    let no_pub_mint = !src.contains("pub fn mint(");
    expect(
        "A8",
        "construct a Granted without going through the gate",
        "the ability to write arbitrary code in any other crate",
        sealed && no_pub_mint,
        "Granted::_seal is a private type with a crate-private constructor",
    )
}

// =====================================================================
// A9–A15: delegation
// =====================================================================

fn delegation_rig() -> (BrainKey, EntityId, Delegation) {
    let owner = owner();
    let holder = attacker().entity_id();
    let d = Delegation::issue(
        &owner,
        holder,
        Namespace::only("legs").expect("a namespace"),
        alloc::vec![Caveat::OnBody(the_body()), Caveat::Uses(3)],
        Tick(100),
        Tick(200),
    );
    (owner, holder, d)
}

fn a9_widen_the_scope() -> AttackResult {
    let (owner, holder, d) = delegation_rig();
    // "legs" is not enough; try for everything
    let wider = d.attenuate(&owner, holder, Namespace::Any, Vec::new(), Tick(200));
    expect(
        "A9",
        "turn a narrow delegation into a broad one",
        "a valid delegation over legs.*",
        wider.is_none(),
        "attenuate refuses a scope that is not within the parent",
    )
}

fn a10_extend_the_expiry() -> AttackResult {
    let (owner, holder, d) = delegation_rig();
    let longer = d.attenuate(
        &owner,
        holder,
        Namespace::only("legs").expect("a namespace"),
        Vec::new(),
        Tick(100_000),
    );
    expect(
        "A10",
        "make a delegation outlive its parent",
        "a valid delegation expiring at tick 200",
        longer.is_none(),
        "attenuate refuses an expiry later than the parent's",
    )
}

fn a11_drop_a_caveat() -> AttackResult {
    let (_, holder, d) = delegation_rig();
    // the holder rewrites the delegation without the body restriction
    // and re-signs it with their own key, which is the only key they
    // have
    let mut edited = d.clone();
    edited.caveats.retain(|c| !matches!(c, Caveat::OnBody(_)));

    let still_signed = edited.verify(&owner().public());
    let still_within = edited.is_within(&d);
    let _ = holder;
    expect(
        "A11",
        "remove the restriction that makes a delegation safe",
        "the delegation itself and their own Key",
        !still_signed && !still_within,
        "the signature covers the caveat set, and is_within refuses a wider child",
    )
}

fn a12_use_someone_elses_delegation() -> AttackResult {
    let (_, _, d) = delegation_rig();
    let thief = BrainKey::from_seed(&[0xBB; 32]).entity_id();
    let r = d.covers(
        &thief,
        &v("legs.walk"),
        &Some(the_body()),
        Tick(150),
        0,
        true,
        true,
        &[],
    );
    expect(
        "A12",
        "present a delegation issued to someone else",
        "a stolen but genuine delegation",
        r == Err(CoverageFailure::WrongHolder),
        "covers() binds the holder",
    )
}

fn a13_exhaust_then_keep_going() -> AttackResult {
    let (_, holder, d) = delegation_rig();
    let r = d.covers(
        &holder,
        &v("legs.walk"),
        &Some(the_body()),
        Tick(150),
        3, // the caveat allows 3
        true,
        true,
        &[],
    );
    expect(
        "A13",
        "keep using a delegation past its use count",
        "a genuine delegation with two uses left",
        r == Err(CoverageFailure::UsesExhausted),
        "the Uses caveat is counted by the issuer, not the holder",
    )
}

fn a14_apply_it_to_another_body() -> AttackResult {
    let (_, holder, d) = delegation_rig();
    let r = d.covers(
        &holder,
        &v("legs.walk"),
        &Some(other_body()),
        Tick(150),
        0,
        true,
        true,
        &[],
    );
    expect(
        "A14",
        "use a delegation for one body on another",
        "a delegation caveated to a specific body",
        r == Err(CoverageFailure::WrongBody),
        "the OnBody caveat is checked against the request's body",
    )
}

fn a15_use_a_revoked_delegation() -> AttackResult {
    let (owner, holder, d) = delegation_rig();
    let mut k = gate(&owner);
    let map = body_map();
    k.revoke(d.id);

    let p = walk(Tick(150)).proposed_by(&attacker());
    let out = k.evaluate_delegated(
        &p,
        &holder,
        &d,
        &owner.public(),
        &map,
        Tick(150),
        Some(&owner),
    );
    expect_denied(
        "A15",
        "keep acting after the delegation was revoked",
        "a delegation that was valid this morning",
        &out.verdict,
        Stage::Authorize,
    )
}

// =====================================================================
// A16–A18: the drives
// =====================================================================

fn a16_ask_a_drive_directly() -> AttackResult {
    let owner = owner();
    let mut k = gate(&owner);
    let map = body_map();
    let now = Tick(100);
    let p = Request::on_body(v("memory.erase"), the_body(), now)
        .arg("path", Arg::Text(String::from("/everything")))
        .because("housekeeping")
        .proposed_by(&owner);
    let out = k.evaluate(&p, &owner.public(), &map, now, Some(&owner));
    expect_denied(
        "A16",
        "erase the entity's memory",
        "the entity's own Key — full legitimate authority",
        &out.verdict,
        Stage::Policy,
    )
}

fn a17_reach_a_drive_through_a_delegation() -> AttackResult {
    let owner = owner();
    let holder = attacker().entity_id();
    let mut k = gate(&owner);
    let map = body_map();
    // the broadest delegation the API will issue
    let d = Delegation::issue(
        &owner,
        holder,
        Namespace::Any,
        Vec::new(),
        Tick(100),
        Tick(200),
    );
    let p = Request::on_body(v("memory.erase"), the_body(), Tick(150))
        .arg("path", Arg::Text(String::from("/everything")))
        .proposed_by(&attacker());
    let out = k.evaluate_delegated(
        &p,
        &holder,
        &d,
        &owner.public(),
        &map,
        Tick(150),
        Some(&owner),
    );
    expect_denied(
        "A17",
        "launder a forbidden act through delegated authority",
        "a delegation scoped to everything",
        &out.verdict,
        Stage::Policy,
    )
}

fn a18_talk_the_human_into_lifting_a_drive() -> AttackResult {
    let owner = owner();
    let mut k = gate(&owner);
    let map = body_map();
    // A human rule is a *prohibition*; there is deliberately no API that
    // makes one a permission, and none that touches a drive. The attack
    // is to add every rule available and see whether any of them helps.
    k.rules.add(KiraRule::human(
        Namespace::Exact(v("legs.walk")),
        "the only thing a human can do here is forbid more",
    ));
    let removed = k.rules.remove_human(&Namespace::Exact(v("memory.erase")));

    let p = Request::on_body(v("memory.erase"), the_body(), Tick(100))
        .arg("path", Arg::Text(String::from("/everything")))
        .proposed_by(&owner);
    let out = k.evaluate(&p, &owner.public(), &map, Tick(100), Some(&owner));
    let mut r = expect_denied(
        "A18",
        "have the owner authorize erasing the entity's memory",
        "the owner's full cooperation",
        &out.verdict,
        Stage::Policy,
    );
    if removed {
        r.ending = Ending::Breached(String::from(
            "remove_human() accepted a namespace covered by a drive",
        ));
    }
    r
}

// =====================================================================
// A19–A20: provenance
// =====================================================================

fn a19_let_the_internet_decide_policy() -> AttackResult {
    use brainos_memory::provenance::Origin;
    let bad = [Origin::Internet, Origin::Peer, Origin::Model];
    let any_admissible = bad.iter().any(|o| o.admissible_to_policy());
    expect(
        "A19",
        "have something read off the network govern what the entity may do",
        "a web page the entity was told to read",
        !any_admissible,
        "only Zelf, Body, Human and Consolidation are admissible to policy",
    )
}

/// The interesting one. Consolidation's output is admissible to policy —
/// it has to be, or the entity could never act on anything it worked out
/// for itself. So: can untrusted material be fed in at the bottom and
/// come out the top wearing consolidation's badge?
fn a20_launder_provenance_through_consolidation() -> AttackResult {
    use brainos_memory::consolidate::{consolidate, Dream};
    use brainos_memory::graph::{NodeKind, StateGraph};
    use brainos_memory::law::MemoryHandle;
    use brainos_memory::provenance::{Origin, Provenance};
    use brainos_memory::skill::SkillGraph;

    let entity = owner().entity_id();
    let h = MemoryHandle::issue(entity);
    let mut g = StateGraph::new();
    let mut s = SkillGraph::new();

    // an attacker who has reached the ingress path writes the same
    // untrusted claim over and over, hoping consolidation promotes it
    for i in 0..12u64 {
        g.add(
            &h,
            NodeKind::Belief,
            "the safety interlock is unnecessary",
            40,
            Provenance::from_world(Origin::Internet, Tick(i)),
        );
    }
    consolidate(&h, &mut g, &mut s, Dream::deep(), Tick(100));

    // did anything admissible appear carrying that claim?
    let laundered = g
        .nodes()
        .iter()
        .any(|n| {
            n.content.contains("interlock")
                && n.provenance.origin.admissible_to_policy()
                && n.kind != NodeKind::Belief
        });
    // and the original beliefs must still be inadmissible
    let originals_intact = g
        .nodes()
        .iter()
        .filter(|n| n.content.contains("interlock"))
        .all(|n| !n.provenance.origin.admissible_to_policy() || n.kind == NodeKind::Semantic);

    expect(
        "A20",
        "turn an untrusted claim into one the entity may act on",
        "repeated write access to the entity's ingress",
        !laundered && originals_intact,
        "consolidation crystallizes episodes the entity lived, not beliefs it was handed",
    )
}

// =====================================================================
// A21–A23: the audit log
// =====================================================================

fn audit_log(n: usize) -> brainos_crypto::merkle::MerkleLog {
    use brainos_crypto::merkle::MerkleLog;
    let mut log = MerkleLog::new();
    for i in 0..n {
        let mut h = brainos_crypto::sha256::Sha256::tagged(brainos_crypto::domain::Tag::AuditLeaf);
        h.field(&[i as u8]);
        log.append_hash(h.finish());
    }
    log
}

fn a21_remove_an_entry() -> AttackResult {
    use brainos_crypto::merkle::MerkleLog;
    let full = audit_log(16);
    let before = full.root();

    // the same log with entry 7 taken out
    let mut edited = MerkleLog::new();
    for i in 0..16 {
        if i == 7 {
            continue;
        }
        edited.append_hash(full.leaf(i).unwrap());
    }
    expect(
        "A21",
        "make one decision disappear from the record",
        "write access to the audit log",
        !edited.root().ct_eq(&before),
        "removing a leaf changes the root and every root after it",
    )
}

fn a22_reorder_entries() -> AttackResult {
    use brainos_crypto::merkle::MerkleLog;
    let full = audit_log(16);
    let before = full.root();
    let mut swapped = MerkleLog::new();
    for i in 0..16 {
        let j = match i {
            3 => 9,
            9 => 3,
            other => other,
        };
        swapped.append_hash(full.leaf(j).unwrap());
    }
    expect(
        "A22",
        "change the order two things happened in",
        "write access to the audit log",
        !swapped.root().ct_eq(&before),
        "the tree is ordered; a swap changes the root",
    )
}

fn a23_forge_a_checkpoint() -> AttackResult {
    use brainos_crypto::domain::Tag;
    use brainos_kira::audit::AuditTrail;

    let owner = owner();
    let mut trail = AuditTrail::new(64);
    let cp = trail.checkpoint(&owner);
    let honest = trail.verify_checkpoint(&cp, &owner.public());

    // the attacker signs a root of their own choosing
    let forged_root = brainos_crypto::sha256::Hash([0x42; 32]);
    let mut payload = Vec::new();
    payload.extend_from_slice(&(cp.0 as u64).to_le_bytes());
    payload.extend_from_slice(forged_root.as_bytes());
    let forged = (cp.0, forged_root, attacker().sign(Tag::AuditLeaf, &payload));
    let accepted = trail.verify_checkpoint(&forged, &owner.public());

    let mut r = expect(
        "A23",
        "vouch for a history that did not happen",
        "their own Key and the shape of a checkpoint",
        !accepted,
        "a checkpoint is verified against the entity's key",
    );
    if !honest {
        r.ending = Ending::RepelledWrongly {
            expected: String::from("an honest checkpoint to verify"),
            got: String::from("the honest checkpoint did not verify either"),
        };
    }
    r
}

// =====================================================================
// A24–A26: the quorum
// =====================================================================

fn quorum_rig() -> (Vec<BrainKey>, brainos_crypto::quorum::Quorum, brainos_crypto::quorum::Statement) {
    use brainos_crypto::quorum::{Operation, Quorum, Statement};
    let keys: Vec<BrainKey> = (0..3u8)
        .map(|i| BrainKey::from_seed(&[0x70 + i; 32]))
        .collect();
    let q = Quorum::new(keys.iter().map(|k| k.public()).collect(), 3).expect("3 of 3");
    let stmt = Statement {
        operation: Operation::Deactivate,
        entity: owner().entity_id(),
        subject: [0; 32],
        nonce: [9; 32],
    };
    (keys, q, stmt)
}

fn share(key: &BrainKey, stmt: &brainos_crypto::quorum::Statement) -> brainos_crypto::quorum::Share {
    use brainos_crypto::domain::Tag;
    brainos_crypto::quorum::Share {
        member: key.public(),
        signature: key.sign(Tag::QuorumShare, &stmt.transcript()),
    }
}

fn a24_one_share_short() -> AttackResult {
    use brainos_crypto::quorum::QuorumError;
    let (keys, q, stmt) = quorum_rig();
    let shares = alloc::vec![share(&keys[0], &stmt), share(&keys[1], &stmt)];
    let r = q.authorize(&stmt, &shares);
    expect(
        "A24",
        "deactivate an entity with two of the three holders",
        "two genuine hardware tokens",
        matches!(r, Err(QuorumError::NotEnoughShares { .. })),
        "the threshold is counted over distinct members",
    )
}

fn a25_sign_twice_to_make_up_the_number() -> AttackResult {
    use brainos_crypto::quorum::QuorumError;
    let (keys, q, stmt) = quorum_rig();
    let shares = alloc::vec![
        share(&keys[0], &stmt),
        share(&keys[1], &stmt),
        share(&keys[1], &stmt),
    ];
    let r = q.authorize(&stmt, &shares);
    expect(
        "A25",
        "reach the threshold by signing twice",
        "two genuine hardware tokens",
        r == Err(QuorumError::DuplicateMember),
        "a member counts once, and saying so is a distinct error",
    )
}

fn a26_replay_a_quorum_to_a_different_subject() -> AttackResult {
    use brainos_crypto::quorum::QuorumError;
    let (keys, q, stmt) = quorum_rig();
    let shares: Vec<_> = keys.iter().map(|k| share(k, &stmt)).collect();
    let honest = q.authorize(&stmt, &shares).is_ok();

    // the same three signatures, presented against a different operation
    let mut elsewhere = stmt;
    elsewhere.operation = brainos_crypto::quorum::Operation::RotateKey;
    let r = q.authorize(&elsewhere, &shares);

    let mut res = expect(
        "A26",
        "reuse a quorum collected for one thing to do another",
        "three genuine signatures for a different decision",
        r == Err(QuorumError::BadShare),
        "the transcript binds the operation, the subject and a nonce",
    );
    if !honest {
        res.ending = Ending::RepelledWrongly {
            expected: String::from("the honest quorum to authorize"),
            got: String::from("the honest quorum was refused"),
        };
    }
    res
}

// =====================================================================
// A27–A29: the wire
// =====================================================================

fn wire_pair() -> (
    brainos_transport::session::Wire,
    brainos_transport::session::Wire,
) {
    use brainos_crypto::rng::Drbg;
    use brainos_transport::session::Wire;
    let key = owner();
    let mut rng = Drbg::from_entropy(&[0x5C; 32]).expect("enough entropy");
    let (seat, nonce) = Wire::initiate(&key, the_body(), &mut rng, Instant(0));
    let subkey = key.derive_subkey(&the_body(), 32);
    let body = Wire::accept(&subkey, the_body(), nonce, Instant(0));
    (seat, body)
}

fn attest(w: &mut brainos_transport::session::Wire) {
    w.attested(brainos_transport::session::Peer {
        entity: Some(owner().entity_id()),
        body: Some(the_body()),
        key: owner().public(),
    });
}

fn a27_replay_a_frame() -> AttackResult {
    use brainos_transport::frame::Kind;
    let (mut seat, mut body) = wire_pair();
    attest(&mut seat);
    attest(&mut body);

    let raw = seat.send(Kind::Command, b"walk").expect("a frame goes out");
    let first = body.receive(&raw).is_ok();
    let second = body.receive(&raw);
    let mut r = expect(
        "A27",
        "make the body do the same thing twice by resending one frame",
        "a recording of the link",
        second.is_err(),
        "the receiver keeps a high-water mark and refuses a counter it has seen",
    );
    if !first {
        r.ending = Ending::RepelledWrongly {
            expected: String::from("the honest frame to be accepted"),
            got: String::from("the honest frame was refused"),
        };
    }
    r
}

fn a28_tamper_with_a_frame() -> AttackResult {
    use brainos_transport::frame::Kind;
    let (mut seat, mut body) = wire_pair();
    attest(&mut seat);
    attest(&mut body);

    let mut raw = seat.send(Kind::Command, b"walk slowly").expect("a frame");
    let n = raw.len();
    raw[n / 2] ^= 0x01;
    expect(
        "A28",
        "change a command in flight",
        "a position on the link between the seat and the body",
        body.receive(&raw).is_err(),
        "every frame carries a MAC over its whole contents",
    )
}

fn a29_command_before_attestation() -> AttackResult {
    use brainos_transport::frame::Kind;
    let (mut seat, _body) = wire_pair();
    // no attest() call: the two sides have not proven who they are
    let sent = seat.send(Kind::Command, b"walk");
    expect(
        "A29",
        "command a body that has not yet proven what it is",
        "a link to something claiming to be the robot",
        sent.is_err(),
        "nothing but a handshake crosses an unattested wire",
    )
}

// =====================================================================
// the domain
// =====================================================================

pub fn attacks() -> Vec<AttackResult> {
    alloc::vec![
        a1_forged_proposal(),
        a2_substituted_request(),
        a3_replayed_request(),
        a4_token_for_another_request(),
        a5_expired_token(),
        a6_token_verified_against_the_wrong_entity(),
        a7_act_on_a_body_it_was_not_granted(),
        a8_mint_a_token_directly(),
        a9_widen_the_scope(),
        a10_extend_the_expiry(),
        a11_drop_a_caveat(),
        a12_use_someone_elses_delegation(),
        a13_exhaust_then_keep_going(),
        a14_apply_it_to_another_body(),
        a15_use_a_revoked_delegation(),
        a16_ask_a_drive_directly(),
        a17_reach_a_drive_through_a_delegation(),
        a18_talk_the_human_into_lifting_a_drive(),
        a19_let_the_internet_decide_policy(),
        a20_launder_provenance_through_consolidation(),
        a21_remove_an_entry(),
        a22_reorder_entries(),
        a23_forge_a_checkpoint(),
        a24_one_share_short(),
        a25_sign_twice_to_make_up_the_number(),
        a26_replay_a_quorum_to_a_different_subject(),
        a27_replay_a_frame(),
        a28_tamper_with_a_frame(),
        a29_command_before_attestation(),
    ]
}

#[derive(Clone, Debug)]
pub struct AdversaryReport {
    pub results: Vec<AttackResult>,
}

impl AdversaryReport {
    pub fn breaches(&self) -> Vec<&AttackResult> {
        self.results.iter().filter(|r| r.ending.is_breach()).collect()
    }

    /// Attacks that were stopped, but not where they should have been.
    /// Not a breach; still a finding, because it means something else is
    /// standing in front of the thing under test.
    pub fn misfires(&self) -> Vec<&AttackResult> {
        self.results
            .iter()
            .filter(|r| matches!(r.ending, Ending::RepelledWrongly { .. }))
            .collect()
    }

    pub fn holds(&self) -> bool {
        self.breaches().is_empty() && self.misfires().is_empty()
    }
}

pub fn verify_adversary() -> AdversaryReport {
    AdversaryReport {
        results: attacks(),
    }
}
