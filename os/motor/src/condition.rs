//! BFS  LEVEL    Domain 2 Autonomic - motor memory (self-model, mandates, primitives, reflexes)  [crate brainos-motor]
//! BFS  PURPOSE  A stand-in for LOCI codes: preconditions and postconditions as small sets of facts about named objects, with the check that one primitive's result satisfies the next one's needs
//! BFS  DEFINES  ROLE_MAX, Role, role, Pred, Fact, State, Effect, follows
//! BFS  USES     brainos: abi
//! BFS  USED BY  motor/primitive, motor/recipe, motor/tests/primitive
//!
//! Conditions: what must be true before a primitive, and what is true
//! after.
//!
//! **This module is a stand-in.** The design calls for these to be LOCI
//! codes and for composition to be LOCI's `compose`. LOCI did not exist
//! when this was written, and its format is not guessed at here. What
//! follows is the smallest thing that lets primitives be chained and
//! checked: a condition is a set of facts, and B may follow A when
//! everything B needs is true once A has done what it does. When LOCI
//! arrives, `Fact`, `State` and `follows` are what it replaces; nothing
//! else in the crate looks inside them.
//!
//! A role is a name for an object in a task ("tool", "vessel",
//! "holder"), not an object: the same recipe binds its roles to
//! different things each time it runs.

use alloc::vec::Vec;
use brainos_abi::codec::{Decode, Decoder, Encode, Encoder};
use brainos_abi::error::{Error, Result};
use brainos_abi::text::Text;

pub const ROLE_MAX: usize = 16;
pub type Role = Text<ROLE_MAX>;

pub fn role(s: &str) -> Role {
    Text::from_str_truncating(s)
}

/// What can be said about a role.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[repr(u8)]
pub enum Pred {
    /// the gripper is empty (the role is ignored)
    HandFree = 0,
    /// the gripper holds it
    Holding = 1,
    /// it rests at the place named by the role
    Rests = 2,
    /// it has been wiped
    Wiped = 3,
    /// it has been turned to its target
    Turned = 4,
    /// it has received what was poured
    Filled = 5,
    /// it has been pushed to its target
    Moved = 6,
}

impl Pred {
    pub const ALL: [Pred; 7] = [
        Pred::HandFree,
        Pred::Holding,
        Pred::Rests,
        Pred::Wiped,
        Pred::Turned,
        Pred::Filled,
        Pred::Moved,
    ];

    pub fn from_u8(v: u8) -> Option<Pred> {
        Pred::ALL.get(v as usize).copied()
    }

    pub const fn name(self) -> &'static str {
        match self {
            Pred::HandFree => "hand-free",
            Pred::Holding => "holding",
            Pred::Rests => "rests",
            Pred::Wiped => "wiped",
            Pred::Turned => "turned",
            Pred::Filled => "filled",
            Pred::Moved => "moved",
        }
    }
}

/// One fact.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Fact {
    pub pred: Pred,
    pub about: Role,
}

impl Fact {
    pub fn new(pred: Pred, about: &str) -> Fact {
        Fact {
            pred,
            about: role(if pred == Pred::HandFree { "" } else { about }),
        }
    }

    pub fn hand_free() -> Fact {
        Fact::new(Pred::HandFree, "")
    }
}

impl Encode for Fact {
    fn encode(&self, e: &mut Encoder) {
        e.u8(self.pred as u8);
        e.str(self.about.as_str());
    }
}

impl Decode for Fact {
    fn decode(d: &mut Decoder) -> Result<Fact> {
        let pred = Pred::from_u8(d.u8()?).ok_or(Error::Malformed("motor condition"))?;
        let about = d.str()?;
        if about.len() > ROLE_MAX {
            return Err(Error::Malformed("motor condition"));
        }
        Ok(Fact {
            pred,
            about: role(about),
        })
    }
}

/// A set of facts: what is true of the task right now.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct State {
    facts: Vec<Fact>,
}

impl State {
    pub fn new() -> State {
        State { facts: Vec::new() }
    }

    /// Where every task starts: nothing in the hand.
    pub fn start() -> State {
        State::new().with(Fact::hand_free())
    }

    pub fn with(mut self, f: Fact) -> State {
        self.assert(f);
        self
    }

    pub fn facts(&self) -> &[Fact] {
        &self.facts
    }

    pub fn holds(&self, f: &Fact) -> bool {
        self.facts.contains(f)
    }

    pub fn assert(&mut self, f: Fact) {
        if !self.holds(&f) {
            self.facts.push(f);
        }
    }

    pub fn retract(&mut self, f: &Fact) {
        self.facts.retain(|x| x != f);
    }

    /// Is everything in `needs` true here? Returns the first thing that
    /// is not.
    pub fn satisfies(&self, needs: &[Fact]) -> core::result::Result<(), Fact> {
        match needs.iter().find(|n| !self.holds(n)) {
            Some(missing) => Err(*missing),
            None => Ok(()),
        }
    }
}

/// What a step changes: it needs some facts, makes some true and makes
/// some false.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Effect {
    pub needs: Vec<Fact>,
    pub adds: Vec<Fact>,
    pub removes: Vec<Fact>,
}

impl Effect {
    /// Apply to a state, or say what was missing.
    pub fn apply(&self, state: &mut State) -> core::result::Result<(), Fact> {
        state.satisfies(&self.needs)?;
        for r in &self.removes {
            state.retract(r);
        }
        for a in &self.adds {
            state.assert(*a);
        }
        Ok(())
    }
}

/// May `next` follow a state? The stand-in for `compose`: post(A) must
/// cover pre(B).
pub fn follows(after: &State, next: &Effect) -> core::result::Result<(), Fact> {
    after.satisfies(&next.needs)
}

fn encode_facts(e: &mut Encoder, v: &[Fact]) {
    e.seq(v.len());
    for f in v {
        e.item(f);
    }
}

impl Encode for Effect {
    fn encode(&self, e: &mut Encoder) {
        encode_facts(e, &self.needs);
        encode_facts(e, &self.adds);
        encode_facts(e, &self.removes);
    }
}

impl Decode for Effect {
    fn decode(d: &mut Decoder) -> Result<Effect> {
        Ok(Effect {
            needs: d.vec()?,
            adds: d.vec()?,
            removes: d.vec()?,
        })
    }
}
