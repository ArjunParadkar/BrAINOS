//! BFS  LEVEL    Domain 2 Autonomic - motor memory (self-model, mandates, primitives, reflexes)  [crate brainos-motor]
//! BFS  PURPOSE  RunMandate: the bounded permission for one run of one primitive (how long, how fast, how hard, where), readable only from a KIRA grant
//! BFS  DEFINES  P_DURATION_MS, P_FORCE_N, P_RATE_DPS, P_TILT_DEG, P_BOX, Workspace, Bounds, Ceiling, MandateRefusal, RunMandate, run_verb
//! BFS  USES     brainos: abi, body, part, rt
//! BFS  USED BY  bodyd/armloop, host/motor_mind, host/motor_seat, host/tests/motor_mandate, motor/lib, motor/run
//!
//! The run mandate: what one grant allows a body to do on its own.
//!
//! A gate evaluation costs about 160 microseconds on a laptop and about
//! ten milliseconds on a microcontroller. A body that moves well decides
//! a thousand times a second. So the gate is asked **once** per run of a
//! primitive, and what it grants is not a motion but a set of bounds:
//! how long, how fast, how hard, and inside what box. The body then
//! decides every tick for itself, and the runner (`run`) refuses any
//! tick outside the bounds.
//!
//! # Where the bounds come from
//!
//! From the arguments of the request the gate granted, and nowhere else.
//! They are ordinary schema parameters (`duration_ms`, `force_n`, ...),
//! so the gate's Validate stage has already checked each one against
//! what the body declared it can survive. `RunMandate::bounded` does
//! not take bounds from its caller: there is no argument through which
//! a caller could widen them.
//!
//! **A bound that was not granted is not defaulted.** A request with no
//! `force_n` does not get "some reasonable force"; it gets no mandate.
//!
//! # What cannot be written
//!
//! A mandate cannot be assembled from its fields:
//!
//! ```compile_fail
//! use brainos_motor::mandate::{Bounds, RunMandate};
//! fn forge(part: brainos_abi::ids::BodyId, verb: brainos_abi::verb::Verb,
//!          grant: brainos_abi::ids::CapabilityId, bounds: Bounds) -> RunMandate {
//!     RunMandate { grant, part, verb, bounds, params: Vec::new() }
//! }
//! ```
//!
//! nor copied, so one grant is one run:
//!
//! ```compile_fail
//! fn twice(m: &brainos_motor::mandate::RunMandate) -> brainos_motor::mandate::RunMandate {
//!     m.clone()
//! }
//! ```
//!
//! and the thing it is made from cannot be assembled either:
//!
//! ```compile_fail
//! use brainos_kira::capability::Granted;
//! fn forge(g: &Granted) -> Granted {
//!     Granted { id: g.id, verb: g.verb, body: g.body, request: g.request,
//!               granted_at: g.granted_at, ttl_ticks: g.ttl_ticks, token: g.token }
//! }
//! ```

use alloc::string::String;
use alloc::vec::Vec;
use brainos_abi::ids::{BodyId, CapabilityId};
use brainos_abi::time::Duration;
use brainos_abi::verb::Verb;
use brainos_body::region::RegionClass;
use brainos_body::schema::{ParamSpec, VerbSpec};
use brainos_part::nerve::Mandate;
use brainos_rt::envelope::Channel;

/// The names of the bounding parameters, the same on every primitive.
pub const P_DURATION_MS: &str = "duration_ms";
pub const P_FORCE_N: &str = "force_n";
pub const P_RATE_DPS: &str = "rate_dps";
pub const P_TILT_DEG: &str = "tilt_deg";
pub const P_BOX: [&str; 6] = ["x_min", "x_max", "y_min", "y_max", "z_min", "z_max"];

/// An axis-aligned box in the body's base frame, metres.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Workspace {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl Workspace {
    pub fn is_sane(&self) -> bool {
        (0..3).all(|i| {
            self.min[i].is_finite() && self.max[i].is_finite() && self.min[i] < self.max[i]
        })
    }

    pub fn contains(&self, p: &[f32; 3]) -> bool {
        (0..3).all(|i| p[i].is_finite() && p[i] >= self.min[i] && p[i] <= self.max[i])
    }
}

/// What one run may not exceed.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Bounds {
    pub duration: Duration,
    /// the hardest the body may push or squeeze, and the largest contact
    /// force it may go on working through, newtons
    pub force_n: f32,
    /// the fastest any revolute joint may be driven, degrees per second
    pub rate_dps: f32,
    /// where the tool may be; `None` when the grant stated no box
    pub workspace: Option<Workspace>,
    /// how far the body may lean; `None` when the grant stated none
    pub tilt_deg: Option<f32>,
}

/// The most a body will ever accept in a mandate: the upper ends of the
/// bounding parameters in its schema.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Ceiling {
    pub max_duration_ms: u32,
    pub max_force_n: f32,
    pub max_rate_dps: f32,
    /// the largest box a grant may name; `None` for a body with no tool
    pub reach: Option<Workspace>,
    /// the largest lean a grant may allow; `None` for a body that
    /// cannot fall
    pub max_tilt_deg: Option<f32>,
}

