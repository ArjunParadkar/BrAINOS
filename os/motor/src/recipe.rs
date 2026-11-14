//! BFS  LEVEL    Domain 2 Autonomic - motor memory (self-model, mandates, primitives, reflexes)  [crate brainos-motor]
//! BFS  PURPOSE  One-shot learning: cuts a single demonstration into primitives at changes of grip and contact, expresses each step's lengths relative to the object it acts on, and checks the steps chain
//! BFS  DEFINES  Place, Sample, Bind, Step, Recipe, LearnError, learn
//! BFS  USES     this crate: condition, primitive
//! BFS  USED BY  entity/entity/motor, entity/tests/motor_memory, host/motor, host/motor_mind, host/motor_seat, motor/tests/recipe
//!
//! One demonstration, one recipe.
//!
//! A demonstration is a recording: where the tool was, how far the
//! fingers were open, what the fingers and the tool felt, and where the
//! named things in the scene were. Learning from it is three plain
//! steps, none of which trains anything:
//!
//! 1. **Cut** it where the hand's relationship to the world changes:
//!    the fingers close on something, let go of something, or the tool
//!    starts or stops touching. Those are the only places a primitive
//!    can begin or end.
//! 2. **Name** each piece by what happened in it (closed on a thing:
//!    grasp; let go: place; touched while holding a tool: wipe; ...) and
//!    measure its lengths **relative to the thing it acted on**: not
//!    "a circle of radius 22 mm" but "a circle 0.73 of the mouth's
//!    radius". That ratio is what carries over to a different mug.
//! 3. **Check** the pieces chain: what each step needs must be true
//!    after the step before it (`condition::follows`, the stand-in for
//!    LOCI's `compose`).
//!
//! The result is a `Recipe`. It is a *candidate* until its own success
//! test has passed; the entity binds it as a belief only then.
//!
//! The cutter is the simple one the brief asks for first: change points
//! on contact and gripper state. It will mis-cut a demonstration in
//! which two things change at once or nothing changes at all, and says
//! so (`LearnError`) rather than guessing.

use crate::condition::{follows, Fact, Pred, State};
use crate::primitive::{Kind, MotorPrimitive};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// A named place in the scene, with the lengths that describe it.
#[derive(Clone, PartialEq, Debug)]
pub struct Place {
    /// "mug.mouth", "brush.grip": the thing, then which part of it
    pub name: String,
    pub pos: [f32; 3],
    pub yaw_deg: f32,
    /// named lengths of the thing: "radius", "depth", "width", ...
    pub attrs: Vec<(String, f32)>,
}

impl Place {
    pub fn thing(&self) -> &str {
        self.name.split('.').next().unwrap_or("")
    }

    pub fn part(&self) -> &str {
        self.name.split('.').nth(1).unwrap_or("")
    }

    pub fn attr(&self, name: &str) -> Option<f32> {
        self.attrs.iter().find(|(n, _)| n == name).map(|(_, v)| *v)
    }
}

/// One instant of a demonstration.
#[derive(Clone, PartialEq, Debug)]
pub struct Sample {
    pub t_ms: f32,
    /// the grasp centre
    pub tool: [f32; 3],
    /// each finger's distance from the centre line
    pub open_m: f32,
    /// wrist roll, degrees
    pub roll_deg: f32,
    pub grip_n: f32,
    pub tool_n: f32,
    pub places: Vec<Place>,
}

/// How a step's parameter is fixed.
#[derive(Clone, PartialEq, Debug)]
pub enum Bind {
    /// this value, whatever the object
    Fixed(f32),
    /// `scale * attribute + offset`, read off the object at run time
    Of {
        attr: String,
        scale: f32,
        offset: f32,
    },
}

impl Bind {
    pub fn value(&self, place: &Place) -> Option<f32> {
        match self {
            Bind::Fixed(v) => Some(*v),
            Bind::Of { attr, scale, offset } => place.attr(attr).map(|a| scale * a + offset),
        }
    }
}

/// One step of a recipe.
#[derive(Clone, PartialEq, Debug)]
pub struct Step {
    pub kind: Kind,
    /// the place it acts on, by name
    pub frame: String,
    /// role names for the condition check
    pub it: String,
    pub to: String,
    pub params: Vec<(String, Bind)>,
}

/// What was learned from one demonstration.
#[derive(Clone, PartialEq, Debug)]
pub struct Recipe {
    pub steps: Vec<Step>,
    /// what is true when it has worked
    pub goal: Vec<Fact>,
}

