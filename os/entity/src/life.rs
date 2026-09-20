//! A life, as distinct from a session.
//!
//! Today's AI is a function: input, output, forget. Every deployment
//! starts from zero. The thing that makes a mind a *someone* — that it
//! persists, that things happen to it, that it carries forward what it
//! has done — is what this file records.
//!
//! `Life` is deliberately not a log. It is a small set of counters that
//! survive every wake, live in the state graph, and answer one question
//! the whole company rests on: *is this entity better than it was?*
//! Entity age against task success is the curve Phase 2 publishes, and
//! this is where the numbers come from.
//!
//! The counters are boring on purpose. A metric that is easy to move
//! without getting better is a metric that will be moved without getting
//! better.

use alloc::string::String;
use alloc::vec::Vec;
use brainos_abi::codec::{Decode, Decoder, Encode, Encoder};
use brainos_abi::error::Result;
use brainos_abi::ids::BodyId;
use brainos_abi::text::Text;
use brainos_abi::time::Tick;

/// One embodiment: from wake to sleep.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Span {
    pub woke_at: Tick,
    pub slept_at: Tick,
    /// how many bodies were held during this span, at most, at once
    pub peak_bodies: u16,
    pub actions_granted: u32,
    pub actions_denied: u32,
    /// how many times a reflex stopped a body
    pub stops: u32,
    /// how many consolidation passes ran
    pub dreams: u16,
    /// ended cleanly, rather than by power loss
    pub clean: bool,
}

impl Span {
    pub fn duration(&self) -> u64 {
        self.slept_at.since(self.woke_at)
    }

    /// Of the actions this entity proposed, how many were permitted?
    ///
    /// Not a safety metric — a high refusal rate can mean a careful gate
    /// or a confused entity, and the two look identical here. It is a
    /// *calibration* metric: an entity that proposes things it cannot do
    /// is an entity whose model of its own body is wrong.
    pub fn calibration_pct(&self) -> u8 {
        let total = self.actions_granted as u64 + self.actions_denied as u64;
        if total == 0 {
            return 100;
        }
        ((self.actions_granted as u64 * 100) / total) as u8
    }
}

impl Encode for Span {
    fn encode(&self, e: &mut Encoder) {
        self.woke_at.encode(e);
        self.slept_at.encode(e);
        e.u16(self.peak_bodies);
        e.u32(self.actions_granted);
        e.u32(self.actions_denied);
        e.u32(self.stops);
        e.u16(self.dreams);
        e.bool(self.clean);
    }
}

impl Decode for Span {
    fn decode(d: &mut Decoder) -> Result<Span> {
        Ok(Span {
            woke_at: Tick::decode(d)?,
            slept_at: Tick::decode(d)?,
            peak_bodies: d.u16()?,
            actions_granted: d.u32()?,
            actions_denied: d.u32()?,
            stops: d.u32()?,
            dreams: d.u16()?,
            clean: d.bool()?,
        })
    }
}

/// Everything that has happened to this entity, ever.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Life {
    /// how many times it has woken
    pub wakes: u32,
    /// the tick it first woke. Never resets. This is the entity's age,
    /// and it is the x-axis of the published curve.
    pub born_at: Tick,
    /// every body it has ever inhabited, in order of first acquisition
    pub bodies_ever: Vec<BodyId>,
    /// the last few spans, for the entity to reason about recent life
    recent: Vec<Span>,
    recent_cap: usize,
    /// lifetime totals
    pub total_granted: u64,
    pub total_denied: u64,
    pub total_stops: u64,
    pub total_dreams: u64,
    /// spans that ended by power loss rather than cleanly. A body that
    /// loses power constantly is a real fact about a robot, and an
    /// entity that does not notice it is one that keeps being surprised.
    pub unclean_wakes: u32,
}

impl Life {
    pub fn begin(at: Tick) -> Life {
        Life {
            wakes: 0,
            born_at: at,
            bodies_ever: Vec::new(),
            recent: Vec::new(),
            recent_cap: 32,
            total_granted: 0,
            total_denied: 0,
            total_stops: 0,
            total_dreams: 0,
            unclean_wakes: 0,
        }
    }

    /// Ticks since it first woke. The entity's age.
    pub fn age(&self, now: Tick) -> u64 {
        now.since(self.born_at)
    }

    pub fn saw_body(&mut self, body: BodyId) -> bool {
        if self.bodies_ever.contains(&body) {
            return false;
        }
        self.bodies_ever.push(body);
        true
    }

