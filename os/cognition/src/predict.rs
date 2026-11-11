//! BFS  LEVEL    Domain 4 Cognition - tiers, placement, attention, escalation  [crate brainos-cognition]
//! BFS  PURPOSE  A running-average forward model that predicts each sensor channel's next value and scores surprise as the miss divided by that channel's usual error
//! BFS  DEFINES  MAX_CHANNELS, Prediction, Surprise, Forward
//! BFS  USES     brainos: abi
//! BFS  USED BY  cognition/ambient, cognition/escalate, cognition/lib, cognition/tests/cognition_suite, host/degrade, nostd/lib
//!
//! Prediction and surprise.
//!
//! The experience loop is built on predictive coding: the entity
//! continuously predicts what the next moment will feel like, compares,
//! and only escalates to expensive cognition when genuinely surprised.
//! This is not an aesthetic choice about how minds work — it is the
//! mechanism that makes continuous operation affordable. An entity that
//! escalated every tick would cost a frontier call per tick.
//!
//! The forward model here is deliberately simple and deliberately
//! *learned from the entity's own history* rather than supplied by a
//! model: an exponentially-weighted expectation per observation channel,
//! with a running estimate of its own error. Surprise is the observation
//! divided by that error — a channel that is always noisy needs a big
//! deviation to be surprising, and a channel that is normally stable
//! needs only a small one.
//!
//! What this cannot do: predict anything it has not seen a channel of
//! before. A genuinely new kind of observation is maximally surprising
//! by construction, which is the correct behaviour — the entity should
//! escalate the first time something new happens.

use alloc::vec::Vec;
use brainos_abi::text::Text;

/// How many channels an entity tracks. Fixed, so prediction is
/// allocation-free in the loop.
pub const MAX_CHANNELS: usize = 32;

/// One channel's expectation.
#[derive(Clone, Copy, Debug, Default)]
struct Channel {
    /// a stable name hash, so channels can be looked up without strings
    key: u64,
    /// exponentially-weighted mean
    mean: f32,
    /// exponentially-weighted mean absolute deviation
    mad: f32,
    samples: u32,
}

/// What the entity expected.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Prediction {
    pub key: u64,
    pub expected: f32,
    /// the typical deviation; how wrong the entity usually is here
    pub tolerance: f32,
    /// false when this channel has never been seen
    pub known: bool,
}

/// How wrong it was.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Surprise {
    pub key: u64,
    pub observed: f32,
    pub expected: f32,
    /// deviation in units of the channel's usual error. 0 is exactly as
    /// expected; 1 is a typical miss; 5 is something happening.
    pub sigmas: f32,
    pub novel: bool,
}

impl Surprise {
    /// Is this worth waking cognition for?
    ///
    /// Three sigmas, or anything novel. The novelty clause is the one
    /// that matters on a body: the first time a leg stalls, there is no
    /// history to be three sigmas away from.
    pub fn is_salient(&self) -> bool {
        self.novel || self.sigmas >= 3.0
    }

    /// Large enough that even a deliberate tier is not enough.
    pub fn is_profound(&self) -> bool {
        self.novel || self.sigmas >= 8.0
    }
}

/// The entity's forward model.
pub struct Forward {
    channels: Vec<Channel>,
    /// how fast expectations move. Low means a long memory and a
    /// correspondingly slow adaptation to a body that has genuinely
    /// changed; high means the entity is surprised by nothing because it
    /// has already accepted whatever just happened as normal.
    alpha: f32,
    /// total surprises above the salience threshold, for telemetry
    pub salient_count: u64,
    pub observations: u64,
}

impl Default for Forward {
    fn default() -> Forward {
        Forward::new()
    }
}

impl Forward {
    pub fn new() -> Forward {
        Forward {
            channels: Vec::new(),
            alpha: 0.1,
            salient_count: 0,
            observations: 0,
        }
    }

    pub fn with_alpha(alpha: f32) -> Forward {
        Forward {
            alpha: alpha.clamp(0.001, 0.9),
            ..Forward::new()
        }
    }

    pub fn known_channels(&self) -> usize {
        self.channels.len()
    }

