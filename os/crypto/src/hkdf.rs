//! BFS  LEVEL    Domain 1 Silicon - the BrAIn Key, hashes, Merkle log, quorum  [crate brainos-crypto]
//! BFS  PURPOSE  HKDF-SHA256 (RFC 5869) key derivation, used to give each body its own subkey so a compromised body never exposes the root key
//! BFS  DEFINES  hkdf_sha256
//! BFS  USES     this crate: hmac, sha256
//! BFS  USED BY  crypto/key, crypto/lib, crypto/tests/crypto_suite, transport/session
//!
//! HKDF-SHA256, from RFC 5869.
//!
//! One place needs it and it matters: per-body subkeys. When the entity
//! incorporates a body it derives a subkey bound to that body's id, and
//! the body authenticates its frames with that subkey rather than with
//! anything derived from the Key itself. A compromised body therefore
//! leaks a key that can impersonate *that body to this entity*, and
//! nothing else — not the entity, not another body, not a past session.

use crate::hmac::HmacSha256;
use crate::sha256::HASH_LEN;
use alloc::vec::Vec;

/// Extract-then-expand. `salt` may be empty; `info` is the binding
/// context and should always name what the key is for.
pub fn hkdf_sha256(salt: &[u8], ikm: &[u8], info: &[u8], out_len: usize) -> Vec<u8> {
    // extract
    let prk = HmacSha256::mac(salt, ikm);

    // expand
    let mut out = Vec::with_capacity(out_len);
    let mut t: Vec<u8> = Vec::new();
    let mut counter: u8 = 1;
    while out.len() < out_len {
        let mut h = HmacSha256::new(prk.as_bytes());
        h.update(&t);
        h.update(info);
        h.update(&[counter]);
        let block = h.finish();
        t = block.as_bytes().to_vec();
        let take = (out_len - out.len()).min(HASH_LEN);
        out.extend_from_slice(&block.as_bytes()[..take]);
        // RFC 5869 caps expansion at 255 blocks
        if counter == 255 {
            break;
        }
        counter += 1;
    }
    out
}
