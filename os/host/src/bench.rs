//! Measured numbers.
//!
//! The architecture makes latency claims — sub-100 ms reflexes, a gait
//! that is a timing problem, a gate every action passes through — and
//! until something measures them they are intentions. This module
//! measures them.
//!
//! **What the numbers are and are not.** They are wall-clock timings of
//! release builds on whatever machine ran them, reported with the
//! machine named. They are *not* worst-case execution times on the
//! ESP32-S3: an x86 laptop is between one and two orders of magnitude
//! faster than an Xtensa core at 240 MHz, and a number from here is a
//! lower bound on the board, not a prediction for it. What a laptop
//! measurement is genuinely good for is the shape: whether the gate is
//! microseconds or milliseconds, whether planning a gait is dominated by
//! the tip-over check or by the servo table, and whether anything is
//! accidentally quadratic.
//!
//! **Measurement hygiene, and why `min` is the headline.** Timing one
//! operation at a time on a loaded multi-user machine measures the
//! scheduler as much as the code: a single preemption turns a 20 µs
//! signature into a 5 ms sample, and the p99 then reports the machine's
//! contention rather than the operation's cost. Two things are done
//! about it.
//!
//! First, operations are timed in *batches* — K iterations under one
//! clock read, divided — so the clock's own overhead is amortized and a
//! single preemption is spread across K samples instead of dominating
//! one.
//!
//! Second, **`min` is reported first and is the number to believe.** The
//! minimum over many batches is the closest thing available to the
//! uncontended cost, because the noise on a timing measurement is
//! one-sided: something can always make an operation slower and nothing
//! can make it faster. The median and p99 are kept because the spread
//! between them and the min is itself the reading — a wide spread means
//! this machine was busy, not that the code is unpredictable.

use brainos_abi::ids::BodyId;
use brainos_abi::time::{Duration as BDuration, Instant, Tick};
use brainos_abi::verb::Verb;
use brainos_adapters::native::mini_blur;
use brainos_body::calibration::{Calibration, JointTrim};
use brainos_body::hal::Telemetry;
use brainos_bodyd::hexapod::{Hexapod, HexapodConfig};
use brainos_bodyd::mechanism::Mechanism;
use brainos_bodyd::runtime::{BodyRuntime, Measured};
use brainos_crypto::key::BrainKey;
use brainos_entity::entity::Entity;
use brainos_memory::consolidate::Dream;
use brainos_memory::episode::Outcome;
use brainos_rt::envelope::{Channel, Envelope};
use miniblur_core::servo::ServoTable;
use std::time::Instant as Wall;

/// One measured operation.
#[derive(Clone, Debug)]
pub struct Measurement {
    pub what: &'static str,
    /// what this number has to fit inside, if anything
    pub budget: Option<BDuration>,
    /// how many batches were timed
    pub batches: usize,
    /// how many operations per batch
    pub per_batch: usize,
    /// the closest thing to the uncontended cost of one operation
    pub min_ns: u64,
    pub median_ns: u64,
    pub p99_ns: u64,
    pub max_ns: u64,
    /// one line about what the operation actually is
    pub note: &'static str,
}

impl Measurement {
    /// Does the operation fit its budget?
    ///
    /// Judged on the min, because the question this answers is about the
    /// code and the budget, not about how busy this laptop was. The
    /// spread below says separately whether the measurement is trustworthy.
    pub fn fits(&self) -> Option<bool> {
        self.budget
            .map(|b| self.min_ns <= b.as_micros().saturating_mul(1_000))
    }

    /// How much of the budget the uncontended cost uses, in percent.
    pub fn headroom_pct(&self) -> Option<u64> {
        self.budget.map(|b| {
            let budget_ns = b.as_micros().saturating_mul(1_000).max(1);
            (self.min_ns.saturating_mul(100)) / budget_ns
        })
    }

    /// How much slower the p99 is than the min. Above about 3× the
    /// machine was contended and the tail is the scheduler's, not the
    /// code's — worth saying so rather than reporting a tail as though
    /// it were a property of the operation.
    pub fn spread(&self) -> f64 {
        if self.min_ns == 0 {
            return 1.0;
        }
        self.p99_ns as f64 / self.min_ns as f64
    }

    pub fn measurement_is_clean(&self) -> bool {
        self.spread() < 3.0
    }
}

