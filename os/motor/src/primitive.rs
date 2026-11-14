//! BFS  LEVEL    Domain 2 Autonomic - motor memory (self-model, mandates, primitives, reflexes)  [crate brainos-motor]
//! BFS  PURPOSE  MotorPrimitive: one learned piece of movement (grasp, place, push, wipe, turn, pour) as strokes in an object's own frame, with what it needs, what it achieves, its bounds and its success tests
//! BFS  DEFINES  Kind, Contact, Until, Stroke, MAX_STROKES, Program, Param, SuccessTest, Policy, Envelope, MotorPrimitive, Given, +5 more
//! BFS  USES     this crate: condition, control | brainos: abi
//! BFS  USED BY  bodyd/armloop, entity/entity/motor, entity/tests/motor_memory, host/motor, host/motor_mind, host/motor_seat, motor/control, motor/recipe, +3 more
//!
//! Motor primitives: muscle memory.
//!
//! A primitive is a short movement that means something: *grasp*,
//! *place*, *wipe*. It is described **relative to the thing it acts
//! on**, in that thing's own frame (the mouth of a mug, a handle, a
//! knob), which is why one demonstration on one mug is enough for a mug
//! of another size somewhere else: the strokes do not change, only the
//! frame and a few lengths do.
//!
//! A primitive is five things:
//!
//! * a **policy**: here, a short program of strokes generated from its
//!   parameters (`program`). A learned network fits the same slot
//!   (`Policy::Int8`); none of the arm primitives uses one yet.
//! * **parameters** relative to an affordance frame
//! * what it **needs** and what it **achieves** (`condition::Effect`)
//! * an **envelope**: the bounds a mandate for it should ask for
//! * **success tests**: what to check afterwards
//!
//! It is stored in the state graph by the entity as a compiled skill,
//! per body class, and handed to the body for the length of one run.
//! Nothing in this module moves anything: a `Program` is a plan the body
//! runs under a `RunMandate`, through the `Runner`.

use crate::condition::{Effect, Fact, Pred};
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use brainos_abi::codec::{Decode, Decoder, Encode, Encoder};
use brainos_abi::error::{Error, Result};
use brainos_abi::verb::Verb;

/// The six kinds in the library, and a plain move between them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Kind {
    Grasp = 0,
    Place = 1,
    Push = 2,
    Wipe = 3,
    Turn = 4,
    Pour = 5,
}

impl Kind {
    pub const ALL: [Kind; 6] = [
        Kind::Grasp,
        Kind::Place,
        Kind::Push,
        Kind::Wipe,
        Kind::Turn,
        Kind::Pour,
    ];

    pub fn from_u8(v: u8) -> Option<Kind> {
        Kind::ALL.get(v as usize).copied()
    }

    pub const fn name(self) -> &'static str {
        match self {
            Kind::Grasp => "grasp",
            Kind::Place => "place",
            Kind::Push => "push",
            Kind::Wipe => "wipe",
            Kind::Turn => "turn",
            Kind::Pour => "pour",
        }
    }

    pub fn from_name(s: &str) -> Option<Kind> {
        Kind::ALL.iter().copied().find(|k| k.name() == s)
    }

    /// Does it matter which way the fingers face? Not for going round
    /// the inside of a mouth, and not for setting down what is already
    /// in the hand: there the heading the hand already has is kept.
    pub const fn heading_matters(self) -> bool {
        !matches!(self, Kind::Wipe | Kind::Place)
    }

    /// The name of the affordance frame this kind is described in.
    pub const fn frame(self) -> &'static str {
        match self {
            Kind::Grasp => "grip",
            Kind::Place => "rest",
            Kind::Push => "side",
            Kind::Wipe => "mouth",
            Kind::Turn => "knob",
            Kind::Pour => "mouth",
        }
    }
}

/// Is touching something the point of a stroke, or a surprise?
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Contact {
    /// touching anything here is unexpected, and the compliance reflex
    /// answers it
    Avoid,
    /// this stroke is meant to touch (wiping, setting down, pushing)
    Expect,
}

