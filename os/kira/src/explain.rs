//! BFS  LEVEL    Domain 3 KIRA - the eight-stage gate; the only path to action  [crate brainos-kira]
//! BFS  PURPOSE  Turns each refusal code into plain sentences for the owner: what was refused, why, and what would change it (or that nothing will)
//! BFS  DEFINES  Remedy, Explanation, explain, say_refusal
//! BFS  USES     brainos: abi | external: DenyCode
//! BFS  USED BY  entity/entity, kira/tests/explain_suite
//!
//! Telling the owner why.
//!
//! A robot that refuses and cannot say why in words its owner
//! understands is a robot the owner switches off. That is not a
//! usability complaint — it is a safety one. An unexplained refusal
//! teaches the person standing next to the machine that the machine is
//! unreliable, and the next thing they do is find the way around it.
//!
//! The gate already produces a refusal with a stage, a code and a
//! detail. All three are written for whoever is reading the code:
//! `PredictedInvariantBreak at Simulate — the commanded value leaves
//! the granted envelope` is exactly right in a log and useless to the
//! person who asked the robot to pick something up.
//!
//! So each refusal gets three sentences, and the third is the one that
//! makes it worth saying at all:
//!
//! * **what** it will not do, in the owner's terms;
//! * **why**, as a fact about the machine or the world rather than
//!   about the software;
//! * **what would change it**, honestly — including saying plainly when
//!   nothing would, because a refusal that implies a workaround where
//!   there is none is worse than a blunt no.
//!
//! That last distinction is `Remedy::Never`, and it is why this is a
//! closed table written by hand rather than a format string. Four of
//! the twenty-five refusals are things the owner cannot change at all:
//! a Level-0 drive, an audit failure, a body that has not attested, and
//! a request whose signature is wrong. Telling someone to "try again
//! with different arguments" when they have hit a drive would be a lie
//! told by a machine about itself.

use alloc::string::String;
use brainos_abi::error::{Denied, DenyCode};
use brainos_abi::verb::Verb;

/// Whose problem is it, and can it be solved?
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Remedy {
    /// the person can change something and ask again
    TheOwner,
    /// something about the body needs attention — charge it, free it,
    /// let it cool, calibrate it
    TheBody,
    /// whoever integrated this robot has to declare something the
    /// manufacturer left out
    TheIntegrator,
    /// the request was malformed; whatever produced it should produce a
    /// different one
    TheAsker,
    /// nothing changes this. Said plainly, because a refusal that
    /// implies a workaround where there is none is worse than a blunt
    /// no.
    Never,
}

impl Remedy {
    pub const fn name(self) -> &'static str {
        match self {
            Remedy::TheOwner => "you can change this",
            Remedy::TheBody => "the robot needs attention",
            Remedy::TheIntegrator => "whoever set this robot up has to declare something",
            Remedy::TheAsker => "the request was malformed",
            Remedy::Never => "nothing changes this",
        }
    }
}

/// A refusal, in words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Explanation {
    /// what it will not do
    pub what: &'static str,
    /// why, as a fact about the machine or the world
    pub why: &'static str,
    /// what would change it — or that nothing would
    pub remedy: &'static str,
    pub whose: Remedy,
}