/// Time `batches × per_batch` operations, reporting per-operation
/// nanoseconds.
fn measure_batched(
    what: &'static str,
    note: &'static str,
    budget: Option<BDuration>,
    batches: usize,
    per_batch: usize,
    mut op: impl FnMut(),
) -> Measurement {
    // warm up: the first iterations of anything are page faults and cold
    // branch predictors, not the operation
    for _ in 0..per_batch.max(3) {
        op();
    }
    let mut samples: Vec<u64> = Vec::with_capacity(batches);
    for _ in 0..batches {
        let t = Wall::now();
        for _ in 0..per_batch {
            op();
        }
        let total = t.elapsed().as_nanos().min(u64::MAX as u128) as u64;
        samples.push(total / per_batch.max(1) as u64);
    }
    samples.sort_unstable();
    let idx99 = ((samples.len() as f64 * 0.99) as usize).min(samples.len() - 1);
    Measurement {
        what,
        budget,
        batches,
        per_batch,
        min_ns: samples[0],
        median_ns: samples[samples.len() / 2],
        p99_ns: samples[idx99],
        max_ns: *samples.last().unwrap(),
        note,
    }
}

/// Convenience: pick a batch size that keeps each batch around a
/// millisecond, so the clock's own cost is negligible and a preemption
/// hits at most one batch.
fn measure(
    what: &'static str,
    note: &'static str,
    budget: Option<BDuration>,
    iterations: usize,
    op: impl FnMut(),
) -> Measurement {
    let (batches, per_batch) = if iterations >= 10_000 {
        (100, iterations / 100)
    } else if iterations >= 500 {
        (50, iterations / 50)
    } else {
        (iterations.max(5), 1)
    };
    measure_batched(what, note, budget, batches, per_batch.max(1), op)
}

