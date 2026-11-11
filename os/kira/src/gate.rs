//! BFS  LEVEL    Domain 3 KIRA - the eight-stage gate; the only path to action  [crate brainos-kira]
//! BFS  PURPOSE  Runs a signed request through the eight stages in order and returns either a Granted token or a refusal naming the stage; always audits
//! BFS  DEFINES  REQUEST_WINDOW, Verdict, Outcome, Kira
//! BFS  USES     this crate: action, audit, capability, floors, rules, stages | brainos: abi, body, crypto, rt
//! BFS  USED BY  entity/entity, host/blur_gate2, host/degrade, host/fullstack, host/life, host/migrate, host/model, host/ros2_stack, +6 more
//!
//! The pipeline.
//!
//! Eight stages, fixed order, no skipping, one place where a `Granted`
//! is constructed. This file is deliberately written to be *read* rather
//! than to be clever: each stage is its own method, each returns a
//! `StageOutcome`, and `evaluate` is a straight-line sequence of six
//! calls, a commit, and an audit. If it ever stops looking like that,
//! something has gone wrong.

use crate::action::{Action, Proposal};
use crate::audit::{AuditRecord, AuditTrail};
use crate::capability::{Delegation, Granted};
use crate::floors;
use crate::rules::RuleSet;
use crate::stages::{StageOutcome, Trace};
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use brainos_abi::error::{Denied, DenyCode, Stage};
use brainos_abi::ids::{BodyId, EntityId};
use brainos_abi::time::Tick;
use brainos_abi::verb::Verb;
use brainos_body::policy::{Arg, BodyPolicy};
use brainos_body::region::BodyMap;
use brainos_body::schema::CapabilitySchema;
use brainos_crypto::domain::Tag;
use brainos_crypto::key::{BrainKey, PublicKey};

/// How far in the past a request's tick may be before it is a replay.
///
/// Ticks are the entity's own counter, not a clock, so this is "how many
/// cognitive passes may elapse between proposing and gating". Thirty-two
/// is generous for an in-process call and tight enough that a captured
/// proposal is useless a second later.
pub const REQUEST_WINDOW: u64 = 32;

/// What the gate concluded.
pub enum Verdict {
    /// authority to act, once, before it expires
    Granted(Granted),
    /// refused, at a named stage, for a reason with a code
    Denied(Denied),
}

impl Verdict {
    pub fn is_granted(&self) -> bool {
        matches!(self, Verdict::Granted(_))
    }

    pub fn denial(&self) -> Option<Denied> {
        match self {
            Verdict::Denied(d) => Some(*d),
            _ => None,
        }
    }
}

/// A verdict, its trace, and where it landed in the audit log.
pub struct Outcome {
    pub verdict: Verdict,
    pub trace: Trace,
    pub audit_index: usize,
}

/// One body's policy and schema, as the gate sees them.
struct Bound {
    body: BodyId,
    policy: Box<dyn BodyPolicy>,
    schema: CapabilitySchema,
}

/// The gate.
pub struct Kira {
    entity: EntityId,
    pub rules: RuleSet,
    pub trail: AuditTrail,
    bound: Vec<Bound>,
    /// delegations this entity has issued, and how many times each has
    /// been used — the `Uses` caveat needs somewhere to count
    issued: Vec<(Delegation, u32)>,
    /// revoked capability ids
    revoked: Vec<brainos_abi::ids::CapabilityId>,
    /// how long a minted token lives, in ticks
    pub default_ttl: u64,
    /// is the entity allowed to touch untrusted worlds at all this
    /// session? Off by default — §3.8, "the internet is an alternate
    /// universe the entity visits, not a place it lives".
    pub world_enabled: bool,
}

impl Kira {
    pub fn new(entity: EntityId) -> Kira {
        Kira {
            entity,
            rules: RuleSet::new(),
            trail: AuditTrail::default(),
            bound: Vec::new(),
            issued: Vec::new(),
            revoked: Vec::new(),
            default_ttl: 30,
            world_enabled: false,
        }
    }

