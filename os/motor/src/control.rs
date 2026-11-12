//! BFS  LEVEL    Domain 2 Autonomic - motor memory (self-model, mandates, primitives, reflexes)  [crate brainos-motor]
//! BFS  PURPOSE  Turns a list of goals into one smooth path: minimum-jerk moves that overlap so the body does not stop between strokes, plus the variable-stiffness joint law
//! BFS  DEFINES  DIMS, minjerk, Goal, Style, Want, Touch, Played, Player, Gain, impedance
//! BFS  USES     this crate: primitive
//! BFS  USED BY  bodyd/armloop, host/motor_seat, host/tests/motor_mandate, motor/primitive, motor/tests/control
//!
//! Fluid movement: one path out of many strokes.
//!
//! A robot that finishes each stroke, stops, and starts the next is
//! doing what it was told and looks like it. A person reaching for a
//! cup has started closing the hand before the arm arrives. The
//! difference is small and mechanical:
//!
//! * each stroke is a **minimum-jerk** move (it starts and ends with no
//!   velocity and no acceleration),
//! * a stroke that need not end at rest lets the next one **start
//!   before it finishes**, and the two are added, so the path bends
//!   through the corner instead of stopping in it,
//! * stiffness is a number that changes along the way, not a constant.
//!
//! `Player` does the first two. `impedance` does the third.
//!
//! Nothing here is specific to an arm: a goal is a short vector, and
//! what its components mean belongs to the body.

use crate::primitive::{Contact, Until};
use alloc::vec::Vec;

/// Components in a goal vector.
pub const DIMS: usize = 7;

/// Minimum-jerk position profile: 0 at 0, 1 at 1, flat at both ends.
pub fn minjerk(u: f32) -> f32 {
    let u = u.clamp(0.0, 1.0);
    u * u * u * (10.0 + u * (-15.0 + 6.0 * u))
}

/// One goal: where to be, and how to be on the way there.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Goal {
    pub at: [f32; DIMS],
    pub squeeze_n: f32,
    pub ms: u32,
    pub stiff: f32,
    pub contact: Contact,
    pub until: Until,
    pub rest: bool,
}

/// How strokes are joined.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Style {
    /// how much of a stroke's time the next may overlap, 0 to 0.9
    pub blend: f32,
    /// a wait after every stroke, in milliseconds
    pub pause_ms: u32,
    /// use this stiffness for every stroke instead of each stroke's own
    pub fixed_stiff: Option<f32>,
}

impl Style {
    /// Overlapping strokes, each with its own stiffness.
    pub const FLUID: Style = Style {
        blend: 0.4,
        pause_ms: 0,
        fixed_stiff: None,
    };

    /// The baseline: stiff, and a full stop between strokes.
    pub const STOP_GO: Style = Style {
        blend: 0.0,
        pause_ms: 150,
        fixed_stiff: Some(1.0),
    };
}

#[derive(Clone, Copy, Debug)]
struct Move {
    delta: [f32; DIMS],
    t0_ms: f32,
    dur_ms: f32,
    goal: usize,
}

/// What the player wants this tick.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Want {
    pub at: [f32; DIMS],
    pub squeeze_n: f32,
    pub stiff: f32,
    pub contact: Contact,
    /// which goal is in charge
    pub goal: usize,
}

/// What the body feels that a stroke can end on.
#[derive(Clone, Copy, Debug, Default)]
pub struct Touch {
    /// force on the tool from the world, newtons
    pub tool_n: f32,
    /// force between the fingers, newtons
    pub grip_n: f32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Played {
    Going,
    Done,
    /// the fingers were told to hold something and felt nothing
    NothingGripped,
}

/// Plays goals as one path.
pub struct Player {
    goals: Vec<Goal>,
    style: Style,
    start: [f32; DIMS],
    /// where the path ends once every started move has finished
    planned: [f32; DIMS],
    moves: Vec<Move>,
    next: usize,
    /// when the goal in charge was satisfied, if it has been
    settled_ms: Option<f32>,
    t_ms: f32,
}

const GRIP_GRACE_MS: f32 = 600.0;

impl Player {
    pub fn new(start: [f32; DIMS], goals: Vec<Goal>, style: Style) -> Player {
        Player {
            goals,
            style,
            start,
            planned: start,
            moves: Vec::new(),
            next: 0,
            settled_ms: None,
            t_ms: 0.0,
        }
    }

    /// Add goals to the end of what is being played. This is how the
    /// next primitive is joined to the current one without a stop.
    pub fn append(&mut self, goals: &[Goal]) {
        self.goals.extend_from_slice(goals);
    }

    pub fn elapsed_ms(&self) -> f32 {
        self.t_ms
    }

    pub fn goals_left(&self) -> usize {
        self.goals.len() - self.next
    }

    /// Is the last goal the one in charge, and may something be joined
    /// onto it now?
    pub fn at_the_seam(&self) -> bool {
        self.next == self.goals.len() && self.ready_for_next()
    }

    fn position(&self) -> [f32; DIMS] {
        let mut x = self.start;
        for m in &self.moves {
            let s = minjerk((self.t_ms - m.t0_ms) / m.dur_ms);
            for k in 0..DIMS {
                x[k] += m.delta[k] * s;
            }
        }
        x
    }

