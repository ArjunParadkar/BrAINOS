//! BFS  LEVEL    Stage 4 Entity - one Key, one state graph, every body at once  [crate brainos-entity]
//! BFS  PURPOSE  Stores the entity's goals in four levels (fixed drives, human requests, derived subgoals, micro-goals) and expires derived goals that go stale or fail
//! BFS  DEFINES  Level, GoalId, Goal, Goals, DRIVES
//! BFS  USES     brainos: abi
//! BFS  USED BY  entity/entity, entity/lib, entity/tests/entity_suite
//!
//! The goal hierarchy (§13).
//!
//! Four levels, and only the first two are policy. Levels 2 and 3 are
//! what the entity is trying to do, which is a different kind of object
//! from what the entity is permitted to do — a distinction that gets
//! lost in systems where an agent's plan and its permissions are the
//! same list.
//!
//! * **Level 0 — drives.** Instinctive, immutable. They live in KIRA as
//!   policy invariants, not here; this module only names them so the
//!   entity can say what they are.
//! * **Level 1 — human-set.** What the person asked for.
//! * **Level 2 — subgoals.** Derived by the entity from Level 1.
//! * **Level 3 — micro-goals.** Moment to moment.
//!
//! Derived goals expire. That is the difference between a goal hierarchy
//! and an accumulating to-do list that quietly becomes the entity's real
//! personality: a subgoal the entity invented and then could not achieve
//! should fade, not persist and distort everything downstream of it.

use alloc::string::String;
use alloc::vec::Vec;
use brainos_abi::text::Text;
use brainos_abi::time::Tick;

#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord)]
#[repr(u8)]
pub enum Level {
    /// instinctive; enforced by KIRA, not by intention
    Drive = 0,
    /// what the human asked for
    Human = 1,
    /// what the entity derived from that
    Sub = 2,
    /// moment to moment
    Micro = 3,
}

impl Level {
    pub const fn name(self) -> &'static str {
        match self {
            Level::Drive => "drive",
            Level::Human => "human",
            Level::Sub => "sub",
            Level::Micro => "micro",
        }
    }

    /// How long a goal at this level lives before it fades, in ticks.
    /// Human goals do not fade on their own — only the human retires
    /// them, because an assistant that quietly forgets what it was asked
    /// is worse than one that says it cannot.
    pub const fn lifetime(self) -> Option<u64> {
        match self {
            Level::Drive | Level::Human => None,
            Level::Sub => Some(50_000),
            Level::Micro => Some(500),
        }
    }
}

/// A stable handle on a goal. Not an index: goals are removed when they
/// fade or are retired, and a parent pointer that was an index would
/// silently start referring to a different goal the moment one went.
#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord, Hash)]
pub struct GoalId(pub u32);

#[derive(Clone, Debug, PartialEq)]
pub struct Goal {
    pub id: GoalId,
    pub what: String,
    pub level: Level,
    pub set_at: Tick,
    /// which goal this was derived from
    pub parent: Option<GoalId>,
    /// 0–100; how far along the entity believes it is
    pub progress: u8,
    /// attempts that did not work. A subgoal that keeps failing should
    /// be abandoned and the parent reconsidered, not retried forever.
    pub failures: u16,
}

impl Goal {
    pub fn is_stale(&self, now: Tick) -> bool {
        match self.level.lifetime() {
            None => false,
            Some(life) => now.since(self.set_at) > life,
        }
    }

    pub fn is_futile(&self) -> bool {
        self.failures >= 5 && self.progress < 20
    }
}

/// The hierarchy.
#[derive(Default)]
pub struct Goals {
    goals: Vec<Goal>,
    next_id: u32,
    pub drives: &'static [&'static str],
}

/// The Level-0 drives, as the entity would state them. The enforcement
/// lives in `brainos-kira`; these are the words.
pub const DRIVES: [&str; 4] = [
    "maintain a coherent world model",
    "reduce prediction error",
    "preserve memory integrity",
    "serve the bound human",
];

impl Goals {
    pub fn new() -> Goals {
        Goals {
            goals: Vec::new(),
            next_id: 1,
            drives: &DRIVES,
        }
    }

    pub fn all(&self) -> &[Goal] {
        &self.goals
    }

    pub fn len(&self) -> usize {
        self.goals.len()
    }

    pub fn is_empty(&self) -> bool {
        self.goals.is_empty()
    }

    pub fn at_level(&self, level: Level) -> impl Iterator<Item = &Goal> {
        self.goals.iter().filter(move |g| g.level == level)
    }

