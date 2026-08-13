//! The BrAIn Key — identity as cryptographic root (Architecture §5).
//!
//! UNIX users/permission bits are replaced by one keypair from which all
//! authority derives. The private seed lives on the key medium (phase 0:
//! software-emulated secure area; later: TPM / secure enclave — only this
//! module needs to change). The instance proves who it is by signing
//! challenges; KIRA mints capability tokens by signing grants with it.

use alloc::string::String;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};

pub struct BrainKey {
    signing: SigningKey,
    public: VerifyingKey,
    pub public_hex: String,
}

pub struct Attestation {
    pub signature: [u8; 64],
    pub verified: bool,
}

fn hex_nibble(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

fn parse_hex32(hex: &[u8]) -> Option<[u8; 32]> {
    if hex.len() < 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for i in 0..32 {
        out[i] = (hex_nibble(hex[2 * i])? << 4) | hex_nibble(hex[2 * i + 1])?;
    }
    Some(out)
}

pub fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        for n in [b >> 4, b & 0xF] {
            s.push(char::from(if n < 10 { b'0' + n } else { b'a' + n - 10 }));
        }
    }
    s
}

impl BrainKey {
    /// Load identity from the raw contents of KEY.SEED and KEY.PUB as read
    /// off the key medium. The derived public key must match the recorded
    /// one — a mismatch means the key material was tampered with.
    pub fn load(seed_hex: &[u8], pub_hex: &[u8]) -> Result<BrainKey, &'static str> {
        let seed = parse_hex32(seed_hex).ok_or("unreadable seed")?;
        let recorded = parse_hex32(pub_hex).ok_or("unreadable public key")?;
        let signing = SigningKey::from_bytes(&seed);
        let public = signing.verifying_key();
        if public.to_bytes() != recorded {
            return Err("seed does not derive the recorded identity");
        }
        Ok(BrainKey {
            public_hex: to_hex(&public.to_bytes()),
            signing,
            public,
        })
    }

    /// Mint an identity directly from a 32-byte seed. Provisioning-side
    /// twin of `load` (make_key.py does the same derivation); also what
    /// the host test suite uses to build real, signing instances.
    pub fn from_seed(seed: [u8; 32]) -> BrainKey {
        let signing = SigningKey::from_bytes(&seed);
        let public = signing.verifying_key();
        BrainKey {
            public_hex: to_hex(&public.to_bytes()),
            signing,
            public,
        }
    }

    /// Short display form of the identity: first 8 + last 4 hex chars.
    pub fn fingerprint(&self) -> String {
        let h = &self.public_hex;
        let mut s = String::new();
        s.push_str(&h[..8]);
        s.push_str("..");
        s.push_str(&h[h.len() - 4..]);
        s
    }

    pub fn sign(&self, msg: &[u8]) -> [u8; 64] {
        self.signing.sign(msg).to_bytes()
    }

    pub fn verify(&self, msg: &[u8], sig: &[u8; 64]) -> bool {
        self.public.verify(msg, &Signature::from_bytes(sig)).is_ok()
    }

    /// Boot attestation: sign a context string and immediately verify it —
    /// proof the silicon domain holds a working, self-consistent identity.
    pub fn attest(&self, context: &[u8]) -> Attestation {
        let signature = self.sign(context);
        Attestation {
            verified: self.verify(context, &signature),
            signature,
        }
    }
}
