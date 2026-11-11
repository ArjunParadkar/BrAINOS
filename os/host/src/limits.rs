//! BFS  LEVEL    Host - the only crate with an operating system (CLI, scenarios, OS nerves)  [crate brainos-host]
//! BFS  PURPOSE  Measures where the system stops scaling (bodies held, memory retrieval, audit log size) and labels each limit as hard, configured or soft
//! BFS  DEFINES  Kind, Limit, Limits, RetrievalPoint, AuditPoint, measure
//! BFS  USES     brainos: abi, adapters, body, crypto, kira, memory, rt, transport | external: std
//! BFS  USED BY  host/tests/limits_suite
//!
//! Where this breaks.
//!
//! Every claim in the architecture has an implied scale, and a claim
//! whose scale has not been measured is a claim with a hole in it. "An
//! entity holds every body at once" — how many? "Retrieval needs no
//! model" — does it still find the right memory at four thousand? "The
//! audit log keeps every leaf forever" — how long is forever, in bytes
//! and in the time it takes to prove something?
//!
//! Part 0 says no artifact ships with a known correctness defect and
//! every claim is backed by something that passes. This module is the
//! other half of that: knowing where the claims stop being true, before
//! somebody finds out in a warehouse.
//!
//! Each limit is reported with what *kind* of limit it is, because the
//! three kinds call for completely different reactions:
//!
//!   **hard**   a constant in the code. Changing it is a code change,
//!              and it is there on purpose.
//!   **bound**  a configured cap. Raising it costs memory and nothing
//!              else, and `Bounds` is where.
//!   **soft**   nothing stops you; it simply gets worse. These are the
//!              dangerous ones, because there is no error to notice.
//!
//! The soft ones are the point of the exercise.

use brainos_abi::ids::{BodyId, EntityId};
use brainos_abi::time::Tick;
use brainos_abi::verb::{Namespace, Verb};
use brainos_crypto::key::BrainKey;
use brainos_crypto::merkle::MerkleLog;
use brainos_crypto::sha256::Sha256;
use brainos_kira::capability::{Caveat, Delegation};
use brainos_memory::graph::{Bounds, NodeKind, StateGraph};
use brainos_memory::index::Index;
use brainos_memory::law::MemoryHandle;
use brainos_memory::provenance::{Origin, Provenance};
use std::time::Instant as Wall;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// a constant in the code
    Hard,
    /// a configured cap; raising it costs memory
    Bound,
    /// nothing stops you, it just gets worse
    Soft,
}

