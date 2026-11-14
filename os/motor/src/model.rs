//! BFS  LEVEL    Domain 2 Autonomic - motor memory (self-model, mandates, primitives, reflexes)  [crate brainos-motor]
//! BFS  PURPOSE  The body's model of itself as a tree of links and joints with limits, where every number says whether its source stated it or it is an assumption
//! BFS  DEFINES  Num, JointKind, JointSpec, Inertial, Shape, Link, Base, Site, ModelError, BodyModel
//! BFS  USES     nothing outside core/alloc
//! BFS  USED BY  bodyd/armloop, bodyd/selfmodel, host/motor, host/tests/motor_mandate, motor/lib, motor/mjcf, motor/run, motor/tests/selfmodel, +1 more
//!
//! The self-model: what a body is, as numbers a simulator can be built
//! from.
//!
//! A `CapabilitySchema` says what a body can be *asked*; it carries no
//! geometry. Geometry lives with each mechanism (`LegGeom`, `ArmConfig`,
//! the BLUR descriptor). This type is the one shape all of them are
//! turned into, so that one emitter (`mjcf`) serves every body.
//!
//! **Every physical number is `Stated` or `Assumed`.** The Mini-Blur
//! geometry states link lengths and joint limits and no mass at all. A
//! simulator needs a mass. The generator does not quietly make one up:
//! it writes `Assumed`, the emitter writes that into the model as a
//! comment and into `assumptions()`, and a result obtained in that model
//! is a result about a body with those assumed numbers.
//!
//! Units: metres, kilograms, degrees, newton-metres, degrees per second.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// A number and where it came from.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Num {
    /// the body's own description states it
    Stated(f32),
    /// nothing states it; this is what the generator used instead
    Assumed(f32),
}

impl Num {
    pub fn value(self) -> f32 {
        match self {
            Num::Stated(v) | Num::Assumed(v) => v,
        }
    }

    pub fn is_assumed(self) -> bool {
        matches!(self, Num::Assumed(_))
    }
}

/// How a joint moves.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum JointKind {
    /// rotation about `axis`; limits in degrees
    Hinge,
    /// translation along `axis`; limits in metres
    Slide,
}

/// One joint, between a link and its parent.
#[derive(Clone, PartialEq, Debug)]
pub struct JointSpec {
    pub name: String,
    pub kind: JointKind,
    /// in the link's own frame
    pub axis: [f32; 3],
    /// where the axis passes through, in the link's own frame
    pub anchor: [f32; 3],
    pub min: f32,
    pub max: f32,
    /// what the actuator can exert: N·m for a hinge, N for a slide
    pub effort: Num,
    /// how fast it can go: deg/s for a hinge, m/s for a slide
    pub rate: Num,
    /// `false` for a joint nothing drives (a spring, a linked follower)
    pub actuated: bool,
    /// this joint follows another: `(index of the leader's link, ratio)`
    pub follows: Option<(usize, f32)>,
    /// the follower is pushed by the leader's actuator as hard as the
    /// leader is (two fingers closed by one mechanism), rather than
    /// dragged along by the leader (two legs on one servo horn)
    pub shares_drive: bool,
    /// passive stiffness about `spring_ref`, N·m/rad; 0 for none
    pub stiffness: f32,
    pub spring_ref: f32,
}

impl JointSpec {
    pub fn hinge(name: &str, axis: [f32; 3], min_deg: f32, max_deg: f32) -> JointSpec {
        JointSpec {
            name: name.to_string(),
            kind: JointKind::Hinge,
            axis,
            anchor: [0.0; 3],
            min: min_deg,
            max: max_deg,
            effort: Num::Assumed(0.0),
            rate: Num::Assumed(0.0),
            actuated: true,
            follows: None,
            shares_drive: false,
            stiffness: 0.0,
            spring_ref: 0.0,
        }
    }

    pub fn slide(name: &str, axis: [f32; 3], min_m: f32, max_m: f32) -> JointSpec {
        JointSpec {
            kind: JointKind::Slide,
            ..JointSpec::hinge(name, axis, min_m, max_m)
        }
    }

    pub fn anchored(mut self, at: [f32; 3]) -> JointSpec {
        self.anchor = at;
        self
    }

    pub fn effort(mut self, e: Num) -> JointSpec {
        self.effort = e;
        self
    }

    pub fn rate(mut self, r: Num) -> JointSpec {
        self.rate = r;
        self
    }

    pub fn following(mut self, leader_link: usize, ratio: f32) -> JointSpec {
        self.follows = Some((leader_link, ratio));
        self.actuated = false;
        self
    }

    /// Follow a leader and be driven with it: the leader's actuator
    /// pushes both joints equally.
    pub fn mirroring(mut self, leader_link: usize, ratio: f32) -> JointSpec {
        self.follows = Some((leader_link, ratio));
        self.actuated = false;
        self.shares_drive = true;
        self
    }

