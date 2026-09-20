//! A loopback link, for the simulator and the test rig.
//!
//! Two queues and a configurable fault model. The fault model is the
//! point: a transport that has only been tested on a perfect link has
//! not been tested. This one can drop, duplicate, reorder and corrupt on
//! demand, deterministically, so "what does the body do when the link is
//! bad" is a test rather than a field report.

use alloc::collections::VecDeque;
use alloc::vec::Vec;
use brainos_crypto::rng::Drbg;

/// How badly the link behaves.
#[derive(Clone, Copy, Debug, Default)]
pub struct Faults {
    /// chance in 1000 that a frame is dropped
    pub drop_per_mille: u32,
    /// chance in 1000 that a frame is delivered twice
    pub duplicate_per_mille: u32,
    /// chance in 1000 that a frame has one bit flipped
    pub corrupt_per_mille: u32,
    /// chance in 1000 that a frame is held back one delivery
    pub reorder_per_mille: u32,
}

impl Faults {
    pub const PERFECT: Faults = Faults {
        drop_per_mille: 0,
        duplicate_per_mille: 0,
        corrupt_per_mille: 0,
        reorder_per_mille: 0,
    };

    /// What a serial line on a robot with brushed motors near it
    /// actually looks like.
    pub const NOISY: Faults = Faults {
        drop_per_mille: 30,
        duplicate_per_mille: 5,
        corrupt_per_mille: 20,
        reorder_per_mille: 10,
    };
}

pub struct Loopback {
    a_to_b: VecDeque<Vec<u8>>,
    b_to_a: VecDeque<Vec<u8>>,
    held_a: Option<Vec<u8>>,
    held_b: Option<Vec<u8>>,
    pub faults: Faults,
    rng: Drbg,
    pub dropped: u64,
    pub duplicated: u64,
    pub corrupted: u64,
    pub reordered: u64,
}

impl Loopback {
    pub fn perfect() -> Loopback {
        Loopback::new(Faults::PERFECT, b"loopback")
    }

    pub fn new(faults: Faults, seed: &[u8]) -> Loopback {
        Loopback {
            a_to_b: VecDeque::new(),
            b_to_a: VecDeque::new(),
            held_a: None,
            held_b: None,
            faults,
            rng: Drbg::deterministic(seed),
            dropped: 0,
            duplicated: 0,
            corrupted: 0,
            reordered: 0,
        }
    }

    fn roll(&mut self, per_mille: u32) -> bool {
        per_mille > 0 && self.rng.below(1000) < per_mille as u64
    }

    fn push(&mut self, to_b: bool, mut frame: Vec<u8>) {
        if self.roll(self.faults.drop_per_mille) {
            self.dropped += 1;
            return;
        }
        if self.roll(self.faults.corrupt_per_mille) && !frame.is_empty() {
            let i = self.rng.below(frame.len() as u64) as usize;
            let bit = self.rng.below(8) as u8;
            frame[i] ^= 1 << bit;
            self.corrupted += 1;
        }
        let duplicate = self.roll(self.faults.duplicate_per_mille);
        let reorder = self.roll(self.faults.reorder_per_mille);

        let q = if to_b { &mut self.a_to_b } else { &mut self.b_to_a };
        if duplicate {
            q.push_back(frame.clone());
        }
        if reorder {
            let held = if to_b { &mut self.held_a } else { &mut self.held_b };
            let previously_held = held.take();
            *held = Some(frame);
            self.reordered += 1;
            if duplicate {
                self.duplicated += 1;
            }
            if let Some(p) = previously_held {
                let q = if to_b { &mut self.a_to_b } else { &mut self.b_to_a };
                q.push_back(p);
            }
            return;
        }
        if duplicate {
            self.duplicated += 1;
        }
        q.push_back(frame);
        // anything held now goes out behind it, which is the reorder
        let held = if to_b { self.held_a.take() } else { self.held_b.take() };
        if let Some(h) = held {
            let q = if to_b { &mut self.a_to_b } else { &mut self.b_to_a };
            q.push_back(h);
        }
    }

    pub fn a_sends(&mut self, frame: Vec<u8>) {
        self.push(true, frame);
    }

    pub fn b_sends(&mut self, frame: Vec<u8>) {
        self.push(false, frame);
    }

    pub fn b_receives(&mut self) -> Option<Vec<u8>> {
        self.a_to_b.pop_front()
    }

    pub fn a_receives(&mut self) -> Option<Vec<u8>> {
        self.b_to_a.pop_front()
    }

    pub fn in_flight(&self) -> usize {
        self.a_to_b.len() + self.b_to_a.len()
    }

    /// Flush anything held back by the reorder model.
    pub fn settle(&mut self) {
        if let Some(h) = self.held_a.take() {
            self.a_to_b.push_back(h);
        }
        if let Some(h) = self.held_b.take() {
            self.b_to_a.push_back(h);
        }
    }
}
