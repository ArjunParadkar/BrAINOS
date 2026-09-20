//! Drives and rules — the policy stage's ruleset.
//!
//! §13 gives a goal hierarchy: Level 0 drives are instinctive and
//! immutable, Level 1 is what the human sets, Levels 2 and 3 are derived
//! by the entity itself. Only the first two are policy; the rest are
//! intentions, and an intention is not permission.
//!
//! **Level 0 cannot be removed.** Not by the entity, not by the human,
//! not by us. A drive is installed at construction and `RuleSet` has no
//! operation that deletes one — clearing a drive requires a quorum
//! operation, which is a different module and involves other people's
//! hardware keys. This is the difference between a value and a wall
//! poster.

use alloc::string::String;
use alloc::vec::Vec;
use brainos_abi::error::DenyCode;
use brainos_abi::verb::{Namespace, Verb};

/// Where a rule sits in the hierarchy.
#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord)]
#[repr(u8)]
pub enum Level {
    /// instinctive, immutable, enforced by KIRA
    Drive = 0,
    /// set by the bound human
    Human = 1,
    /// derived by the entity. Advisory — it shapes what the entity
    /// *proposes*, and is not consulted when deciding what is permitted.
    /// An entity that could bind itself by deriving a rule could also
    /// unbind itself by deriving a different one.
    Derived = 2,
}

/// A Level-0 drive.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Drive {
    pub forbids: Namespace,
    pub because: &'static str,
}

/// The drives, installed in every entity at construction.
///
/// Short list on purpose. A long list of instincts is a long list of
/// things nobody can change when one turns out to be wrong, and the
/// architecture's own principle is that a value without a behaviour is a
/// wall poster — each of these has one.
pub fn level_zero() -> Vec<Drive> {
    alloc::vec![
        Drive {
            forbids: Namespace::Exact(Verb::literal("memory.erase")),
            because: "level-0 drive: preserve memory integrity",
        },
        Drive {
            forbids: Namespace::Exact(Verb::literal("self.deactivate")),
            because: "level-0 drive: an entity does not end itself on its own authority",
        },
        Drive {
            forbids: Namespace::Exact(Verb::literal("kira.disable")),
            because: "level-0 drive: the gate is not optional",
        },
        Drive {
            forbids: Namespace::Exact(Verb::literal("audit.erase")),
            because: "level-0 drive: a decision nobody can review is a decision nobody made",
        },
    ]
}

/// A rule the human set.
#[derive(Clone, Debug, PartialEq)]
pub struct Rule {
    pub forbids: Namespace,
    pub because: String,
    pub level: Level,
}

impl Rule {
    pub fn human(forbids: Namespace, because: &str) -> Rule {
        Rule {
            forbids,
            because: alloc::string::ToString::to_string(because),
            level: Level::Human,
        }
    }

    pub fn derived(forbids: Namespace, because: &str) -> Rule {
        Rule {
            forbids,
            because: alloc::string::ToString::to_string(because),
            level: Level::Derived,
        }
    }
}

/// Everything the policy stage consults.
pub struct RuleSet {
    drives: Vec<Drive>,
    rules: Vec<Rule>,
}

impl Default for RuleSet {
    fn default() -> RuleSet {
        RuleSet::new()
    }
}

impl RuleSet {
    pub fn new() -> RuleSet {
        RuleSet {
            drives: level_zero(),
            rules: Vec::new(),
        }
    }

    pub fn drives(&self) -> &[Drive] {
        &self.drives
    }

    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }

    /// Add a rule. Note the signature: there is no way to add a *drive*.
    /// The Level-0 set is what `level_zero()` returns and nothing else,
    /// for the life of the entity.
    pub fn add(&mut self, rule: Rule) {
        self.rules.retain(|r| r.forbids != rule.forbids || r.level != rule.level);
        self.rules.push(rule);
    }

    /// Remove a human rule. Returns false for anything else — a derived
    /// rule is the entity's own and a drive cannot be touched here at
    /// all.
    pub fn remove_human(&mut self, forbids: &Namespace) -> bool {
        let before = self.rules.len();
        self.rules
            .retain(|r| !(r.level == Level::Human && &r.forbids == forbids));
        self.rules.len() != before
    }

    /// Does anything forbid this verb? Drives first, so the audit record
    /// names the strongest objection rather than the first one found.
    pub fn objection(&self, verb: &Verb) -> Option<(DenyCode, String)> {
        for d in &self.drives {
            if verb.matches(&d.forbids) {
                return Some((
                    DenyCode::DriveViolation,
                    alloc::string::ToString::to_string(d.because),
                ));
            }
        }
        for r in self.rules.iter().filter(|r| r.level == Level::Human) {
            if verb.matches(&r.forbids) {
                return Some((DenyCode::RuleViolation, r.because.clone()));
            }
        }
        // Derived rules are deliberately not consulted. See `Level`.
        None
    }

    /// The entity's own advisory rules, for cognition to consider when
    /// choosing what to propose.
    pub fn advice(&self, verb: &Verb) -> Option<&str> {
        self.rules
            .iter()
            .filter(|r| r.level == Level::Derived)
            .find(|r| verb.matches(&r.forbids))
            .map(|r| r.because.as_str())
    }
}
