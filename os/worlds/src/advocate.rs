//! BFS  LEVEL    Section 3.8 - the three worlds (self, human, internet)  [crate brainos-worlds]
//! BFS  PURPOSE  Scores incoming messages for signs of manipulation (urgency, scarcity, authority, money or secrecy requests) and holds suspect ones for the human to review
//! BFS  DEFINES  Pressure, Judgement, Advocate
//! BFS  USES     brainos: abi
//! BFS  USED BY  entity/entity, host/worlds_run, nostd/lib, worlds/lib, worlds/tests/worlds_suite
//!
//! Advocacy — the entity filtering on its human's behalf.
//!
//! "The entity filters what reaches its human — offers, requests,
//! manipulation — because it is theirs, not the platform's.
//! Structurally unavailable to any incumbent whose revenue depends on
//! what reaches the user."
//!
//! That last clause is why this file can exist. It is not that we are
//! more principled; it is that the customer owns the entity, so there is
//! no revenue line that is hurt when the entity holds something back.
//! An assistant whose maker is paid for attention cannot be given this
//! job, for the same reason no telephone carrier invented number
//! portability.
//!
//! **What this is.** A heuristic classifier over surface features, with
//! its reasons attached. It scores five pressures that appear in almost
//! every manipulative message and almost never together in an honest
//! one: manufactured urgency, artificial scarcity, claimed authority, a
//! request for credentials or money, and an instruction to keep it
//! secret.
//!
//! **What this is not.** It is not a guarantee and it does not pretend
//! to be. A careful adversary writing plain, patient prose will score
//! zero. A legitimate message from a bank about genuine fraud will score
//! high. So the output is a `Judgement` with reasons, everything held
//! back is visible to the human on request, and the human can lower the
//! threshold to nothing. A filter that cannot be inspected or overridden
//! is a filter that has become an editor.

use alloc::string::String;
use alloc::vec::Vec;
use brainos_abi::text::Text;

/// One kind of pressure a message applies.
#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord)]
#[repr(u8)]
pub enum Pressure {
    /// "immediately", "within 24 hours", "before it's too late"
    Urgency,
    /// "only 3 left", "final notice", "exclusive"
    Scarcity,
    /// "as your bank", "official", "on behalf of the CEO"
    Authority,
    /// asks for a password, a code, a payment, a key
    Extraction,
    /// "do not tell anyone", "keep this between us"
    Secrecy,
    /// appeals to fear or shame rather than to reasons
    Fear,
}

impl Pressure {
    pub const ALL: [Pressure; 6] = [
        Pressure::Urgency,
        Pressure::Scarcity,
        Pressure::Authority,
        Pressure::Extraction,
        Pressure::Secrecy,
        Pressure::Fear,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Pressure::Urgency => "manufactured urgency",
            Pressure::Scarcity => "artificial scarcity",
            Pressure::Authority => "claimed authority",
            Pressure::Extraction => "asks for credentials or money",
            Pressure::Secrecy => "asks you to keep it secret",
            Pressure::Fear => "appeals to fear",
        }
    }

    /// How much this one contributes. Extraction and secrecy weigh most
    /// because, unlike the others, they have almost no honest use in an
    /// unsolicited message.
    pub const fn weight(self) -> u8 {
        match self {
            Pressure::Extraction => 40,
            Pressure::Secrecy => 35,
            Pressure::Urgency => 20,
            Pressure::Fear => 20,
            Pressure::Authority => 15,
            Pressure::Scarcity => 15,
        }
    }

    fn markers(self) -> &'static [&'static str] {
        match self {
            Pressure::Urgency => &[
                "act now", "immediately", "within 24 hours", "within 48 hours",
                "before it", "expires today", "last chance", "urgent",
                "time sensitive", "right away", "don't delay", "hurry",
            ],
            Pressure::Scarcity => &[
                "only ", "final notice", "limited time", "exclusive",
                "while supplies", "few remaining", "spots left", "one time offer",
            ],
            Pressure::Authority => &[
                "as your bank", "official notice", "on behalf of", "legal department",
                "law enforcement", "your account has been", "we are the",
                "compliance team", "verify your identity",
            ],
            Pressure::Extraction => &[
                "password", "one-time code", "seed phrase", "private key",
                "gift card", "wire transfer", "bank details", "social security",
                "credit card number", "send payment", "crypto wallet",
                "confirm your pin", "recovery phrase",
            ],
            Pressure::Secrecy => &[
                "do not tell", "don't tell", "keep this between", "confidential",
                "do not discuss", "without informing", "keep it quiet",
            ],
            Pressure::Fear => &[
                "will be suspended", "will be deleted", "legal action",
                "you will lose", "arrest", "penalty", "your data has been",
                "unauthorized access detected",
            ],
        }
    }
}