/// The table. One row per refusal code, and no two rows alike.
///
/// Written by hand on purpose. A generated sentence would say the same
/// thing about a flat battery and a forbidden act, and those are not
/// the same thing to the person standing there.
pub const fn explain(code: DenyCode) -> Explanation {
    use DenyCode::*;
    match code {
        Unparseable => Explanation {
            what: "I could not make sense of what I was asked to do.",
            why: "The instruction did not describe an action I could take.",
            remedy: "Ask again in a different way.",
            whose: Remedy::TheAsker,
        },
        UnknownVerb => Explanation {
            what: "I do not know how to do that.",
            why: "None of the bodies I have offers that action.",
            remedy: "I can tell you what this body can do, if that helps.",
            whose: Remedy::TheOwner,
        },
        BadSignature => Explanation {
            what: "I will not act on that.",
            why: "The instruction did not come from me. Something else produced it.",
            remedy: "Nothing you can do here. This is the case the signature exists for, and it is working.",
            whose: Remedy::Never,
        },
        StaleRequest => Explanation {
            what: "That instruction is too old to act on.",
            why: "It was made a while ago and I have no way to know whether it still applies.",
            remedy: "Ask me again now.",
            whose: Remedy::TheAsker,
        },
        UnattestedBody => Explanation {
            what: "I will not move that.",
            why: "It has not proved to me what it is. I only inhabit bodies that can.",
            remedy: "The body has to complete its handshake. Until it does I will not command it.",
            whose: Remedy::Never,
        },
        NoSuchLimb => Explanation {
            what: "I cannot do that with this body.",
            why: "This body has no part that performs that action.",
            remedy: "A body that has one would do it. This one will not.",
            whose: Remedy::TheOwner,
        },
        BodyLacksVerb => Explanation {
            what: "This body does not do that.",
            why: "It has the right sort of part, but it never told me it could do this.",
            remedy: "If it can, whoever set it up has to say so in its description.",
            whose: Remedy::TheIntegrator,
        },
        NoSuchBody => Explanation {
            what: "I do not have that body any more.",
            why: "It was released, or it has not been acquired.",
            remedy: "Connect it again and I will take it on.",
            whose: Remedy::TheOwner,
        },
        CapabilityDoesNotCover => Explanation {
            what: "I was not given permission for that.",
            why: "The standing authority I am acting under does not cover this action.",
            remedy: "Whoever issued that authority can issue a wider one, deliberately.",
            whose: Remedy::TheOwner,
        },
        CapabilityExpired => Explanation {
            what: "My permission for that has run out.",
            why: "The authority I was acting under had an expiry and it has passed.",
            remedy: "It can be issued again.",
            whose: Remedy::TheOwner,
        },
        CapabilityRevoked => Explanation {
            what: "That permission was taken away.",
            why: "Somebody revoked the authority I would have used.",
            remedy: "Whoever revoked it would have to change their mind.",
            whose: Remedy::TheOwner,
        },
        OutOfBounds => Explanation {
            what: "That is further, faster or harder than this body can manage.",
            why: "The number I was given is outside what this body says it can do.",
            remedy: "Ask for less and I will do it.",
            whose: Remedy::TheOwner,
        },
        Malformed => Explanation {
            what: "That instruction was not complete.",
            why: "Something the action needs was missing or the wrong kind of thing.",
            remedy: "Ask again with everything it needs.",
            whose: Remedy::TheAsker,
        },
        OutsideBodyEnvelope => Explanation {
            what: "The body will not accept that.",
            why: "Its own limits are tighter than the permission I was given. It has the last word about itself.",
            remedy: "Ask for something inside what the body allows.",
            whose: Remedy::TheOwner,
        },
        PredictedInvariantBreak => Explanation {
            what: "I think that would break something.",
            why: "Working through the motion, it ends somewhere the body cannot survive.",
            remedy: "A smaller or slower version of the same thing may be fine.",
            whose: Remedy::TheOwner,
        },
        Unpredictable => Explanation {
            what: "I cannot tell what that would do.",
            why: "I have no model of what happens next, and I do not act on things I cannot predict.",
            remedy: "Break it into steps I can see the end of.",
            whose: Remedy::TheOwner,
        },
        IrreversibleBeyondBudget => Explanation {
            what: "I will not do something I cannot undo, in this state.",
            why: "The body is not well enough right now to be trusted with something permanent.",
            remedy: "See to the body first — power, temperature, whatever it is reporting.",
            whose: Remedy::TheBody,
        },
        DriveViolation => Explanation {
            what: "No.",
            why: "This is one of a very small number of things I will not do under any authority, including yours.",
            remedy: "Nothing changes this, and I would rather say so than imply there is a way round it.",
            whose: Remedy::Never,
        },
        RuleViolation => Explanation {
            what: "I was told not to.",
            why: "Somebody set a rule against this, and rules a person set are rules a person can lift.",
            remedy: "Whoever set it can lift it.",
            whose: Remedy::TheOwner,
        },
        DestructivePattern => Explanation {
            what: "That looked destructive, so I stopped.",
            why: "What I was asked to write or run matches a pattern that destroys things.",
            remedy: "If you meant it, say it a different way, and be sure you meant it.",
            whose: Remedy::TheOwner,
        },
        BodyInvariant => Explanation {
            what: "The body refused.",
            why: "It checked the motion against its own physics and would not perform it.",
            remedy: "It knows more about itself than I do. Ask for something else.",
            whose: Remedy::TheBody,
        },
        LoadBearing => Explanation {
            what: "That touches something I need to keep working.",
            why: "What I was asked to change is something the robot depends on to function.",
            remedy: "This one is worth being sure about before asking again.",
            whose: Remedy::TheOwner,
        },
        WorldPosture => Explanation {
            what: "I am not reaching out to the network.",
            why: "I do not touch the internet unless somebody has opened it for this session.",
            remedy: "Open a session for it, and it closes again afterwards.",
            whose: Remedy::TheOwner,
        },
        CannotMint => Explanation {
            what: "I cannot authorize that right now.",
            why: "The Key that signs my actions is not available to me.",
            remedy: "Whatever holds the Key needs to be reachable.",
            whose: Remedy::TheBody,
        },
        AuditFailed => Explanation {
            what: "I stopped, and I will keep stopping.",
            why: "I could not record what I was about to do, and I do not act without a record.",
            remedy: "Nothing you can ask me changes this. The record has to be fixed first.",
            whose: Remedy::Never,
        },
    }
}

impl Explanation {
    /// The three sentences, with the verb that was asked for.
    pub fn say(&self, verb: &Verb) -> String {
        let mut s = String::new();
        s.push_str(self.what);
        s.push(' ');
        s.push_str("You asked me to ");
        s.push_str(verb.action());
        s.push_str(". ");
        s.push_str(self.why);
        s.push(' ');
        s.push_str(self.remedy);
        s
    }
}

/// What the entity says when it refuses.
pub fn say_refusal(d: &Denied, verb: &Verb) -> String {
    explain(d.code).say(verb)
}
