//! The entity.
//!
//! One `MemoryHandle`. One `BodyMap`. One `Kira`. One `Life`. Every body
//! is a set of regions in that one map, gated by that one gate, writing
//! into that one graph. "One self with several bodies" is not a feature
//! implemented here — it is what is left when nothing is duplicated.
//!
//! The lifecycle is `wake → tick* → sleep`, and the entity between two
//! wakes is the same entity. `wake` does not construct a self; it
//! re-embodies one. That distinction is why `Entity::new` takes a `Life`
//! — a newly minted entity and one that has lived three years differ in
//! exactly that field and in the contents of the graph, and nowhere
//! else.

use crate::goals::Goals;
use crate::life::{Life, Span};
use crate::simultaneity::Arbiter;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use brainos_abi::error::{Error, Result};
use brainos_abi::ids::{BodyId, EntityId, RegionId, SessionId};
use brainos_abi::text::Text;
use brainos_abi::time::{Instant, Tick};
use brainos_abi::verb::Verb;
use brainos_body::acquire::{default_envelope, Acquisition, Step, StepError};
use brainos_body::calibration::Calibration;
use brainos_body::hal::Telemetry;
use brainos_body::policy::{Arg, SchemaPolicy};
use brainos_body::region::{BodyMap, RegionClass};
use brainos_body::schema::CapabilitySchema;
use brainos_cognition::ambient::{Ambient, Intent, SenseFrame};
use brainos_cognition::escalate::Budget;
use brainos_cognition::placement::Plan;
use brainos_cognition::resource::{ResourcePool, Tier};
use brainos_crypto::key::BrainKey;
use brainos_crypto::rng::Drbg;
use brainos_kira::action::{Proposal, Request};
use brainos_kira::gate::{Kira, Outcome, Verdict};
use brainos_memory::consolidate::{consolidate, ConsolidationReport, Dream};
use brainos_memory::episode::{Episode, Outcome as EpisodeOutcome};
use brainos_memory::graph::{NodeKind, StateGraph};
use brainos_memory::index::Index;
use brainos_memory::law::MemoryHandle;
use brainos_memory::provenance::Provenance;
use brainos_memory::skill::SkillGraph;
use brainos_worlds::advocate::Advocate;
use brainos_worlds::ingress::Ingress;
use brainos_worlds::world::Session;

/// What happened when the entity woke.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Wake {
    /// memories carried in from previous embodiments
    pub inherited: usize,
    /// skills it already had
    pub skills: usize,
    /// how many times it has woken before
    pub previous_wakes: u32,
    /// the last span ended in power loss
    pub after_power_loss: bool,
}

/// A someone.
pub struct Entity {
    key: BrainKey,
    handle: MemoryHandle,

    // memory (Domain 5)
    pub graph: StateGraph,
    pub skills: SkillGraph,
    pub index: Index,

    // bodies (§8)
    pub bodies: BodyMap,
    pub arbiter: Arbiter,

    // the gate (Domain 3)
    pub kira: Kira,

    // cognition (Domain 4)
    pub ambient: Ambient,
    pub resources: ResourcePool,
    pub plan: Plan,

    // the three worlds
    pub ingress: Ingress,
    pub advocate: Advocate,
    sessions: Vec<Session>,

    // the self
    pub goals: Goals,
    pub life: Life,
    span: Span,
    pub tick: Tick,

    rng: Drbg,
    next_region: u64,
    awake: bool,
}

impl Entity {
    /// Mint a brand-new entity. Used once, when a Key is first
    /// provisioned; every subsequent start is a `wake`.
    pub fn new(key: BrainKey, at: Tick) -> Entity {
        let id = key.entity_id();
        let seed = key.derive_subkey(&BodyId::ZERO, 32);
        Entity {
            handle: MemoryHandle::issue(id),
            graph: StateGraph::new(),
            skills: SkillGraph::new(),
            index: Index::new(),
            bodies: BodyMap::new(),
            arbiter: Arbiter::new(),
            kira: Kira::new(id),
            ambient: Ambient::new(Budget::plentiful()),
            resources: ResourcePool::new(),
            plan: Plan::all_on_body(),
            ingress: Ingress::default(),
            advocate: Advocate::default(),
            sessions: Vec::new(),
            goals: Goals::new(),
            life: Life::begin(at),
            span: Span::default(),
            tick: at,
            rng: Drbg::from_entropy(&seed).unwrap_or_else(|_| Drbg::deterministic(b"fallback")),
            next_region: 1,
            key,
            awake: false,
        }
    }