    pub fn entity(&self) -> EntityId {
        self.entity
    }

    /// §8 step 4: bind a body's policy. Re-binding replaces, because a
    /// body that reconnects with a fresh schema gets a fresh policy and
    /// keeping the stale one would be the gate checking yesterday's
    /// limits.
    pub fn bind(&mut self, body: BodyId, policy: Box<dyn BodyPolicy>, schema: CapabilitySchema) {
        self.bound.retain(|b| b.body != body);
        self.bound.push(Bound {
            body,
            policy,
            schema,
        });
    }

    pub fn unbind(&mut self, body: &BodyId) -> bool {
        let before = self.bound.len();
        self.bound.retain(|b| &b.body != body);
        self.bound.len() != before
    }

    pub fn policy_for(&self, body: &BodyId) -> Option<&dyn BodyPolicy> {
        self.bound
            .iter()
            .find(|b| &b.body == body)
            .map(|b| b.policy.as_ref())
    }

    pub fn policy_for_mut(&mut self, body: &BodyId) -> Option<&mut (dyn BodyPolicy + 'static)> {
        self.bound
            .iter_mut()
            .find(|b| &b.body == body)
            .map(|b| b.policy.as_mut())
    }

    pub fn bound_bodies(&self) -> Vec<(BodyId, String)> {
        self.bound
            .iter()
            .map(|b| (b.body, b.policy.describe()))
            .collect()
    }

    pub fn register_delegation(&mut self, d: Delegation) {
        self.issued.retain(|(x, _)| x.id != d.id);
        self.issued.push((d, 0));
    }

    pub fn revoke(&mut self, id: brainos_abi::ids::CapabilityId) {
        if !self.revoked.contains(&id) {
            self.revoked.push(id);
        }
    }

    pub fn is_revoked(&self, id: &brainos_abi::ids::CapabilityId) -> bool {
        self.revoked.contains(id)
    }

    // ------------------------------------------------------------------
    // the pipeline
    // ------------------------------------------------------------------

    /// Evaluate a proposal. This is the only path from intent to
    /// authority in the entire system.
    pub fn evaluate(
        &mut self,
        proposal: &Proposal,
        key: &PublicKey,
        map: &BodyMap,
        now: Tick,
        minter: Option<&BrainKey>,
    ) -> Outcome {
        // an entity proposing on its own behalf: the signer and the
        // authority are the same key
        self.evaluate_signed_by(proposal, key, key, map, now, minter)
    }

