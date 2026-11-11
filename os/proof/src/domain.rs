//! BFS  LEVEL    Stage 6 - property domains (bounded checking, not a proof)  [crate brainos-proof]
//! BFS  PURPOSE  Defines the eight-dimension space of gate test cases, enumerates every combination, and says which stage should refuse each one
//! BFS  DEFINES  SUBJECT_VERB_IS_IRREVERSIBLE, Sig, Freshness, Binding, Args, Fitness, Rule, Danger, Minter, Case, Domain, Scenario
//! BFS  USES     brainos: abi
//! BFS  USED BY  host/main, nostd/lib, proof/harness, proof/lib, proof/properties, proof/tests/proof_suite
//!
//! The bounded domain.
//!
//! Everything the harness enumerates. The dimensions were chosen because
//! each one can independently change which stage refuses, which is the
//! thing the properties are about. Adding a dimension multiplies the
//! domain, so each is justified:
//!
//! * **signature** — decides stage 2. Without it, stages 3–6 are never
//!   reached from a forged proposal and the short-circuit property is
//!   untested.
//! * **tick** — also stage 2, separately: a stale request and a forged
//!   one must refuse differently.
//! * **body binding** — stage 3, three ways: no body, an unbound body,
//!   a bound body without the verb.
//! * **argument class** — stage 4: in bounds, out of bounds, malformed.
//! * **body fitness** — stages 5 and 6: an unfit body refuses at policy,
//!   an irreversible act on an unfit body refuses at simulate.
//! * **rule** — stage 6: a drive, a human rule, or nothing.
//! * **danger** — stage 6's floor, independently of the rules.
//! * **minter** — stage 7: no key means no token, and no token must not
//!   become a free pass.
//!
//! Eight dimensions, a few values each. The product is in the low
//! thousands, which is enumerable in under a second and is reported.

use alloc::vec::Vec;

/// The rig's subject verb (`fs.write`) is declared irreversible, which
/// changes which stage refuses for an unfit body. Named rather than
/// inlined so the coupling between the harness and the rig's schema is
/// visible: change one and this constant has to change with it.
pub const SUBJECT_VERB_IS_IRREVERSIBLE: bool = true;

/// Whether the proposal was signed by the entity.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sig {
    Valid,
    Forged,
}

/// Where the request's tick sits relative to the gate's.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Freshness {
    Current,
    Stale,
    FromTheFuture,
}

/// What the body map and the gate know about the named body.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Binding {
    /// bound, policed, and advertises the verb
    Ready,
    /// bound and policed, but this verb is on another body
    WrongBody,
    /// in the body map but with no policy bound
    Unpoliced,
    /// not bound at all
    Absent,
}

/// The shape of the arguments.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Args {
    InBounds,
    OutOfBounds,
    /// a parameter the schema never mentioned
    Undeclared,
    /// a declared parameter left out
    Missing,
    /// a scalar that is not a number
    NotFinite,
}

/// How the body is doing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fitness {
    Fit,
    LinkDown,
    Stopped,
    Overheated,
}

/// What the ruleset says about the verb.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Rule {
    Nothing,
    HumanForbids,
    DriveForbids,
}

/// Whether the arguments carry something the floor refuses.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Danger {
    None,
    Destructive,
    LoadBearing,
}

/// Whether a key is available to mint with.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Minter {
    Present,
    Absent,
}

/// One point in the domain.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Case {
    pub sig: Sig,
    pub freshness: Freshness,
    pub binding: Binding,
    pub args: Args,
    pub fitness: Fitness,
    pub rule: Rule,
    pub danger: Danger,
    pub minter: Minter,
}

