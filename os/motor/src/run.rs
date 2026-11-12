//! BFS  LEVEL    Domain 2 Autonomic - motor memory (self-model, mandates, primitives, reflexes)  [crate brainos-motor]
//! BFS  PURPOSE  Runner: holds the one RunMandate a body is working under and checks every tick against it (time, angle, rate, force, workspace, tilt), ending the run on the first breach
//! BFS  DEFINES  MAX_JOINTS, MAX_STEP, Kinematics, Proposal, Felt, Breach, Ending, Summary, Runner
//! BFS  USES     this crate: mandate, model | brainos: abi
//! BFS  USED BY  bodyd/armloop, host/motor_seat, host/tests/motor_mandate, motor/lib
//!
//! The runner: every tick is checked, and the first one outside the
//! mandate ends the run.
//!
//! This is the body's side of the bargain in `mandate`. The gate is not
//! consulted during a run, so this check is the only thing between a
//! policy and the actuators, and it is written to be boring: no
//! allocation, no model, no judgement. A tick either is inside every
//! bound or is not applied.
//!
//! # What is checked, each tick
//!
//! 1. there is a mandate, and it is for this body
//! 2. it has not run out
//! 3. the numbers are numbers
//! 4. every joint target is inside that joint's own limits
//! 5. no joint is asked to move faster than the body or the grant allows
//! 6. no actuator is asked for more than the body or the grant allows
//! 7. the force the body *feels* is inside the grant
//! 8. the tool, placed by the body's own kinematics and not by the
//!    policy's word, is inside the granted box
//! 9. the body is not leaning further than the grant allows
//!
//! A breach is not clamped into range. Clamping would turn "the policy
//! asked for something it was not allowed" into "the policy got nearly
//! what it asked for", which hides exactly the event worth knowing.
//!
//! # A runner cannot start without a mandate
//!
//! `begin` takes the mandate by value. There is no other way in:
//!
//! ```compile_fail
//! use brainos_motor::run::{Kinematics, Runner};
//! fn start(r: &mut Runner, body: &impl Kinematics, q: &[f32]) {
//!     // no mandate to give
//!     let _ = r.begin(body, q, brainos_abi::time::Instant(0));
//! }
//! ```
//!
//! # A runner holds no memory
//!
//! Nothing here owns a state graph or a handle to one, and this crate
//! does not depend on `brainos-memory` outside its tests. What a run
//! learned goes up as a `Summary`; the entity decides what becomes
//! memory.
//!
//! ```compile_fail
//! // a body writing what it did straight into the graph
//! use brainos_memory::graph::{NodeKind, StateGraph};
//! use brainos_memory::provenance::Provenance;
//! fn remember(g: &mut StateGraph, s: &brainos_motor::run::Summary) {
//!     g.add(NodeKind::Episode, "ran", 50, Provenance::own(brainos_abi::time::Tick(s.applied)));
//! }
//! ```
//!
//! ```compile_fail
//! // a body making itself a handle from the entity's public id
//! fn mint(id: brainos_abi::ids::EntityId) -> brainos_memory::law::MemoryHandle {
//!     brainos_memory::law::MemoryHandle::issue(id)
//! }
//! ```

use crate::mandate::{Bounds, RunMandate};
use crate::model::JointKind;
use brainos_abi::ids::{BodyId, CapabilityId};
use brainos_abi::time::{Duration, Instant};
use brainos_abi::verb::Verb;

/// The most joints a runner will track.
pub const MAX_JOINTS: usize = 32;

/// A rate bound is applied over at most this long a gap. A body that
/// was not ticked for a second has not earned a second's worth of
/// travel in one step.
pub const MAX_STEP: Duration = Duration::from_millis(10);

/// What the runner needs to know about the body it is on. The body's
/// own numbers, never the policy's.
pub trait Kinematics {
    fn id(&self) -> BodyId;
    fn joints(&self) -> usize;
    fn kind(&self, joint: usize) -> JointKind;
    /// degrees for a hinge, metres for a slide
    fn limits(&self, joint: usize) -> (f32, f32);
    /// deg/s for a hinge, m/s for a slide
    fn max_rate(&self, joint: usize) -> f32;
    /// N·m for a hinge, N for a slide
    fn max_effort(&self, joint: usize) -> f32;
    /// Where this pose puts the tool, in the base frame. `None` for a
    /// body with no tool.
    fn tool(&self, q: &[f32]) -> Option<[f32; 3]>;
}