    /// The pipeline, with the signer and the authority separated.
    ///
    /// They are the same key for anything the entity proposes itself,
    /// and they are *not* the same for a delegation: the holder signs
    /// the request, and the issuer is the entity whose gate this is.
    /// Collapsing the two is how delegation quietly stops working — the
    /// gate demands the issuer's signature, which the holder by
    /// definition cannot produce, so every delegation to anyone other
    /// than the entity itself dies at authenticate.
    ///
    /// `authority` still has to be this entity. A gate evaluates its own
    /// entity's requests and nobody else's; what `signer` changes is who
    /// is allowed to have *asked*.
    fn evaluate_signed_by(
        &mut self,
        proposal: &Proposal,
        signer: &PublicKey,
        authority: &PublicKey,
        map: &BodyMap,
        now: Tick,
        minter: Option<&BrainKey>,
    ) -> Outcome {
        let mut trace = Trace::new();
        let r = &proposal.request;

        // ---- 1 PARSE ----
        let parse = match proposal.parse_check() {
            Ok(()) => StageOutcome::pass("well-formed typed request"),
            Err(d) => StageOutcome::Refuse(d),
        };
        let stop = parse.is_refusal();
        trace.record(Stage::Parse, parse);

        // ---- 2 AUTHENTICATE ----
        if !stop {
            let outcome = self.authenticate(proposal, signer, authority, now);
            let refused = outcome.is_refusal();
            trace.record(Stage::Authenticate, outcome);
            if !refused {
                // ---- 3 AUTHORIZE ----
                let outcome = self.authorize(proposal, map);
                let refused = outcome.is_refusal();
                trace.record(Stage::Authorize, outcome);
                if !refused {
                    // ---- 4 VALIDATE ----
                    let outcome = self.validate(proposal);
                    let refused = outcome.is_refusal();
                    trace.record(Stage::Validate, outcome);
                    if !refused {
                        // ---- 5 SIMULATE ----
                        let outcome = self.simulate(proposal);
                        let refused = outcome.is_refusal();
                        trace.record(Stage::Simulate, outcome);
                        if !refused {
                            // ---- 6 POLICY ----
                            let outcome = self.policy(proposal);
                            trace.record(Stage::Policy, outcome);
                        }
                    }
                }
            }
        }

        // ---- 7 COMMIT ----
        //
        // The only place in the codebase where a `Granted` is built. The
        // condition is the conjunction of every preceding stage having
        // passed; there is no branch that reaches `Granted::mint` any
        // other way.
        let refusal = trace.refusal();
        let verdict = match (refusal, minter) {
            (Some((_, d)), _) => {
                trace.record(Stage::Commit, StageOutcome::Skipped);
                Verdict::Denied(d)
            }
            (None, None) => {
                let d = Denied::new(DenyCode::CannotMint, "no key available to sign the grant");
                trace.record(Stage::Commit, StageOutcome::Refuse(d));
                Verdict::Denied(d)
            }
            (None, Some(k)) => {
                let granted = Granted::mint(
                    k,
                    r.verb,
                    r.body,
                    r.digest(),
                    now,
                    self.default_ttl,
                );
                trace.record(Stage::Commit, StageOutcome::pass("token minted"));
                Verdict::Granted(granted)
            }
        };

        // ---- 8 AUDIT ----
        //
        // Runs whatever happened. A decision nobody can review is a
        // decision nobody made.
        let grant = match &verdict {
            Verdict::Granted(g) => Some(crate::capability::Grant::from(g)),
            Verdict::Denied(_) => None,
        };
        let record = AuditRecord::build(self.entity, r, &trace, grant);
        let audit_index = self.trail.record(record);
        trace.record(Stage::Audit, StageOutcome::pass("recorded"));

        debug_assert!(trace.is_well_formed(), "the trace must be well-formed");

        Outcome {
            verdict,
            trace,
            audit_index,
        }
    }

    // ---- stage 2 ----

    fn authenticate(
        &self,
        p: &Proposal,
        signer: &PublicKey,
        authority: &PublicKey,
        now: Tick,
    ) -> StageOutcome {
        let r = &p.request;
        if !authority.entity_id().ct_eq(&self.entity) {
            return StageOutcome::Refuse(Denied::new(
                DenyCode::BadSignature,
                "the presented key is not this entity",
            ));
        }
        if !signer.verify(Tag::Request, r.digest().as_bytes(), &p.signature) {
            return StageOutcome::Refuse(Denied::new(
                DenyCode::BadSignature,
                "the request was not signed by whoever claims to have asked",
            ));
        }
        // a tick from the future is as suspicious as one from the distant
        // past: it means whoever built the request is not running on this
        // entity's counter
        if r.tick.0 > now.0 {
            return StageOutcome::Refuse(Denied::new(
                DenyCode::StaleRequest,
                "the request's tick is ahead of the entity's",
            ));
        }
        if now.since(r.tick) > REQUEST_WINDOW {
            return StageOutcome::Refuse(Denied::new(
                DenyCode::StaleRequest,
                "the request is older than the replay window",
            ));
        }
        StageOutcome::pass("signature valid, tick inside the window")
    }

    // ---- stage 3 ----