/// When a stroke is over.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Until {
    /// when its time is up
    Time,
    /// when the tool feels at least this much force, or its time is up
    Touch(f32),
    /// when the fingers hold with at least this much force; if the time
    /// runs out first the stroke has failed
    Gripped(f32),
}

/// One stroke: go there, like this.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Stroke {
    /// where the grasp centre goes, metres, in the affordance frame
    pub to: [f32; 3],
    /// heading of the line between the fingers, relative to the frame
    pub yaw_deg: f32,
    /// pitch of the tool axis; -90 points straight down
    pub pitch_deg: f32,
    /// extra wrist roll on top of what the heading needs
    pub twist_deg: f32,
    /// how far each finger sits from the centre line
    pub open_m: f32,
    /// the most the fingers may squeeze during this stroke, newtons
    pub squeeze_n: f32,
    /// how long the stroke takes when nothing interrupts it
    pub ms: u32,
    /// stiffness, as a fraction of the body's full stiffness
    pub stiff: f32,
    pub contact: Contact,
    pub until: Until,
    /// must the body come to rest here before the next stroke starts?
    /// `false` lets the next stroke blend in before this one ends
    pub rest: bool,
}

impl Stroke {
    pub fn to(at: [f32; 3], ms: u32) -> Stroke {
        Stroke {
            to: at,
            yaw_deg: 0.0,
            pitch_deg: -90.0,
            twist_deg: 0.0,
            open_m: 0.0,
            squeeze_n: 0.0,
            ms,
            stiff: 1.0,
            contact: Contact::Avoid,
            until: Until::Time,
            rest: false,
        }
    }

    pub fn open(mut self, m: f32) -> Stroke {
        self.open_m = m;
        self
    }

    pub fn squeeze(mut self, n: f32) -> Stroke {
        self.squeeze_n = n;
        self
    }

    pub fn soft(mut self, stiff: f32) -> Stroke {
        self.stiff = stiff;
        self
    }

    pub fn touching(mut self) -> Stroke {
        self.contact = Contact::Expect;
        self
    }

    pub fn until(mut self, u: Until) -> Stroke {
        self.until = u;
        self.rest = true;
        self
    }

    pub fn resting(mut self) -> Stroke {
        self.rest = true;
        self
    }

    pub fn pitched(mut self, deg: f32) -> Stroke {
        self.pitch_deg = deg;
        self
    }

    pub fn twisted(mut self, deg: f32) -> Stroke {
        self.twist_deg = deg;
        self
    }

    pub fn heading(mut self, deg: f32) -> Stroke {
        self.yaw_deg = deg;
        self
    }
}

/// The most strokes one program may have.
pub const MAX_STROKES: usize = 128;

pub type Program = Vec<Stroke>;

/// One parameter of a primitive, with the range it was built for.
#[derive(Clone, PartialEq, Debug)]
pub struct Param {
    pub name: String,
    pub min: f32,
    pub max: f32,
    pub default: f32,
}

impl Param {
    pub fn new(name: &str, min: f32, max: f32, default: f32) -> Param {
        Param {
            name: name.to_string(),
            min,
            max,
            default,
        }
    }
}

/// What to check after a run.
#[derive(Clone, Copy, PartialEq, Debug)]
#[repr(u8)]
pub enum SuccessTest {
    /// the fingers still feel the object
    Held = 0,
    /// the hand is empty
    Released = 1,
    /// the object is within this many metres of where it was meant to go
    Within(f32) = 2,
    /// at least this fraction of the target surface was touched
    Covered(f32) = 3,
    /// the thing turned to within this many degrees of the target
    Angle(f32) = 4,
    /// at least this fraction arrived
    Transferred(f32) = 5,
    /// the object is still upright
    Upright = 6,
}

impl Encode for SuccessTest {
    fn encode(&self, e: &mut Encoder) {
        let (tag, v) = match *self {
            SuccessTest::Held => (0u8, 0.0),
            SuccessTest::Released => (1, 0.0),
            SuccessTest::Within(v) => (2, v),
            SuccessTest::Covered(v) => (3, v),
            SuccessTest::Angle(v) => (4, v),
            SuccessTest::Transferred(v) => (5, v),
            SuccessTest::Upright => (6, 0.0),
        };
        e.u8(tag);
        e.f32(v);
    }
}

