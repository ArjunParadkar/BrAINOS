//! BFS  LEVEL    Phase 0 - Mini-Blur hexapod as numbers (shared by firmware and seat)  [crate miniblur-core]
//! BFS  PURPOSE  Lost-link timer: fires once when nothing is heard from the seat for timeout_ms so the body stops and stands; re-arms on the next message
//! BFS  DEFINES  LinkWatch
//! BFS  USES     nothing outside core/alloc
//! BFS  USED BY  core/runtime
//!
//! Stop-on-lost-link. The seat pings every few seconds and every command is
//! a heartbeat; if the body hears nothing for `timeout_ms` it stops the gait
//! and stands. Domain 2 holds the body in a safe state until the mind
//! returns (Architecture §14). The seat mirrors the same clock so KIRA
//! refuses a walk at POLICY when the link is already stale.

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct LinkWatch {
    pub last_heard_ms: u64,
    pub timeout_ms: u32,
    /// set once the watchdog has fired; cleared by the next heartbeat
    pub tripped: bool,
}

impl LinkWatch {
    pub fn new(now_ms: u64, timeout_ms: u32) -> LinkWatch {
        LinkWatch { last_heard_ms: now_ms, timeout_ms, tripped: false }
    }
    pub fn heard(&mut self, now_ms: u64) {
        self.last_heard_ms = now_ms;
        self.tripped = false;
    }
    pub fn age_ms(&self, now_ms: u64) -> u64 {
        now_ms.saturating_sub(self.last_heard_ms)
    }
    pub fn alive(&self, now_ms: u64) -> bool {
        self.age_ms(now_ms) <= self.timeout_ms as u64
    }
    /// Poll from the reflex loop. Returns true exactly once when the link
    /// goes stale — the moment to stop and stand.
    pub fn poll(&mut self, now_ms: u64) -> bool {
        if !self.alive(now_ms) && !self.tripped {
            self.tripped = true;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fires_once_when_the_link_goes_stale_and_rearms_on_heartbeat() {
        let mut w = LinkWatch::new(0, 500);
        assert!(w.alive(400));
        assert!(!w.poll(400));
        assert!(!w.alive(501));
        assert!(w.poll(501), "fires at the first stale poll");
        assert!(!w.poll(900), "does not fire again");
        w.heard(1000);
        assert!(w.alive(1200));
        assert!(w.poll(1600), "fires again after a fresh loss");
    }
}