    /// A stable 64-bit key for a channel name. FNV-1a: tiny, fast, and
    /// deterministic across platforms, which retrieval and the proof
    /// harness both need.
    pub fn key_of(name: &str) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in name.as_bytes() {
            h ^= *b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01B3);
        }
        h
    }

    pub fn predict(&self, key: u64) -> Prediction {
        match self.channels.iter().find(|c| c.key == key) {
            Some(c) => Prediction {
                key,
                expected: c.mean,
                // The tolerance floor is *relative*, and that matters. A
                // channel that has read exactly 7.4 V two hundred times
                // has an observed deviation of zero, and dividing by a
                // tiny absolute floor would make a 10 mV ripple read as
                // ten sigmas — an entity permanently alarmed by a
                // perfectly healthy power rail. One percent of the
                // reading is the honest claim: no sensor on a robot is
                // better than that, so no deviation smaller than that is
                // evidence of anything.
                tolerance: c.mad.max(c.mean.abs() * 0.01).max(1e-3),
                known: c.samples >= 3,
            },
            None => Prediction {
                key,
                expected: 0.0,
                tolerance: 1.0,
                known: false,
            },
        }
    }

    /// Observe a value, returning how surprising it was, and fold it in.
    ///
    /// Note the order: surprise is computed against the expectation the
    /// entity *held*, and only then does the expectation move. Updating
    /// first would mean the entity is never surprised, which is a bug
    /// that looks like serenity.
    pub fn observe(&mut self, key: u64, value: f32) -> Surprise {
        let p = self.predict(key);
        // Novel means *the first time*, not "not yet confident". The
        // distinction is not pedantic: a channel needs a few samples
        // before it has any idea of its own error, and reporting each of
        // those as novel means the entity announces the same new thing
        // three times running — which then damps its own salience for
        // the occasion that actually matters.
        let first_time = !self.channels.iter().any(|c| c.key == key);
        let deviation = (value - p.expected).abs();
        let sigmas = if p.known {
            deviation / p.tolerance
        } else {
            // building a baseline. Not surprised, because there is
            // nothing yet to be surprised against.
            0.0
        };
        let s = Surprise {
            key,
            observed: value,
            expected: p.expected,
            sigmas,
            novel: first_time,
        };

        // fold in
        match self.channels.iter_mut().find(|c| c.key == key) {
            Some(c) => {
                let err = value - c.mean;
                c.mean += self.alpha * err;
                c.mad += self.alpha * (err.abs() - c.mad);
                c.samples = c.samples.saturating_add(1);
            }
            None => {
                if self.channels.len() < MAX_CHANNELS {
                    self.channels.push(Channel {
                        key,
                        mean: value,
                        mad: 0.0,
                        samples: 1,
                    });
                }
                // at capacity, a new channel is simply not tracked. It
                // stays permanently novel and therefore permanently
                // salient, which is noisy but never silently wrong.
            }
        }

        self.observations = self.observations.saturating_add(1);
        if s.is_salient() {
            self.salient_count = self.salient_count.saturating_add(1);
        }
        s
    }

    pub fn observe_named(&mut self, name: &str, value: f32) -> Surprise {
        self.observe(Forward::key_of(name), value)
    }

    /// What fraction of observations were salient. A well-calibrated
    /// entity in a familiar situation should be low single digits; an
    /// entity that is surprised half the time is in a situation it does
    /// not understand and should say so.
    pub fn surprise_rate_pct(&self) -> u8 {
        if self.observations == 0 {
            return 0;
        }
        ((self.salient_count * 100) / self.observations).min(100) as u8
    }

    /// Forget a channel — used when a body is released and its
    /// proprioception no longer exists.
    pub fn forget(&mut self, key: u64) -> bool {
        let before = self.channels.len();
        self.channels.retain(|c| c.key != key);
        self.channels.len() != before
    }

    pub fn describe(&self) -> Text<96> {
        let mut t = Text::new();
        t.push_str("forward: ");
        t.push_u64(self.channels.len() as u64);
        t.push_str(" channels, surprise ");
        t.push_u64(self.surprise_rate_pct() as u64);
        t.push('%');
        t
    }
}