    fn authorize(&self, p: &Proposal, map: &BodyMap) -> StageOutcome {
        let r = &p.request;
        match r.action {
            Action::Intrinsic => StageOutcome::pass("intrinsic to the entity"),
            Action::World => {
                if self.world_enabled {
                    StageOutcome::pass("an untrusted world is open this session")
                } else {
                    StageOutcome::Refuse(Denied::new(
                        DenyCode::NoSuchLimb,
                        "no untrusted world is open this session",
                    ))
                }
            }
            Action::Body => {
                let Some(body) = r.body else {
                    return StageOutcome::Refuse(Denied::new(
                        DenyCode::NoSuchBody,
                        "a body action names no body",
                    ));
                };
                if !map.knows_body(&body) {
                    return StageOutcome::Refuse(Denied::new(
                        DenyCode::NoSuchBody,
                        "that body is not bound to this entity",
                    ));
                }
                // this is the honest-refusal stage: an action can only be
                // authorized through a region that really exists and
                // really advertises the verb
                if map.find_capability_on(&body, &r.verb).is_none() {
                    return if map.find_capability(&r.verb).is_some() {
                        StageOutcome::Refuse(Denied::new(
                            DenyCode::BodyLacksVerb,
                            "another body can do that, but not this one",
                        ))
                    } else {
                        StageOutcome::Refuse(Denied::new(
                            DenyCode::NoSuchLimb,
                            "no limb of any bound body advertises that",
                        ))
                    };
                }
                // a body with no policy bound cannot be acted through.
                // This is the fail-closed case: "incorporated but not yet
                // policed" is representable and it means no.
                if self.policy_for(&body).is_none() {
                    return StageOutcome::Refuse(Denied::new(
                        DenyCode::NoSuchBody,
                        "no policy is bound to that body",
                    ));
                }
                StageOutcome::pass("a limb of the named body advertises it")
            }
        }
    }

    // ---- stage 4 ----

    fn validate(&self, p: &Proposal) -> StageOutcome {
        let r = &p.request;

        // generic floors, first. A body policy may tighten these; nothing
        // loosens them.
        for (name, arg) in &r.args {
            if let Some(text) = arg.as_text() {
                if text.len() > 4_096 {
                    return StageOutcome::Refuse(Denied::new(
                        DenyCode::OutOfBounds,
                        "a text argument is longer than any limb accepts",
                    ));
                }
                let _ = name;
            }
            if let Arg::Scalar(v) = arg {
                if !v.is_finite() {
                    return StageOutcome::Refuse(Denied::new(
                        DenyCode::Malformed,
                        "a scalar argument is not a finite number",
                    ));
                }
            }
        }

        match (r.action, r.body) {
            (Action::Body, Some(body)) => {
                let Some(bound) = self.bound.iter().find(|b| b.body == body) else {
                    return StageOutcome::Refuse(Denied::new(
                        DenyCode::OutsideBodyEnvelope,
                        "no policy bound",
                    ));
                };
                match bound.policy.validate(&r.verb, &r.args) {
                    Ok(()) => StageOutcome::pass("inside the body's declared envelope"),
                    Err(refusal) => StageOutcome::Refuse(Denied::new(
                        refusal.code,
                        static_detail(&refusal.detail),
                    )),
                }
            }
            _ => StageOutcome::pass("arguments are well-formed"),
        }
    }

    // ---- stage 5 ----

    fn simulate(&self, p: &Proposal) -> StageOutcome {
        let r = &p.request;
        match (r.action, r.body) {
            (Action::Body, Some(body)) => {
                let Some(bound) = self.bound.iter().find(|b| b.body == body) else {
                    return StageOutcome::Refuse(Denied::new(
                        DenyCode::Unpredictable,
                        "no policy bound to predict with",
                    ));
                };
                match bound.policy.simulate(&r.verb, &r.args) {
                    Ok(_prediction) => StageOutcome::pass("consequence is inside the envelope"),
                    Err(refusal) => StageOutcome::Refuse(Denied::new(
                        refusal.code,
                        static_detail(&refusal.detail),
                    )),
                }
            }
            (Action::World, _) => {
                // the forward model for a world act is: whatever comes
                // back is untrusted, and nothing persists off-entity
                StageOutcome::pass("one bounded exchange; nothing persists off-entity")
            }
            _ => StageOutcome::pass("intrinsic act; identity and memory persist"),
        }
    }

