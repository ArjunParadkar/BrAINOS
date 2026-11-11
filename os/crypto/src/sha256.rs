//! BFS  LEVEL    Domain 1 Silicon - the BrAIn Key, hashes, Merkle log, quorum  [crate brainos-crypto]
//! BFS  PURPOSE  SHA-256 written out from FIPS 180-4: the one hash used for body ids, the audit log, key derivation and export signatures
//! BFS  DEFINES  HASH_LEN, Hash, Sha256, sha256
//! BFS  USES     this crate: domain | brainos: abi
//! BFS  USED BY  blur/census, blur/firmware, body/acquire, body/schema, crypto/hkdf, crypto/hmac, crypto/key, crypto/lib, +17 more
//!
//! SHA-256, from FIPS 180-4.
//!
//! Written out rather than pulled in. It is 120 lines, it has published
//! test vectors, and it is the single hash the whole substrate uses — for
//! body ids, the Merkle audit log, key derivation, and the export
//! signature. One hash, understood completely, beats three hashes chosen
//! by three different dependencies.

use crate::domain::Tag;
use core::fmt;

pub const HASH_LEN: usize = 32;
const BLOCK_LEN: usize = 64;

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

const IV: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

/// A 32-byte digest.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Hash(pub [u8; HASH_LEN]);

impl Hash {
    pub const ZERO: Hash = Hash([0u8; HASH_LEN]);

    pub fn as_bytes(&self) -> &[u8; HASH_LEN] {
        &self.0
    }

    /// Constant-time comparison. Digests are compared in places where a
    /// mismatch decides whether a body is who it says it is.
    pub fn ct_eq(&self, other: &Hash) -> bool {
        let mut diff = 0u8;
        for i in 0..HASH_LEN {
            diff |= self.0[i] ^ other.0[i];
        }
        diff == 0
    }

    pub fn to_id(self) -> brainos_abi::ids::Id32 {
        brainos_abi::ids::Id32(self.0)
    }

    pub fn write_hex(&self, out: &mut [u8; 64]) {
        self.to_id().write_hex(out)
    }
}

impl fmt::Debug for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_id().fingerprint())
    }
}

#[derive(Clone)]
pub struct Sha256 {
    state: [u32; 8],
    buf: [u8; BLOCK_LEN],
    buf_len: usize,
    total_len: u64,
}

impl Default for Sha256 {
    fn default() -> Sha256 {
        Sha256::new()
    }
}

impl Sha256 {
    pub fn new() -> Sha256 {
        Sha256 {
            state: IV,
            buf: [0u8; BLOCK_LEN],
            buf_len: 0,
            total_len: 0,
        }
    }

    /// Start a hash already bound to a purpose. Every structural hash in
    /// BrAInOS starts here rather than at `new`, so two different kinds
    /// of value can never collide by containing the same bytes.
    pub fn tagged(tag: Tag) -> Sha256 {
        let mut h = Sha256::new();
        h.update(tag.as_str().as_bytes());
        h.update(&[0x00]); // the tag is not part of the payload
        h
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.total_len = self.total_len.wrapping_add(data.len() as u64);

        if self.buf_len > 0 {
            let room = BLOCK_LEN - self.buf_len;
            let take = room.min(data.len());
            self.buf[self.buf_len..self.buf_len + take].copy_from_slice(&data[..take]);
            self.buf_len += take;
            data = &data[take..];
            if self.buf_len == BLOCK_LEN {
                let block = self.buf;
                self.compress(&block);
                self.buf_len = 0;
            }
        }

        while data.len() >= BLOCK_LEN {
            let mut block = [0u8; BLOCK_LEN];
            block.copy_from_slice(&data[..BLOCK_LEN]);
            self.compress(&block);
            data = &data[BLOCK_LEN..];
        }

        if !data.is_empty() {
            self.buf[..data.len()].copy_from_slice(data);
            self.buf_len = data.len();
        }
    }

    /// Absorb a length-prefixed field. Concatenating `a` then `b` and
    /// concatenating `ab` then `` hash the same; prefixing the length
    /// makes them different, which is what stops a two-field structure
    /// from colliding with a one-field one.
    pub fn field(&mut self, data: &[u8]) {
        self.update(&(data.len() as u64).to_le_bytes());
        self.update(data);
    }

    pub fn finish(mut self) -> Hash {
        let bit_len = self.total_len.wrapping_mul(8);
        self.update(&[0x80]);
        while self.buf_len != BLOCK_LEN - 8 {
            self.update(&[0x00]);
        }
        // the length field is not part of the message length
        self.total_len = 0;
        self.update(&bit_len.to_be_bytes());
        debug_assert_eq!(self.buf_len, 0);

        let mut out = [0u8; HASH_LEN];
        for (i, w) in self.state.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&w.to_be_bytes());
        }
        Hash(out)
    }

    fn compress(&mut self, block: &[u8; BLOCK_LEN]) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                block[i * 4],
                block[i * 4 + 1],
                block[i * 4 + 2],
                block[i * 4 + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.state;

        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);

            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }

        for (s, v) in self.state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *s = s.wrapping_add(v);
        }
    }
}

/// One-shot, untagged. Only for implementing HMAC and for test vectors;
/// structural hashes should use `Sha256::tagged`.
pub fn sha256(data: &[u8]) -> Hash {
    let mut h = Sha256::new();
    h.update(data);
    h.finish()
}