#[derive(Clone, PartialEq, Debug)]
pub enum LearnError {
    /// fewer than two samples
    TooShort,
    /// the fingers closed on something and no named place was near
    NothingNear(f32),
    /// a piece that fits no primitive in the library
    Unnamed { from_ms: f32, to_ms: f32 },
    /// the library has no primitive of the kind a piece needs
    NotInLibrary(Kind),
    /// step `index` needs something the steps before it do not provide
    DoesNotChain { index: usize, missing: Fact },
    /// the demonstration ended with something still in the hand
    EndsHolding,
}

const GRIP_ON_N: f32 = 0.8;
const TOUCH_ON_N: f32 = 0.6;
/// A change must last this long to count; shorter is chatter.
const HOLD_MS: f32 = 60.0;
const NEAR_M: f32 = 0.06;

fn dist(a: &[f32; 3], b: &[f32; 3]) -> f32 {
    libm::sqrtf((0..3).map(|k| (a[k] - b[k]) * (a[k] - b[k])).sum())
}

fn nearest<'a>(places: &'a [Place], to: &[f32; 3], wanted: &[&str], not_thing: &str) -> Option<&'a Place> {
    places
        .iter()
        .filter(|p| wanted.contains(&p.part()) && p.thing() != not_thing)
        .min_by(|a, b| {
            dist(&a.pos, to)
                .partial_cmp(&dist(&b.pos, to))
                .unwrap_or(core::cmp::Ordering::Equal)
        })
}

/// A debounced on/off signal: the index at which each change settles.
fn changes(samples: &[Sample], on: impl Fn(&Sample) -> bool) -> Vec<(usize, bool)> {
    let mut out = Vec::new();
    let mut state = false;
    let mut i = 0;
    while i < samples.len() {
        let now = on(&samples[i]);
        if now != state {
            // does it stay changed?
            let mut j = i;
            while j < samples.len() && on(&samples[j]) == now && samples[j].t_ms - samples[i].t_ms < HOLD_MS {
                j += 1;
            }
            if j == samples.len() || samples[j].t_ms - samples[i].t_ms >= HOLD_MS {
                state = now;
                out.push((i, now));
            }
        }
        i += 1;
    }
    out
}

/// Express a measured length relative to an attribute of the object, if
/// the object has that attribute; otherwise keep the number.
fn relative(measured: f32, place: &Place, attr: &str) -> Bind {
    match place.attr(attr) {
        Some(a) if a > 1e-6 => Bind::Of {
            attr: attr.to_string(),
            scale: measured / a,
            offset: 0.0,
        },
        _ => Bind::Fixed(measured),
    }
}

/// Express a measured length as an attribute of the object plus a fixed
/// amount. One demonstration cannot tell "0.77 of the radius" from "8 mm
/// less than the radius"; they differ only on a mug of another size.
/// This is the rule used for lengths that end where a held tool meets
/// the object (how far out a wipe reaches, how deep it goes): the
/// difference is the tool's own size, which does not change with the
/// object. It is a prior, written down here, not something learned.
fn offset_from(measured: f32, place: &Place, attr: &str) -> Bind {
    match place.attr(attr) {
        Some(a) => Bind::Of {
            attr: attr.to_string(),
            scale: 1.0,
            offset: measured - a,
        },
        None => Bind::Fixed(measured),
    }
}

