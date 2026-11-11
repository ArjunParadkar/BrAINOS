//! BFS  LEVEL    Domain 1 Silicon - the BrAIn Key, hashes, Merkle log, quorum  [crate brainos-crypto]
//! BFS  PURPOSE  HMAC-SHA256 (RFC 2104) keyed checksum, used for key derivation and for cheap per-frame authentication between parties sharing a secret
//! BFS  DEFINES  HmacSha256
//! BFS  USES     this crate: sha256
//! BFS  USED BY  crypto/hkdf, crypto/lib, crypto/rng, crypto/tests/crypto_suite, transport/frame
//!
//! HMAC-SHA256, from RFC 2104.
//!
//! Used for key derivation (via HKDF) and for the transport's frame
//! authentication, where a full signature per frame would cost more than
//! the frame. Not used for anything that crosses a trust boundary
//! between parties who do not already share a secret — that is what the
//! Key is for.

use crate::sha256::{Hash, Sha256, HASH_LEN};

const BLOCK_LEN: usize = 64;

#[derive(Clone)]
pub struct HmacSha256 {
    inner: Sha256,
    outer_key: [u8; BLOCK_LEN],
}

impl HmacSha256 {
    pub fn new(key: &[u8]) -> HmacSha256 {
        // RFC 2104: a key longer than the block is hashed first; a
        // shorter one is zero-padded
        let mut k = [0u8; BLOCK_LEN];
        if key.len() > BLOCK_LEN {
            k[..HASH_LEN].copy_from_slice(crate::sha256::sha256(key).as_bytes());
        } else {
            k[..key.len()].copy_from_slice(key);
        }

        let mut ipad = [0x36u8; BLOCK_LEN];
        let mut outer_key = [0x5cu8; BLOCK_LEN];
        for i in 0..BLOCK_LEN {
            ipad[i] ^= k[i];
            outer_key[i] ^= k[i];
        }

        let mut inner = Sha256::new();
        inner.update(&ipad);
        HmacSha256 { inner, outer_key }
    }

    pub fn update(&mut self, data: &[u8]) {
        self.inner.update(data);
    }

    pub fn finish(self) -> Hash {
        let inner = self.inner.finish();
        let mut outer = Sha256::new();
        outer.update(&self.outer_key);
        outer.update(inner.as_bytes());
        outer.finish()
    }

    pub fn mac(key: &[u8], data: &[u8]) -> Hash {
        let mut h = HmacSha256::new(key);
        h.update(data);
        h.finish()
    }

    /// Constant-time verification. Comparing a MAC with `==` is the
    /// textbook timing leak; this is why the function exists at all.
    pub fn verify(key: &[u8], data: &[u8], tag: &Hash) -> bool {
        HmacSha256::mac(key, data).ct_eq(tag)
    }
}
