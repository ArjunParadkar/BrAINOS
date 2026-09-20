//! Ambient operation — the loop that replaces turn-taking.
//!
//! "A sense → predict → error → attend loop replaces turn-based
//! interaction. The entity runs continuously and surfaces only what
//! needs a human."
//!
//! This is the shape of that loop, and the shape is the whole claim. A
//! turn-based assistant is a function: input, output, forget. An ambient
//! entity has a life: it is always running, mostly unsurprised, and the
//! interesting question is not "what did it reply" but "what did it
//! decide was worth mentioning".
//!
//! One pass:
//!
//! ```text
//!   SENSE      what the bodies delivered this tick
//!   PREDICT    what the forward model expected
//!   ERROR      the difference, in units of the entity's own uncertainty
//!   ATTEND     is any of it salient enough to act on or to surface?
//!   ESCALATE   if so, how far up does this warrant going?
//! ```
//!
//! Steps that follow — proposing, gating, acting, consolidating — are
//! not here. They belong to the entity, and keeping them out means this
//! loop cannot accidentally act: it produces an `Intent`, and an intent
//! is a thought.

use crate::escalate::{self, Budget, Escalation};
use crate::predict::{Forward, Surprise};
use crate::resource::Tier;
use crate::salience::{Salience, Salient};
use alloc::string::String;
use alloc::vec::Vec;
use brainos_abi::ids::BodyId;
use brainos_abi::time::{Instant, Tick};

/// What the bodies delivered this tick.
#[derive(Clone, Debug, Default)]
pub struct SenseFrame {
    pub at: Instant,
    /// a complete line from the human, if one arrived
    pub human_said: Option<String>,
    /// named scalar observations from every body, already normalized
    pub channels: Vec<(String, f32, Option<BodyId>)>,
    /// a body stopped itself
    pub stops: Vec<(BodyId, &'static str)>,
    /// the highest tier currently reachable
    pub ceiling: Tier,
}

impl SenseFrame {
    pub fn quiet(at: Instant) -> SenseFrame {
        SenseFrame {
            at,
            human_said: None,
            channels: Vec::new(),
            stops: Vec::new(),
            ceiling: Tier::Frontier,
        }
    }

    pub fn saying(mut self, what: &str) -> SenseFrame {
        self.human_said = Some(alloc::string::ToString::to_string(what));
        self
    }

    pub fn sensing(mut self, name: &str, value: f32) -> SenseFrame {
        self.channels
            .push((alloc::string::ToString::to_string(name), value, None));
        self
    }