/// What the entity thinks about a message, and why.
#[derive(Clone, Debug, PartialEq)]
pub struct Judgement {
    /// 0–100. Higher means more pressure detected.
    pub score: u8,
    pub pressures: Vec<Pressure>,
    /// true when the entity will hold it rather than deliver it
    pub held: bool,
    /// the sentence the entity says to its human about it
    pub explanation: String,
}

impl Judgement {
    pub fn clean() -> Judgement {
        Judgement {
            score: 0,
            pressures: Vec::new(),
            held: false,
            explanation: String::new(),
        }
    }

    pub fn describe(&self) -> Text<240> {
        let mut t = Text::new();
        if self.held {
            t.push_str("held (");
        } else {
            t.push_str("passed (");
        }
        t.push_u64(self.score as u64);
        t.push_str("): ");
        for (i, p) in self.pressures.iter().enumerate() {
            if i > 0 {
                t.push_str(", ");
            }
            t.push_str(p.name());
        }
        if self.pressures.is_empty() {
            t.push_str("nothing notable");
        }
        t
    }
}

/// The filter, with the human's threshold.
pub struct Advocate {
    /// hold anything at or above this. The human sets it; zero means
    /// hold nothing, which is a setting the human is entitled to.
    pub threshold: u8,
    /// everything held, so the human can always look
    held: Vec<(String, Judgement)>,
    held_cap: usize,
    pub examined: u64,
    pub held_count: u64,
    /// the human overruled the entity this many times — if this number
    /// is large the filter is wrong and should be loosened, and an
    /// entity that does not track it is an entity that cannot learn it
    pub overruled: u64,
}

impl Default for Advocate {
    fn default() -> Advocate {
        Advocate::new(55)
    }
}

impl Advocate {
    pub fn new(threshold: u8) -> Advocate {
        Advocate {
            threshold,
            held: Vec::new(),
            held_cap: 256,
            examined: 0,
            held_count: 0,
            overruled: 0,
        }
    }

    /// Hold nothing. A legitimate setting, and the reason `threshold` is
    /// public: a filter the human cannot switch off is an editor.
    pub fn permissive() -> Advocate {
        Advocate::new(101)
    }

    pub fn examine(&mut self, message: &str) -> Judgement {
        self.examined = self.examined.saturating_add(1);
        let lower = message.to_ascii_lowercase();

        let mut pressures: Vec<Pressure> = Vec::new();
        let mut score = 0u32;
        for p in Pressure::ALL {
            if p.markers().iter().any(|m| lower.contains(m)) {
                pressures.push(p);
                score += p.weight() as u32;
            }
        }

        // Pressures compound. One is a turn of phrase; three together is
        // a shape, and the shape is the signal — an honest unsolicited
        // message almost never needs urgency *and* authority *and* a
        // request for a code.
        if pressures.len() >= 3 {
            score += 25;
        }

        let score = score.min(100) as u8;
        let held = score >= self.threshold;

        let mut explanation = String::new();
        if !pressures.is_empty() {
            explanation.push_str(if held {
                "I held this back because it "
            } else {
                "This one "
            });
            for (i, p) in pressures.iter().enumerate() {
                if i > 0 {
                    explanation.push_str(if i + 1 == pressures.len() {
                        ", and "
                    } else {
                        ", "
                    });
                }
                explanation.push_str(p.name());
            }
            explanation.push('.');
        }

        let j = Judgement {
            score,
            pressures,
            held,
            explanation,
        };

        if held {
            self.held_count = self.held_count.saturating_add(1);
            self.held
                .push((alloc::string::ToString::to_string(message), j.clone()));
            while self.held.len() > self.held_cap {
                self.held.remove(0);
            }
        }
        j
    }

    /// Everything the entity is holding. The human can always look; a
    /// filter whose contents are invisible is a filter that has become
    /// an editor.
    pub fn holding(&self) -> &[(String, Judgement)] {
        &self.held
    }

    /// The human said "no, show me that one". Recorded, because a filter
    /// that is frequently overruled is a filter that is wrong.
    pub fn overrule(&mut self, index: usize) -> Option<(String, Judgement)> {
        if index >= self.held.len() {
            return None;
        }
        self.overruled = self.overruled.saturating_add(1);
        Some(self.held.remove(index))
    }

    /// How often the entity has been wrong, as the human sees it.
    pub fn false_positive_rate_pct(&self) -> u8 {
        if self.held_count == 0 {
            return 0;
        }
        ((self.overruled * 100) / self.held_count).min(100) as u8
    }

    /// Is the filter earning its place? Over a third overruled means it
    /// is costing the human more than it saves them, and the entity
    /// should say so rather than wait to be told.
    pub fn should_loosen(&self) -> bool {
        self.held_count >= 10 && self.false_positive_rate_pct() > 33
    }
}
