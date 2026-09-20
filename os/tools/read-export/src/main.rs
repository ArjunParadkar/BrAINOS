//! A standalone reader for a BrAInOS entity export.
//!
//! ```sh
//! cargo run -p read-export -- entity.brainex
//! ```
//!
//! **Why this exists.** The freedom principle says the customer owns
//! their entity permanently and that leaving means abandoning a life
//! rather than losing access to one. That is easy to write in a document
//! and worth very little unless somebody can actually read the file
//! without us.
//!
//! So this program depends on no BrAInOS crate. Its whole dependency
//! list is `ed25519-dalek`, because nobody should hand-roll a signature
//! scheme; SHA-256 is implemented below from FIPS 180-4 in about eighty
//! lines so the reader does not even need that. Everything else — the
//! layout, the Merkle construction, the domain tags, the node encoding —
//! is implemented from the specification that ships *inside every
//! export*, in its `readme` section.
//!
//! It verifies the signature, recomputes the Merkle root, and prints
//! what the entity remembers. If BrAIn Corp ceased to exist tomorrow,
//! this file and an export would be enough. Jibo's owners learned the
//! inverse in 2019.

use std::env;
use std::fs;
use std::process::ExitCode;

// =====================================================================
// SHA-256, from FIPS 180-4
// =====================================================================

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

fn sha256(data: &[u8]) -> [u8; 32] {
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut msg = data.to_vec();
    let bit_len = (data.len() as u64) * 8;
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    for block in msg.chunks(64) {
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
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (slot, v) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *slot = slot.wrapping_add(v);
        }
    }

    let mut out = [0u8; 32];
    for (i, v) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes());
    }
    out
}

/// `tag ‖ 0x00 ‖ payload`, as the specification's domain-separation
/// rule describes.
fn tagged(tag: &str, payload: &[u8]) -> Vec<u8> {
    let mut m = Vec::with_capacity(tag.len() + 1 + payload.len());
    m.extend_from_slice(tag.as_bytes());
    m.push(0);
    m.extend_from_slice(payload);
    m
}

fn tagged_hash(tag: &str, fields: &[&[u8]]) -> [u8; 32] {
    let mut buf = tagged(tag, &[]);
    for f in fields {
        buf.extend_from_slice(&(f.len() as u64).to_le_bytes());
        buf.extend_from_slice(f);
    }
    sha256(&buf)
}

// =====================================================================
// the reader
// =====================================================================

struct Cursor<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        if self.b.len() < self.i + n {
            return Err(format!("truncated at byte {}", self.i));
        }
        let s = &self.b[self.i..self.i + n];
        self.i += n;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, String> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }
    fn u32(&mut self) -> Result<u32, String> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn u64(&mut self) -> Result<u64, String> {
        let b = self.take(8)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(b);
        Ok(u64::from_le_bytes(a))
    }
    fn bytes(&mut self) -> Result<&'a [u8], String> {
        let n = self.u32()? as usize;
        self.take(n)
    }
    fn string(&mut self) -> Result<String, String> {
        let b = self.bytes()?;
        String::from_utf8(b.to_vec()).map_err(|_| "not utf-8".to_string())
    }
    fn opt<T>(&mut self, f: impl FnOnce(&mut Self) -> Result<T, String>) -> Result<Option<T>, String> {
        match self.u8()? {
            0 => Ok(None),
            1 => f(self).map(Some),
            _ => Err("a presence byte must be 0 or 1".into()),
        }
    }
}

struct Section {
    kind: u8,
    bytes: Vec<u8>,
}

fn section_digest(s: &Section) -> [u8; 32] {
    tagged_hash("brainos/v1/export", &[&[s.kind], &s.bytes])
}

fn node(l: &[u8; 32], r: &[u8; 32]) -> [u8; 32] {
    let mut buf = tagged("brainos/v1/audit-node", &[]);
    buf.extend_from_slice(l);
    buf.extend_from_slice(r);
    sha256(&buf)
}

/// The Merkle root, exactly as the specification describes: a binary
/// tree over the section hashes, with the odd node promoted rather than
/// duplicated.
fn merkle_root(leaves: &[[u8; 32]]) -> [u8; 32] {
    if leaves.is_empty() {
        return [0u8; 32];
    }
    let mut level: Vec<[u8; 32]> = leaves.to_vec();
    while level.len() > 1 {
        let mut next = Vec::new();
        let mut i = 0;
        while i + 1 < level.len() {
            next.push(node(&level[i], &level[i + 1]));
            i += 2;
        }
        if i < level.len() {
            next.push(level[i]);
        }
        level = next;
    }
    level[0]
}

const ORIGINS: [&str; 7] = [
    "self",
    "body",
    "human",
    "internet",
    "peer",
    "model",
    "consolidation",
];
const KINDS: [&str; 9] = [
    "episode",
    "semantic",
    "belief",
    "skill",
    "calibration",
    "audit",
    "body",
    "goal",
    "known",
];

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn main() -> ExitCode {
    let Some(path) = env::args().nth(1) else {
        eprintln!(
            "usage: read-export <file.brainex>\n\n\
             A standalone reader for a BrAInOS entity export. Depends on no\n\
             BrAInOS crate: the layout comes from the specification that ships\n\
             inside the file."
        );
        return ExitCode::FAILURE;
    };
    let raw = match fs::read(&path) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("cannot read {path}: {e}");
            return ExitCode::FAILURE;
        }
    };

    match read(&raw) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("this is not a readable export: {e}");
            ExitCode::FAILURE
        }
    }
}