/// Why a grant did not become a run mandate.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MandateRefusal {
    /// the grant did not state this bound
    Missing(&'static str),
    /// the bound is not a usable number
    Unusable(&'static str),
    /// some of the six box edges were given and some were not
    PartialBox,
}

/// Permission for one run of one primitive on one body.
///
/// There is one constructor, and it consumes a `Mandate`, which exists
/// only because a `Granted` was verified against the entity's key.
#[derive(Debug)]
pub struct RunMandate {
    grant: CapabilityId,
    part: BodyId,
    verb: Verb,
    bounds: Bounds,
    /// the primitive's own parameters, as granted
    params: Vec<(String, f32)>,
}

impl RunMandate {
    /// Read the bounds out of what was granted.
    pub fn bounded(mandate: Mandate) -> Result<RunMandate, MandateRefusal> {
        let need = |name: &'static str| -> Result<f32, MandateRefusal> {
            let v = mandate.scalar(name).ok_or(MandateRefusal::Missing(name))? as f32;
            if v.is_finite() && v > 0.0 {
                Ok(v)
            } else {
                Err(MandateRefusal::Unusable(name))
            }
        };
        let duration_ms = need(P_DURATION_MS)?;
        let force_n = need(P_FORCE_N)?;
        let rate_dps = need(P_RATE_DPS)?;

        let edges: Vec<Option<f64>> = P_BOX.iter().map(|n| mandate.scalar(n)).collect();
        let workspace = if edges.iter().all(|e| e.is_none()) {
            None
        } else if edges.iter().any(|e| e.is_none()) {
            return Err(MandateRefusal::PartialBox);
        } else {
            let e: Vec<f32> = edges.iter().map(|e| e.unwrap_or(0.0) as f32).collect();
            let w = Workspace {
                min: [e[0], e[2], e[4]],
                max: [e[1], e[3], e[5]],
            };
            if !w.is_sane() {
                return Err(MandateRefusal::Unusable("workspace"));
            }
            Some(w)
        };
        let tilt_deg = match mandate.scalar(P_TILT_DEG) {
            None => None,
            Some(t) if t.is_finite() && t > 0.0 => Some(t as f32),
            Some(_) => return Err(MandateRefusal::Unusable(P_TILT_DEG)),
        };

        let bounding = |n: &str| {
            n == P_DURATION_MS
                || n == P_FORCE_N
                || n == P_RATE_DPS
                || n == P_TILT_DEG
                || P_BOX.contains(&n)
        };
        let mut params = Vec::new();
        for name in mandate.arg_names() {
            if bounding(name) {
                continue;
            }
            if let Some(v) = mandate.scalar(name) {
                params.push((String::from(name), v as f32));
            }
        }

        Ok(RunMandate {
            grant: mandate.grant(),
            part: mandate.part(),
            verb: *mandate.verb(),
            bounds: Bounds {
                duration: Duration::from_millis(duration_ms as u64),
                force_n,
                rate_dps,
                workspace,
                tilt_deg,
            },
            params,
        })
    }

    /// Which grant this run is under, for the audit trail.
    pub fn grant(&self) -> CapabilityId {
        self.grant
    }

    pub fn part(&self) -> BodyId {
        self.part
    }

    /// Which primitive was granted.
    pub fn verb(&self) -> &Verb {
        &self.verb
    }

    pub fn bounds(&self) -> &Bounds {
        &self.bounds
    }

    /// One of the primitive's own parameters, as granted.
    pub fn param(&self, name: &str) -> Option<f32> {
        self.params.iter().find(|(n, _)| n == name).map(|(_, v)| *v)
    }
}

/// A primitive's verb, with the bounding parameters every primitive
/// carries. The primitive's own parameters are added with `.with(..)`.
///
/// The ranges come from the body's ceiling, so the gate's Validate stage
/// refuses a request for more than the body says it can survive before
/// any mandate exists.
pub fn run_verb(verb: Verb, summary: &str, ceiling: &Ceiling) -> VerbSpec {
    let mut spec = VerbSpec::new(verb, RegionClass::Effector, summary)
        .with(ParamSpec::count(P_DURATION_MS, 1, ceiling.max_duration_ms as i64))
        .with(ParamSpec::scalar(
            P_FORCE_N,
            Channel::Force,
            0.0,
            ceiling.max_force_n as f64,
        ))
        .with(ParamSpec::scalar(
            P_RATE_DPS,
            Channel::Rate,
            0.0,
            ceiling.max_rate_dps as f64,
        ))
        .taking_up_to(ceiling.max_duration_ms);
    if let Some(reach) = ceiling.reach {
        for (i, name) in P_BOX.iter().enumerate() {
            let axis = i / 2;
            spec = spec.with(ParamSpec::scalar(
                name,
                Channel::Clearance,
                reach.min[axis] as f64,
                reach.max[axis] as f64,
            ));
        }
    }
    if let Some(t) = ceiling.max_tilt_deg {
        spec = spec.with(ParamSpec::scalar(P_TILT_DEG, Channel::Tilt, 0.0, t as f64));
    }
    spec
}