    pub fn from_body(mut self, body: BodyId, name: &str, value: f32) -> SenseFrame {
        self.channels
            .push((alloc::string::ToString::to_string(name), value, Some(body)));
        self
    }
}

/// What this tick concluded.
#[derive(Clone, Debug)]
pub enum Intent {
    /// nothing salient; the autonomic layer absorbed it
    Idle { status: String },
    /// something worth thinking about, at this tier
    Think {
        escalation: Escalation,
        about: String,
        surprise: Surprise,
    },
    /// something the human should hear
    Surface { items: Vec<Salient> },
    /// a body stopped and needs a decision
    Attend {
        body: BodyId,
        why: &'static str,
        escalation: Escalation,
    },
}

impl Intent {
    pub fn is_idle(&self) -> bool {
        matches!(self, Intent::Idle { .. })
    }
}

/// One thing the entity has recently told its human about.
#[derive(Clone, Copy, Debug)]
struct Surfaced {
    /// the channel, and which severity band it was in
    channel: u64,
    band: u8,
    repeats: u8,
    at: Tick,
}

/// Which severity band an observation falls in.
///
/// Damping is keyed on the channel *and* the band, because a channel
/// with two regimes has two stories. A leg that draws 1.9 A under heavy
/// motion — often, and harmlessly — and 2.8 A because it has stalled —
/// rarely, and not harmlessly — is reporting two different things down
/// one wire, and a counter that cannot tell them apart lets the common
/// one silence the rare one.
///
/// Bands are octaves of deviation: 1-2 sigmas, 2-4, 4-8, and so on.
/// Coarse on purpose. A finer split would make every slightly-worse
/// occurrence a fresh event and undo the damping entirely.
pub fn severity_band(sigmas: f32) -> u8 {
    if !sigmas.is_finite() || sigmas < 1.0 {
        return 0;
    }
    let mut band = 0u8;
    let mut v = sigmas;
    while v >= 2.0 && band < 15 {
        v /= 2.0;
        band += 1;
    }
    band
}

/// How long before one repeat is forgiven.
///
/// At roughly a tick every ten milliseconds this is about ten seconds.
/// The number that matters is the ratio between it and how often a
/// genuine fault recurs: a fault that comes back inside the window is
/// the same event still happening, and one that comes back after it is
/// news again.
pub const FORGIVE_AFTER: u64 = 1_000;

/// What the entity is currently paying attention to.
#[derive(Clone, Debug, Default)]
pub struct Attention {
    /// the salient things this tick, strongest first
    pub items: Vec<Salient>,
    /// how many consecutive ticks have been quiet
    pub quiet_ticks: u64,
}

/// The ambient loop.
pub struct Ambient {
    pub forward: Forward,
    pub budget: Budget,
    pub attention: Attention,
    /// how many times the tier below has failed at the current problem
    lower_failures: u32,
    /// What has been surfaced recently: channel, repeats, when, and how
    /// extreme it was.
    ///
    /// All four are load-bearing, and a twenty-thousand-tick run found
    /// out why by scoring 37% recall with two separate bugs in this one
    /// field.
    ///
    /// **The tick.** Damping without it means an entity that surfaced a
    /// recurring fault six times goes permanently silent about it. A leg
    /// that keeps stalling should keep being mentioned; the tenth
    /// occurrence is worse news than the first, not less interesting.
    ///
    /// **The severity.** Damping on channel identity alone means a
    /// frequent mild anomaly masks a rare severe one on the same
    /// channel. In the run that found it, a leg drawing 1.9 A under
    /// heavy motion — often, and harmlessly — saturated the counter for
    /// `leg.current`, so a leg drawing 2.8 A because it had stalled was
    /// never mentioned at all. Fifty-four stalls, none surfaced. "It is
    /// much worse than last time" is new information, whatever the
    /// channel.
    surfaced: Vec<Surfaced>,
    pub ticks: u64,
    pub escalations: u64,
}

impl Default for Ambient {
    fn default() -> Ambient {
        Ambient::new(Budget::plentiful())
    }
}

impl Ambient {
    pub fn new(budget: Budget) -> Ambient {
        Ambient {
            forward: Forward::new(),
            budget,
            attention: Attention::default(),
            lower_failures: 0,
            surfaced: Vec::new(),
            ticks: 0,
            escalations: 0,
        }
    }

    /// The tier below failed at whatever is currently being worked on.
    pub fn note_failure(&mut self) {
        self.lower_failures = self.lower_failures.saturating_add(1);
    }

    pub fn note_success(&mut self) {
        self.lower_failures = 0;
    }

    /// One pass.
    pub fn tick(
        &mut self,
        sense: &SenseFrame,
        now: Tick,
        have_practiced_skill: impl Fn(&str) -> bool,
        consequence_of: impl Fn(&str) -> u8,
    ) -> Intent {
        self.ticks = self.ticks.saturating_add(1);
        self.budget.tick();

        // --- a stopped body outranks everything ---
        //
        // Not because it is more surprising — a body that stops when it
        // should is entirely expected — but because it is the one thing
        // that is already a decision waiting on the entity. Ranking it
        // by surprise would mean a well-behaved reflex is ignored.
        if let Some((body, why)) = sense.stops.first() {
            let s = Surprise {
                key: Forward::key_of("body.stop"),
                observed: 1.0,
                expected: 0.0,
                sigmas: 10.0,
                novel: false,
            };
            let escalation = escalate::decide(
                &s,
                90,
                false,
                self.lower_failures,
                false,
                &mut self.budget,
                sense.ceiling,
            );
            if escalation.happened() {
                self.escalations = self.escalations.saturating_add(1);
            }
            self.attention.quiet_ticks = 0;
            return Intent::Attend {
                body: *body,
                why,
                escalation,
            };
        }

        // --- SENSE → PREDICT → ERROR ---
        let mut worst: Option<(Surprise, String)> = None;
        for (name, value, _body) in &sense.channels {
            let s = self.forward.observe_named(name, *value);
            let replace = match &worst {
                None => true,
                Some((w, _)) => s.sigmas > w.sigmas || (s.novel && !w.novel),
            };
            if replace {
                worst = Some((s, name.clone()));
            }
        }

        // --- the human speaking is the one thing the rhythm never
        //     predicts, so it always attends ---
        if let Some(said) = &sense.human_said {
            let s = Surprise {
                key: Forward::key_of("human.said"),
                observed: 1.0,
                expected: 0.0,
                sigmas: 4.0,
                novel: false,
            };
            let escalation = escalate::decide(
                &s,
                consequence_of(said),
                have_practiced_skill(said),
                self.lower_failures,
                true,
                &mut self.budget,
                sense.ceiling,
            );
            if escalation.happened() {
                self.escalations = self.escalations.saturating_add(1);
            }
            self.attention.quiet_ticks = 0;
            return Intent::Think {
                escalation,
                about: said.clone(),
                surprise: s,
            };
        }

        // --- ATTEND ---
        match worst {
            Some((s, name)) if s.is_salient() => {
                let repeats = self.repeat_count(s.key, now, s.sigmas);
                let salience = Salience {
                    surprise: s.sigmas,
                    consequence: consequence_of(&name),
                    relevance: 40,
                    urgency: if s.is_profound() { 80 } else { 30 },
                    repeats,
                    refusal: false,
                };
                let escalation = escalate::decide(
                    &s,
                    salience.consequence,
                    have_practiced_skill(&name),
                    self.lower_failures,
                    false,
                    &mut self.budget,
                    sense.ceiling,
                );
                if escalation.happened() {
                    self.escalations = self.escalations.saturating_add(1);
                }
                self.attention.quiet_ticks = 0;

                if salience.should_surface() {
                    self.note_surfaced(s.key, now, s.sigmas);
                    let item = Salient::new(&name, salience);
                    self.attention.items = alloc::vec![item.clone()];
                    return Intent::Surface {
                        items: alloc::vec![item],
                    };
                }

                Intent::Think {
                    escalation,
                    about: name,
                    surprise: s,
                }
            }
            _ => {
                self.attention.quiet_ticks = self.attention.quiet_ticks.saturating_add(1);
                self.attention.items.clear();
                let mut status = String::new();
                status.push_str("tick ");
                push_u64(&mut status, now.0);
                status.push_str(" · nothing surprising · surprise rate ");
                push_u64(&mut status, self.forward.surprise_rate_pct() as u64);
                status.push('%');
                Intent::Idle { status }
            }
        }
    }

