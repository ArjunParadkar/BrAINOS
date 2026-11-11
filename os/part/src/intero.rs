//! BFS  LEVEL    Parts and nerves - every piece of hardware as a body part  [crate brainos-part]
//! BFS  PURPOSE  Interoceptor: decides which part readings are worth remembering (threshold crossings and faults) and summarises the rest
//! BFS  DEFINES  NotedKind, Noted, Interoceptor
//! BFS  USES     brainos: abi, body
//! BFS  USED BY  entity/entity, entity/entity/parts, part/lib
//!
//! Interoception — which readings become memory.
//!
//! A machine's parts report constantly: a GPU temperature every second
//! is 86,400 readings a day from one sense of one part. Writing them
//! all into the state graph would bury everything else the entity
//! knows, and nearly all of them say "same as before".
//!
//! The rule is the one the ten-day run already found for small bodies
//! (`docs/OS_STATUS.md`, "a small body loses recall, not attention"):
//! **keep the faults, summarise the routine.**
//!
//! | what happened | what is noted | salience |
//! |---|---|---|
//! | a vital crossed into a worse band | the crossing, with the value | 60 / 90 / 100 |
//! | a vital came back to nominal | the recovery | 40 |
//! | a vital keeps flipping bands | once: "is unstable" | 70 |
//! | nothing changed for a whole window | one summary: min, mean, max | 15 |
//!
//! Crossings into *critical* or *failed* are never rate-limited. The
//! flapping limit applies only to the boundary between nominal and
//! degraded, where a value hovering at a threshold would otherwise
//! write a memory on every reading.
//!
//! **This type does not write memory.** It returns `Noted` values. The
//! entity, which holds the only `MemoryHandle`, turns them into nodes
//! with the part as their provenance. What it keeps between calls is
//! one small running tally per sense — a minimum, a maximum, a sum, a
//! count — which is a summary in progress, not a history, and its size
//! is fixed by how many senses exist.

use alloc::string::String;
use alloc::vec::Vec;
use brainos_abi::ids::BodyId;
use brainos_abi::text::Text;
use brainos_abi::time::{Duration, Instant};
use brainos_body::vital::{Felt, Health, SENSE_NAME_MAX};
use core::fmt::Write;

/// What kind of thing was worth noting.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NotedKind {
    /// a vital moved to a worse band
    Worse { from: Health, to: Health },
    /// a vital returned to nominal
    Recovered { from: Health },
    /// a vital is flipping between bands; further flips are not noted
    /// until the window ends
    Unstable,
    /// a window passed with nothing else to say
    Summary,
}

/// Something worth remembering about a part.
#[derive(Clone, Debug)]
pub struct Noted {
    pub part: BodyId,
    pub sense: Text<SENSE_NAME_MAX>,
    pub kind: NotedKind,
    /// the sentence to remember
    pub text: String,
    /// how much it matters, 0..100; becomes the memory's salience
    pub salience: u8,
    pub at: Instant,
}

struct Track {
    part: BodyId,
    sense: Text<SENSE_NAME_MAX>,
    health: Option<Health>,
    n: u32,
    min: f64,
    max: f64,
    sum: f64,
    window_start: Instant,
    flips: u8,
    muted: bool,
}

/// The filter between what parts report and what the entity remembers.
pub struct Interoceptor {
    tracks: Vec<Track>,
    window: Duration,
    max_flips: u8,
}

impl Default for Interoceptor {
    fn default() -> Interoceptor {
        Interoceptor::new(Duration::from_secs(600), 4)
    }
}

impl Interoceptor {
    /// `window` is how often steady state is summarised. `max_flips` is
    /// how many nominal/degraded crossings one sense may note per
    /// window before it is called unstable and muted.
    pub fn new(window: Duration, max_flips: u8) -> Interoceptor {
        Interoceptor {
            tracks: Vec::new(),
            window,
            max_flips,
        }
    }

    pub fn tracked(&self) -> usize {
        self.tracks.len()
    }

    /// Forget a part that has left.
    pub fn release(&mut self, part: &BodyId) {
        self.tracks.retain(|t| t.part != *part);
    }

    fn track(&mut self, f: &Felt) -> &mut Track {
        let i = match self
            .tracks
            .iter()
            .position(|t| f.is(&t.part, t.sense.as_str()))
        {
            Some(i) => i,
            None => {
                self.tracks.push(Track {
                    part: f.part,
                    sense: f.sense,
                    health: None,
                    n: 0,
                    min: f64::INFINITY,
                    max: f64::NEG_INFINITY,
                    sum: 0.0,
                    window_start: f.at,
                    flips: 0,
                    muted: false,
                });
                self.tracks.len() - 1
            }
        };
        &mut self.tracks[i]
    }

