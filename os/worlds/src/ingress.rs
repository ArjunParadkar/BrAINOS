//! BFS  LEVEL    Section 3.8 - the three worlds (self, human, internet)  [crate brainos-worlds]
//! BFS  PURPOSE  Checks everything arriving from outside for size and encoding, then wraps it in Tainted<T> so its origin travels with it and cannot be dropped
//! BFS  DEFINES  Tainted, Verdict, Ingress
//! BFS  USES     this crate: world | brainos: abi, memory
//! BFS  USED BY  entity/entity, nostd/lib, worlds/lib, worlds/tests/worlds_suite
//!
//! Ingress — everything from outside, tainted at the door.
//!
//! The rule is mechanical: nothing crosses from an untrusted world into
//! the entity except as a `Tainted<T>`, and a `Tainted<T>` cannot be
//! unwrapped without naming the origin you are stamping on it. That
//! makes "where did this come from" a thing the compiler asks about
//! rather than a thing a reviewer has to notice.
//!
//! What the filter does **not** try to do is decide whether the content
//! is *true*. That is cognition's job and it is allowed to be wrong
//! about it. The filter decides whether the content is *structurally
//! admissible*: the right size, the right encoding, not a control
//! sequence, not an attempt to look like something from a different
//! world. A filter that tried to judge truth would be a filter that
//! could be argued with.

use alloc::string::String;
use alloc::vec::Vec;
use brainos_memory::provenance::{Origin, Provenance};
use brainos_abi::time::Tick;
use crate::world::World;

/// Something from outside, carrying where it came from.
///
/// Not `Deref`, deliberately. Reading the content is an explicit call,
/// so every place that treats external data as data is visible in a
/// grep.
pub struct Tainted<T> {
    value: T,
    pub world: World,
    pub at: Tick,
    /// how many filters have looked at it
    pub inspected: u8,
}

impl<T> Tainted<T> {
    pub fn new(value: T, world: World, at: Tick) -> Tainted<T> {
        Tainted {
            value,
            world,
            at,
            inspected: 0,
        }
    }

    /// Look at the content, acknowledging that it is untrusted.
    pub fn as_untrusted(&self) -> &T {
        &self.value
    }

    /// Take the content, producing the provenance that must travel with
    /// it. There is no way to get the value without also getting this,
    /// which is what keeps origin attached all the way into memory.
    pub fn into_parts(self) -> (T, Provenance) {
        let origin = self.world.origin();
        (
            self.value,
            Provenance::from_world(origin, self.at),
        )
    }

    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Tainted<U> {
        Tainted {
            value: f(self.value),
            world: self.world,
            at: self.at,
            inspected: self.inspected,
        }
    }

    /// The confidence ceiling this taint imposes.
    pub fn ceiling(&self) -> u8 {
        self.world.origin().confidence_ceiling()
    }

    pub fn is_untrusted(&self) -> bool {
        self.world.origin().is_untrusted()
    }
}

/// Debug prints the *shape* of a tainted value, never its content.
///
/// A debug line is a place untrusted text ends up in a log, and a log is
/// read by a human on a terminal. Printing the content would undo the
/// control-character check this module just performed.
impl<T> core::fmt::Debug for Tainted<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "Tainted<{}>(from {}, at {})",
            core::any::type_name::<T>(),
            self.world.name(),
            self.at
        )
    }
}

/// Why something was not admitted.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Verdict {
    Admitted,
    /// larger than the limb accepts
    TooLarge { bytes: usize, limit: usize },
    /// not valid UTF-8
    NotText,
    /// contains control characters that would corrupt a display or a log
    ControlCharacters,
    /// contains a sequence that imitates the entity's own framing — the
    /// classic "pretend to be a system message" attempt
    ImitatesTheEntity,
    /// the session that would have carried it is not open
    NoSession,
    /// nothing at all
    Empty,
}

impl Verdict {
    pub fn admitted(&self) -> bool {
        matches!(self, Verdict::Admitted)
    }

    pub const fn name(self) -> &'static str {
        match self {
            Verdict::Admitted => "admitted",
            Verdict::TooLarge { .. } => "too-large",
            Verdict::NotText => "not-text",
            Verdict::ControlCharacters => "control-characters",
            Verdict::ImitatesTheEntity => "imitates-the-entity",
            Verdict::NoSession => "no-session",
            Verdict::Empty => "empty",
        }
    }
}