/// Learn a recipe from one demonstration.
pub fn learn(samples: &[Sample], library: &[MotorPrimitive]) -> Result<Recipe, LearnError> {
    if samples.len() < 2 {
        return Err(LearnError::TooShort);
    }
    let has = |k: Kind| library.iter().find(|p| p.kind == k).ok_or(LearnError::NotInLibrary(k));

    let grips = changes(samples, |s| s.grip_n > GRIP_ON_N);
    let touches = changes(samples, |s| s.tool_n > TOUCH_ON_N);

    let mut steps: Vec<Step> = Vec::new();
    let mut held: Option<String> = None;
    // how far below the grasp centre the held thing's working end is
    let mut tool_len = 0.0f32;
    let mut events: Vec<(usize, u8, bool)> = Vec::new();
    events.extend(grips.iter().map(|(i, on)| (*i, 0u8, *on)));
    events.extend(touches.iter().map(|(i, on)| (*i, 1u8, *on)));
    events.sort_by_key(|e| e.0);

    let mut touch_from: Option<usize> = None;
    // the furthest out and the deepest the wipe in progress has reached
    let mut wiped: Option<(f32, f32)> = None;
    for (i, what, on) in events {
        let s = &samples[i];
        match (what, on) {
            // the fingers closed on something
            (0, true) => {
                let p = nearest(&s.places, &s.tool, &["grip"], "")
                    .filter(|p| dist(&p.pos, &s.tool) < NEAR_M)
                    .ok_or(LearnError::NothingNear(s.t_ms))?;
                has(Kind::Grasp)?;
                // the gap between the fingers at the instant they first feel it
                let width = 2.0 * s.open_m;
                steps.push(Step {
                    kind: Kind::Grasp,
                    frame: p.name.clone(),
                    it: p.thing().to_string(),
                    to: String::new(),
                    params: alloc::vec![("width_m".to_string(), relative(width, p, "width"))],
                });
                tool_len = p.attr("reach").unwrap_or(0.0);
                held = Some(p.thing().to_string());
            }
            // the fingers let go
            (0, false) => {
                let Some(thing) = held.take() else { continue };
                has(Kind::Place)?;
                let p = nearest(&s.places, &s.tool, &["rest"], &thing)
                    .filter(|p| dist(&p.pos, &s.tool) < NEAR_M + tool_len)
                    .ok_or(LearnError::NothingNear(s.t_ms))?;
                let width = steps
                    .iter()
                    .rev()
                    .find(|st| st.kind == Kind::Grasp && st.it == thing)
                    .and_then(|st| st.params.first().cloned())
                    .map(|(_, b)| b)
                    .unwrap_or(Bind::Fixed(0.03));
                steps.push(Step {
                    kind: Kind::Place,
                    frame: p.name.clone(),
                    it: thing,
                    to: p.thing().to_string(),
                    params: alloc::vec![
                        ("width_m".to_string(), width),
                        ("tool_m".to_string(), Bind::Fixed((s.tool[2] - p.pos[2]).max(0.0))),
                    ],
                });
                tool_len = 0.0;
            }
            (1, true) => touch_from = Some(i),
            // the tool stopped touching: what was that contact?
            (1, false) => {
                let Some(from) = touch_from.take() else { continue };
                let piece = &samples[from..=i];
                let mid = &piece[piece.len() / 2];
                // the working end is below the grasp centre
                let tip = |q: &Sample| [q.tool[0], q.tool[1], q.tool[2] - tool_len];
                if held.is_some() {
                    let Some(p) = nearest(&mid.places, &tip(mid), &["mouth", "patch"], held.as_deref().unwrap_or("")) else {
                        return Err(LearnError::Unnamed { from_ms: piece[0].t_ms, to_ms: s.t_ms });
                    };
                    if dist(&p.pos, &tip(mid)) > NEAR_M + 0.08 {
                        // touching while carrying something, nowhere
                        // near a surface to wipe: setting it down, which
                        // the release that follows will name
                        continue;
                    }
                    has(Kind::Wipe)?;
                    let mut radius = 0.0f32;
                    let mut depth = 0.0f32;
                    let mut swept = 0.0f32;
                    let mut last_angle: Option<f32> = None;
                    for q in piece {
                        let t = tip(q);
                        let (dx, dy) = (t[0] - p.pos[0], t[1] - p.pos[1]);
                        let r = libm::sqrtf(dx * dx + dy * dy);
                        radius = radius.max(r);
                        depth = depth.max(p.pos[2] - t[2]);
                        if r > 0.004 {
                            let a = libm::atan2f(dy, dx);
                            if let Some(b) = last_angle {
                                let mut d = a - b;
                                while d > core::f32::consts::PI {
                                    d -= core::f32::consts::TAU;
                                }
                                while d < -core::f32::consts::PI {
                                    d += core::f32::consts::TAU;
                                }
                                swept += libm::fabsf(d);
                            }
                            last_angle = Some(a);
                        }
                    }
                    let turns = (swept / core::f32::consts::TAU / 2.0).clamp(1.0, 3.0);
                    // a wipe that directly follows a wipe of the same
                    // place is the same wipe, interrupted: keep the
                    // furthest it reached either time
                    if let Some(prev) = steps.last_mut() {
                        if prev.kind == Kind::Wipe && prev.frame == p.name {
                            let (r0, d0) = wiped.unwrap_or((0.0, 0.0));
                            let (r1, d1) = (radius.max(r0), depth.max(d0));
                            wiped = Some((r1, d1));
                            for (n, b) in prev.params.iter_mut() {
                                if n == "radius_m" {
                                    *b = offset_from(r1, p, "radius");
                                } else if n == "depth_m" {
                                    *b = offset_from(d1 / crate::primitive::DEEP, p, "depth");
                                }
                            }
                            continue;
                        }
                    }
                    wiped = Some((radius, depth));
                    steps.push(Step {
                        kind: Kind::Wipe,
                        frame: p.name.clone(),
                        it: p.thing().to_string(),
                        to: String::new(),
                        params: alloc::vec![
                            ("radius_m".to_string(), offset_from(radius, p, "radius")),
                            ("depth_m".to_string(), offset_from(depth / crate::primitive::DEEP, p, "depth")),
                            ("turns".to_string(), Bind::Fixed(libm::roundf(turns))),
                            ("tool_m".to_string(), Bind::Fixed(tool_len)),
                            ("open_m".to_string(), Bind::Fixed(mid.open_m)),
                        ],
                    });
                } else {
                    // touching with an empty hand: a push, if the thing
                    // nearest the tool moved
                    // (brushing against something with nothing pushable
                    // nearby is not a step: the fingers leaving what
                    // they just set down, for one)
                    let Some(p0) = nearest(&piece[0].places, &piece[0].tool, &["side"], "")
                        .filter(|p| dist(&p.pos, &piece[0].tool) < NEAR_M + 0.04)
                    else {
                        continue;
                    };
                    let p1 = s.places.iter().find(|p| p.name == p0.name).unwrap_or(p0);
                    let moved = dist(&p0.pos, &p1.pos);
                    if moved < 0.01 {
                        continue;
                    }
                    has(Kind::Push)?;
                    steps.push(Step {
                        kind: Kind::Push,
                        frame: p0.name.clone(),
                        it: p0.thing().to_string(),
                        to: String::new(),
                        params: alloc::vec![
                            ("radius_m".to_string(), relative(p0.attr("radius").unwrap_or(0.025), p0, "radius")),
                            ("distance_m".to_string(), Bind::Fixed(moved)),
                        ],
                    });
                }
            }
            _ => {}
        }
    }
    if held.is_some() {
        return Err(LearnError::EndsHolding);
    }
    if steps.is_empty() {
        return Err(LearnError::Unnamed {
            from_ms: samples[0].t_ms,
            to_ms: samples[samples.len() - 1].t_ms,
        });
    }

    // do the steps chain?
    let mut state = State::start();
    for (index, st) in steps.iter().enumerate() {
        let prim = has(st.kind)?;
        // a wipe needs a tool in the hand: bind that role to whatever
        // the recipe is holding at this point
        let holding = state
            .facts()
            .iter()
            .find(|f| f.pred == Pred::Holding)
            .map(|f| f.about.as_str().to_string())
            .unwrap_or_default();
        let mut effect = prim.bound(&st.it, &st.to);
        for f in effect.needs.iter_mut().chain(effect.adds.iter_mut()).chain(effect.removes.iter_mut()) {
            if f.about.as_str() == "tool" || f.about.as_str() == "vessel" {
                *f = Fact::new(f.pred, &holding);
            }
        }
        follows(&state, &effect).map_err(|missing| LearnError::DoesNotChain { index, missing })?;
        let _ = effect.apply(&mut state);
    }
    let goal = state.facts().to_vec();
    Ok(Recipe { steps, goal })
}