    /// Hear one reading. `health` is the part's own judgement of it
    /// (`PartSchema::judge`), or `None` for a sense that is not a
    /// vital. `label` and `unit` are for the sentence.
    pub fn hear(&mut self, f: &Felt, health: Option<Health>, label: &str, unit: &str) -> Option<Noted> {
        let max_flips = self.max_flips;
        let t = self.track(f);
        if f.value.is_finite() {
            t.n = t.n.saturating_add(1);
            t.min = t.min.min(f.value);
            t.max = t.max.max(f.value);
            t.sum += f.value;
        }
        let now = health?;
        let before = t.health.unwrap_or(Health::Nominal);
        t.health = Some(now);
        if now == before {
            return None;
        }

        let serious = now >= Health::Critical || before >= Health::Critical;
        if !serious {
            t.flips = t.flips.saturating_add(1);
            if t.muted {
                return None;
            }
            if t.flips > max_flips {
                t.muted = true;
                let mut text = String::new();
                let _ = write!(
                    text,
                    "{label}: {} is unstable, crossing its warning threshold repeatedly (now {:.1}{unit})",
                    f.sense.as_str(),
                    f.value
                );
                return Some(Noted {
                    part: f.part,
                    sense: f.sense,
                    kind: NotedKind::Unstable,
                    text,
                    salience: 70,
                    at: f.at,
                });
            }
        }

        let mut text = String::new();
        let (kind, salience) = if now > before {
            let _ = write!(
                text,
                "{label}: {} became {} at {:.1}{unit} (was {})",
                f.sense.as_str(),
                now.name(),
                f.value,
                before.name()
            );
            (
                NotedKind::Worse { from: before, to: now },
                match now {
                    Health::Failed => 100,
                    Health::Critical => 90,
                    _ => 60,
                },
            )
        } else if now == Health::Nominal {
            let _ = write!(
                text,
                "{label}: {} recovered to nominal at {:.1}{unit} (was {})",
                f.sense.as_str(),
                f.value,
                before.name()
            );
            (NotedKind::Recovered { from: before }, 40)
        } else {
            // better, but not well: still worth knowing where it stands
            let _ = write!(
                text,
                "{label}: {} eased to {} at {:.1}{unit} (was {})",
                f.sense.as_str(),
                now.name(),
                f.value,
                before.name()
            );
            (NotedKind::Recovered { from: before }, 50)
        };
        Some(Noted {
            part: f.part,
            sense: f.sense,
            kind,
            text,
            salience,
            at: f.at,
        })
    }