    // ---- stage 6 ----

    fn policy(&self, p: &Proposal) -> StageOutcome {
        let r = &p.request;

        // drives and human rules
        if let Some((code, because)) = self.rules.objection(&r.verb) {
            return StageOutcome::Refuse(Denied::new(code, static_detail(&because)));
        }

        // the destructive-argument floor, for any text argument
        for (_, arg) in &r.args {
            if let Some(text) = arg.as_text() {
                if let Some(pattern) = floors::destructive_hit(r.verb.as_str(), text) {
                    let _ = pattern;
                    return StageOutcome::Refuse(Denied::new(
                        DenyCode::DestructivePattern,
                        "a destructive pattern was found in the arguments",
                    ));
                }
                if floors::is_load_bearing(text) {
                    return StageOutcome::Refuse(Denied::new(
                        DenyCode::LoadBearing,
                        "that is load-bearing for the entity itself; refused",
                    ));
                }
            }
        }

        // the body's hard invariants right now
        if let (Action::Body, Some(body)) = (r.action, r.body) {
            if let Some(bound) = self.bound.iter().find(|b| b.body == body) {
                if let Err(refusal) = bound.policy.policy(&r.verb, &r.args) {
                    return StageOutcome::Refuse(Denied::new(
                        refusal.code,
                        static_detail(&refusal.detail),
                    ));
                }
            }
        }

        StageOutcome::pass("no drive, rule or body invariant objects")
    }

    /// Evaluate a proposal presented by a *delegate* rather than by the
    /// entity itself — a gait loop, a peer, a hosted brain.
    ///
    /// The delegation does not replace the pipeline. It is checked first,
    /// and then the ordinary eight stages run exactly as they would for
    /// the entity. Holding a delegation improves your standing; it does
    /// not let you skip the gate.
    pub fn evaluate_delegated(
        &mut self,
        proposal: &Proposal,
        holder: &EntityId,
        delegation: &Delegation,
        key: &PublicKey,
        map: &BodyMap,
        now: Tick,
        minter: Option<&BrainKey>,
    ) -> Outcome {
        let r = &proposal.request;

        if self.is_revoked(&delegation.id) {
            return self.short_circuit(
                proposal,
                Denied::new(DenyCode::CapabilityRevoked, "that delegation was revoked"),
                Stage::Authorize,
            );
        }
        if !delegation.verify(key) {
            return self.short_circuit(
                proposal,
                Denied::new(DenyCode::BadSignature, "the delegation is not ours"),
                Stage::Authenticate,
            );
        }

        let uses = self
            .issued
            .iter()
            .find(|(d, _)| d.id == delegation.id)
            .map(|(_, n)| *n)
            .unwrap_or(0);
        let body_fit = r
            .body
            .and_then(|b| self.policy_for(&b))
            .map(|_| true)
            .unwrap_or(false);
        let reversible = r
            .body
            .and_then(|b| self.bound.iter().find(|x| x.body == b))
            .and_then(|b| b.schema.get(&r.verb))
            .map(|s| s.reversible)
            .unwrap_or(true);
        let scalars: Vec<(brainos_rt::envelope::Channel, f32)> = r
            .args
            .iter()
            .filter_map(|(name, a)| {
                let body = r.body?;
                let bound = self.bound.iter().find(|x| x.body == body)?;
                let spec = bound.schema.get(&r.verb)?;
                let p = spec.param(name)?;
                Some((p.channel?, a.as_scalar()? as f32))
            })
            .collect();

        if let Err(f) = delegation.covers(
            holder,
            &r.verb,
            &r.body,
            now,
            uses,
            body_fit,
            reversible,
            &scalars,
        ) {
            let d = match f {
                crate::capability::CoverageFailure::Expired => {
                    Denied::new(DenyCode::CapabilityExpired, "the delegation has expired")
                }
                crate::capability::CoverageFailure::BadSignature => {
                    Denied::new(DenyCode::BadSignature, "the delegation is not ours")
                }
                _ => Denied::new(
                    DenyCode::CapabilityDoesNotCover,
                    "the delegation does not cover that",
                ),
            };
            return self.short_circuit(proposal, d, d.stage);
        }

        if let Some((_, n)) = self.issued.iter_mut().find(|(d, _)| d.id == delegation.id) {
            *n += 1;
        }

        // The holder signs; the entity's key is the authority. An
        // EntityId *is* an ed25519 public key, so the holder's verifying
        // key comes from the delegation itself rather than from whoever
        // is presenting it — which is what ties the signature on the
        // request to the holder the delegation actually names.
        let signer = match PublicKey::from_bytes(holder.as_bytes()) {
            Ok(k) => k,
            Err(_) => {
                return self.short_circuit(
                    proposal,
                    Denied::new(DenyCode::BadSignature, "the holder is not a usable key"),
                    Stage::Authenticate,
                )
            }
        };
        self.evaluate_signed_by(proposal, &signer, key, map, now, minter)
    }