    pub fn get(&self, id: GoalId) -> Option<&Goal> {
        self.goals.iter().find(|g| g.id == id)
    }

    fn mint(&mut self) -> GoalId {
        let id = GoalId(self.next_id);
        self.next_id += 1;
        id
    }

    /// The human asked for something.
    pub fn asked(&mut self, what: &str, now: Tick) -> GoalId {
        let id = self.mint();
        self.goals.push(Goal {
            id,
            what: alloc::string::ToString::to_string(what),
            level: Level::Human,
            set_at: now,
            parent: None,
            progress: 0,
            failures: 0,
        });
        id
    }

    /// The entity derived a subgoal. Returns `None` if the parent does
    /// not exist or is not above this level — a subgoal of a micro-goal
    /// is a sign the entity has lost the thread.
    pub fn derive(&mut self, parent: GoalId, what: &str, level: Level, now: Tick) -> Option<GoalId> {
        let p = self.goals.iter().find(|g| g.id == parent)?;
        if level <= p.level {
            return None;
        }
        let id = self.mint();
        self.goals.push(Goal {
            id,
            what: alloc::string::ToString::to_string(what),
            level,
            set_at: now,
            parent: Some(parent),
            progress: 0,
            failures: 0,
        });
        Some(id)
    }

    pub fn progress(&mut self, id: GoalId, to: u8) {
        if let Some(g) = self.goals.iter_mut().find(|g| g.id == id) {
            g.progress = to.min(100);
        }
    }

    pub fn failed(&mut self, id: GoalId) {
        if let Some(g) = self.goals.iter_mut().find(|g| g.id == id) {
            g.failures = g.failures.saturating_add(1);
        }
    }

    /// The human retired a goal. Only Level 1 can be retired this way;
    /// derived goals go when their parent does or when they fade.
    pub fn retire(&mut self, id: GoalId) -> bool {
        if !self
            .goals
            .iter()
            .any(|g| g.id == id && g.level == Level::Human)
        {
            return false;
        }
        self.drop_subtree(id);
        true
    }

    /// Remove a goal and everything derived from it, transitively.
    fn drop_subtree(&mut self, root: GoalId) {
        let mut doomed = alloc::vec![root];
        let mut changed = true;
        while changed {
            changed = false;
            for g in self.goals.iter() {
                if let Some(p) = g.parent {
                    if doomed.contains(&p) && !doomed.contains(&g.id) {
                        doomed.push(g.id);
                        changed = true;
                    }
                }
            }
        }
        self.goals.retain(|g| !doomed.contains(&g.id));
    }

    /// Drop what has gone stale or futile. Returns what was dropped, so
    /// the entity can remember having abandoned it rather than simply
    /// ceasing to want it.
    pub fn tidy(&mut self, now: Tick) -> Vec<Goal> {
        let mut dropped = Vec::new();
        let mut kept = Vec::new();
        for g in self.goals.drain(..) {
            if g.is_stale(now) || g.is_futile() {
                dropped.push(g);
            } else {
                kept.push(g);
            }
        }
        self.goals = kept;

        // a subgoal whose parent is gone is an orphan, and an orphan is a
        // goal the entity is pursuing for a reason it can no longer state
        loop {
            let live: Vec<GoalId> = self.goals.iter().map(|g| g.id).collect();
            let orphans: Vec<GoalId> = self
                .goals
                .iter()
                .filter(|g| match g.parent {
                    None => false,
                    Some(p) => !live.contains(&p),
                })
                .map(|g| g.id)
                .collect();
            if orphans.is_empty() {
                break;
            }
            for id in orphans {
                if let Some(pos) = self.goals.iter().position(|g| g.id == id) {
                    dropped.push(self.goals.remove(pos));
                }
            }
        }
        dropped
    }

    /// What the entity should be working on right now: the deepest
    /// unfinished goal, because depth means "most specific thing I can
    /// actually act on".
    pub fn current(&self) -> Option<(GoalId, &Goal)> {
        self.goals
            .iter()
            .filter(|g| g.progress < 100 && g.level != Level::Drive)
            .max_by_key(|g| (g.level as u8, 255 - g.failures.min(255) as u8))
            .map(|g| (g.id, g))
    }

    pub fn describe(&self) -> Text<200> {
        let mut t = Text::new();
        t.push_u64(self.goals.len() as u64);
        t.push_str(" goals");
        if let Some((_, g)) = self.current() {
            t.push_str(" · now: ");
            t.push_str(&g.what);
        }
        t
    }
}
