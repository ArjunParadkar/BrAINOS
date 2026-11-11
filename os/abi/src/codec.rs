//! BFS  LEVEL    Foundation - shared vocabulary (ids, time, verbs, codec, errors)  [crate brainos-abi]
//! BFS  PURPOSE  Turns values into bytes and back so each value has exactly one encoding, which is what makes hashes and signatures over it meaningful
//! BFS  DEFINES  MAX_LEN, Encoder, Decoder, Encode, Decode
//! BFS  USES     this crate: error
//! BFS  USED BY  abi/ids, abi/lib, abi/tests/abi_suite, abi/text, abi/time, abi/verb, abi/version, body/calibration, +18 more
//!
//! The canonical codec.
//!
//! One value has exactly one encoding. That property is load-bearing in
//! three places: the audit log is a Merkle tree over encoded decisions,
//! the entity export is signed, and body attestation hashes a capability
//! schema. If a value could encode two ways, all three of those
//! signatures would prove nothing about the value — only about some
//! bytes that happened to be handed to the hash.
//!
//! So the rules are boring on purpose:
//!
//! * Integers are fixed-width little-endian. No varints, no
//!   "shortest form" subtleties, nothing where a decoder has to reject a
//!   non-minimal encoding to stay canonical.
//! * Lengths are `u32` little-endian, checked against a hard cap.
//! * Floats encode as their IEEE-754 bits, with every NaN normalized to
//!   one quiet NaN, because otherwise a single bit of NaN payload would
//!   be a covert channel through a signed structure.
//! * Sequences are length-then-items. Maps are not primitive: anything
//!   map-shaped encodes as a sequence its owner has already sorted, so
//!   there is no iteration-order ambiguity to have a bug in.
//!
//! The encoder never fails. It writes into a `Vec` and the capacity cap
//! is enforced where a length is written, so a structure that is too
//! large to encode is caught with a `Capacity` error at the field that
//! made it too large, not with an out-of-memory abort three layers up.

use crate::error::{Error, Result};
use alloc::vec::Vec;

/// The largest single length-prefixed run the codec will encode or
/// accept. Anything genuinely larger is a stream, and streams are chunked
/// by the transport, not encoded as one field.
pub const MAX_LEN: usize = 16 * 1024 * 1024;

/// The IEEE-754 bit pattern every NaN is normalized to before encoding.
const CANONICAL_NAN_F32: u32 = 0x7FC0_0000;
const CANONICAL_NAN_F64: u64 = 0x7FF8_0000_0000_0000;

#[derive(Default)]
pub struct Encoder {
    out: Vec<u8>,
    /// set when a length exceeded `MAX_LEN`; checked once at `finish`
    overflow: Option<&'static str>,
}

impl Encoder {
    pub fn new() -> Encoder {
        Encoder {
            out: Vec::new(),
            overflow: None,
        }
    }

    pub fn with_capacity(n: usize) -> Encoder {
        Encoder {
            out: Vec::with_capacity(n),
            overflow: None,
        }
    }

    pub fn len(&self) -> usize {
        self.out.len()
    }

    pub fn is_empty(&self) -> bool {
        self.out.is_empty()
    }

    /// Raw bytes with no length prefix. Only for fixed-size fields whose
    /// width is known statically on both sides (ids, signatures, hashes).
    pub fn raw(&mut self, b: &[u8]) {
        self.out.extend_from_slice(b);
    }

    pub fn u8(&mut self, v: u8) {
        self.out.push(v);
    }

    pub fn bool(&mut self, v: bool) {
        // exactly 0 or 1 — a decoder rejects anything else, so `bool`
        // round-trips instead of silently accepting 2 as true
        self.out.push(v as u8);
    }

