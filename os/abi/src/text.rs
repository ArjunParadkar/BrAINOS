//! BFS  LEVEL    Foundation - shared vocabulary (ids, time, verbs, codec, errors)  [crate brainos-abi]
//! BFS  PURPOSE  `Text<N>`: a fixed-capacity UTF-8 string that needs no allocator, cuts on a character boundary, and records when it truncated
//! BFS  DEFINES  Text
//! BFS  USES     this crate: codec, error
//! BFS  USED BY  abi/lib, abi/tests/abi_suite, abi/verb, body/calibration, body/vital, bodyd/mechanism, bodyd/runtime, cognition/escalate, +31 more
//!
//! `Text<N>` — a string with a ceiling.
//!
//! Domain 2 runs in a servo loop on a microcontroller with no allocator,
//! and it still has to carry a verb name, a deny reason, a body label.
//! `String` cannot go there. `&str` cannot own. So: a fixed-capacity
//! UTF-8 buffer that is `Copy`-free but allocation-free, truncates on a
//! character boundary rather than splitting a codepoint, and tells you
//! when it truncated instead of silently losing the tail.
//!
//! The ceiling is a feature everywhere, not only on the MCU. A deny
//! reason that can be arbitrarily long is a deny reason an attacker can
//! use to flood an audit log.

use crate::codec::{Decode, Decoder, Encode, Encoder};
use crate::error::{Error, Result};
use core::fmt;

#[derive(Clone, Copy)]
pub struct Text<const N: usize> {
    buf: [u8; N],
    len: u16,
    /// true when a push or a `from_str` dropped characters off the end
    truncated: bool,
}

impl<const N: usize> Text<N> {
    pub const CAPACITY: usize = N;

    pub const fn new() -> Text<N> {
        Text {
            buf: [0u8; N],
            len: 0,
            truncated: false,
        }
    }

    /// Fill from a `&str`, truncating on a character boundary if needed.
    /// Truncation is recorded, never silent.
    pub fn from_str_truncating(s: &str) -> Text<N> {
        let mut t = Text::new();
        t.push_str(s);
        t
    }

    /// Fill from a `&str`, refusing if it does not fit. Use this when the
    /// value is load-bearing — a verb, a path — and a truncated version
    /// would be a *different* value that happens to be a prefix.
    pub fn from_str_exact(s: &str) -> Result<Text<N>> {
        if s.len() > N {
            return Err(Error::Capacity("text exceeds capacity"));
        }
        let mut t = Text::new();
        t.buf[..s.len()].copy_from_slice(s.as_bytes());
        t.len = s.len() as u16;
        Ok(t)
    }

    pub fn as_str(&self) -> &str {
        // invariant: `buf[..len]` is always a whole number of complete
        // codepoints, maintained by push_str's boundary walk
        core::str::from_utf8(&self.buf[..self.len as usize]).unwrap_or("")
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.buf[..self.len as usize]
    }

    pub fn len(&self) -> usize {
        self.len as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn was_truncated(&self) -> bool {
        self.truncated
    }

    pub fn clear(&mut self) {
        self.len = 0;
        self.truncated = false;
    }

    /// Append, stopping at the last character boundary that fits.
    pub fn push_str(&mut self, s: &str) {
        let room = N - self.len as usize;
        if s.len() <= room {
            let at = self.len as usize;
            self.buf[at..at + s.len()].copy_from_slice(s.as_bytes());
            self.len += s.len() as u16;
            return;
        }
        // walk back to a boundary so a multi-byte character is never
        // split into invalid UTF-8
        let mut cut = room;
        while cut > 0 && !s.is_char_boundary(cut) {
            cut -= 1;
        }
        let at = self.len as usize;
        self.buf[at..at + cut].copy_from_slice(&s.as_bytes()[..cut]);
        self.len += cut as u16;
        self.truncated = true;
    }

    pub fn push(&mut self, c: char) {
        let mut b = [0u8; 4];
        self.push_str(c.encode_utf8(&mut b));
    }

    /// Append a `u64` in decimal. `write!` needs `core::fmt` machinery
    /// that pulls in more than a servo loop should carry, and numbers in
    /// deny reasons are common enough to be worth the twenty lines.
    pub fn push_u64(&mut self, mut v: u64) {
        if v == 0 {
            self.push('0');
            return;
        }
        let mut digits = [0u8; 20];
        let mut n = 0;
        while v > 0 {
            digits[n] = b'0' + (v % 10) as u8;
            v /= 10;
            n += 1;
        }
        while n > 0 {
            n -= 1;
            self.push(digits[n] as char);
        }
    }

    pub fn push_i64(&mut self, v: i64) {
        if v < 0 {
            self.push('-');
            self.push_u64(v.unsigned_abs());
        } else {
            self.push_u64(v as u64);
        }
    }

    pub fn starts_with(&self, p: &str) -> bool {
        self.as_str().starts_with(p)
    }

    pub fn contains(&self, p: &str) -> bool {
        self.as_str().contains(p)
    }
}

impl<const N: usize> Default for Text<N> {
    fn default() -> Text<N> {
        Text::new()
    }
}

impl<const N: usize> PartialEq for Text<N> {
    fn eq(&self, other: &Text<N>) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}

impl<const N: usize> Eq for Text<N> {}

impl<const N: usize> PartialEq<str> for Text<N> {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl<const N: usize> PartialOrd for Text<N> {
    fn partial_cmp(&self, other: &Text<N>) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<const N: usize> Ord for Text<N> {
    fn cmp(&self, other: &Text<N>) -> core::cmp::Ordering {
        self.as_bytes().cmp(other.as_bytes())
    }
}

impl<const N: usize> core::hash::Hash for Text<N> {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        self.as_bytes().hash(state)
    }
}

impl<const N: usize> fmt::Display for Text<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl<const N: usize> fmt::Debug for Text<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.as_str())
    }
}

impl<const N: usize> fmt::Write for Text<N> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.push_str(s);
        Ok(())
    }
}

impl<const N: usize> Encode for Text<N> {
    fn encode(&self, e: &mut Encoder) {
        e.str(self.as_str());
    }
}

impl<const N: usize> Decode for Text<N> {
    fn decode(d: &mut Decoder) -> Result<Text<N>> {
        // exact, not truncating: a decoded value that silently shortened
        // would not round-trip, and round-tripping is the whole contract
        Text::from_str_exact(d.str()?)
    }
}