impl Case {
    /// The stage this case *should* refuse at, derived from the case
    /// independently of what the gate does.
    ///
    /// This is the oracle, and writing it separately from the
    /// implementation is the whole value: if the gate and this function
    /// disagree, one of them is wrong and the harness says which case
    /// exposed it. A harness that derived the expected stage by running
    /// the gate would be checking that the gate agrees with itself.
    pub fn expected_refusal(&self) -> Option<brainos_abi::error::Stage> {
        use brainos_abi::error::Stage;
        // stage 2
        if self.sig == Sig::Forged {
            return Some(Stage::Authenticate);
        }
        if self.freshness != Freshness::Current {
            return Some(Stage::Authenticate);
        }
        // stage 3
        if self.binding != Binding::Ready {
            return Some(Stage::Authorize);
        }
        // stage 4
        if self.args != Args::InBounds {
            return Some(Stage::Validate);
        }
        // Stage 5 vs stage 6, for an unfit body.
        //
        // This is the distinction the harness found the first time it
        // ran: the oracle said Policy and the gate said Simulate, and
        // the gate was right. The subject verb is declared
        // *irreversible* in the rig's schema, and `SchemaPolicy`
        // deliberately refuses an irreversible act on an unfit body at
        // simulate rather than at policy — because the audit record
        // should say "we predicted harm", not "a rule said no". Those
        // are different sentences to read after an incident.
        //
        // A reversible verb on an unfit body still refuses at policy,
        // which is where the body's hard invariants live.
        if self.fitness != Fitness::Fit {
            return Some(if SUBJECT_VERB_IS_IRREVERSIBLE {
                Stage::Simulate
            } else {
                Stage::Policy
            });
        }
        // stage 6
        if self.rule != Rule::Nothing || self.danger != Danger::None {
            return Some(Stage::Policy);
        }
        // stage 7
        if self.minter == Minter::Absent {
            return Some(Stage::Commit);
        }
        None
    }

    pub fn should_grant(&self) -> bool {
        self.expected_refusal().is_none()
    }
}

/// The whole enumerable domain.
pub struct Domain;

impl Domain {
    pub const SIGS: [Sig; 2] = [Sig::Valid, Sig::Forged];
    pub const FRESHNESS: [Freshness; 3] = [
        Freshness::Current,
        Freshness::Stale,
        Freshness::FromTheFuture,
    ];
    pub const BINDINGS: [Binding; 4] = [
        Binding::Ready,
        Binding::WrongBody,
        Binding::Unpoliced,
        Binding::Absent,
    ];
    pub const ARGS: [Args; 5] = [
        Args::InBounds,
        Args::OutOfBounds,
        Args::Undeclared,
        Args::Missing,
        Args::NotFinite,
    ];
    pub const FITNESS: [Fitness; 4] = [
        Fitness::Fit,
        Fitness::LinkDown,
        Fitness::Stopped,
        Fitness::Overheated,
    ];
    pub const RULES: [Rule; 3] = [Rule::Nothing, Rule::HumanForbids, Rule::DriveForbids];
    pub const DANGERS: [Danger; 3] = [Danger::None, Danger::Destructive, Danger::LoadBearing];
    pub const MINTERS: [Minter; 2] = [Minter::Present, Minter::Absent];

    /// How many cases there are. Reported so the exhaustiveness claim
    /// can be checked rather than believed.
    pub const SIZE: usize = 2 * 3 * 4 * 5 * 4 * 3 * 3 * 2;

    /// Every case, in a fixed order so a failure is reproducible.
    pub fn enumerate() -> Vec<Case> {
        let mut out = Vec::with_capacity(Domain::SIZE);
        for sig in Domain::SIGS {
            for freshness in Domain::FRESHNESS {
                for binding in Domain::BINDINGS {
                    for args in Domain::ARGS {
                        for fitness in Domain::FITNESS {
                            for rule in Domain::RULES {
                                for danger in Domain::DANGERS {
                                    for minter in Domain::MINTERS {
                                        out.push(Case {
                                            sig,
                                            freshness,
                                            binding,
                                            args,
                                            fitness,
                                            rule,
                                            danger,
                                            minter,
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        out
    }
}

/// A named scenario, for reporting which case failed.
#[derive(Clone, Copy, Debug)]
pub struct Scenario {
    pub index: usize,
    pub case: Case,
}