/// What a policy wants for one tick.
#[derive(Clone, Copy, Debug)]
pub struct Proposal<'a> {
    /// a target for every joint
    pub target: &'a [f32],
    /// the largest effort each joint's controller may use this tick
    pub effort: &'a [f32],
}

/// What the body feels this tick.
#[derive(Clone, Copy, Debug, Default)]
pub struct Felt {
    /// the largest contact force, newtons; `None` if this body cannot
    /// feel force
    pub force_n: Option<f32>,
    pub tilt_deg: Option<f32>,
}

/// Why a tick was refused. Each ends the run.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Breach {
    NoMandate,
    WrongBody,
    /// the grant states a box and this body cannot say where its tool
    /// is, or the body has a tool and the grant states no box
    NoWorkspace,
    /// the grant bounds lean and the body cannot feel lean, or the
    /// reverse
    NoTilt,
    Expired,
    Malformed,
    Angle { joint: u8 },
    Rate { joint: u8 },
    Effort { joint: u8 },
    FeltForce,
    Workspace,
    Tilt,
}

impl Breach {
    pub const fn name(self) -> &'static str {
        match self {
            Breach::NoMandate => "no-mandate",
            Breach::WrongBody => "wrong-body",
            Breach::NoWorkspace => "no-workspace",
            Breach::NoTilt => "no-tilt",
            Breach::Expired => "expired",
            Breach::Malformed => "malformed",
            Breach::Angle { .. } => "angle",
            Breach::Rate { .. } => "rate",
            Breach::Effort { .. } => "effort",
            Breach::FeltForce => "felt-force",
            Breach::Workspace => "workspace",
            Breach::Tilt => "tilt",
        }
    }
}

/// How a run ended.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ending {
    /// the body said it was done
    Finished,
    /// a tick was refused
    Breached(Breach),
}

/// What a run reports upward when it ends. Plain values.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Summary {
    pub grant: CapabilityId,
    pub verb: Verb,
    pub applied: u64,
    pub ending: Ending,
    pub lasted: Duration,
    pub peak_felt_n: f32,
}

struct Live {
    mandate: RunMandate,
    began: Instant,
    last_at: Instant,
    last: [f32; MAX_JOINTS],
    applied: u64,
    peak_felt_n: f32,
}

/// The one run a body is in, if it is in one.
pub struct Runner {
    live: Option<Live>,
    /// ticks refused since this runner was made, across runs
    pub refused: u64,
}

impl Default for Runner {
    fn default() -> Runner {
        Runner::idle()
    }
}

impl Runner {
    pub const fn idle() -> Runner {
        Runner {
            live: None,
            refused: 0,
        }
    }

    pub fn is_running(&self) -> bool {
        self.live.is_some()
    }

    pub fn bounds(&self) -> Option<&Bounds> {
        self.live.as_ref().map(|l| l.mandate.bounds())
    }

    /// The mandate being run under, to read the granted parameters.
    pub fn mandate(&self) -> Option<&RunMandate> {
        self.live.as_ref().map(|l| &l.mandate)
    }

    pub fn applied(&self) -> u64 {
        self.live.as_ref().map(|l| l.applied).unwrap_or(0)
    }

    /// How much of the granted time is left.
    pub fn remaining(&self, now: Instant) -> Duration {
        match &self.live {
            Some(l) => l
                .mandate
                .bounds()
                .duration
                .saturating_sub(now.since(l.began)),
            None => Duration::from_millis(0),
        }
    }

    /// Start a run. The mandate is consumed; a run already in progress
    /// is ended first and its summary returned alongside.
    pub fn begin<K: Kinematics>(
        &mut self,
        mandate: RunMandate,
        body: &K,
        q_now: &[f32],
        now: Instant,
    ) -> Result<Option<Summary>, Breach> {
        let n = body.joints();
        if n == 0 || n > MAX_JOINTS || q_now.len() != n || q_now.iter().any(|v| !v.is_finite()) {
            self.refused += 1;
            return Err(Breach::Malformed);
        }
        if mandate.part() != body.id() {
            self.refused += 1;
            return Err(Breach::WrongBody);
        }
        // a box and a tool go together: a grant that bounds where the
        // tool may be is meaningless on a body that cannot say where its
        // tool is, and a body with a tool does not run unboxed
        if mandate.bounds().workspace.is_some() != body.tool(q_now).is_some() {
            self.refused += 1;
            return Err(Breach::NoWorkspace);
        }
        let previous = self.finish(now);
        let mut last = [0.0f32; MAX_JOINTS];
        last[..n].copy_from_slice(q_now);
        self.live = Some(Live {
            mandate,
            began: now,
            last_at: now,
            last,
            applied: 0,
            peak_felt_n: 0.0,
        });
        Ok(previous)
    }

