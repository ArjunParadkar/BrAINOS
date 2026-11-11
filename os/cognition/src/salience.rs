//! BFS  LEVEL    Domain 4 Cognition - tiers, placement, attention, escalation  [crate brainos-cognition]
//! BFS  PURPOSE  Scores how much an event matters with a fixed, readable weighted sum, with thresholds for interrupting the human, mentioning it, or staying quiet
//! BFS  DEFINES  Salience, Salient
//! BFS  USES     brainos: abi
//! BFS  USED BY  cognition/ambient, cognition/lib, cognition/tests/cognition_suite
//!
//! Attention — deciding what, of everything happening, matters.
//!
//! An entity running ambiently sees far more than it should react to.
//! The failure mode in both directions is real: an entity that surfaces
//! everything is a notification firehose nobody keeps, and one that
//! surfaces nothing is a machine that quietly fails.
//!
//! Salience here is a small weighted sum, written out rather than
//! learned, because the weights are a *policy* about what the entity
//! owes its human and should be readable and arguable. A learned
//! attention model would be better at predicting what gets clicked,
//! which is precisely the metric the principles reject.

use brainos_abi::text::Text;

/// Why something might matter.
#[derive(Clone, Copy, Debug, Default)]
pub struct Salience {
    /// how far from expectation, in sigmas
    pub surprise: f32,
    /// could this hurt someone or break something? 0–100
    pub consequence: u8,
    /// is it about something the human asked for? 0–100
    pub relevance: u8,
    /// does it need an answer soon? 0–100
    pub urgency: u8,
    /// has the entity already told them about this? each repeat halves it
    pub repeats: u8,
    /// did a gate refuse something the human should know about?
    pub refusal: bool,
}

impl Salience {
    /// The score, 0–100.
    ///
    /// Consequence answers *if this is real, how bad is it*. Surprise
    /// answers *how much reason is there to think something happened*.
    /// The two **multiply**, and a twenty-thousand-tick run is what
    /// settled that they should.
    ///
    /// They used to add, which made consequence a constant the channel
    /// carried: a three-sigma blip on a robot's tilt sensor — six tenths
    /// of a degree, well inside its own noise — interrupted a human,
    /// because tilt is important on a body that can fall. An entity that
    /// interrupts on the strength of which channel a reading came from,
    /// while the reading is indistinguishable from normal, is broken in
    /// a way that is hard to notice from a unit test and obvious after
    /// four hours of operation.
    ///
    /// A refusal is treated as evidence in its own right. The gate
    /// refusing something *is* the event; there is no surprise term to
    /// wait for.
    ///
    /// ```text
    ///   evidence    0–100%  how much reason to think something happened
    ///   stake       0–400   consequence, scaled by evidence
    ///   surprise    0–60    the deviation itself
    ///   relevance   0–50
    ///   urgency     0–50
    ///   refusal     0 or 60
    ///   ────────────────────
    ///   total       0–620
    /// ```
    pub fn score(&self) -> u8 {
        let evidence_pct: u32 = if self.refusal {
            100
        } else {
            (self.surprise.clamp(0.0, 10.0) * 10.0) as u32
        };

        let mut s = 0u32;
        // how bad it would be, scaled by how much reason there is to
        // think it happened
        s += (self.consequence as u32 * evidence_pct) / 100 * 4;
        s += (self.surprise.clamp(0.0, 10.0) * 6.0) as u32;
        s += self.relevance as u32 / 2;
        s += self.urgency as u32 / 2;
        if self.refusal {
            s += 60;
        }

        // repetition damps: telling someone the same thing five times is
        // how an assistant becomes something people mute
        let damped = s >> self.repeats.min(6);
        ((damped * 100) / 620).min(100) as u8
    }

    /// Above this, tell the human now.
    pub const INTERRUPT: u8 = 60;
    /// Above this, put it where they will see it later.
    pub const SURFACE: u8 = 25;

    pub fn should_interrupt(&self) -> bool {
        self.score() >= Salience::INTERRUPT
    }

    pub fn should_surface(&self) -> bool {
        self.score() >= Salience::SURFACE
    }
}

/// Something worth the human's attention, with the reason attached.
#[derive(Clone, Debug)]
pub struct Salient {
    pub what: alloc::string::String,
    pub salience: Salience,
}

impl Salient {
    pub fn new(what: &str, salience: Salience) -> Salient {
        Salient {
            what: alloc::string::ToString::to_string(what),
            salience,
        }
    }

    pub fn describe(&self) -> Text<200> {
        let mut t = Text::new();
        t.push_u64(self.salience.score() as u64);
        t.push_str(if self.salience.should_interrupt() {
            " NOW "
        } else if self.salience.should_surface() {
            " later "
        } else {
            " quiet "
        });
        t.push_str(&self.what);
        t
    }
}