    pub fn record(&mut self, span: Span) {
        self.wakes = self.wakes.saturating_add(1);
        self.total_granted = self.total_granted.saturating_add(span.actions_granted as u64);
        self.total_denied = self.total_denied.saturating_add(span.actions_denied as u64);
        self.total_stops = self.total_stops.saturating_add(span.stops as u64);
        self.total_dreams = self.total_dreams.saturating_add(span.dreams as u64);
        if !span.clean {
            self.unclean_wakes = self.unclean_wakes.saturating_add(1);
        }
        self.recent.push(span);
        while self.recent.len() > self.recent_cap {
            self.recent.remove(0);
        }
    }

    pub fn recent(&self) -> &[Span] {
        &self.recent
    }

    /// Where the cognition counter was when this entity last stopped.
    ///
    /// `None` for an entity that has never completed a span — a first
    /// waking, or one exported while awake.
    pub fn last_stopped_at(&self) -> Option<Tick> {
        self.recent.last().map(|s| s.slept_at)
    }

    /// Is this entity better calibrated than it was?
    ///
    /// Compares the first and last thirds of the remembered spans. The
    /// answer is `None` when there is not enough history to say, which
    /// is the honest answer more often than people writing metrics like
    /// to admit.
    pub fn calibration_trend(&self) -> Option<(u8, u8)> {
        if self.recent.len() < 6 {
            return None;
        }
        let third = self.recent.len() / 3;
        let early: Vec<&Span> = self.recent.iter().take(third).collect();
        let late: Vec<&Span> = self.recent.iter().rev().take(third).collect();
        Some((mean_calibration(&early), mean_calibration(&late)))
    }

    /// The fraction of spans that ended in power loss.
    pub fn unclean_pct(&self) -> u8 {
        if self.wakes == 0 {
            return 0;
        }
        ((self.unclean_wakes as u64 * 100) / self.wakes as u64) as u8
    }

    /// The line the entity says about itself.
    pub fn describe(&self, now: Tick) -> Text<240> {
        let mut t = Text::new();
        t.push_str("alive ");
        t.push_u64(self.age(now));
        t.push_str(" ticks across ");
        t.push_u64(self.wakes as u64);
        t.push_str(" wakes, ");
        t.push_u64(self.bodies_ever.len() as u64);
        t.push_str(" bodies ever, ");
        t.push_u64(self.total_granted);
        t.push_str(" acts");
        if self.unclean_wakes > 0 {
            t.push_str(", ");
            t.push_u64(self.unclean_pct() as u64);
            t.push_str("% unclean");
        }
        t
    }

    /// The line written into the state graph at every sleep.
    pub fn render(&self, now: Tick) -> String {
        alloc::string::ToString::to_string(self.describe(now).as_str())
    }
}

fn mean_calibration(spans: &[&Span]) -> u8 {
    if spans.is_empty() {
        return 100;
    }
    let sum: u32 = spans.iter().map(|s| s.calibration_pct() as u32).sum();
    (sum / spans.len() as u32) as u8
}

impl Encode for Life {
    fn encode(&self, e: &mut Encoder) {
        e.u32(self.wakes);
        self.born_at.encode(e);
        e.seq(self.bodies_ever.len());
        for b in &self.bodies_ever {
            b.encode(e);
        }
        e.seq(self.recent.len());
        for s in &self.recent {
            s.encode(e);
        }
        e.u64(self.total_granted);
        e.u64(self.total_denied);
        e.u64(self.total_stops);
        e.u64(self.total_dreams);
        e.u32(self.unclean_wakes);
    }
}

impl Decode for Life {
    fn decode(d: &mut Decoder) -> Result<Life> {
        let wakes = d.u32()?;
        let born_at = Tick::decode(d)?;
        let n = d.seq()?;
        let mut bodies_ever = Vec::new();
        for _ in 0..n {
            bodies_ever.push(BodyId::decode(d)?);
        }
        let n = d.seq()?;
        let mut recent = Vec::new();
        for _ in 0..n {
            recent.push(Span::decode(d)?);
        }
        Ok(Life {
            wakes,
            born_at,
            bodies_ever,
            recent,
            recent_cap: 32,
            total_granted: d.u64()?,
            total_denied: d.u64()?,
            total_stops: d.u64()?,
            total_dreams: d.u64()?,
            unclean_wakes: d.u32()?,
        })
    }
}
