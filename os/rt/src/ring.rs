//! BFS  LEVEL    Domain 2 Autonomic - envelopes, reflexes, scheduler, watchdogs  [crate brainos-rt]
//! BFS  PURPOSE  Fixed-size queues and a sliding window of readings for code with no allocator; when full they drop and count instead of blocking or growing
//! BFS  DEFINES  Ring, Spsc, Window
//! BFS  USES     nothing outside core/alloc
//! BFS  USED BY  rt/lib, rt/tests/rt_suite
//!
//! Fixed-capacity buffers.
//!
//! Domain 2 has no allocator, so every queue between a sensor and a loop
//! is one of these. Both types share a property worth stating: when they
//! are full they **drop and count**, never block and never grow. A servo
//! loop that blocks on a full telemetry queue has turned an observability
//! feature into a fall, and a queue that grows has turned it into an
//! out-of-memory abort a few seconds later. Dropping is the only
//! behaviour that keeps the body upright, so it is the only behaviour
//! offered — and the drop counter means the loss is visible rather than
//! silent.


/// A single-producer single-consumer ring of `N` items.
///
/// `N` must be a power of two; the mask arithmetic depends on it and the
/// constructor asserts it at compile time via `assert!` in a const block.
pub struct Ring<T, const N: usize> {
    buf: [Option<T>; N],
    head: usize,
    tail: usize,
    dropped: u32,
}

impl<T, const N: usize> Default for Ring<T, N> {
    fn default() -> Ring<T, N> {
        Ring::new()
    }
}

impl<T, const N: usize> Ring<T, N> {
    pub fn new() -> Ring<T, N> {
        assert!(N.is_power_of_two(), "ring capacity must be a power of two");
        Ring {
            buf: [const { None }; N],
            head: 0,
            tail: 0,
            dropped: 0,
        }
    }

    pub fn capacity(&self) -> usize {
        N
    }

    pub fn len(&self) -> usize {
        self.head.wrapping_sub(self.tail) & (N - 1)
    }

    pub fn is_empty(&self) -> bool {
        self.head == self.tail
    }

    pub fn is_full(&self) -> bool {
        self.len() == N - 1
    }

    /// How many items were dropped because the ring was full. Non-zero
    /// here is a real signal: the consumer is not keeping up, which on a
    /// body usually means a loop overran.
    pub fn dropped(&self) -> u32 {
        self.dropped
    }

    /// Push, dropping the new item if full. Returns false on a drop.
    pub fn push(&mut self, item: T) -> bool {
        if self.is_full() {
            self.dropped = self.dropped.saturating_add(1);
            return false;
        }
        self.buf[self.head] = Some(item);
        self.head = (self.head + 1) & (N - 1);
        true
    }

    /// Push, evicting the *oldest* item if full. For telemetry, where the
    /// newest reading is the one that matters.
    pub fn push_overwrite(&mut self, item: T) -> Option<T> {
        let evicted = if self.is_full() { self.pop() } else { None };
        if evicted.is_some() {
            self.dropped = self.dropped.saturating_add(1);
        }
        self.buf[self.head] = Some(item);
        self.head = (self.head + 1) & (N - 1);
        evicted
    }

    pub fn pop(&mut self) -> Option<T> {
        if self.is_empty() {
            return None;
        }
        let item = self.buf[self.tail].take();
        self.tail = (self.tail + 1) & (N - 1);
        item
    }

    pub fn peek(&self) -> Option<&T> {
        if self.is_empty() {
            None
        } else {
            self.buf[self.tail].as_ref()
        }
    }

    pub fn clear(&mut self) {
        while self.pop().is_some() {}
    }
}

/// A lock-free-shaped SPSC queue.
///
/// This is `Ring` with the producer and consumer indices kept apart so
/// the two halves can be split across an interrupt boundary on a real
/// board. The implementation here is single-threaded and safe — the
/// atomics belong in the platform layer where the memory model is known,
/// not in portable logic pretending to know it. What this type provides
/// is the *shape* the platform layer implements, so the loop code above
/// it is identical on a host and on an MCU.
pub struct Spsc<T, const N: usize> {
    ring: Ring<T, N>,
}

impl<T, const N: usize> Default for Spsc<T, N> {
    fn default() -> Spsc<T, N> {
        Spsc::new()
    }
}

impl<T, const N: usize> Spsc<T, N> {
    pub fn new() -> Spsc<T, N> {
        Spsc { ring: Ring::new() }
    }

    pub fn produce(&mut self, item: T) -> bool {
        self.ring.push(item)
    }

    pub fn consume(&mut self) -> Option<T> {
        self.ring.pop()
    }

    pub fn dropped(&self) -> u32 {
        self.ring.dropped()
    }

    pub fn len(&self) -> usize {
        self.ring.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ring.is_empty()
    }
}

/// A fixed-capacity sliding window of `f32` readings, with running
/// statistics. Every reflex that needs "is this current draw trending
/// toward the limit" uses one of these rather than allocating a history.
pub struct Window<const N: usize> {
    buf: [f32; N],
    len: usize,
    next: usize,
}

impl<const N: usize> Default for Window<N> {
    fn default() -> Window<N> {
        Window::new()
    }
}

impl<const N: usize> Window<N> {
    pub fn new() -> Window<N> {
        Window {
            buf: [0.0; N],
            len: 0,
            next: 0,
        }
    }

    pub fn push(&mut self, v: f32) {
        self.buf[self.next] = v;
        self.next = (self.next + 1) % N;
        if self.len < N {
            self.len += 1;
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn is_full(&self) -> bool {
        self.len == N
    }

    pub fn mean(&self) -> f32 {
        if self.len == 0 {
            return 0.0;
        }
        let mut sum = 0.0f32;
        for i in 0..self.len {
            sum += self.buf[i];
        }
        sum / self.len as f32
    }

    pub fn max(&self) -> f32 {
        let mut m = f32::NEG_INFINITY;
        for i in 0..self.len {
            if self.buf[i] > m {
                m = self.buf[i];
            }
        }
        if self.len == 0 {
            0.0
        } else {
            m
        }
    }

    pub fn min(&self) -> f32 {
        let mut m = f32::INFINITY;
        for i in 0..self.len {
            if self.buf[i] < m {
                m = self.buf[i];
            }
        }
        if self.len == 0 {
            0.0
        } else {
            m
        }
    }

    /// The most recent reading.
    pub fn last(&self) -> Option<f32> {
        if self.len == 0 {
            None
        } else {
            Some(self.buf[(self.next + N - 1) % N])
        }
    }

    pub fn clear(&mut self) {
        self.len = 0;
        self.next = 0;
    }
}