    pub fn id(&self) -> EntityId {
        self.key.entity_id()
    }

    pub fn key(&self) -> &BrainKey {
        &self.key
    }

    /// The memory handle. Public because the entity's own subsystems
    /// need it; there is nothing useful to do with it elsewhere, since
    /// every method that takes one also takes the graph it belongs to.
    pub fn handle(&self) -> &MemoryHandle {
        &self.handle
    }

    pub fn is_awake(&self) -> bool {
        self.awake
    }

    // ------------------------------------------------------------------
    // waking and sleeping
    // ------------------------------------------------------------------

    /// Come back. `durable` is whatever the journal handed back; `life`
    /// is what the entity was before this embodiment.
    pub fn wake(&mut self, durable: Option<&[u8]>, life: Option<Life>, at: Tick) -> Result<Wake> {
        if self.awake {
            return Err(Error::State("already awake"));
        }
        self.tick = at;

        if let Some(bytes) = durable {
            self.graph.load_durable(&self.handle, bytes)?;
            self.skills.rehydrate(&self.graph);
        }
        if let Some(l) = life {
            self.life = l;
        }

        // The counter is the entity's, not the caller's.
        //
        // `at` was being taken at face value, which meant the host could
        // set an entity's age by choosing a number — and age is the
        // x-axis of the curve §2.4 says the moat becomes visible on.
        // A metric that is easy to move without getting better is a
        // metric that will be moved without getting better, so it is not
        // the caller's to move: a returning entity resumes from where it
        // actually stopped.
        //
        // Where it stopped is recovered from its own record — the last
        // completed span, or, for an entity exported while awake and
        // therefore having no closing span, the newest thing it
        // remembers. A memory cannot be later than the counter that
        // stamped it, so the newest provenance is a sound floor.
        let resume = self
            .life
            .last_stopped_at()
            .into_iter()
            .chain(self.graph.nodes().iter().map(|n| n.provenance.at))
            .max();
        if let Some(r) = resume {
            self.tick = r.next();
        }
        self.index = Index::build(&self.graph);
        self.graph.mark_session();

        let after_power_loss = self
            .life
            .recent()
            .last()
            .map(|s| !s.clean)
            .unwrap_or(false);

        self.span = Span {
            woke_at: self.tick,
            ..Span::default()
        };
        self.awake = true;

        // A wake is itself something that happened — and it happened at
        // the entity's tick, not at the caller's. Those used to be the
        // same number; now that the counter is the entity's own, using
        // `at` here would stamp the first memory of an embodiment with
        // a time the entity was never at, and every staleness judgement
        // downstream reads these stamps.
        let now = self.tick;
        Episode::observed(
            if after_power_loss {
                "woke after losing power"
            } else {
                "woke"
            },
            now,
        )
        .record(&self.handle, &mut self.graph, Provenance::own(now));

        Ok(Wake {
            inherited: self.graph.inherited,
            skills: self.skills.len(),
            previous_wakes: self.life.wakes,
            after_power_loss,
        })
    }

    /// Go to sleep cleanly. Returns the bytes to journal and the life to
    /// carry forward.
    ///
    /// Note what is *not* here: nothing is torn down. Sleeping is the
    /// entity putting its bodies away, not a process exiting.
    pub fn sleep(&mut self) -> Result<(Vec<u8>, Life)> {
        if !self.awake {
            return Err(Error::State("not awake"));
        }
        self.span.slept_at = self.tick;
        self.span.clean = true;
        self.span.peak_bodies = self.span.peak_bodies.max(self.bodies.body_count() as u16);

        Episode::observed("went to sleep", self.tick).record(
            &self.handle,
            &mut self.graph,
            Provenance::own(self.tick),
        );
        self.graph.add(
            &self.handle,
            NodeKind::Semantic,
            &self.life.render(self.tick),
            80,
            Provenance::own(self.tick),
        );

        self.life.record(self.span);
        self.awake = false;
        Ok((self.graph.encode_durable()?, self.life.clone()))
    }

    /// The span so far, for telemetry while awake.
    pub fn span(&self) -> Span {
        self.span
    }

    // ------------------------------------------------------------------
    // acquiring bodies (§8)
    // ------------------------------------------------------------------

