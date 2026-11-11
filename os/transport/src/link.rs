//! BFS  LEVEL    Foundation - the wire (frames, sessions, links)  [crate brainos-transport]
//! BFS  PURPOSE  Tracks when the other side was last heard (good, quiet, lost, closed) plus ping timing; reports silence but never stops a body itself
//! BFS  DEFINES  Health, LinkStats, Link
//! BFS  USES     brainos: abi
//! BFS  USED BY  transport/lib, transport/tests/transport_suite
//!
//! Link health.
//!
//! This layer answers one question — *when did I last hear from the
//! other side* — and refuses to answer any others. In particular it
//! never decides to stop a body. Deciding is not a nerve's job, and a
//! transport that stops actuators has become a safety layer with no
//! envelope and no audit trail.
//!
//! What it does provide is the number `brainos-rt` needs: `silence`, so
//! a reflex can compare it against its envelope's `link_grace` and make
//! the decision in the place where decisions have consequences attached.

use brainos_abi::text::Text;
use brainos_abi::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Health {
    /// heard from recently
    Good,
    /// nothing for a while, but not yet past the grace period
    Quiet,
    /// past the grace period
    Lost,
    /// deliberately shut
    Closed,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct LinkStats {
    pub sent: u64,
    pub received: u64,
    pub refused: u64,
    pub replays: u64,
    /// round-trip time of the last ping/pong
    pub rtt: Duration,
    /// the worst round trip seen
    pub worst_rtt: Duration,
    /// pings sent with no pong
    pub unanswered: u32,
}

/// One link's health.
pub struct Link {
    last_heard: Instant,
    last_sent: Instant,
    /// past this, `Health::Quiet`
    pub quiet_after: Duration,
    /// past this, `Health::Lost`
    pub grace: Duration,
    /// how often to ping when nothing else is going out
    pub keepalive: Duration,
    closed: bool,
    pub stats: LinkStats,
    /// counter of the last ping sent, so a pong can be matched to it
    pending_ping: Option<(u64, Instant)>,
}

impl Link {
    pub fn new(now: Instant) -> Link {
        Link {
            last_heard: now,
            last_sent: now,
            quiet_after: Duration::from_millis(200),
            grace: Duration::from_millis(1_500),
            keepalive: Duration::from_millis(100),
            closed: false,
            stats: LinkStats::default(),
            pending_ping: None,
        }
    }

    /// A tight link for a body that can fall: quiet at 50 ms, lost at
    /// 200 ms, which matches the envelope grace in `brainos-rt`.
    pub fn for_a_body_that_can_fall(now: Instant) -> Link {
        Link {
            quiet_after: Duration::from_millis(50),
            grace: Duration::from_millis(200),
            keepalive: Duration::from_millis(20),
            ..Link::new(now)
        }
    }

    pub fn heard(&mut self, now: Instant) {
        self.last_heard = now;
        self.stats.received = self.stats.received.saturating_add(1);
    }

    pub fn sent(&mut self, now: Instant) {
        self.last_sent = now;
        self.stats.sent = self.stats.sent.saturating_add(1);
    }

    pub fn refused(&mut self, was_replay: bool) {
        self.stats.refused = self.stats.refused.saturating_add(1);
        if was_replay {
            self.stats.replays = self.stats.replays.saturating_add(1);
        }
    }

    pub fn close(&mut self) {
        self.closed = true;
    }

    /// How long since anything arrived.
    pub fn silence(&self, now: Instant) -> Duration {
        now.since(self.last_heard)
    }

    pub fn health(&self, now: Instant) -> Health {
        if self.closed {
            return Health::Closed;
        }
        let s = self.silence(now);
        if s > self.grace {
            Health::Lost
        } else if s > self.quiet_after {
            Health::Quiet
        } else {
            Health::Good
        }
    }

    /// Is it time to send a keepalive? Only when nothing else has gone
    /// out — a link carrying commands at 100 Hz does not need pings.
    pub fn should_ping(&self, now: Instant) -> bool {
        !self.closed && now.since(self.last_sent) >= self.keepalive
    }

    pub fn ping_sent(&mut self, counter: u64, now: Instant) {
        if self.pending_ping.is_some() {
            self.stats.unanswered = self.stats.unanswered.saturating_add(1);
        }
        self.pending_ping = Some((counter, now));
        self.sent(now);
    }

    /// A pong arrived. Returns the round trip if it matched an
    /// outstanding ping.
    pub fn pong(&mut self, counter: u64, now: Instant) -> Option<Duration> {
        match self.pending_ping {
            Some((c, at)) if c == counter => {
                let rtt = now.since(at);
                self.stats.rtt = rtt;
                if rtt > self.stats.worst_rtt {
                    self.stats.worst_rtt = rtt;
                }
                self.pending_ping = None;
                self.heard(now);
                Some(rtt)
            }
            _ => {
                // an unmatched pong is still evidence the far end is
                // alive, which is the only thing this layer is for
                self.heard(now);
                None
            }
        }
    }

    pub fn describe(&self, now: Instant) -> Text<128> {
        let mut t = Text::new();
        t.push_str(match self.health(now) {
            Health::Good => "good",
            Health::Quiet => "quiet",
            Health::Lost => "LOST",
            Health::Closed => "closed",
        });
        t.push_str(" silence=");
        t.push_u64(self.silence(now).as_millis());
        t.push_str("ms rtt=");
        t.push_u64(self.stats.rtt.as_millis());
        t.push_str("ms");
        if self.stats.replays > 0 {
            t.push_str(" replays=");
            t.push_u64(self.stats.replays);
        }
        t
    }
}
