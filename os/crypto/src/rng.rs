//! BFS  LEVEL    Domain 1 Silicon - the BrAIn Key, hashes, Merkle log, quorum  [crate brainos-crypto]
//! BFS  PURPOSE  Deterministic random generator: each platform seeds it once with at least 32 bytes of entropy and it supplies every nonce after that
//! BFS  DEFINES  Drbg
//! BFS  USES     this crate: hmac, sha256 | brainos: abi
//! BFS  USED BY  blur/acquire, blur/tests/blur_suite, body/acquire, body/tests/body_suite, crypto/lib, crypto/tests/crypto_suite, entity/entity, host/blur_gate2, +12 more
//!
//! A hash-based DRBG.
//!
//! Nonces need to be unpredictable and the platforms this runs on do not
//! all have a `getrandom`. A UEFI core has whatever entropy it scraped at
//! boot; an ESP32 has a hardware RNG behind a peripheral register; a host
//! has the OS. Rather than have each of them differ, every platform
//! supplies *seed material* once and this deterministic generator
//! supplies every nonce after that.
//!
//! The construction is HMAC-DRBG's shape without the full NIST
//! reseed-counter machinery: state `v` and key `k`, each output
//! ratcheting both forward, so a leaked output does not reveal the
//! previous ones (backtracking resistance) and a leaked state does not
//! reveal the past.
//!
//! What it is **not**: a source of entropy. Seed it badly and it produces
//! predictable nonces forever. `from_entropy` therefore takes at least 32
//! bytes and refuses less, rather than stretching whatever it was given.

use crate::hmac::HmacSha256;
use crate::sha256::HASH_LEN;
use brainos_abi::error::{Error, Result};

pub struct Drbg {
    k: [u8; HASH_LEN],
    v: [u8; HASH_LEN],
    /// outputs since the last reseed; a caller can watch this to know
    /// when to feed more entropy in
    counter: u64,
}

impl Drbg {
    /// Seed. Refuses fewer than 32 bytes: stretching four bytes of
    /// entropy into a stream that *looks* random is exactly the failure
    /// this refusal exists to make impossible.
    pub fn from_entropy(seed: &[u8]) -> Result<Drbg> {
        if seed.len() < 32 {
            return Err(Error::Crypto("drbg needs at least 32 bytes of entropy"));
        }
        let mut d = Drbg {
            k: [0x00; HASH_LEN],
            v: [0x01; HASH_LEN],
            counter: 0,
        };
        d.update(Some(seed));
        Ok(d)
    }

    /// Deterministic seeding, for tests and for the proof harness where
    /// reproducibility is the point. Never used on a body.
    pub fn deterministic(label: &[u8]) -> Drbg {
        let mut seed = [0u8; 32];
        let h = crate::sha256::sha256(label);
        seed.copy_from_slice(h.as_bytes());
        let mut d = Drbg {
            k: [0x00; HASH_LEN],
            v: [0x01; HASH_LEN],
            counter: 0,
        };
        d.update(Some(&seed));
        d
    }

    fn update(&mut self, provided: Option<&[u8]>) {
        let mut h = HmacSha256::new(&self.k);
        h.update(&self.v);
        h.update(&[0x00]);
        if let Some(p) = provided {
            h.update(p);
        }
        self.k = *h.finish().as_bytes();

        self.v = *HmacSha256::mac(&self.k, &self.v).as_bytes();

        if let Some(p) = provided {
            let mut h = HmacSha256::new(&self.k);
            h.update(&self.v);
            h.update(&[0x01]);
            h.update(p);
            self.k = *h.finish().as_bytes();
            self.v = *HmacSha256::mac(&self.k, &self.v).as_bytes();
        }
    }

    /// Mix in fresh entropy without discarding what is already there.
    pub fn reseed(&mut self, extra: &[u8]) {
        self.update(Some(extra));
        self.counter = 0;
    }

    pub fn outputs_since_reseed(&self) -> u64 {
        self.counter
    }

    pub fn fill(&mut self, out: &mut [u8]) {
        let mut written = 0;
        while written < out.len() {
            self.v = *HmacSha256::mac(&self.k, &self.v).as_bytes();
            let take = (out.len() - written).min(HASH_LEN);
            out[written..written + take].copy_from_slice(&self.v[..take]);
            written += take;
        }
        // ratchet: the state that produced this output cannot be
        // recovered from the state that follows it
        self.update(None);
        self.counter = self.counter.saturating_add(1);
    }

    pub fn nonce32(&mut self) -> [u8; 32] {
        let mut n = [0u8; 32];
        self.fill(&mut n);
        n
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut b = [0u8; 8];
        self.fill(&mut b);
        u64::from_le_bytes(b)
    }

    /// A uniform value in `0..n`, by rejection. Modulo would bias the
    /// low values, which matters in the proof harness where a biased
    /// sampler means unexplored states reported as explored.
    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 {
            return 0;
        }
        let zone = u64::MAX - (u64::MAX % n);
        loop {
            let v = self.next_u64();
            if v < zone {
                return v % n;
            }
        }
    }
}
