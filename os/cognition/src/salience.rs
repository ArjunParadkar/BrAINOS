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
    /// Consequence dominates, and the weights are chosen so that a
    /// maximally consequential event clears the interrupt threshold *on
    /// its own* — an unsurprising, unrequested, unhurried event that
    /// could injure someone still gets said out loud. On a laptop a
    /// surprising event is interesting; on a body a consequential one is
    /// the only kind that must not be missed.
    ///
    /// ```text
    ///   surprise    0–60    capped at ten sigmas
    ///   consequence 0–400   four times the others: the dominant term
    ///   relevance   0–50
    ///   urgency     0–50
    ///   refusal     0 or 60
    ///   ─────────────────
    ///   total       0–620
    /// ```
    pub fn score(&self) -> u8 {
        let mut s = 0u32;
        s += (self.surprise.clamp(0.0, 10.0) * 6.0) as u32;
        s += self.consequence as u32 * 4;
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