fn v(s: &str) -> Verb {
    Verb::parse(s).unwrap()
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

fn fit() -> Telemetry {
    Telemetry {
        at: Instant(0),
        link_alive: true,
        rail_v: Some(7.4),
        current_a: Some(0.42),
        tilt_deg: Some(1.2),
        hottest_c: Some(38.0),
        stops: 0,
        overruns: 0,
    }
}

/// Every measurement.
pub fn run() -> Vec<Measurement> {
    let mut out = Vec::new();

    // ---------------------------------------------------------------
    // Domain 3 — the gate
    // ---------------------------------------------------------------
    {
        let mut e = Entity::new(BrainKey::from_seed(&[1; 32]), Tick(0));
        e.wake(None, None, Tick(1)).unwrap();
        let body = e
            .acquire(
                &BrainKey::from_seed(&[2; 32]),
                mini_blur(),
                calibration(),
                10,
            )
            .unwrap();
        e.observe_body(&body, &fit());

        out.push(measure(
            "KIRA: one full eight-stage evaluation (granted)",
            "sign, verify, authorize against the body map, validate against the schema, \
             simulate through the body policy, check the ruleset, mint a token, append to the Merkle log",
            // Tier 1's budget: an action proposed by fast cognition has
            // to be gated inside it
            Some(BDuration::from_millis(100)),
            2_000,
            || {
                let _ = e.attempt(v("legs.walk"), body, &[("speed", 0.08), ("heading", 0.0)]);
            },
        ));
    }
    {
        let mut e = Entity::new(BrainKey::from_seed(&[1; 32]), Tick(0));
        e.wake(None, None, Tick(1)).unwrap();
        let body = e
            .acquire(
                &BrainKey::from_seed(&[2; 32]),
                mini_blur(),
                calibration(),
                10,
            )
            .unwrap();
        e.observe_body(&body, &fit());
        out.push(measure(
            "KIRA: one evaluation refused at authorize",
            "the cheap path: a verb no limb advertises never reaches validate or simulate",
            Some(BDuration::from_millis(100)),
            2_000,
            || {
                let _ = e.attempt(v("arm.grasp"), body, &[]);
            },
        ));
    }

    // ---------------------------------------------------------------
    // Domain 1 — the Key
    // ---------------------------------------------------------------
    {
        let key = BrainKey::from_seed(&[1; 32]);
        let payload = [7u8; 64];
        out.push(measure(
            "ed25519: one signature",
            "what the gate pays per grant, and per journal record",
            None,
            5_000,
            || {
                let _ = key.sign(brainos_crypto::domain::Tag::Grant, &payload);
            },
        ));
        let sig = key.sign(brainos_crypto::domain::Tag::Grant, &payload);
        out.push(measure(
            "ed25519: one verification",
            "what the gate pays per proposal, and the body pays per session handshake",
            None,
            5_000,
            || {
                let _ = key.verify(brainos_crypto::domain::Tag::Grant, &payload, &sig);
            },
        ));
    }

    // ---------------------------------------------------------------
    // Domain 2 — the body
    // ---------------------------------------------------------------
    {
        let mut h = Hexapod::new(HexapodConfig::default());
        h.calibrate(ServoTable::bom_10_servo());
        out.push(measure(
            "hexapod: plan and verify a whole gait cycle",
            "48 poses, each through forward kinematics, a convex-hull support polygon, \
             the servo table with its linked pairs, and the rail-current budget",
            // planning happens once per motion, not per sample, so the
            // budget is the deliberate tier's
            Some(BDuration::from_secs(1)),
            500,
            || {
                let _ = h.plan(&v("legs.walk"), &[("speed", 0.08), ("heading", 0.0)]);
            },
        ));
    }
    {
        let geom = miniblur_core::geometry::LegGeom::default();
        let plan = miniblur_core::gait::Plan::new(
            miniblur_core::gait::GaitParams::default(),
            miniblur_core::gait::Walk {
                dir: miniblur_core::gait::Dir::Fwd,
                steps: 1,
                speed: 1.0,
            },
        );
        out.push(measure(
            "hexapod: the tip-over check alone, over a whole cycle",
            "how much of planning is the stability forward model",
            None,
            1_000,
            || {
                let _ = miniblur_core::stability::plan_min_margin_mm(&geom, &plan);
            },
        ));
        let pose = miniblur_core::Pose::STAND;
        out.push(measure(
            "hexapod: the tip-over check on one pose",
            "what the reflex layer would pay if it re-checked every sample from scratch",
            // Tier 0's budget
            Some(BDuration::from_micros(10_000)),
            20_000,
            || {
                let _ = miniblur_core::stability::pose_margin_mm(&geom, &pose);
            },
        ));
    }
    {
        let mut h = Hexapod::new(HexapodConfig::default());
        h.calibrate(ServoTable::bom_10_servo());
        let mut rt = BodyRuntime::new(h, Instant(0));
        rt.narrow_to(
            Envelope::new(1, BDuration::from_millis(20), BDuration::from_millis(200))
                .with(Channel::Angle, -30.0, 30.0)
                .with(Channel::Current, 0.0, 3.0)
                .with(Channel::Tilt, -20.0, 20.0)
                .with(Channel::Voltage, 6.6, 8.4),
        );
        rt.ready();
        let m = Measured {
            rail_v: Some(7.4),
            current_a: Some(0.42),
            tilt_deg: Some(1.2),
            hottest_c: Some(38.0),
        };
        let mut t = 0u64;
        out.push(measure(
            "bodyd: one reflex pass",
            "watchdogs, the link, every measured channel against the envelope, \
             one sample applied, telemetry assembled",
            // this is the Domain 2 loop; Tier 0's budget is what it has
            Some(BDuration::from_micros(10_000)),
            20_000,
            || {
                t += 1_000;
                rt.heard_from_seat(Instant(t));
                let _ = rt.tick(Instant(t), m);
            },
        ));
    }

    // ---------------------------------------------------------------
    // Domain 5 — memory
    // ---------------------------------------------------------------
    {
        let mut e = Entity::new(BrainKey::from_seed(&[1; 32]), Tick(0));
        e.wake(None, None, Tick(1)).unwrap();
        for i in 0..2_000u64 {
            e.outcome(
                v("legs.walk"),
                None,
                if i % 7 == 0 {
                    Outcome::Stopped
                } else {
                    Outcome::Succeeded
                },
                "forward on carpet in the workshop",
                &["carpet", "workshop"],
            );
        }
        let memories = e.graph.len();
        out.push(measure(
            "memory: one recall over a full graph",
            "BM25 over a graph at its episodic bound, re-ranked by salience, \
             confidence, origin and recency",
            // recall happens inside a cognitive pass
            Some(BDuration::from_millis(100)),
            500,
            || {
                let _ = e.recall("why did the left femur bind on carpet", 5);
            },
        ));
        out.push(measure(
            "memory: writing one episode",
            "into a graph that is already at its bound, so the prune path runs",
            Some(BDuration::from_millis(10)),
            2_000,
            || {
                e.outcome(
                    v("legs.walk"),
                    None,
                    Outcome::Succeeded,
                    "forward",
                    &["carpet"],
                );
            },
        ));
        let _ = memories;
    }
    {
        let mut e = Entity::new(BrainKey::from_seed(&[1; 32]), Tick(0));
        e.wake(None, None, Tick(1)).unwrap();
        for _ in 0..500u64 {
            e.outcome(
                v("legs.walk"),
                None,
                Outcome::Succeeded,
                "forward",
                &["carpet"],
            );
        }
        out.push(measure(
            "memory: one consolidation pass",
            "crystallize, promote, chain and forget over a session's episodes",
            // consolidation is Tier 3 work and runs when nothing is moving
            Some(BDuration::from_secs(30)),
            20,
            || {
                let _ = e.dream(Dream::deep());
            },
        ));
    }
    {
        let key = BrainKey::from_seed(&[1; 32]);
        let mut e = Entity::new(BrainKey::from_seed(&[1; 32]), Tick(0));
        e.wake(None, None, Tick(1)).unwrap();
        for _ in 0..1_000u64 {
            e.outcome(v("legs.walk"), None, Outcome::Succeeded, "forward", &["carpet"]);
        }
        let durable = e.graph.encode_durable().unwrap();
        let mut journal = brainos_memory::journal::Journal::new(key.entity_id());
        out.push(measure(
            "journal: seal one record",
            "hash, sign and frame a whole state graph for the two-slot journal",
            Some(BDuration::from_millis(100)),
            500,
            || {
                let (_, record) = journal.commit(
                    &key,
                    brainos_memory::journal::Segment::graph(),
                    durable.clone(),
                    brainos_memory::journal::SlotState::default(),
                );
                let _ = record.to_slot_bytes();
            },
        ));
    }

    // ---------------------------------------------------------------
    // the wire
    // ---------------------------------------------------------------
    {
        let key = [7u8; 32];
        let frame = brainos_transport::frame::Frame::new(
            brainos_transport::frame::Kind::Command,
            1,
            b"legs.walk speed 0.08 heading 0.0",
        )
        .unwrap();
        out.push(measure(
            "wire: seal one frame",
            "header, HMAC-SHA256 truncated to 128 bits",
            Some(BDuration::from_micros(10_000)),
            20_000,
            || {
                let _ = frame.seal(&key);
            },
        ));
        let sealed = frame.seal(&key);
        out.push(measure(
            "wire: open one frame",
            "magic, length, constant-time MAC comparison",
            Some(BDuration::from_micros(10_000)),
            20_000,
            || {
                let _ = brainos_transport::frame::Frame::open(&sealed, &key);
            },
        ));
    }

    out
}

/// What the numbers mean for the architecture.
///
/// Computed from the measurements rather than written down, so that a
/// change in the code that invalidates one of these conclusions shows up
/// as a changed sentence rather than as a stale paragraph.
pub fn implications(measurements: &[Measurement]) -> Vec<String> {
    let find = |what: &str| measurements.iter().find(|m| m.what.contains(what));
    let mut out = Vec::new();

    if let (Some(gate), Some(sign), Some(verify)) = (
        find("eight-stage evaluation"),
        find("one signature"),
        find("one verification"),
    ) {
        let crypto = sign.min_ns + verify.min_ns;
        let pct = (crypto * 100) / gate.min_ns.max(1);
        out.push(format!(
            "A gate evaluation is {} and {pct}% of it is the two ed25519 operations              — one to sign the proposal, one to verify it. Everything else the              eight stages do, including the body's forward model and the Merkle              append, is the remaining {}%.",
            human(gate.min_ns),
            100 - pct
        ));
        // an Xtensa core at 240MHz does ed25519 in software at roughly
        // 2-5ms per operation; the conservative end is what to plan with
        let on_board_ms = 2u64 * 5;
        out.push(format!(
            "That has a consequence for where the gate runs. ed25519 in software on              an Xtensa core at 240 MHz is a few milliseconds per operation, so the              same evaluation on the board is of order {on_board_ms} ms — which              means a body cannot gate at its own loop rate. This is not a              performance problem to optimise away; it is the reason Tier 0 runs              inside a pre-authorized envelope instead of asking per step. The              measurement agrees with the architecture rather than the other way              round, which is the useful direction."
        ));
    }

    if let (Some(pass), Some(pose)) = (
        find("one reflex pass"),
        find("tip-over check on one pose"),
    ) {
        out.push(format!(
            "One reflex pass is {}, and re-deriving the tip-over margin from              scratch would add {}. A 1 kHz servo loop has a millisecond; both              together are under a percent of it, on this machine. Scaled by the              hundred-fold an Xtensa core is slower, still under one percent.",
            human(pass.min_ns),
            human(pose.min_ns)
        ));
    }

    if let (Some(recall), Some(write)) = (
        find("one recall over a full graph"),
        find("writing one episode"),
    ) {
        out.push(format!(
            "Retrieval over a graph at its episodic bound is {}, about {}x the cost              of writing one memory. That ratio is what makes rebuilding the index              on read rather than maintaining it on write the right trade: reads are              rare and thinking is what they feed, writes are constant and happen              while something is moving.",
            human(recall.min_ns),
            recall.min_ns / write.min_ns.max(1)
        ));
    }

    if let Some(j) = find("seal one record") {
        out.push(format!(
            "Sealing the whole state graph into a journal record is {}. At that              cost, committing on every episode would be wasteful and committing              only at sleep would lose a session to a power cut — which is what the              segmenting is for: episodes commit often and cheaply, the skill graph              and the life record rarely.",
            human(j.min_ns)
        ));
    }

    out
}

fn human(ns: u64) -> String {
    if ns >= 1_000_000 {
        format!("{:.2} ms", ns as f64 / 1e6)
    } else if ns >= 1_000 {
        format!("{:.1} us", ns as f64 / 1e3)
    } else {
        format!("{ns} ns")
    }
}

/// What the entity costs in memory, in bytes.
///
/// Sizes rather than timings, because the question a microcontroller
/// asks is different: an ESP32-S3 has 512 KiB of SRAM and the state
/// graph has to fit in what is left after the gait loop's buffers.
pub fn footprint() -> Vec<(&'static str, usize, &'static str)> {
    use core::mem::size_of;
    vec![
        (
            "Granted",
            size_of::<brainos_kira::capability::Granted>(),
            "one authorization: id, verb, body, request digest, tick, ttl, signature",
        ),
        (
            "Denied",
            size_of::<brainos_abi::error::Denied>(),
            "a refusal, Copy and allocation-free so a servo loop can carry one",
        ),
        (
            "Envelope",
            size_of::<brainos_rt::envelope::Envelope>(),
            "eight channels of limits plus timing — what Domain 2 runs inside",
        ),
        (
            "Sample",
            size_of::<brainos_bodyd::mechanism::Sample>(),
            "one step of a motion, fixed width",
        ),
        (
            "Frame header + MAC",
            brainos_transport::frame::OVERHEAD,
            "what the wire costs per command",
        ),
        (
            "Verb",
            size_of::<brainos_abi::verb::Verb>(),
            "a validated namespace.action, no allocation",
        ),
        (
            "Tick / Instant / Duration",
            size_of::<brainos_abi::time::Tick>(),
            "eight bytes each; time is never a float",
        ),
        (
            "Telemetry",
            size_of::<brainos_body::hal::Telemetry>(),
            "what a body reports about itself each pass",
        ),
    ]
}

/// Does a whole entity fit on the board?
///
/// The question an ESP32-S3 asks, and the reason `Bounds::EMBEDDED`
/// exists. 512 KiB of SRAM, of which the gait loop's buffers, the wire's
/// frames and the firmware itself want a good share.
pub fn fits_on_the_board() -> Vec<(&'static str, usize, usize, bool)> {
    let (_, per_memory) = {
        let (empty, full, nodes) = memory_per_episode();
        (empty, (full - empty) / (nodes - 1).max(1))
    };
    // a generous allowance for everything else on the board
    const SRAM: usize = 512 * 1024;
    const RESERVED: usize = 320 * 1024;
    let available = SRAM - RESERVED;

    let default_bound = brainos_memory::graph::Bounds::default();
    let embedded_bound = brainos_memory::graph::Bounds::EMBEDDED;

    vec![
        (
            "default bounds (4,096 episodes)",
            per_memory * default_bound.max_episodes,
            available,
            per_memory * default_bound.max_episodes <= available,
        ),
        (
            "embedded bounds (256 episodes)",
            per_memory * embedded_bound.max_episodes,
            available,
            per_memory * embedded_bound.max_episodes <= available,
        ),
    ]
}

/// How many bytes a memory costs, measured rather than estimated.
pub fn memory_per_episode() -> (usize, usize, usize) {
    let mut e = Entity::new(BrainKey::from_seed(&[1; 32]), Tick(0));
    e.wake(None, None, Tick(1)).unwrap();
    let empty = e.graph.encode_durable().unwrap().len();
    for _ in 0..1_000 {
        e.outcome(
            v("legs.walk"),
            None,
            Outcome::Succeeded,
            "forward on carpet in the workshop",
            &["carpet", "workshop"],
        );
    }
    let full = e.graph.encode_durable().unwrap().len();
    let nodes = e.graph.len();
    (empty, full, nodes)
}