impl Decode for SuccessTest {
    fn decode(d: &mut Decoder) -> Result<SuccessTest> {
        let (tag, v) = (d.u8()?, d.f32()?);
        Ok(match tag {
            0 => SuccessTest::Held,
            1 => SuccessTest::Released,
            2 => SuccessTest::Within(v),
            3 => SuccessTest::Covered(v),
            4 => SuccessTest::Angle(v),
            5 => SuccessTest::Transferred(v),
            6 => SuccessTest::Upright,
            _ => return Err(Error::Malformed("motor success test")),
        })
    }
}

/// How the movement itself is produced.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Policy {
    /// strokes generated from the parameters by `program`
    Strokes,
    /// an int8 network, named by the hash of its weights
    Int8([u8; 32]),
}

/// The bounds a run of this primitive should be granted.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Envelope {
    pub duration_ms: u32,
    pub force_n: f32,
    pub rate_dps: f32,
    /// how far outside the strokes' own extent the granted box reaches
    pub margin_m: f32,
}

/// One piece of muscle memory.
#[derive(Clone, PartialEq, Debug)]
pub struct MotorPrimitive {
    pub verb: Verb,
    pub kind: Kind,
    /// which body class this was compiled for
    pub body_class: String,
    pub policy: Policy,
    pub params: Vec<Param>,
    /// roles: "it" is the object acted on, "to" where it goes
    pub effect: Effect,
    pub envelope: Envelope,
    pub tests: Vec<SuccessTest>,
}

impl MotorPrimitive {
    pub fn param(&self, name: &str) -> Option<&Param> {
        self.params.iter().find(|p| p.name == name)
    }

    /// The effect with its roles bound to this task's names.
    pub fn bound(&self, it: &str, to: &str) -> Effect {
        let bind = |f: &Fact| {
            let about = match f.about.as_str() {
                "it" => it,
                "to" => to,
                other => other,
            };
            Fact::new(f.pred, about)
        };
        Effect {
            needs: self.effect.needs.iter().map(bind).collect(),
            adds: self.effect.adds.iter().map(bind).collect(),
            removes: self.effect.removes.iter().map(bind).collect(),
        }
    }
}

impl Encode for MotorPrimitive {
    fn encode(&self, e: &mut Encoder) {
        e.str(self.verb.as_str());
        e.u8(self.kind as u8);
        e.str(&self.body_class);
        match &self.policy {
            Policy::Strokes => e.u8(0),
            Policy::Int8(h) => {
                e.u8(1);
                e.raw(h);
            }
        }
        e.seq(self.params.len());
        for p in &self.params {
            e.str(&p.name);
            e.f32(p.min);
            e.f32(p.max);
            e.f32(p.default);
        }
        e.item(&self.effect);
        e.u32(self.envelope.duration_ms);
        e.f32(self.envelope.force_n);
        e.f32(self.envelope.rate_dps);
        e.f32(self.envelope.margin_m);
        e.seq(self.tests.len());
        for t in &self.tests {
            e.item(t);
        }
    }
}

impl Decode for MotorPrimitive {
    fn decode(d: &mut Decoder) -> Result<MotorPrimitive> {
        let verb = Verb::parse(d.str()?)?;
        let kind = Kind::from_u8(d.u8()?).ok_or(Error::Malformed("motor primitive kind"))?;
        let body_class = d.str()?.to_string();
        let policy = match d.u8()? {
            0 => Policy::Strokes,
            1 => {
                let mut h = [0u8; 32];
                d.read_exact(&mut h)?;
                Policy::Int8(h)
            }
            _ => return Err(Error::Malformed("motor primitive policy")),
        };
        let n = d.seq()?;
        let mut params = Vec::new();
        for _ in 0..n {
            let name = d.str()?.to_string();
            params.push(Param {
                name,
                min: d.f32()?,
                max: d.f32()?,
                default: d.f32()?,
            });
        }
        let effect = d.item()?;
        let envelope = Envelope {
            duration_ms: d.u32()?,
            force_n: d.f32()?,
            rate_dps: d.f32()?,
            margin_m: d.f32()?,
        };
        let tests = d.vec()?;
        Ok(MotorPrimitive {
            verb,
            kind,
            body_class,
            policy,
            params,
            effect,
            envelope,
            tests,
        })
    }
}