    pub fn u16(&mut self, v: u16) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }

    pub fn u32(&mut self, v: u32) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }

    pub fn u64(&mut self, v: u64) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }

    pub fn i32(&mut self, v: i32) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }

    pub fn i64(&mut self, v: i64) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }

    pub fn f32(&mut self, v: f32) {
        let bits = if v.is_nan() {
            CANONICAL_NAN_F32
        } else if v == 0.0 {
            0 // normalize -0.0 to +0.0; the sign of zero is never meaning
        } else {
            v.to_bits()
        };
        self.u32(bits);
    }

    pub fn f64(&mut self, v: f64) {
        let bits = if v.is_nan() {
            CANONICAL_NAN_F64
        } else if v == 0.0 {
            0
        } else {
            v.to_bits()
        };
        self.u64(bits);
    }

    /// A length, checked against `MAX_LEN`. Every length-prefixed field
    /// goes through here, so the cap is enforced in exactly one place.
    pub fn len_prefix(&mut self, n: usize, what: &'static str) {
        if n > MAX_LEN {
            if self.overflow.is_none() {
                self.overflow = Some(what);
            }
            self.u32(0);
        } else {
            self.u32(n as u32);
        }
    }

    pub fn bytes(&mut self, b: &[u8]) {
        self.len_prefix(b.len(), "byte string");
        if b.len() <= MAX_LEN {
            self.out.extend_from_slice(b);
        }
    }

    pub fn str(&mut self, s: &str) {
        self.len_prefix(s.len(), "string");
        if s.len() <= MAX_LEN {
            self.out.extend_from_slice(s.as_bytes());
        }
    }

    /// Open a sequence of `n` items. The items follow immediately; the
    /// caller is responsible for writing exactly `n` of them.
    pub fn seq(&mut self, n: usize) {
        self.len_prefix(n, "sequence");
    }

    /// Encode an optional as a presence byte then the value.
    pub fn opt<T: Encode>(&mut self, v: &Option<T>) {
        match v {
            Some(x) => {
                self.bool(true);
                x.encode(self);
            }
            None => self.bool(false),
        }
    }

    pub fn item<T: Encode>(&mut self, v: &T) {
        v.encode(self);
    }

    /// Finish, or report the first field that blew the capacity cap.
    pub fn finish(self) -> Result<Vec<u8>> {
        match self.overflow {
            Some(w) => Err(Error::Capacity(w)),
            None => Ok(self.out),
        }
    }

    /// Finish, panicking on overflow. Only for call sites that encode
    /// statically-bounded structures — used by hashing helpers where the
    /// alternative is a `Result` on every hash.
    pub fn finish_bounded(self) -> Vec<u8> {
        debug_assert!(self.overflow.is_none(), "bounded encode overflowed");
        self.out
    }
}

pub struct Decoder<'a> {
    input: &'a [u8],
    pos: usize,
}