    pub fn passive(mut self, stiffness: f32, spring_ref: f32) -> JointSpec {
        self.actuated = false;
        self.stiffness = stiffness;
        self.spring_ref = spring_ref;
        self
    }

    pub fn admits(&self, v: f32) -> bool {
        v.is_finite() && v >= self.min && v <= self.max
    }
}

/// Mass properties given outright rather than derived from a shape.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Inertial {
    pub com: [f32; 3],
    /// orientation of the principal axes, w x y z
    pub quat: [f32; 4],
    pub diag: [f32; 3],
}

/// The solid a link is drawn and weighed as.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Shape {
    /// a rod from the link origin to `tip`, with this radius
    Rod { radius: f32 },
    /// a box centred on the link origin, half-sizes
    Block { half: [f32; 3] },
    /// a flat bar from the link origin along +x to `tip`: `half_t` thick
    /// across y, `half_w` wide across z. A finger: two of these grip
    /// with faces, where two rods would grip along a line and let what
    /// they hold swing
    Slab { half_t: f32, half_w: f32 },
    /// no solid: mass properties come from `Link::inertial`
    None,
}

/// One rigid piece.
#[derive(Clone, PartialEq, Debug)]
pub struct Link {
    pub name: String,
    /// index of the parent link; `None` for the base
    pub parent: Option<usize>,
    /// where this link's frame sits in its parent's frame
    pub origin: [f32; 3],
    /// and how it is turned there, w x y z
    pub quat: [f32; 4],
    /// the joint to the parent; `None` for the base or a welded piece
    pub joint: Option<JointSpec>,
    /// the far end of the link in its own frame
    pub tip: [f32; 3],
    pub shape: Shape,
    pub mass: Num,
    pub inertial: Option<Inertial>,
    /// does the far end touch the world (a foot, a fingertip)?
    pub touches: bool,
}

impl Link {
    pub fn new(name: &str, parent: Option<usize>) -> Link {
        Link {
            name: name.to_string(),
            parent,
            origin: [0.0; 3],
            quat: [1.0, 0.0, 0.0, 0.0],
            joint: None,
            tip: [0.0; 3],
            shape: Shape::None,
            mass: Num::Assumed(0.0),
            inertial: None,
            touches: false,
        }
    }

    pub fn at(mut self, origin: [f32; 3]) -> Link {
        self.origin = origin;
        self
    }

    /// Turn the link about its parent's z axis.
    pub fn yawed(mut self, deg: f32) -> Link {
        let h = deg * core::f32::consts::PI / 360.0;
        self.quat = [libm::cosf(h), 0.0, 0.0, libm::sinf(h)];
        self
    }

    pub fn turned(mut self, quat: [f32; 4]) -> Link {
        self.quat = quat;
        self
    }

    pub fn jointed(mut self, j: JointSpec) -> Link {
        self.joint = Some(j);
        self
    }

    pub fn rod(mut self, tip: [f32; 3], radius: f32) -> Link {
        self.tip = tip;
        self.shape = Shape::Rod { radius };
        self
    }

    pub fn slab(mut self, length: f32, half_t: f32, half_w: f32) -> Link {
        self.tip = [length, 0.0, 0.0];
        self.shape = Shape::Slab { half_t, half_w };
        self
    }

    pub fn block(mut self, half: [f32; 3]) -> Link {
        self.shape = Shape::Block { half };
        self
    }

    pub fn weighing(mut self, m: Num) -> Link {
        self.mass = m;
        self
    }

    pub fn with_inertial(mut self, i: Inertial) -> Link {
        self.inertial = Some(i);
        self
    }

    pub fn touching(mut self) -> Link {
        self.touches = true;
        self
    }
}

/// Is the base bolted down or free?
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Base {
    Fixed,
    /// free, starting this high above the ground
    Floating { height_m: f32 },
}

/// A named point on a link: a foot, a tool tip.
#[derive(Clone, PartialEq, Debug)]
pub struct Site {
    pub name: String,
    pub link: usize,
    pub pos: [f32; 3],
}

/// Why a model was refused.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ModelError {
    Empty,
    /// the first link must be the base and nothing else may be
    BadRoot,
    /// a parent index that is not an earlier link
    BadParent,
    /// a limit that is not finite or not ordered
    BadLimit,
    /// a joint follows something that has no joint
    BadFollow,
    /// two links, two joints or two sites with one name
    DuplicateName,
    /// a number that is not finite, or a mass below zero
    BadNumber,
}