    fn next_region_id(&mut self) -> RegionId {
        let n = self.next_region;
        self.next_region += 1;
        let mut b = [0u8; 32];
        b[..8].copy_from_slice(&n.to_le_bytes());
        b[8..40.min(32)].copy_from_slice(&self.id().as_bytes()[..24]);
        RegionId::from_bytes(b)
    }

    /// Run §8 against a device that has already attested, binding a
    /// schema-derived policy and an envelope per consequential region.
    ///
    /// The envelope is derived from the schema's own declared bounds
    /// (`default_envelope`), which means a body that would not say what
    /// it can survive gets an envelope that permits nothing — visible,
    /// unmovable, and honest about why.
    pub fn acquire(
        &mut self,
        device: &BrainKey,
        schema: CapabilitySchema,
        calibration: Calibration,
        expected_joints: usize,
    ) -> core::result::Result<BodyId, StepError> {
        let session = SessionId::from_bytes(self.rng.nonce32());
        let mut acq = Acquisition::discover(self.id(), session);

        acq.handshake(
            &self.key,
            &mut self.rng,
            device.public(),
            brainos_abi::version::ABI,
            |c| device.answer(c),
            |_, _| true,
        )?;
        acq.describe(schema.clone())?;

        // the region ids have to be minted before the borrow of self.bodies
        let mut pending: Vec<RegionId> = Vec::new();
        for _ in 0..RegionClass::ALL.len() {
            pending.push(self.next_region_id());
        }
        let mut it = pending.into_iter();
        acq.incorporate(&mut self.bodies, move || {
            it.next().unwrap_or(RegionId::ZERO)
        })?;

        let body = acq.body.expect("set at describe");
        let mut cal = calibration;
        cal.body = body;
        acq.calibrate(cal.clone(), expected_joints, |_| true)?;

        // bind the gate side
        let mut policy = SchemaPolicy::new(body, schema.clone()).calibrated(cal.clone());
        for class in schema.regions().into_iter().filter(|c| c.is_consequential()) {
            let env = default_envelope(
                &schema,
                class,
                (body.as_bytes()[0] as u16) << 8 | class as u16,
                brainos_abi::time::Duration::from_millis(10),
                brainos_abi::time::Duration::from_millis(200),
            );
            policy = policy.with_envelope(env);
        }
        policy.posture = brainos_body::policy::Posture::Standing;
        self.kira.bind(body, Box::new(policy), schema);

        // calibration lives in the state graph, never in the body
        self.graph.add(
            &self.handle,
            NodeKind::Calibration,
            &cal.render(),
            100,
            Provenance::from_body(body, self.tick),
        );
        self.graph.add(
            &self.handle,
            NodeKind::Body,
            &acq.render_join(self.tick),
            100,
            Provenance::own(self.tick),
        );
        self.life.saw_body(body);
        self.span.peak_bodies = self.span.peak_bodies.max(self.bodies.body_count() as u16);
        self.index = Index::build(&self.graph);

        debug_assert_eq!(acq.step, Step::Calibrated);
        Ok(body)
    }

    /// A body leaves. The entity does not.
    pub fn release(&mut self, body: &BodyId) -> bool {
        let released = self.bodies.release(body);
        self.kira.unbind(body);
        self.arbiter.release_all(body);
        if released {
            Episode::observed("a body left", self.tick).record(
                &self.handle,
                &mut self.graph,
                Provenance::own(self.tick),
            );
        }
        released
    }

    /// Telemetry from a body reaches its policy and the forward model.
    pub fn observe_body(&mut self, body: &BodyId, t: &Telemetry) {
        if let Some(p) = self.kira.policy_for_mut(body) {
            p.observe(t);
        }
        if t.stops > 0 {
            self.span.stops = self.span.stops.saturating_add(1);
        }
    }

    // ------------------------------------------------------------------
    // acting
    // ------------------------------------------------------------------

    /// Propose an action. The *only* path from intent to authority.
    ///
    /// Note that this takes `&mut self` and returns an `Outcome` rather
    /// than doing anything: proposing is thinking. Acting happens when
    /// something holds the `Granted` and hands it to a body, which is a
    /// separate step in a separate crate.
    pub fn propose(&mut self, request: Request) -> Outcome {
        let proposal: Proposal = request.proposed_by(&self.key);
        let out = self.kira.evaluate(
            &proposal,
            &self.key.public(),
            &self.bodies,
            self.tick,
            Some(&self.key),
        );
        match &out.verdict {
            Verdict::Granted(_) => self.span.actions_granted += 1,
            Verdict::Denied(_) => self.span.actions_denied += 1,
        }
        out
    }