/// Reads a parameter by name, falling back to the primitive's default
/// only when the caller did not supply it.
pub struct Given<'a> {
    pub primitive: &'a MotorPrimitive,
    pub values: &'a [(&'a str, f32)],
}

impl Given<'_> {
    pub fn get(&self, name: &str) -> f32 {
        if let Some((_, v)) = self.values.iter().find(|(n, _)| *n == name) {
            return *v;
        }
        self.primitive.param(name).map(|p| p.default).unwrap_or(0.0)
    }

    /// Is every supplied value one this primitive has, and inside the
    /// range it was built for?
    pub fn in_range(&self) -> core::result::Result<(), &'static str> {
        for (n, v) in self.values {
            match self.primitive.param(n) {
                Some(p) if v.is_finite() && *v >= p.min && *v <= p.max => {}
                Some(_) => return Err("a parameter is outside the range this primitive was built for"),
                None => return Err("this primitive has no such parameter"),
            }
        }
        Ok(())
    }
}

/// How far down a wipe's lower ring is, as a fraction of the depth.
/// Short of the bottom, so the tool is not dragged across the floor.
pub const DEEP: f32 = 0.7;
/// How far the desk arm's fingertips reach below its grasp centre. This
/// library is compiled for that body class.
const TIP_BELOW: f32 = 0.026;
const HOVER: f32 = 0.06;
/// The clearance used when something long is in the hand.
const LOW_HOVER: f32 = 0.03;