/// A whole body.
#[derive(Clone, PartialEq, Debug)]
pub struct BodyModel {
    pub name: String,
    pub base: Base,
    pub links: Vec<Link>,
    pub sites: Vec<Site>,
    /// how far the base may lean before the body counts as fallen
    pub tilt_limit_deg: Option<f32>,
    /// free-text notes about what this model leaves out
    pub omits: Vec<String>,
}

impl BodyModel {
    pub fn new(name: &str, base: Base) -> BodyModel {
        BodyModel {
            name: name.to_string(),
            base,
            links: Vec::new(),
            sites: Vec::new(),
            tilt_limit_deg: None,
            omits: Vec::new(),
        }
    }

    /// Add a link; returns its index.
    pub fn push(&mut self, link: Link) -> usize {
        self.links.push(link);
        self.links.len() - 1
    }

    pub fn site(&mut self, name: &str, link: usize, pos: [f32; 3]) {
        self.sites.push(Site {
            name: name.to_string(),
            link,
            pos,
        });
    }

    pub fn omit(&mut self, what: &str) {
        self.omits.push(what.to_string());
    }

    pub fn joints(&self) -> impl Iterator<Item = (usize, &JointSpec)> {
        self.links
            .iter()
            .enumerate()
            .filter_map(|(i, l)| l.joint.as_ref().map(|j| (i, j)))
    }

    /// The joints something drives, in link order. This order is the
    /// order of a policy's action vector.
    pub fn actuated(&self) -> impl Iterator<Item = (usize, &JointSpec)> {
        self.joints().filter(|(_, j)| j.actuated)
    }

    pub fn total_mass(&self) -> f32 {
        self.links.iter().map(|l| l.mass.value()).sum()
    }

    pub fn children(&self, of: usize) -> impl Iterator<Item = usize> + '_ {
        self.links
            .iter()
            .enumerate()
            .filter(move |(_, l)| l.parent == Some(of))
            .map(|(i, _)| i)
    }

    /// Every number this model had to assume, as sentences.
    pub fn assumptions(&self) -> Vec<String> {
        let mut out = Vec::new();
        for l in &self.links {
            if l.mass.is_assumed() {
                out.push(alloc::format!(
                    "mass of {} assumed {:.4} kg",
                    l.name,
                    l.mass.value()
                ));
            }
            if let Some(j) = &l.joint {
                if j.actuated && j.effort.is_assumed() {
                    out.push(alloc::format!(
                        "effort of {} assumed {:.4}",
                        j.name,
                        j.effort.value()
                    ));
                }
                if j.actuated && j.rate.is_assumed() {
                    out.push(alloc::format!(
                        "rate of {} assumed {:.2}",
                        j.name,
                        j.rate.value()
                    ));
                }
            }
        }
        out
    }

    /// Is this a model an emitter can be trusted with?
    pub fn validate(&self) -> Result<(), ModelError> {
        if self.links.is_empty() {
            return Err(ModelError::Empty);
        }
        if self.links[0].parent.is_some() || self.links[0].joint.is_some() {
            return Err(ModelError::BadRoot);
        }
        let finite3 = |v: &[f32; 3]| v.iter().all(|x| x.is_finite());
        for (i, l) in self.links.iter().enumerate() {
            match l.parent {
                None if i != 0 => return Err(ModelError::BadRoot),
                Some(p) if p >= i => return Err(ModelError::BadParent),
                _ => {}
            }
            if !finite3(&l.origin)
                || !finite3(&l.tip)
                || !l.quat.iter().all(|x| x.is_finite())
                || !l.mass.value().is_finite()
                || l.mass.value() < 0.0
            {
                return Err(ModelError::BadNumber);
            }
            if let Some(j) = &l.joint {
                if !(j.min.is_finite() && j.max.is_finite() && j.min < j.max) {
                    return Err(ModelError::BadLimit);
                }
                if !finite3(&j.axis) || j.axis == [0.0; 3] {
                    return Err(ModelError::BadNumber);
                }
                if let Some((leader, ratio)) = j.follows {
                    let ok = self
                        .links
                        .get(leader)
                        .map(|x| x.joint.is_some())
                        .unwrap_or(false);
                    if !ok || leader == i || !ratio.is_finite() {
                        return Err(ModelError::BadFollow);
                    }
                }
            }
        }
        // names are unique within a kind, as in the simulator: a link
        // and its joint may share one
        let dup = |mut names: Vec<&str>| {
            names.sort_unstable();
            names.windows(2).any(|w| w[0] == w[1])
        };
        if dup(self.links.iter().map(|l| l.name.as_str()).collect())
            || dup(self.joints().map(|(_, j)| j.name.as_str()).collect())
            || dup(self.sites.iter().map(|s| s.name.as_str()).collect())
        {
            return Err(ModelError::DuplicateName);
        }
        if self.sites.iter().any(|s| s.link >= self.links.len()) {
            return Err(ModelError::BadParent);
        }
        Ok(())
    }
}