impl Recipe {
    /// One line per step, readable and parseable:
    /// `wipe frame=mug.mouth it=mug to= radius_m=radius*0.73+0 turns=1`
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        for s in &self.steps {
            out.push_str(s.kind.name());
            out.push_str(&alloc::format!(" frame={} it={} to={}", s.frame, s.it, s.to));
            for (n, b) in &s.params {
                match b {
                    Bind::Fixed(v) => out.push_str(&alloc::format!(" {n}={v}")),
                    Bind::Of { attr, scale, offset } => {
                        out.push_str(&alloc::format!(" {n}={attr}*{scale}+{offset}"))
                    }
                }
            }
            out.push('\n');
        }
        out
    }

    pub fn from_text(text: &str) -> Option<Recipe> {
        let mut steps = Vec::new();
        for line in text.lines().filter(|l| !l.trim().is_empty() && !l.starts_with('#')) {
            let mut f = line.split_whitespace();
            let kind = Kind::from_name(f.next()?)?;
            let mut step = Step {
                kind,
                frame: String::new(),
                it: String::new(),
                to: String::new(),
                params: Vec::new(),
            };
            for kv in f {
                let (k, v) = kv.split_once('=')?;
                match k {
                    "frame" => step.frame = v.to_string(),
                    "it" => step.it = v.to_string(),
                    "to" => step.to = v.to_string(),
                    _ => {
                        let bind = match v.split_once('*') {
                            Some((attr, rest)) => {
                                let (scale, offset) = rest.split_once('+')?;
                                Bind::Of {
                                    attr: attr.to_string(),
                                    scale: scale.parse().ok()?,
                                    offset: offset.parse().ok()?,
                                }
                            }
                            None => Bind::Fixed(v.parse().ok()?),
                        };
                        step.params.push((k.to_string(), bind));
                    }
                }
            }
            steps.push(step);
        }
        if steps.is_empty() {
            return None;
        }
        Some(Recipe { steps, goal: Vec::new() })
    }
}