/// The strokes for one run, in the affordance frame.
///
/// For `Push` the frame's x axis points from the object toward where it
/// should go. For `Pour` the frame's x axis points away from the arm's
/// base and the vessel is tipped toward +y. `tool_m` is how far below
/// the grasp centre the working end of a held tool is.
pub fn program(g: &Given) -> core::result::Result<Program, &'static str> {
    g.in_range()?;
    let p = g.primitive;
    let squeeze = g.get("squeeze_n");
    let mut out: Program = Vec::new();
    match p.kind {
        Kind::Grasp => {
            let half = g.get("width_m") * 0.5;
            let wide = half + 0.014;
            let pitch = g.get("pitch_deg");
            if pitch > -45.0 {
                // from the side: come in along the frame's x axis
                out.push(Stroke::to([-0.07, 0.0, 0.0], 900).open(wide).pitched(pitch));
                out.push(Stroke::to([0.0, 0.0, 0.0], 700).open(wide).pitched(pitch).resting());
                out.push(
                    Stroke::to([0.0, 0.0, 0.0], 900)
                        .open((half - 0.006).max(0.0))
                        .squeeze(squeeze)
                        .pitched(pitch)
                        .touching()
                        .until(Until::Gripped(squeeze * 0.5)),
                );
                out.push(Stroke::to([0.0, 0.0, HOVER], 800).open((half - 0.006).max(0.0)).squeeze(squeeze).pitched(pitch).touching().resting());
            } else {
                out.push(Stroke::to([0.0, 0.0, HOVER], 900).open(wide));
                out.push(Stroke::to([0.0, 0.0, 0.0], 700).open(wide).resting());
                out.push(
                    Stroke::to([0.0, 0.0, 0.0], 900)
                        .open((half - 0.006).max(0.0))
                        .squeeze(squeeze)
                        .touching()
                        .until(Until::Gripped(squeeze * 0.5)),
                );
                out.push(
                    Stroke::to([0.0, 0.0, HOVER], 800)
                        .open((half - 0.006).max(0.0))
                        .squeeze(squeeze)
                        .touching()
                        .resting(),
                );
            }
        }
        Kind::Place => {
            let half = g.get("width_m") * 0.5;
            let held = (half - 0.006).max(0.0);
            let tool = g.get("tool_m");
            out.push(Stroke::to([0.0, 0.0, LOW_HOVER + tool], 900).open(held).squeeze(squeeze).touching());
            out.push(
                Stroke::to([0.0, 0.0, tool - 0.004], 900)
                    .open(held)
                    .squeeze(squeeze)
                    .soft(0.35)
                    .touching()
                    .until(Until::Touch(g.get("touch_n"))),
            );
            out.push(Stroke::to([0.0, 0.0, tool], 500).open(half + 0.014).touching().resting());
            out.push(Stroke::to([0.0, 0.0, LOW_HOVER + tool], 700).open(half + 0.014));
        }
        Kind::Push => {
            let r = g.get("radius_m");
            let d = g.get("distance_m");
            // `height_m` is how high the fingertips ride; the grasp centre
            // this body steers is that much above them
            let low = g.get("height_m") + TIP_BELOW;
            // the fingers are closed and in line with the push, so the thing
            // meets the flat outer face of the leading finger. Side by side,
            // a round thing wedges itself into the seam between them
            let gap = 0.010;
            // arrive behind the thing and stop before coming down: a descent
            // blended into the approach can land on it
            out.push(Stroke::to([-(r + 0.035), 0.0, HOVER], 900).resting());
            out.push(Stroke::to([-(r + 0.035), 0.0, low], 600).resting());
            out.push(
                Stroke::to([d - r - gap, 0.0, low], (6000.0 * (d + 0.035)) as u32 + 400)
                    
                    .touching()
                    .resting(),
            );
            out.push(Stroke::to([d - r - gap - 0.03, 0.0, low], 400).touching());
            out.push(Stroke::to([d - r - gap - 0.03, 0.0, HOVER], 600));
        }
        Kind::Wipe => {
            let radius = g.get("radius_m");
            let depth = g.get("depth_m");
            let turns = g.get("turns").max(1.0) as usize;
            let tool = g.get("tool_m");
            let held = g.get("open_m");
            let press = g.get("press_m");
            let grip = |s: Stroke| s.open(held).squeeze(squeeze);
            // arrive over the mouth and stop there before going in: a
            // descent blended into the approach clips the rim
            out.push(grip(Stroke::to([0.0, 0.0, HOVER + tool], 900)).touching().resting());
            out.push(grip(Stroke::to([0.0, 0.0, tool - depth * 0.5], 700)).touching().soft(0.5));
            // two levels, so the whole wall is reached, each traced as a
            // ring pressed a little past the wall
            // the path between two points of the ring cuts inside it, so
            // the points are set out by as much as a chord falls short
            let per_turn = 16usize;
            let ring = (radius + press) / libm::cosf(core::f32::consts::PI / per_turn as f32);
            for (level, z) in [depth * 0.35, depth * DEEP].iter().enumerate() {
                // a fifth of a turn over, so the ring closes on itself
                for k in 0..=(turns * per_turn + 3) {
                    let a = (k as f32) * core::f32::consts::TAU / per_turn as f32
                        + level as f32 * 0.3;
                    let at = [ring * libm::cosf(a), ring * libm::sinf(a), tool - z];
                    // soft enough that a light mug is not pushed across the table
                    out.push(grip(Stroke::to(at, if k == 0 { 500 } else { 190 })).touching().soft(0.18));
                }
            }
            out.push(grip(Stroke::to([0.0, 0.0, tool - depth * 0.5], 400)).touching().soft(0.5));
            out.push(grip(Stroke::to([0.0, 0.0, HOVER + tool], 800)).touching());
        }
        Kind::Turn => {
            let half = g.get("width_m") * 0.5;
            // seen from above, the thing turns anticlockwise for a positive
            // angle; the wrist's roll runs the other way
            let angle = g.get("angle_deg");
            let twist = -angle;
            let held = (half - 0.006).max(0.0);
            out.push(Stroke::to([0.0, 0.0, HOVER], 900).open(half + 0.014));
            out.push(Stroke::to([0.0, 0.0, 0.0], 700).open(half + 0.014).resting());
            out.push(
                Stroke::to([0.0, 0.0, 0.0], 900)
                    .open(held)
                    .squeeze(squeeze)
                    .touching()
                    .until(Until::Gripped(squeeze * 0.5)),
            );
            out.push(
                Stroke::to([0.0, 0.0, 0.0], (libm::fabsf(angle) * 14.0) as u32 + 500)
                    .open(held)
                    .squeeze(squeeze)
                    .twisted(twist)
                    .touching()
                    .resting(),
            );
            out.push(Stroke::to([0.0, 0.0, 0.0], 400).open(half + 0.014).twisted(twist).touching().resting());
            out.push(Stroke::to([0.0, 0.0, HOVER], 700).open(half + 0.014).twisted(twist));
        }
        Kind::Pour => {
            let held = g.get("open_m");
            let angle = g.get("angle_deg");
            let side = g.get("offset_m");
            let up = g.get("height_m");
            let grip = |s: Stroke| s.open(held).squeeze(squeeze).pitched(0.0).touching();
            out.push(grip(Stroke::to([0.0, -side, up + 0.03], 1200)));
            out.push(grip(Stroke::to([0.0, -side, up], 600)).resting());
            out.push(grip(Stroke::to([0.0, -side, up], (libm::fabsf(angle) * 16.0) as u32 + 600)).twisted(angle).resting());
            out.push(grip(Stroke::to([0.0, -side, up], 1500)).twisted(angle).resting());
            out.push(grip(Stroke::to([0.0, -side, up], (libm::fabsf(angle) * 12.0) as u32 + 500)).resting());
            out.push(grip(Stroke::to([0.0, -side, up + 0.03], 500)));
        }
    }
    if out.len() > MAX_STROKES {
        return Err("the program is longer than a body will run");
    }
    Ok(out)
}