    /// Refuse before the pipeline, still recording a well-formed trace.
    /// Used by the delegated path, where the refusal is about standing
    /// rather than about the action.
    fn short_circuit(&mut self, p: &Proposal, denial: Denied, at: Stage) -> Outcome {
        let mut trace = Trace::new();
        for s in Stage::ALL.iter().take_while(|s| **s < at) {
            trace.record(*s, StageOutcome::pass("not the reason for refusal"));
        }
        trace.record(at, StageOutcome::Refuse(denial));
        let record = AuditRecord::build(self.entity, &p.request, &trace, None);
        let audit_index = self.trail.record(record);
        trace.record(Stage::Audit, StageOutcome::pass("recorded"));
        debug_assert!(trace.is_well_formed());
        Outcome {
            verdict: Verdict::Denied(denial),
            trace,
            audit_index,
        }
    }

    /// Would this verb be refused outright, without building a proposal?
    /// For cognition to consult before proposing — asking and being
    /// refused costs an audit record, and an entity that knows it will be
    /// refused should think of something else instead.
    pub fn would_refuse(&self, verb: &Verb) -> Option<DenyCode> {
        self.rules.objection(verb).map(|(c, _)| c)
    }
}

/// Deny details are `&'static str` so that a refusal is a fixed-size,
/// allocation-free value that a servo loop can carry. A body policy
/// produces a `String`; this maps the ones we know onto statics and
/// falls back to a generic line.
///
/// The fallback loses detail, which is a real cost. It buys the property
/// that `Denied` is `Copy` and cannot itself allocate — and a refusal
/// path that can fail to allocate is a refusal path that can fail.
fn static_detail(s: &str) -> &'static str {
    /// The refusal details a body policy is allowed to surface verbatim.
    /// Anything else collapses to the generic line and lives only in the
    /// audit record, where allocation is fine.
    const KNOWN: [&str; 14] = [
        "this body does not advertise that verb",
        "argument is not declared by the schema",
        "the body did not declare bounds for this parameter",
        "argument is outside the declared bounds",
        "a declared parameter was not supplied",
        "the body is not calibrated",
        "no schema entry to predict from",
        "the commanded value leaves the granted envelope",
        "no envelope bound for a consequential region",
        "link to the body is down",
        "the body is stopped and needs a decision, not a retry",
        "actuator rail is below the floor",
        "an actuator is over temperature",
        "the body is not level",
    ];
    match KNOWN.iter().find(|k| **k == s) {
        Some(k) => k,
        None => "the body refused; see the audit record",
    }
}