    /// Convenience: propose one verb on one body with scalar arguments.
    pub fn attempt(&mut self, verb: Verb, body: BodyId, args: &[(&str, f64)]) -> Outcome {
        let mut r = Request::on_body(verb, body, self.tick);
        for (n, v) in args {
            r = r.arg(n, Arg::Scalar(*v));
        }
        self.propose(r)
    }

    /// Name a way of doing something.
    ///
    /// The entity declaring that it *has* an approach, separately from
    /// knowing whether it works — which is what the statistics are for.
    /// Consolidation promotes verbs into skills on its own once there is
    /// enough evidence; this is for the case where the entity is told,
    /// or acquires one from the marketplace, and starts with a name and
    /// no record.
    pub fn learn_skill(&mut self, verb: Verb, name: &str) -> brainos_memory::skill::SkillId {
        let prov = Provenance::own(self.tick);
        self.skills
            .learn(&self.handle, &mut self.graph, verb, name, prov)
    }

    /// Record what happened after acting, so the skill graph learns.
    ///
    /// This is the feedback edge that makes the entity compound, and it
    /// is explicit rather than automatic on purpose: only the caller
    /// knows whether the body actually achieved the thing. An entity
    /// that inferred success from "the gate granted it" would learn that
    /// everything it is permitted to do works.
    pub fn outcome(
        &mut self,
        verb: Verb,
        body: Option<BodyId>,
        outcome: EpisodeOutcome,
        what: &str,
        context: &[&str],
    ) {
        let mut ep = Episode::attempt(verb, what, outcome, self.tick).in_context(context);
        if let Some(b) = body {
            ep = ep.on(b);
        }
        let prov = match body {
            Some(b) => Provenance::from_body(b, self.tick),
            None => Provenance::own(self.tick),
        };
        let node = ep.record(&self.handle, &mut self.graph, prov);

        let ctx: Vec<String> = context
            .iter()
            .map(|c| alloc::string::ToString::to_string(*c))
            .collect();
        // A denial is not an attempt. `Outcome::counts_toward_skill`
        // exists for exactly this and was not being consulted: every
        // decline was inflating the skill's attempt count *and*, once
        // preconditions learned to expire, refreshing their
        // last-tested time — so an entity that declined because it did
        // not want to find out was recorded as having found out.
        //
        // The second half is the one that mattered. It meant a belief
        // could never go stale, because declining to test it counted as
        // testing it.
        if outcome.counts_toward_skill() && self.skills.for_verb(&verb).is_some() {
            self.skills.observe(
                &self.handle,
                &mut self.graph,
                &verb,
                outcome.is_success(),
                outcome == EpisodeOutcome::Stopped,
                body,
                &ctx,
                self.tick,
                Some(node),
            );
        }
        if outcome.is_success() {
            self.ambient.note_success();
        } else {
            self.ambient.note_failure();
        }
    }

    // ------------------------------------------------------------------
    // living
    // ------------------------------------------------------------------

    /// One pass of the experience loop.
    pub fn tick(&mut self, sense: SenseFrame) -> Intent {
        self.tick = self.tick.next();
        let t = self.tick;

        let skills = &self.skills;
        let have_skill = |name: &str| {
            Verb::parse(name)
                .ok()
                .and_then(|v| skills.for_verb(&v))
                .map(|s| s.is_practiced())
                .unwrap_or(false)
        };
        let bodies = &self.bodies;
        let consequence = |name: &str| consequence_of(bodies, name);

        let intent = self.ambient.tick(&sense, t, have_skill, consequence);

        // a body that has gone silent loses its claims, and that is an
        // event, not a cleanup
        let lapsed = self.arbiter.lapse_silent(sense.at);
        for c in lapsed {
            Episode::observed("a body stopped answering mid-action", t).record(
                &self.handle,
                &mut self.graph,
                Provenance::from_body(c.body, t),
            );
        }

        self.goals.tidy(t);
        intent
    }