    fn ready_for_next(&self) -> bool {
        let Some(last) = self.moves.last() else {
            return true;
        };
        let g = &self.goals[last.goal];
        let rest = g.rest || self.style.blend <= 0.0;
        if rest {
            match self.settled_ms {
                Some(at) => self.t_ms >= at + self.style.pause_ms as f32,
                None => false,
            }
        } else {
            self.t_ms >= last.t0_ms + last.dur_ms * (1.0 - self.style.blend)
        }
    }

    /// Advance by `dt_ms` and say what is wanted now.
    pub fn step(&mut self, dt_ms: f32, touch: &Touch) -> (Want, Played) {
        self.t_ms += dt_ms.max(0.0);

        // has the goal in charge been satisfied?
        if self.settled_ms.is_none() {
            if let Some(last) = self.moves.last().copied() {
                let g = self.goals[last.goal];
                let timed_out = self.t_ms >= last.t0_ms + last.dur_ms;
                let settled = match g.until {
                    Until::Time => timed_out,
                    Until::Touch(n) => {
                        if touch.tool_n >= n && !timed_out {
                            // stop where the touch happened: the rest of
                            // this move is not made
                            let s = minjerk((self.t_ms - last.t0_ms) / last.dur_ms);
                            if let Some(m) = self.moves.last_mut() {
                                for k in 0..DIMS {
                                    let made = m.delta[k] * s;
                                    self.planned[k] -= m.delta[k] - made;
                                    self.start[k] += made;
                                    m.delta[k] = 0.0;
                                }
                            }
                            true
                        } else {
                            timed_out
                        }
                    }
                    Until::Gripped(n) => {
                        if touch.grip_n >= n {
                            true
                        } else if self.t_ms >= last.t0_ms + last.dur_ms + GRIP_GRACE_MS {
                            let at = self.position();
                            return (self.want(at, last.goal), Played::NothingGripped);
                        } else {
                            false
                        }
                    }
                };
                if settled {
                    self.settled_ms = Some(self.t_ms);
                }
            }
        }

        // start the next goal if its turn has come
        while self.next < self.goals.len() && self.ready_for_next() {
            let g = self.goals[self.next];
            let mut delta = [0.0f32; DIMS];
            for k in 0..DIMS {
                delta[k] = g.at[k] - self.planned[k];
            }
            self.planned = g.at;
            self.moves.push(Move {
                delta,
                t0_ms: self.t_ms,
                dur_ms: (g.ms as f32).max(1.0),
                goal: self.next,
            });
            self.next += 1;
            self.settled_ms = None;
            // fold finished moves into the start so the list stays short
            let t = self.t_ms;
            let mut folded = self.start;
            self.moves.retain(|m| {
                let done = t >= m.t0_ms + m.dur_ms;
                if done {
                    for k in 0..DIMS {
                        folded[k] += m.delta[k];
                    }
                }
                !done
            });
            self.start = folded;
            if self.moves.is_empty() {
                // the move just pushed cannot have finished, so this
                // cannot happen; kept so a zero-length list is never read
                break;
            }
        }

        let at = self.position();
        let in_charge = self.next.saturating_sub(1);
        let done = self.next == self.goals.len()
            && self.settled_ms.is_some()
            && self.moves.iter().all(|m| self.t_ms >= m.t0_ms + m.dur_ms);
        (
            self.want(at, in_charge),
            if done || self.goals.is_empty() {
                Played::Done
            } else {
                Played::Going
            },
        )
    }

    fn want(&self, at: [f32; DIMS], goal: usize) -> Want {
        let g = self.goals.get(goal);
        let own = g.map(|g| g.stiff).unwrap_or(1.0);
        // while two strokes overlap, the softer one governs
        let softest = self
            .moves
            .iter()
            .filter(|m| self.t_ms < m.t0_ms + m.dur_ms)
            .map(|m| self.goals[m.goal].stiff)
            .fold(own, f32::min);
        Want {
            at,
            squeeze_n: g.map(|g| g.squeeze_n).unwrap_or(0.0),
            stiff: self.style.fixed_stiff.unwrap_or(softest).clamp(0.02, 1.0),
            contact: g.map(|g| g.contact).unwrap_or(Contact::Avoid),
            goal,
        }
    }
}

/// Gains for one joint at full stiffness.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Gain {
    /// effort per unit of position error
    pub kp: f32,
    /// effort per unit of velocity error
    pub kd: f32,
}

/// The joint law: a spring toward the target whose stiffness is `stiff`
/// of full, a damper scaled to match, and a feed-forward term for what
/// the body already knows it must hold up. The result is clipped to
/// `cap`, the effort the tick was allowed.
pub fn impedance(
    gain: Gain,
    stiff: f32,
    target: f32,
    target_rate: f32,
    q: f32,
    dq: f32,
    feed_forward: f32,
    cap: f32,
) -> f32 {
    let s = stiff.clamp(0.0, 1.0);
    let u = s * gain.kp * (target - q) + libm::sqrtf(s) * gain.kd * (target_rate - dq) + feed_forward;
    u.clamp(-cap, cap)
}