    /// Close every window that has run its length, returning one
    /// summary per sense that had readings, and start the next. `label`
    /// gives the part's name for the sentence.
    pub fn summarise(&mut self, now: Instant, label: impl Fn(&BodyId) -> String) -> Vec<Noted> {
        let mut out = Vec::new();
        for t in &mut self.tracks {
            let elapsed = now.checked_since(t.window_start).unwrap_or(Duration::ZERO);
            if elapsed < self.window {
                continue;
            }
            if t.n > 0 {
                let mut text = String::new();
                let _ = write!(
                    text,
                    "{}: {} steady, {:.1} to {:.1} (mean {:.1}) over {} readings, {}",
                    label(&t.part),
                    t.sense.as_str(),
                    t.min,
                    t.max,
                    t.sum / t.n as f64,
                    t.n,
                    t.health.unwrap_or(Health::Nominal).name()
                );
                out.push(Noted {
                    part: t.part,
                    sense: t.sense,
                    kind: NotedKind::Summary,
                    text,
                    salience: 15,
                    at: now,
                });
            }
            t.n = 0;
            t.min = f64::INFINITY;
            t.max = f64::NEG_INFINITY;
            t.sum = 0.0;
            t.window_start = now;
            t.flips = 0;
            t.muted = false;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use brainos_body::vital::Band;

    fn id(b: u8) -> BodyId {
        BodyId::from_bytes([b; 32])
    }
    fn band() -> Band {
        Band::rising(85.0, 95.0)
    }
    fn hear(i: &mut Interoceptor, v: f64, at: u64) -> Option<Noted> {
        let f = Felt::new(id(1), "temp.c", v, Instant(at));
        i.hear(&f, Some(band().judge(v)), "gpu", "C")
    }

    #[test]
    fn steady_state_writes_nothing() {
        let mut i = Interoceptor::default();
        for t in 0..10_000u64 {
            assert!(hear(&mut i, 50.0 + (t % 5) as f64, t).is_none());
        }
        assert_eq!(i.tracked(), 1);
    }

    #[test]
    fn a_crossing_is_noted_once_with_the_value_and_recovery_is_noted_too() {
        let mut i = Interoceptor::default();
        assert!(hear(&mut i, 60.0, 1).is_none());
        let n = hear(&mut i, 88.0, 2).expect("crossing into degraded");
        assert_eq!(n.kind, NotedKind::Worse { from: Health::Nominal, to: Health::Degraded });
        assert!(n.text.contains("88.0C") && n.text.contains("degraded"), "{}", n.text);
        assert_eq!(n.salience, 60);
        assert!(hear(&mut i, 89.0, 3).is_none(), "the same band is not news");
        let c = hear(&mut i, 96.0, 4).unwrap();
        assert_eq!(c.salience, 90);
        let r = hear(&mut i, 70.0, 5).unwrap();
        assert_eq!(r.kind, NotedKind::Recovered { from: Health::Critical });
    }

    #[test]
    fn a_first_reading_that_is_already_bad_is_noted() {
        let mut i = Interoceptor::default();
        assert!(hear(&mut i, 97.0, 1).is_some());
    }

    #[test]
    fn a_value_hovering_at_the_threshold_is_called_unstable_once_then_muted() {
        let mut i = Interoceptor::new(Duration::from_secs(600), 4);
        let mut notes = Vec::new();
        for t in 0..200u64 {
            let v = if t % 2 == 0 { 84.0 } else { 86.0 };
            if let Some(n) = hear(&mut i, v, t) {
                notes.push(n.kind);
            }
        }
        assert_eq!(notes.len(), 5, "{notes:?}");
        assert_eq!(notes.last(), Some(&NotedKind::Unstable));
        assert_eq!(notes.iter().filter(|k| **k == NotedKind::Unstable).count(), 1);
    }

    #[test]
    fn a_critical_crossing_is_never_muted() {
        let mut i = Interoceptor::new(Duration::from_secs(600), 2);
        for t in 0..50u64 {
            hear(&mut i, if t % 2 == 0 { 84.0 } else { 86.0 }, t);
        }
        let n = hear(&mut i, 99.0, 60).expect("critical must get through a muted sense");
        assert_eq!(n.salience, 90);
    }

    #[test]
    fn a_window_of_routine_becomes_one_summary_and_the_window_restarts() {
        let mut i = Interoceptor::new(Duration::from_secs(10), 4);
        for t in 0..100u64 {
            hear(&mut i, 40.0 + (t % 10) as f64, t * 1_000);
        }
        let none = i.summarise(Instant(5_000_000), |_| "gpu".to_string());
        assert!(none.is_empty(), "summarised before the window ended");
        let s = i.summarise(Instant(10_000_000), |_| "gpu".to_string());
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].kind, NotedKind::Summary);
        assert!(s[0].text.contains("40.0 to 49.0") && s[0].text.contains("100 readings"), "{}", s[0].text);
        assert_eq!(s[0].salience, 15);
        assert!(i.summarise(Instant(10_000_001), |_| "gpu".to_string()).is_empty());
    }

    #[test]
    fn a_sense_that_is_not_a_vital_is_only_ever_summarised() {
        let mut i = Interoceptor::new(Duration::from_secs(1), 4);
        for t in 0..10u64 {
            let f = Felt::new(id(2), "util", 0.9, Instant(t));
            assert!(i.hear(&f, None, "gpu", "").is_none());
        }
        assert_eq!(i.summarise(Instant(2_000_000), |_| "gpu".to_string()).len(), 1);
    }

    #[test]
    fn what_it_keeps_is_bounded_by_the_number_of_senses_not_readings() {
        let mut i = Interoceptor::default();
        for t in 0..50_000u64 {
            for p in 0..4u8 {
                let f = Felt::new(id(p), "temp.c", 50.0, Instant(t));
                i.hear(&f, Some(Health::Nominal), "x", "C");
            }
        }
        assert_eq!(i.tracked(), 4);
        i.release(&id(0));
        assert_eq!(i.tracked(), 3);
    }
}