    /// How many times this channel, at this severity, has recently been
    /// surfaced. One repeat is forgiven per `FORGIVE_AFTER` ticks.
    fn repeat_count(&self, key: u64, now: Tick, severity: f32) -> u8 {
        let band = severity_band(severity);
        self.surfaced
            .iter()
            .find(|s| s.channel == key && s.band == band)
            .map(|s| {
                let forgiven = (now.since(s.at) / FORGIVE_AFTER).min(u8::MAX as u64) as u8;
                s.repeats.saturating_sub(forgiven)
            })
            .unwrap_or(0)
    }

    fn note_surfaced(&mut self, key: u64, now: Tick, severity: f32) {
        let band = severity_band(severity);
        let decayed = self.repeat_count(key, now, severity);
        match self
            .surfaced
            .iter_mut()
            .find(|s| s.channel == key && s.band == band)
        {
            Some(s) => {
                s.repeats = decayed.saturating_add(1);
                s.at = now;
            }
            None => self.surfaced.push(Surfaced {
                channel: key,
                band,
                repeats: 1,
                at: now,
            }),
        }
        // a channel with many bands is a channel with a lot going on;
        // bound it so a noisy sensor cannot grow this without limit
        while self.surfaced.len() > 128 {
            self.surfaced.remove(0);
        }
    }

    /// Forget what has been surfaced, so a genuinely new occurrence of
    /// something old is heard again. Called when the human acknowledges.
    pub fn acknowledge(&mut self) {
        self.surfaced.clear();
    }

    /// Has the entity been quiet long enough to be worth consolidating?
    ///
    /// Consolidation rewrites memory, and a rewrite racing a gait loop is
    /// the kind of thing that is fine in testing and catastrophic once.
    /// Quiet is the proxy for "no body is in motion".
    pub fn should_consolidate(&self, after_quiet_ticks: u64) -> bool {
        self.attention.quiet_ticks >= after_quiet_ticks
    }

    /// A consolidation pass just ran.
    ///
    /// Resets the quiet counter, so the next one waits another whole
    /// idle period rather than firing on every subsequent tick. Without
    /// it, a night's idleness runs eight hundred passes where four would
    /// do — which a twenty-thousand-tick day found by reporting exactly
    /// that number.
    pub fn consolidated(&mut self) {
        self.attention.quiet_ticks = 0;
    }
}

fn push_u64(s: &mut String, mut v: u64) {
    if v == 0 {
        s.push('0');
        return;
    }
    let mut buf = [0u8; 20];
    let mut n = 0;
    while v > 0 {
        buf[n] = b'0' + (v % 10) as u8;
        v /= 10;
        n += 1;
    }
    while n > 0 {
        n -= 1;
        s.push(buf[n] as char);
    }
}