fn read(raw: &[u8]) -> Result<(), String> {
    let mut c = Cursor { b: raw, i: 0 };

    // --- header ---
    if c.take(8)? != b"BRAINEX1" {
        return Err("no BRAINEX1 magic".into());
    }
    let version = c.u16()?;
    if version != 1 {
        return Err(format!("format version {version}; this reader knows 1"));
    }

    // --- manifest ---
    let entity = c.take(32)?.to_vec();
    let exported_at = c.u64()?;
    let manifest_version = c.u16()?;
    let produced_by = c.string()?;
    let node_count = c.u32()?;
    let skill_count = c.u32()?;
    let body_count = c.u32()?;
    let wakes = c.u32()?;
    let age = c.u64()?;
    let omission_count = c.u32()?;
    let mut omissions = Vec::new();
    for _ in 0..omission_count {
        omissions.push(c.string()?);
    }
    let manifest_end = c.i;

    // --- sections ---
    let section_count = c.u32()?;
    let mut sections = Vec::new();
    for _ in 0..section_count {
        let kind = c.u8()?;
        let bytes = c.bytes()?.to_vec();
        sections.push(Section { kind, bytes });
    }

    let root = c.take(32)?.to_vec();
    let signature = c.take(64)?.to_vec();
    if c.i != raw.len() {
        return Err("trailing bytes after the signature".into());
    }

    println!("BrAInOS entity export");
    println!("=====================\n");
    println!("  entity        {}", hex(&entity));
    println!("  produced by   {produced_by}");
    println!("  format        v{manifest_version}");
    println!("  exported at   tick {exported_at}");
    println!("  age           {age} ticks over {wakes} wakes");
    println!("  contents      {node_count} memories, {skill_count} skills, {body_count} bodies");

    // --- check the Merkle root ---
    // The leaves are the section digests themselves. The archive appends
    // already-hashed sections to the log, so the audit-leaf tag does not
    // appear here — only the audit-node tag, when the tree is built.
    let leaves: Vec<[u8; 32]> = sections.iter().map(section_digest).collect();
    let recomputed = merkle_root(&leaves);
    let root_ok = recomputed.as_slice() == root.as_slice();
    println!(
        "\n  merkle root   {} ({})",
        hex(&root),
        if root_ok {
            "matches the sections present"
        } else {
            "DOES NOT MATCH — the file has been altered"
        }
    );

    // --- check the signature ---
    //
    // over SHA-256( tagged(manifest) ‖ root ), tagged again for the
    // signature itself, exactly as the specification says
    let manifest_bytes = &raw[10..manifest_end];
    let mut payload = tagged("brainos/v1/export", &[]);
    payload.extend_from_slice(&(manifest_bytes.len() as u64).to_le_bytes());
    payload.extend_from_slice(manifest_bytes);
    payload.extend_from_slice(&(root.len() as u64).to_le_bytes());
    payload.extend_from_slice(&root);
    let signed = sha256(&payload);

    let sig_ok = {
        use ed25519_dalek::{Signature, Verifier, VerifyingKey};
        let mut key_bytes = [0u8; 32];
        key_bytes.copy_from_slice(&entity);
        let mut sig_bytes = [0u8; 64];
        sig_bytes.copy_from_slice(&signature);
        match VerifyingKey::from_bytes(&key_bytes) {
            Ok(k) => k
                .verify(&tagged("brainos/v1/export", &signed), &Signature::from_bytes(&sig_bytes))
                .is_ok(),
            Err(_) => false,
        }
    };
    println!(
        "  signature     {} ({})",
        &hex(&signature)[..32],
        if sig_ok {
            "verifies under the entity's own key"
        } else {
            "DOES NOT VERIFY"
        }
    );

    if !omissions.is_empty() {
        println!("\n  what is deliberately not in this file:");
        for o in &omissions {
            println!("    - {o}");
        }
    }

    // --- the memories ---
    let Some(memory) = sections.iter().find(|s| s.kind == 0) else {
        return Err("no memory section".into());
    };
    let mut m = Cursor {
        b: &memory.bytes,
        i: 0,
    };
    let _next_id = m.u64()?;
    let count = m.u32()?;
    println!("\n  {count} memories:\n");

    let mut by_kind = [0usize; 9];
    let mut shown = 0;
    for _ in 0..count {
        let id = m.u64()?;
        let kind = m.u8()? as usize;
        let content = m.string()?;
        let confidence = m.u8()?;
        let _salience = m.u8()?;
        let origin = m.u8()? as usize;
        let _body = m.opt(|c| c.take(32).map(|b| b.to_vec()))?;
        let _peer = m.opt(|c| c.take(32).map(|b| b.to_vec()))?;
        let tick = m.u64()?;
        let _reinforced = m.u16()?;
        let edges = m.u32()?;
        for _ in 0..edges {
            let _to = m.u64()?;
            let _kind = m.u8()?;
        }
        let _retrievals = m.u32()?;

        if kind < by_kind.len() {
            by_kind[kind] += 1;
        }
        if shown < 12 {
            println!(
                "    #{id:<5} {:<12} {:>3}%  from {:<14} t{tick:<7} {}",
                KINDS.get(kind).copied().unwrap_or("?"),
                confidence,
                ORIGINS.get(origin).copied().unwrap_or("?"),
                content
            );
            shown += 1;
        }
    }
    if count as usize > shown {
        println!("    … and {} more", count as usize - shown);
    }

    println!("\n  by kind:");
    for (i, n) in by_kind.iter().enumerate() {
        if *n > 0 {
            println!("    {:<14} {n}", KINDS[i]);
        }
    }

    println!(
        "\n  Read with no BrAInOS code: this program's whole dependency list is
  ed25519-dalek, and SHA-256 is eighty lines above. The layout came from
  the specification that ships inside the file."
    );

    if root_ok && sig_ok {
        Ok(())
    } else {
        Err("the file did not verify".into())
    }
}