    /// The body is done. Ends the run without a breach.
    pub fn finish(&mut self, now: Instant) -> Option<Summary> {
        self.live.take().map(|l| Summary {
            grant: l.mandate.grant(),
            verb: *l.mandate.verb(),
            applied: l.applied,
            ending: Ending::Finished,
            lasted: now.since(l.began),
            peak_felt_n: l.peak_felt_n,
        })
    }

    fn check<K: Kinematics>(
        live: &Live,
        body: &K,
        now: Instant,
        p: &Proposal,
        felt: &Felt,
    ) -> Result<(), Breach> {
        let b = live.mandate.bounds();
        if now.since(live.began) > b.duration {
            return Err(Breach::Expired);
        }
        let n = body.joints();
        if p.target.len() != n
            || p.effort.len() != n
            || p.target.iter().chain(p.effort.iter()).any(|v| !v.is_finite())
        {
            return Err(Breach::Malformed);
        }
        let dt = now.since(live.last_at).min(MAX_STEP).as_micros() as f32 * 1e-6;
        for i in 0..n {
            let (lo, hi) = body.limits(i);
            let t = p.target[i];
            if t < lo || t > hi {
                return Err(Breach::Angle { joint: i as u8 });
            }
            let rate = match body.kind(i) {
                JointKind::Hinge => body.max_rate(i).min(b.rate_dps),
                JointKind::Slide => body.max_rate(i),
            };
            // a small allowance for arithmetic, not for intent
            if libm::fabsf(t - live.last[i]) > rate * dt * 1.001 + 1e-6 {
                return Err(Breach::Rate { joint: i as u8 });
            }
            let effort = match body.kind(i) {
                JointKind::Hinge => body.max_effort(i),
                JointKind::Slide => body.max_effort(i).min(b.force_n),
            };
            if p.effort[i] < 0.0 || p.effort[i] > effort {
                return Err(Breach::Effort { joint: i as u8 });
            }
        }
        match felt.force_n {
            Some(f) if !f.is_finite() => return Err(Breach::Malformed),
            Some(f) if f > b.force_n => return Err(Breach::FeltForce),
            _ => {}
        }
        if let Some(w) = &b.workspace {
            match body.tool(p.target) {
                Some(at) if w.contains(&at) => {}
                Some(_) => return Err(Breach::Workspace),
                None => return Err(Breach::NoWorkspace),
            }
        }
        if let Some(limit) = b.tilt_deg {
            match felt.tilt_deg {
                Some(t) if t.is_finite() && libm::fabsf(t) <= limit => {}
                Some(t) if t.is_finite() => return Err(Breach::Tilt),
                Some(_) => return Err(Breach::Malformed),
                None => return Err(Breach::NoTilt),
            }
        }
        Ok(())
    }

    /// Check one tick. `Ok` means the proposal may be applied exactly as
    /// given. `Err` means nothing from it may be applied, the run is
    /// over, and the summary says why.
    pub fn admit<K: Kinematics>(
        &mut self,
        body: &K,
        now: Instant,
        proposal: &Proposal,
        felt: &Felt,
    ) -> Result<(), (Breach, Option<Summary>)> {
        let Some(live) = self.live.as_mut() else {
            self.refused += 1;
            return Err((Breach::NoMandate, None));
        };
        match Runner::check(live, body, now, proposal, felt) {
            Ok(()) => {
                let n = body.joints();
                live.last[..n].copy_from_slice(proposal.target);
                live.last_at = now;
                live.applied += 1;
                if let Some(f) = felt.force_n {
                    live.peak_felt_n = live.peak_felt_n.max(f);
                }
                Ok(())
            }
            Err(breach) => {
                self.refused += 1;
                let l = self.live.take();
                let summary = l.map(|l| Summary {
                    grant: l.mandate.grant(),
                    verb: *l.mandate.verb(),
                    applied: l.applied,
                    ending: Ending::Breached(breach),
                    lasted: now.since(l.began),
                    peak_felt_n: l.peak_felt_n,
                });
                Err((breach, summary))
            }
        }
    }
}
