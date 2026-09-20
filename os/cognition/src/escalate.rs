//! Escalation — when to reach higher, and when to stop.
//!
//! Escalation is a decision with a cost, so it is made explicitly rather
//! than by a threshold buried in a loop. Four things push the entity up
//! a tier, and one thing pulls it back down.
//!
//! Up: genuine surprise, consequence, a skill it does not have, and a
//! repeated failure at the current tier. Down: a budget. The budget
//! exists because the failure mode of an escalating system without one
//! is an entity that spends its way to the frontier on every tick, and
//! the failure mode of that on a body is an entity that is thinking
//! beautifully while falling over.

use crate::predict::Surprise;
use crate::resource::Tier;
use brainos_abi::text::Text;

/// Why the entity reached higher.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Reason {
    /// the observation was far from prediction
    Surprised,
    /// something new, with no history at all
    Novel,
    /// the consequence is large enough to be worth thinking about
    Consequential,
    /// there is no practiced skill for this
    NoSkill,
    /// the tier below tried and failed
    LowerTierFailed,
    /// a human asked directly
    Asked,
    /// consolidation, which is always Tier 3 work
    Consolidating,
}

impl Reason {
    pub const fn name(self) -> &'static str {
        match self {
            Reason::Surprised => "surprised",
            Reason::Novel => "novel",
            Reason::Consequential => "consequential",
            Reason::NoSkill => "no practiced skill",
            Reason::LowerTierFailed => "the tier below failed",
            Reason::Asked => "asked",
            Reason::Consolidating => "consolidating",
        }
    }

    /// How far up this reason justifies going on its own.
    pub const fn warrants(self) -> Tier {
        match self {
            Reason::Surprised | Reason::NoSkill => Tier::Deliberate,
            Reason::Novel | Reason::LowerTierFailed | Reason::Consolidating => Tier::Frontier,
            Reason::Consequential => Tier::Deliberate,
            Reason::Asked => Tier::Fast,
        }
    }
}

/// A decision to escalate, or not to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Escalation {
    pub to: Tier,
    pub reason: Option<Reason>,
    /// what it will cost, against the budget
    pub cost: u32,
    /// true when the entity wanted to go higher and could not afford to
    pub budget_limited: bool,
}

impl Escalation {
    pub fn none() -> Escalation {
        Escalation {
            to: Tier::Autonomic,
            reason: None,
            cost: 0,
            budget_limited: false,
        }
    }

    pub fn happened(&self) -> bool {
        self.reason.is_some()
    }

    pub fn describe(&self) -> Text<96> {
        let mut t = Text::new();
        match self.reason {
            None => t.push_str("no escalation"),
            Some(r) => {
                t.push_str("→ ");
                t.push_str(self.to.name());
                t.push_str(" (");
                t.push_str(r.name());
                t.push(')');
                if self.budget_limited {
                    t.push_str(" [budget-limited]");
                }
            }
        }
        t
    }
}

/// How much escalation the entity can afford.
///
/// A leaky bucket rather than a hard quota: the entity accumulates
/// allowance over time and spends it on escalations. That shape matters
/// because it permits a burst when something genuinely happens while
/// still bounding the long-run rate — which is exactly the behaviour
/// wanted from something that must be cheap while idle and expensive
/// when it matters.
pub struct Budget {
    allowance: u32,
    cap: u32,
    per_tick: u32,
    pub spent: u64,
    pub refused: u64,
}

impl Budget {
    pub fn new(cap: u32, per_tick: u32) -> Budget {
        Budget {
            allowance: cap,
            cap,
            per_tick,
            spent: 0,
            refused: 0,
        }
    }

    /// A generous budget for a workstation body; a frontier call every
    /// few seconds is affordable there.
    pub fn plentiful() -> Budget {
        Budget::new(1_000, 20)
    }

    /// A tight one for a battery-powered body.
    pub fn frugal() -> Budget {
        Budget::new(200, 2)
    }

    pub fn tick(&mut self) {
        self.allowance = self.allowance.saturating_add(self.per_tick).min(self.cap);
    }

    pub fn available(&self) -> u32 {
        self.allowance
    }

    pub fn can_afford(&self, cost: u32) -> bool {
        self.allowance >= cost
    }

    pub fn spend(&mut self, cost: u32) -> bool {
        if self.allowance >= cost {
            self.allowance -= cost;
            self.spent = self.spent.saturating_add(cost as u64);
            true
        } else {
            self.refused = self.refused.saturating_add(1);
            false
        }
    }
}

/// Decide whether and how far to escalate.
///
/// Arguments are the situation, not a model's opinion of it: how
/// surprising, how consequential, whether a practiced skill exists, and
/// how many times the tier below has already failed. Each is something
/// the entity can determine from its own state, which is what keeps this
/// decision out of the hands of the thing being escalated to.
#[allow(clippy::too_many_arguments)]
pub fn decide(
    surprise: &Surprise,
    consequence: u8,
    have_practiced_skill: bool,
    lower_failures: u32,
    human_asked: bool,
    budget: &mut Budget,
    ceiling: Tier,
) -> Escalation {
    // strongest reason wins, and the order is the priority
    let reason = if surprise.novel {
        Some(Reason::Novel)
    } else if lower_failures >= 2 {
        Some(Reason::LowerTierFailed)
    } else if consequence >= 70 {
        Some(Reason::Consequential)
    } else if surprise.is_salient() {
        Some(Reason::Surprised)
    } else if !have_practiced_skill {
        Some(Reason::NoSkill)
    } else if human_asked {
        Some(Reason::Asked)
    } else {
        None
    };

    let Some(reason) = reason else {
        return Escalation::none();
    };

    let wanted = reason.warrants().min(ceiling);
    let cost = cost_of(wanted);

    if budget.spend(cost) {
        return Escalation {
            to: wanted,
            reason: Some(reason),
            cost,
            budget_limited: wanted < reason.warrants(),
        };
    }

    // cannot afford it: step down until something is affordable, rather
    // than doing nothing. An entity that refuses to think because the
    // frontier is expensive should still think locally.
    let mut tier = wanted;
    while let Some(lower) = step_down(tier) {
        let c = cost_of(lower);
        if budget.spend(c) {
            return Escalation {
                to: lower,
                reason: Some(reason),
                cost: c,
                budget_limited: true,
            };
        }
        tier = lower;
    }
    Escalation {
        to: Tier::Autonomic,
        reason: Some(reason),
        cost: 0,
        budget_limited: true,
    }
}

fn step_down(t: Tier) -> Option<Tier> {
    match t {
        Tier::Frontier => Some(Tier::Deliberate),
        Tier::Deliberate => Some(Tier::Fast),
        Tier::Fast => None,
        Tier::Autonomic => None,
    }
}

pub const fn cost_of(t: Tier) -> u32 {
    match t {
        Tier::Autonomic => 0,
        Tier::Fast => 1,
        Tier::Deliberate => 10,
        Tier::Frontier => 100,
    }
}