impl<'a> Decoder<'a> {
    pub fn new(input: &'a [u8]) -> Decoder<'a> {
        Decoder { input, pos: 0 }
    }

    pub fn remaining(&self) -> usize {
        self.input.len() - self.pos
    }

    pub fn is_empty(&self) -> bool {
        self.remaining() == 0
    }

    pub fn position(&self) -> usize {
        self.pos
    }

    /// Every decode of every trailing structure ends here: a decoder that
    /// still has bytes left decoded something that was not what the
    /// caller thought it was. Trailing garbage is rejected, because a
    /// structure with ignorable trailing bytes is a structure with two
    /// encodings, and see the module note about why that matters.
    pub fn finish(self) -> Result<()> {
        if self.pos == self.input.len() {
            Ok(())
        } else {
            Err(Error::Malformed("trailing bytes after value"))
        }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.remaining() < n {
            return Err(Error::Truncated);
        }
        let s = &self.input[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    pub fn read_exact(&mut self, out: &mut [u8]) -> Result<()> {
        let n = out.len();
        out.copy_from_slice(self.take(n)?);
        Ok(())
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    pub fn bool(&mut self) -> Result<bool> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(Error::Malformed("bool must be 0 or 1")),
        }
    }

    pub fn u16(&mut self) -> Result<u16> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn u32(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn u64(&mut self) -> Result<u64> {
        let b = self.take(8)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(b);
        Ok(u64::from_le_bytes(a))
    }

    pub fn i32(&mut self) -> Result<i32> {
        self.u32().map(|v| v as i32)
    }

    pub fn i64(&mut self) -> Result<i64> {
        self.u64().map(|v| v as i64)
    }

    /// Decode a float, **rejecting** any encoding the encoder would not
    /// have produced.
    ///
    /// The encoder normalizes every NaN to one quiet NaN and −0.0 to
    /// +0.0. If the decoder accepted the un-normalized forms, decoding
    /// would not be injective: two different byte strings would produce
    /// the same value, and a signature over the bytes would not be a
    /// signature over the value. That is the exact property the audit
    /// Merkle log, the export signature and the body-identity digest
    /// rest on.
    ///
    /// Found by the hostile-input harness, which mutates a valid
    /// encoding and checks that anything which decodes re-encodes to its
    /// own bytes. Three of four hundred mutated commands did not, and
    /// every one of them was a float whose sign or NaN payload the
    /// encoder would have flattened.
    pub fn f32(&mut self) -> Result<f32> {
        let bits = self.u32()?;
        if bits == 0x8000_0000 {
            return Err(Error::Malformed("negative zero is not a canonical encoding"));
        }
        let v = f32::from_bits(bits);
        if v.is_nan() && bits != CANONICAL_NAN_F32 {
            return Err(Error::Malformed("non-canonical NaN"));
        }
        Ok(v)
    }

    pub fn f64(&mut self) -> Result<f64> {
        let bits = self.u64()?;
        if bits == 0x8000_0000_0000_0000 {
            return Err(Error::Malformed("negative zero is not a canonical encoding"));
        }
        let v = f64::from_bits(bits);
        if v.is_nan() && bits != CANONICAL_NAN_F64 {
            return Err(Error::Malformed("non-canonical NaN"));
        }
        Ok(v)
    }

    /// A length prefix, bounded both by `MAX_LEN` and — crucially — by
    /// the bytes actually remaining. Without the second check a hostile
    /// 4-byte header could make the decoder try to reserve 4 GiB.
    pub fn len_prefix(&mut self) -> Result<usize> {
        let n = self.u32()? as usize;
        if n > MAX_LEN {
            return Err(Error::Capacity("declared length exceeds MAX_LEN"));
        }
        if n > self.remaining() {
            return Err(Error::Truncated);
        }
        Ok(n)
    }

    pub fn bytes(&mut self) -> Result<&'a [u8]> {
        let n = self.len_prefix()?;
        self.take(n)
    }

    pub fn str(&mut self) -> Result<&'a str> {
        let b = self.bytes()?;
        core::str::from_utf8(b).map_err(|_| Error::Malformed("string is not utf-8"))
    }

    /// Open a sequence. The returned count is guaranteed not to exceed
    /// the bytes remaining, so `for _ in 0..n` cannot be made to spin on
    /// a fabricated count — though a caller decoding multi-byte items
    /// still meets `Truncated` on the item that runs out.
    pub fn seq(&mut self) -> Result<usize> {
        self.len_prefix()
    }

    pub fn opt<T: Decode>(&mut self) -> Result<Option<T>> {
        if self.bool()? {
            T::decode(self).map(Some)
        } else {
            Ok(None)
        }
    }

    pub fn item<T: Decode>(&mut self) -> Result<T> {
        T::decode(self)
    }

    /// Decode a sequence of items into a `Vec`.
    pub fn vec<T: Decode>(&mut self) -> Result<Vec<T>> {
        let n = self.seq()?;
        let mut v = Vec::new();
        v.try_reserve(n.min(1024))
            .map_err(|_| Error::Capacity("sequence allocation"))?;
        for _ in 0..n {
            v.push(T::decode(self)?);
        }
        Ok(v)
    }
}

pub trait Encode {
    fn encode(&self, e: &mut Encoder);

    /// Encode to a fresh `Vec`. The canonical bytes of this value.
    fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut e = Encoder::new();
        self.encode(&mut e);
        e.finish()
    }
}

pub trait Decode: Sized {
    fn decode(d: &mut Decoder) -> Result<Self>;

    /// Decode from a complete buffer, rejecting trailing bytes.
    fn from_bytes(b: &[u8]) -> Result<Self> {
        let mut d = Decoder::new(b);
        let v = Self::decode(&mut d)?;
        d.finish()?;
        Ok(v)
    }
}

// --- blanket implementations for the primitives ---

macro_rules! prim {
    ($t:ty, $enc:ident, $dec:ident) => {
        impl Encode for $t {
            fn encode(&self, e: &mut Encoder) {
                e.$enc(*self);
            }
        }
        impl Decode for $t {
            fn decode(d: &mut Decoder) -> Result<$t> {
                d.$dec()
            }
        }
    };
}

prim!(u8, u8, u8);
prim!(u16, u16, u16);
prim!(u32, u32, u32);
prim!(u64, u64, u64);
prim!(i32, i32, i32);
prim!(i64, i64, i64);
prim!(f32, f32, f32);
prim!(f64, f64, f64);
prim!(bool, bool, bool);

impl Encode for alloc::string::String {
    fn encode(&self, e: &mut Encoder) {
        e.str(self);
    }
}

impl Decode for alloc::string::String {
    fn decode(d: &mut Decoder) -> Result<alloc::string::String> {
        d.str().map(alloc::string::ToString::to_string)
    }
}

impl<T: Encode> Encode for Vec<T> {
    fn encode(&self, e: &mut Encoder) {
        e.seq(self.len());
        for item in self {
            item.encode(e);
        }
    }
}

impl<T: Decode> Decode for Vec<T> {
    fn decode(d: &mut Decoder) -> Result<Vec<T>> {
        d.vec()
    }
}