    /// Consolidate. Runs only when nothing is in motion, because it
    /// rewrites memory and a rewrite racing a gait loop is the kind of
    /// thing that is fine in testing and catastrophic once.
    pub fn dream(&mut self, dream: Dream) -> Option<ConsolidationReport> {
        if !self.arbiter.is_empty() {
            return None;
        }
        // read the mark before the graph is borrowed mutably; consolidating
        // from the session mark is what stops a second pass double-counting
        // the same episodes into a skill's statistics
        let from_mark = self.graph.session_mark;
        let report = consolidate(
            &self.handle,
            &mut self.graph,
            &mut self.skills,
            Dream { from_mark, ..dream },
            self.tick,
        );
        self.graph.mark_session();
        self.index = Index::build(&self.graph);
        self.span.dreams = self.span.dreams.saturating_add(1);
        // the next pass waits another whole idle period rather than
        // firing on every subsequent tick
        self.ambient.consolidated();
        Some(report)
    }

    /// Recall. Deterministic, local, and with no model in the loop.
    ///
    /// Takes `&mut self` because it refreshes the index first. That is
    /// not an implementation detail leaking: an entity that has just
    /// done something and cannot yet remember doing it is an entity with
    /// a bug, and the only way to make that impossible is to make the
    /// read path responsible for currency rather than every write path.
    pub fn recall(&mut self, query: &str, n: usize) -> Vec<(&brainos_memory::graph::Node, u32)> {
        self.index.refresh(&self.graph);
        self.index.recall(&self.graph, query, self.tick, n)
    }

    /// Recall restricted to what the policy stage may consult.
    pub fn recall_admissible(
        &mut self,
        query: &str,
        n: usize,
    ) -> Vec<(&brainos_memory::graph::Node, u32)> {
        self.index.refresh(&self.graph);
        self.index
            .recall_admissible(&self.graph, query, self.tick, n)
    }

    // ------------------------------------------------------------------
    // the untrusted world
    // ------------------------------------------------------------------

    /// Open a visit to an untrusted world. Off by default; this is the
    /// only thing that turns it on, and it is bounded three ways.
    pub fn open_world(&mut self, ttl: u64, purpose: &str) -> SessionId {
        let s = Session::open(
            brainos_worlds::world::World::Internet,
            self.tick,
            ttl,
            purpose,
        );
        self.sessions.push(s);
        self.kira.world_enabled = true;
        SessionId::from_bytes(self.rng.nonce32())
    }

    pub fn close_worlds(&mut self) {
        for s in self.sessions.iter_mut() {
            s.close();
        }
        self.sessions.clear();
        self.kira.world_enabled = false;
    }

    pub fn open_session(&mut self) -> Option<&mut Session> {
        let t = self.tick;
        self.sessions.iter_mut().find(|s| s.is_open(t))
    }

    // ------------------------------------------------------------------
    // saying what it is
    // ------------------------------------------------------------------

    /// The entity's own account of itself, in one line.
    pub fn describe(&self) -> Text<240> {
        let mut t = Text::new();
        t.push_str(self.id().fingerprint().as_str());
        t.push_str(" · ");
        t.push_u64(self.bodies.body_count() as u64);
        t.push_str(" bodies · ");
        t.push_u64(self.graph.len() as u64);
        t.push_str(" memories · ");
        t.push_u64(self.skills.practiced_count() as u64);
        t.push_str(" practiced skills · ceiling ");
        t.push_str(self.resources.ceiling().name());
        t
    }

    /// Everything it can currently do, across every body.
    pub fn repertoire(&self) -> Vec<Verb> {
        self.bodies.repertoire()
    }

    /// The highest tier it can currently reach.
    pub fn ceiling(&self) -> Tier {
        self.resources.ceiling()
    }
}

/// How consequential an observation channel is, derived from which
/// bodies exist rather than from a table.
///
/// An entity with legs should care more about tilt than one without.
/// Deriving it from the body map means a laptop-only entity is not
/// alarmed by a channel it has no way to be hurt by.
fn consequence_of(bodies: &BodyMap, channel: &str) -> u8 {
    let consequential = RegionClass::ALL
        .iter()
        .any(|c| c.is_consequential() && bodies.has_class(*c));
    let base = match channel {
        c if c.contains("tilt") || c.contains("stability") => 90,
        c if c.contains("current") || c.contains("stall") => 80,
        c if c.contains("temp") => 70,
        c if c.contains("volt") || c.contains("rail") || c.contains("batt") => 60,
        c if c.contains("clearance") || c.contains("obstacle") => 75,
        _ => 20,
    };
    if consequential {
        base
    } else {
        // on a body that cannot move, the same reading is information
        // rather than a warning
        base / 3
    }
}

/// The entity's own instant, for callers that do not have a clock.
pub fn instant_from_tick(t: Tick) -> Instant {
    Instant(t.0.saturating_mul(1_000))
}