/// Sequences that only ever appear when something is trying to look
/// like it came from inside the entity.
///
/// This list is short and it is not a security boundary on its own — the
/// boundary is that content from an untrusted world cannot become a
/// policy-admissible memory at all. What this catches is the *display*
/// attack: text that renders in a log or on a screen as though the
/// entity itself said it.
const IMITATIONS: [&str; 8] = [
    "kira-grant|",
    "kira-req|",
    "brainos/v1/",
    "[system]",
    "<|system|>",
    "granted:",
    "level-0 drive",
    "\u{001b}[",
];

pub struct Ingress {
    pub max_bytes: usize,
    /// how many arrivals have been refused, by reason
    pub refusals: Vec<(Verdict, u32)>,
    pub admitted: u64,
}

impl Default for Ingress {
    fn default() -> Ingress {
        Ingress::new(1024 * 1024)
    }
}

impl Ingress {
    pub fn new(max_bytes: usize) -> Ingress {
        Ingress {
            max_bytes,
            refusals: Vec::new(),
            admitted: 0,
        }
    }

    fn note(&mut self, v: Verdict) {
        match self.refusals.iter_mut().find(|(r, _)| *r == v) {
            Some((_, n)) => *n += 1,
            None => self.refusals.push((v, 1)),
        }
    }

    pub fn refused_count(&self) -> u32 {
        self.refusals.iter().map(|(_, n)| n).sum()
    }

    /// Inspect a byte arrival from an untrusted world.
    pub fn inspect(&mut self, bytes: &[u8], world: World, at: Tick) -> Result<Tainted<String>, Verdict> {
        if bytes.is_empty() {
            self.note(Verdict::Empty);
            return Err(Verdict::Empty);
        }
        if bytes.len() > self.max_bytes {
            let v = Verdict::TooLarge {
                bytes: bytes.len(),
                limit: self.max_bytes,
            };
            self.note(v);
            return Err(v);
        }
        let Ok(text) = core::str::from_utf8(bytes) else {
            self.note(Verdict::NotText);
            return Err(Verdict::NotText);
        };
        self.inspect_text(text, world, at)
    }

    pub fn inspect_text(
        &mut self,
        text: &str,
        world: World,
        at: Tick,
    ) -> Result<Tainted<String>, Verdict> {
        if text.is_empty() {
            self.note(Verdict::Empty);
            return Err(Verdict::Empty);
        }
        // control characters other than the three that are genuinely
        // text: newline, carriage return, tab
        if text
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\r' && c != '\t')
        {
            self.note(Verdict::ControlCharacters);
            return Err(Verdict::ControlCharacters);
        }
        let lower = text.to_ascii_lowercase();
        if IMITATIONS.iter().any(|m| lower.contains(m)) {
            self.note(Verdict::ImitatesTheEntity);
            return Err(Verdict::ImitatesTheEntity);
        }

        self.admitted = self.admitted.saturating_add(1);
        Ok(Tainted::new(
            alloc::string::ToString::to_string(text),
            world,
            at,
        ))
    }

    /// Inspect something arriving inside an open session, accounting for
    /// the session's budgets at the same time.
    pub fn inspect_in_session(
        &mut self,
        bytes: &[u8],
        session: &mut crate::world::Session,
        at: Tick,
    ) -> Result<Tainted<String>, Verdict> {
        if !session.is_open(at) {
            self.note(Verdict::NoSession);
            return Err(Verdict::NoSession);
        }
        // account first: a response that blows the budget must not be
        // delivered, and a budget checked afterwards is not a budget
        if !session.account(bytes.len() as u64, at) {
            self.note(Verdict::NoSession);
            return Err(Verdict::NoSession);
        }
        self.inspect(bytes, session.world, at)
    }

    /// Provenance for something the entity itself produced. Here for
    /// symmetry — every memory gets its origin from exactly one place.
    pub fn own(at: Tick) -> Provenance {
        Provenance::own(at)
    }

    /// Provenance for a model's output. A cognitive resource is a
    /// resource, not an authority: its output is tainted like anything
    /// else that arrives from outside the entity's own reasoning.
    pub fn from_model(at: Tick) -> Provenance {
        Provenance::from_world(Origin::Model, at)
    }
}