/// The library for the desk arm. Ranges are what each primitive was
/// built and tested for; a value outside them is refused, not tried.
pub fn desk_arm_library() -> Vec<MotorPrimitive> {
    let class = "desk_arm";
    let f = Fact::new;
    let common = |mut v: Vec<Param>| {
        v.push(Param::new("squeeze_n", 1.0, 20.0, 8.0));
        v
    };
    let mk = |kind: Kind, params: Vec<Param>, effect: Effect, envelope: Envelope, tests: Vec<SuccessTest>| {
        let verb = match kind {
            Kind::Grasp => Verb::literal("arm.grasp"),
            Kind::Place => Verb::literal("arm.place"),
            Kind::Push => Verb::literal("arm.push"),
            Kind::Wipe => Verb::literal("arm.wipe"),
            Kind::Turn => Verb::literal("arm.turn"),
            Kind::Pour => Verb::literal("arm.pour"),
        };
        MotorPrimitive {
            verb,
            kind,
            body_class: class.to_string(),
            policy: Policy::Strokes,
            params,
            effect,
            envelope,
            tests,
        }
    };
    alloc::vec![
        mk(
            Kind::Grasp,
            common(alloc::vec![
                Param::new("width_m", 0.008, 0.07, 0.03),
                Param::new("pitch_deg", -90.0, 0.0, -90.0),
            ]),
            Effect {
                needs: alloc::vec![Fact::hand_free()],
                adds: alloc::vec![f(Pred::Holding, "it")],
                removes: alloc::vec![Fact::hand_free(), f(Pred::Rests, "it")],
            },
            Envelope { duration_ms: 6000, force_n: 20.0, rate_dps: 180.0, margin_m: 0.05 },
            alloc::vec![SuccessTest::Held],
        ),
        mk(
            Kind::Place,
            common(alloc::vec![
                Param::new("width_m", 0.008, 0.07, 0.03),
                Param::new("tool_m", 0.0, 0.12, 0.0),
                Param::new("touch_n", 0.5, 10.0, 2.0),
            ]),
            Effect {
                needs: alloc::vec![f(Pred::Holding, "it")],
                adds: alloc::vec![Fact::hand_free(), f(Pred::Rests, "it")],
                removes: alloc::vec![f(Pred::Holding, "it")],
            },
            Envelope { duration_ms: 6000, force_n: 20.0, rate_dps: 180.0, margin_m: 0.05 },
            alloc::vec![SuccessTest::Released, SuccessTest::Within(0.015), SuccessTest::Upright],
        ),
        mk(
            Kind::Push,
            alloc::vec![
                Param::new("radius_m", 0.01, 0.05, 0.025),
                Param::new("distance_m", 0.02, 0.15, 0.08),
                Param::new("height_m", 0.005, 0.05, 0.02),
                Param::new("squeeze_n", 1.0, 20.0, 8.0),
            ],
            Effect {
                needs: alloc::vec![Fact::hand_free()],
                adds: alloc::vec![f(Pred::Moved, "it")],
                removes: alloc::vec![],
            },
            Envelope { duration_ms: 9000, force_n: 12.0, rate_dps: 120.0, margin_m: 0.05 },
            alloc::vec![SuccessTest::Within(0.02), SuccessTest::Upright],
        ),
        mk(
            Kind::Wipe,
            common(alloc::vec![
                Param::new("radius_m", 0.005, 0.06, 0.02),
                Param::new("depth_m", 0.005, 0.12, 0.05),
                Param::new("turns", 1.0, 3.0, 1.0),
                Param::new("tool_m", 0.0, 0.14, 0.08),
                Param::new("open_m", 0.0, 0.045, 0.004),
                Param::new("press_m", 0.0, 0.01, 0.004),
            ]),
            Effect {
                needs: alloc::vec![f(Pred::Holding, "tool")],
                adds: alloc::vec![f(Pred::Wiped, "it")],
                removes: alloc::vec![],
            },
            Envelope { duration_ms: 16000, force_n: 12.0, rate_dps: 180.0, margin_m: 0.05 },
            alloc::vec![SuccessTest::Covered(0.8), SuccessTest::Upright],
        ),
        mk(
            Kind::Turn,
            common(alloc::vec![
                Param::new("width_m", 0.008, 0.07, 0.03),
                Param::new("angle_deg", -90.0, 90.0, 60.0),
            ]),
            Effect {
                needs: alloc::vec![Fact::hand_free()],
                adds: alloc::vec![f(Pred::Turned, "it")],
                removes: alloc::vec![],
            },
            Envelope { duration_ms: 8000, force_n: 20.0, rate_dps: 180.0, margin_m: 0.05 },
            alloc::vec![SuccessTest::Angle(10.0), SuccessTest::Released],
        ),
        mk(
            Kind::Pour,
            common(alloc::vec![
                Param::new("open_m", 0.0, 0.045, 0.012),
                Param::new("angle_deg", -130.0, 130.0, 110.0),
                Param::new("offset_m", 0.02, 0.10, 0.05),
                Param::new("height_m", 0.02, 0.20, 0.10),
            ]),
            Effect {
                needs: alloc::vec![f(Pred::Holding, "vessel")],
                adds: alloc::vec![f(Pred::Filled, "it")],
                removes: alloc::vec![],
            },
            Envelope { duration_ms: 12000, force_n: 20.0, rate_dps: 180.0, margin_m: 0.06 },
            alloc::vec![SuccessTest::Transferred(0.8), SuccessTest::Held],
        ),
    ]
}

/// Where an affordance frame is, in the body's base frame.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Frame {
    pub pos: [f32; 3],
    /// heading of the frame's x axis, seen from above
    pub yaw_deg: f32,
}

/// Put a program into the base frame: each stroke becomes a goal vector
/// `[x, y, z, finger heading, pitch, twist, opening]`.
pub fn place(program: &[Stroke], frame: &Frame) -> Vec<crate::control::Goal> {
    let (s, c) = (
        libm::sinf(frame.yaw_deg.to_radians()),
        libm::cosf(frame.yaw_deg.to_radians()),
    );
    program
        .iter()
        .map(|k| crate::control::Goal {
            at: [
                frame.pos[0] + c * k.to[0] - s * k.to[1],
                frame.pos[1] + s * k.to[0] + c * k.to[1],
                frame.pos[2] + k.to[2],
                frame.yaw_deg + k.yaw_deg,
                k.pitch_deg,
                k.twist_deg,
                k.open_m,
            ],
            squeeze_n: k.squeeze_n,
            ms: k.ms,
            stiff: k.stiff,
            contact: k.contact,
            until: k.until,
            rest: k.rest,
        })
        .collect()
}