impl Kind {
    pub const fn name(self) -> &'static str {
        match self {
            Kind::Hard => "hard",
            Kind::Bound => "bound",
            Kind::Soft => "soft",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Limit {
    pub what: &'static str,
    pub kind: Kind,
    /// the number, in whatever unit `what` implies
    pub value: String,
    /// what happens at or past it
    pub consequence: String,
    /// where it is set, for the ones that can be changed
    pub set_in: &'static str,
}

#[derive(Clone, Debug)]
pub struct Limits {
    pub limits: Vec<Limit>,
    /// the retrieval-at-scale measurement, which is the soft limit most
    /// likely to bite and the hardest to notice
    pub retrieval: Vec<RetrievalPoint>,
    /// audit proof cost as the log grows
    pub audit: Vec<AuditPoint>,
}

/// Can the entity still find the one memory that matters, among n?
#[derive(Clone, Copy, Debug)]
pub struct RetrievalPoint {
    pub memories: usize,
    /// where the planted memory ranked, 1 being first. 0 means it was
    /// not in the top ten at all.
    pub rank_of_the_needle: usize,
    /// microseconds to build the index
    pub build_us: u64,
    /// microseconds for one query
    pub query_us: u64,
    pub index_bytes: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct AuditPoint {
    pub decisions: usize,
    /// hashes in an inclusion proof
    pub proof_path: usize,
    /// microseconds to recompute the root
    pub root_us: u64,
    /// microseconds to produce one inclusion proof. Still linear: the
    /// path is logarithmic but building it walks the whole tree, and
    /// proving is an after-the-incident operation rather than a hot one
    pub prove_us: u64,
    /// bytes of leaf hashes kept forever
    pub leaf_bytes: usize,
}

fn micros(f: impl FnOnce()) -> u64 {
    let t = Wall::now();
    f();
    t.elapsed().as_micros() as u64
}

/// Grow a graph to `n` memories with one planted needle, and see whether
/// the entity can still find it.
///
/// The distractors are deliberately *similar* — same shape of sentence,
/// same vocabulary, different specifics — because a haystack of
/// unrelated text would make BM25 look better than it is. The needle is
/// the only memory that mentions the thing being asked about.
fn retrieval_at(n: usize) -> RetrievalPoint {
    let h = MemoryHandle::issue(EntityId::from_bytes([0x7A; 32]));
    let mut g = StateGraph::with_bounds(Bounds {
        max_episodes: 200_000,
        max_semantic: 200_000,
        max_beliefs: 200_000,
        max_audit: 200_000,
        max_content: 1_024,
        max_state: 4_096,
    });

    for i in 0..n {
        let content = format!(
            "legs.walk :: crossed the workshop floor near bay {} carrying nothing :: succeeded",
            i % 97
        );
        g.add(
            &h,
            NodeKind::Episode,
            &content,
            90,
            Provenance::from_world(Origin::Body, Tick(i as u64)),
        );
    }
    // the needle, planted in the middle
    let needle = g.add(
        &h,
        NodeKind::Semantic,
        "the calibration jig lives in the second drawer of the blue cabinet",
        90,
        Provenance::own(Tick((n / 2) as u64)),
    );

    let mut idx = Index::new();
    let build_us = micros(|| idx = Index::build(&g));
    let index_bytes = idx.approximate_bytes();

    let mut hits = Vec::new();
    let query_us = micros(|| hits = idx.recall(&g, "where is the calibration jig kept", Tick(n as u64), 10));
    let rank_of_the_needle = hits
        .iter()
        .position(|(node, _)| node.id == needle)
        .map(|i| i + 1)
        .unwrap_or(0);

    RetrievalPoint {
        memories: g.len(),
        rank_of_the_needle,
        build_us,
        query_us,
        index_bytes,
    }
}

fn audit_at(n: usize) -> AuditPoint {
    let mut log = MerkleLog::new();
    for i in 0..n {
        let mut h = Sha256::tagged(brainos_crypto::domain::Tag::AuditLeaf);
        h.field(&(i as u64).to_le_bytes());
        log.append_hash(h.finish());
    }
    let root_us = micros(|| {
        let _ = log.root();
    });
    let prove_us = micros(|| {
        let _ = log.prove(n / 2);
    });
    let proof_path = log.prove(n / 2).map(|p| p.path_len()).unwrap_or(0);
    AuditPoint {
        decisions: n,
        proof_path,
        root_us,
        prove_us,
        leaf_bytes: n * 32,
    }
}

/// How deep can a delegation chain go before attenuation runs out of
/// room to narrow?
fn delegation_depth() -> usize {
    let key = BrainKey::from_seed(&[0x6A; 32]);
    let holder = key.entity_id();
    let mut d = Delegation::issue(
        &key,
        holder,
        Namespace::Any,
        alloc_caveats(),
        Tick(0),
        Tick(10_000),
    );
    let mut depth = 1;
    // each step narrows the expiry by one tick, which is the only
    // dimension that can be narrowed indefinitely
    for step in 1..10_000u64 {
        match d.attenuate(
            &key,
            holder,
            d.scope,
            Vec::new(),
            Tick(10_000 - step),
        ) {
            Some(next) => {
                d = next;
                depth += 1;
            }
            None => break,
        }
        if depth >= 4_096 {
            break;
        }
    }
    depth
}

fn alloc_caveats() -> Vec<Caveat> {
    vec![Caveat::WhileFit]
}

/// How many bodies can one entity hold, and what does each cost?
fn bodies_per_entity() -> (usize, usize) {
    use brainos_body::region::{BodyMap, Region, RegionClass};
    let mut map = BodyMap::new();
    let schema = brainos_adapters::native::mini_blur();
    let mut n = 0usize;
    // stop at a thousand: past that the question is not "can it" but
    // "why would one entity hold a thousand bodies", which is a fleet
    // and belongs to the registry rather than to an entity
    while n < 1_000 {
        let body = BodyId::from_bytes([(n % 251) as u8; 32]);
        let body = BodyId::from_bytes({
            let mut b = *body.as_bytes();
            b[0] = (n & 0xff) as u8;
            b[1] = ((n >> 8) & 0xff) as u8;
            b
        });
        let mut r = Region::new(
            brainos_abi::ids::RegionId::from_bytes([(n % 251) as u8; 32]),
            body,
            RegionClass::Legs,
            "legs",
        );
        r.advertise(Verb::literal("legs.walk"));
        if map.incorporate(body, "body", schema.clone(), vec![r]).is_err() {
            break;
        }
        n += 1;
    }
    (map.body_count(), n)
}

pub fn measure() -> Limits {
    let mut limits = Vec::new();

    let (held, _) = bodies_per_entity();
    limits.push(Limit {
        what: "bodies one entity holds at once",
        kind: Kind::Soft,
        value: format!("{held}+ accepted"),
        consequence: String::from(
            "nothing refuses. The body map is a linear scan, so verb lookup is O(bodies × \
             regions) and an entity with a thousand bodies pays for all of them on every \
             gate evaluation. Past a few dozen this is a fleet, and a fleet belongs to the \
             registry — which cannot command — rather than to one entity",
        ),
        set_in: "no cap; brainos_body::region::BodyMap",
    });

    limits.push(Limit {
        what: "episodes an entity remembers",
        kind: Kind::Bound,
        value: format!(
            "{} by default, {} embedded",
            Bounds::default().max_episodes,
            Bounds::EMBEDDED.max_episodes
        ),
        consequence: String::from(
            "the oldest and least salient are dropped. At ~120 bytes each the default is \
             ~490 KiB, which an ESP32-S3 with 512 KiB of SRAM in total does not have — \
             which is what Bounds::EMBEDDED is for",
        ),
        set_in: "brainos_memory::graph::Bounds",
    });

    limits.push(Limit {
        what: "candidate preconditions per skill",
        kind: Kind::Hard,
        value: format!("{}", brainos_memory::skill::MAX_PRECONDITIONS),
        consequence: String::from(
            "the weakest untested candidate is evicted — a hunch is forgotten before a \
             finding. Unbounded until a skill was serialized into a node, at which point \
             an unbounded skill became an unbounded node",
        ),
        set_in: "brainos_memory::skill::MAX_PRECONDITIONS",
    });

    limits.push(Limit {
        what: "bytes a node's structured form may hold",
        kind: Kind::Bound,
        value: format!("{}", Bounds::default().max_state),
        consequence: String::from(
            "the write is refused and the caller told, rather than truncated. Half a \
             serialization is a corrupt memory, not a smaller one",
        ),
        set_in: "brainos_memory::graph::Bounds::max_state",
    });

    limits.push(Limit {
        what: "delegation chain depth",
        kind: Kind::Soft,
        value: format!("{}+ verified", delegation_depth()),
        consequence: String::from(
            "each link narrows, never widens, so depth costs verification time and \
             nothing else. The practical limit is the expiry: a chain can only be as \
             long as there are ticks to give away",
        ),
        set_in: "no cap; bounded by the expiry being narrowed",
    });

    limits.push(Limit {
        what: "payload in one transport frame",
        kind: Kind::Hard,
        value: format!("{} bytes", brainos_transport::frame::MAX_PAYLOAD),
        consequence: String::from(
            "larger payloads are refused at the sender. Sized so a frame fits inside a \
             single BLE MTU-bounded write with room for the header and MAC",
        ),
        set_in: "brainos_transport::frame::MAX_PAYLOAD",
    });

    limits.push(Limit {
        what: "envelope channels",
        kind: Kind::Hard,
        value: format!("{}", brainos_rt::envelope::Channel::ALL.len()),
        consequence: String::from(
            "a body that needs a ninth physical quantity cannot express it, and an \
             unstated channel is a violation rather than a pass — so the failure is a \
             refusal to move, not an unchecked motion",
        ),
        set_in: "brainos_rt::envelope::Channel",
    });

    limits.push(Limit {
        what: "the retrieval index's own footprint",
        kind: Kind::Soft,
        value: String::from("about 210 bytes per memory"),
        consequence: String::from(
            "measured below rather than estimated. At the default bound of 4,096 \
             episodes the index is ~870 KiB — larger than the memories it indexes, and \
             on its own more than an ESP32-S3's entire 512 KiB of SRAM. The earlier \
             footprint number counted the memories and not the index; the real embedded \
             constraint is the index, and Bounds::EMBEDDED at 256 episodes puts it \
             around 55 KiB",
        ),
        set_in: "follows from Bounds and brainos_memory::index",
    });

    limits.push(Limit {
        what: "producing one audit inclusion proof",
        kind: Kind::Soft,
        value: String::from("linear in the whole history"),
        consequence: String::from(
            "the path is logarithmic but building it walks every level, so proving one \
             decision out of a million takes most of a second. Acceptable because \
             proving happens after an incident rather than in the loop — unlike the \
             root, which every checkpoint needs and which is now logarithmic",
        ),
        set_in: "brainos_crypto::merkle::MerkleLog::prove",
    });

    limits.push(Limit {
        what: "term length for retrieval",
        kind: Kind::Hard,
        value: String::from("8 bytes, truncating"),
        consequence: String::from(
            "words sharing their first eight bytes collide — \"calibration\" and \
             \"calibrated\" are one term. Deliberate: it is a stemmer that needs no \
             dictionary and no model, and the collisions it causes are mostly the ones a \
             stemmer would want",
        ),
        set_in: "brainos_memory::index::TERM_MAX",
    });

    let retrieval: Vec<RetrievalPoint> = [100usize, 1_000, 4_096, 16_384]
        .iter()
        .map(|n| retrieval_at(*n))
        .collect();

    let audit: Vec<AuditPoint> = [1_000usize, 100_000, 1_000_000]
        .iter()
        .map(|n| audit_at(*n))
        .collect();

    Limits {
        limits,
        retrieval,
        audit,
    }
}

impl Limits {
    /// The one that would actually hurt: can the entity still find what
    /// it knows, at the bound it ships with?
    pub fn retrieval_holds_at_default_bounds(&self) -> bool {
        self.retrieval
            .iter()
            .filter(|p| p.memories <= Bounds::default().max_episodes + 1)
            .all(|p| p.rank_of_the_needle == 1)
    }
}
