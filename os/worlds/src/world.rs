//! BFS  LEVEL    Section 3.8 - the three worlds (self, human, internet)  [crate brainos-worlds]
//! BFS  PURPOSE  Defines the three worlds (self, human, internet), how far each is trusted, and the time- and size-limited Session needed before touching the internet
//! BFS  DEFINES  World, Posture, Session
//! BFS  USES     brainos: abi, memory
//! BFS  USED BY  entity/entity, host/worlds_run, nostd/lib, worlds/ingress, worlds/lib, worlds/tests/worlds_suite
//!
//! The three worlds and the posture the entity holds toward each.

use brainos_abi::text::Text;
use brainos_abi::time::{Duration, Tick};
use brainos_memory::provenance::Origin;

#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord)]
#[repr(u8)]
pub enum World {
    /// its own internals — state, bodies, cognitive resources
    Zelf = 0,
    /// the physical space and the people in it
    Human = 1,
    /// an alternate universe of information and services
    Internet = 2,
}

impl World {
    pub const ALL: [World; 3] = [World::Zelf, World::Human, World::Internet];

    pub const fn name(self) -> &'static str {
        match self {
            World::Zelf => "self",
            World::Human => "human",
            World::Internet => "internet",
        }
    }

    pub const fn posture(self) -> Posture {
        match self {
            World::Zelf => Posture::Trusted,
            World::Human => Posture::Consequential,
            World::Internet => Posture::Untrusted,
        }
    }

    pub const fn origin(self) -> Origin {
        match self {
            World::Zelf => Origin::Zelf,
            World::Human => Origin::Human,
            World::Internet => Origin::Internet,
        }
    }

    /// Must a session be opened before the entity may touch this world?
    /// Only the internet. The entity's own internals are always
    /// reachable, and the human world is where it lives.
    pub const fn needs_a_session(self) -> bool {
        matches!(self, World::Internet)
    }
}

/// How the entity holds itself toward a world.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Posture {
    /// its own; KIRA governs all mutation
    Trusted,
    /// every action gated, because nothing here is undoable
    Consequential,
    /// opt-in per session; every ingress filtered
    Untrusted,
}

/// One opened visit to an untrusted world.
///
/// Sessions are bounded in three ways at once — time, ingress count, and
/// bytes — because each bounds a different failure. Time bounds "the
/// entity forgot it had the door open"; count bounds a loop that fetches
/// forever; bytes bound a single response that is a denial of service.
#[derive(Clone, Copy, Debug)]
pub struct Session {
    pub world: World,
    pub opened_at: Tick,
    pub expires_at: Tick,
    /// why the human opened it; recorded, never consulted
    pub purpose_len: u16,
    pub max_ingress: u32,
    pub max_bytes: u64,
    ingress_count: u32,
    bytes: u64,
    closed: bool,
}

impl Session {
    /// Open a visit. There is no "open forever" constructor.
    pub fn open(world: World, now: Tick, ttl: u64, purpose: &str) -> Session {
        Session {
            world,
            opened_at: now,
            expires_at: now.plus(ttl),
            purpose_len: purpose.len().min(u16::MAX as usize) as u16,
            max_ingress: 64,
            max_bytes: 8 * 1024 * 1024,
            ingress_count: 0,
            bytes: 0,
            closed: false,
        }
    }

    pub fn bounded(mut self, max_ingress: u32, max_bytes: u64) -> Session {
        self.max_ingress = max_ingress;
        self.max_bytes = max_bytes;
        self
    }

    pub fn is_open(&self, now: Tick) -> bool {
        !self.closed
            && now.0 <= self.expires_at.0
            && self.ingress_count < self.max_ingress
            && self.bytes < self.max_bytes
    }

    /// Why it is not open, for the refusal.
    pub fn why_closed(&self, now: Tick) -> Option<&'static str> {
        if self.closed {
            Some("the session was closed")
        } else if now.0 > self.expires_at.0 {
            Some("the session expired")
        } else if self.ingress_count >= self.max_ingress {
            Some("the session's fetch budget is used up")
        } else if self.bytes >= self.max_bytes {
            Some("the session's byte budget is used up")
        } else {
            None
        }
    }

    /// Record one arrival. Returns false when it would exceed a bound —
    /// and the caller must then not deliver it, because a bound that is
    /// reported after the fact is not a bound.
    pub fn account(&mut self, bytes: u64, now: Tick) -> bool {
        if !self.is_open(now) {
            return false;
        }
        if self.bytes.saturating_add(bytes) > self.max_bytes {
            self.bytes = self.max_bytes;
            return false;
        }
        self.ingress_count += 1;
        self.bytes += bytes;
        true
    }

    pub fn close(&mut self) {
        self.closed = true;
    }

    pub fn ingress_count(&self) -> u32 {
        self.ingress_count
    }

    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    pub fn remaining(&self, now: Tick) -> Duration {
        Duration::from_micros(self.expires_at.0.saturating_sub(now.0))
    }

    pub fn describe(&self) -> Text<128> {
        let mut t = Text::new();
        t.push_str(self.world.name());
        t.push_str(" session ");
        t.push_u64(self.ingress_count as u64);
        t.push('/');
        t.push_u64(self.max_ingress as u64);
        t.push_str(" fetches, ");
        t.push_u64(self.bytes / 1024);
        t.push_str("KiB");
        if self.closed {
            t.push_str(" CLOSED");
        }
        t
    }
}
