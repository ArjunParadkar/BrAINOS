//! API key reader/writer — the ANTHROPIC record on p3 (BRAINOS-SECURE).
//!
//! p3 is the software-emulated secure element minted by tools/make_key.py.
//! Its format (canonical, defined there) is a flat sequence of TLV records:
//!
//!   record := magic[4] "BSEC" | type u16 LE | flags u16 LE | length u32 LE | value[length]
//!
//!   type 1  BRAINKEY   the brain key record (UTF-8 JSON, public)
//!   type 2  ANTHROPIC  the raw API key bytes (SECRET — never logged)
//!
//! Records are packed from offset 0; the first byte that is not 'B' ends the
//! sequence; the area is zero-padded. The core never reads p3; the body's
//! operator reads it once at start and rewrites it on `set_key`.

use anyhow::{bail, Context, Result};
use std::path::Path;

const SEC_MAGIC: &[u8; 4] = b"BSEC";
const SEC_T_ANTHROPIC: u16 = 2;
const HDR: usize = 12;
const P3_SIZE: usize = 4 * 1024 * 1024;

/// Try to read the ANTHROPIC_API_KEY from multiple sources in order:
///   1. p3 secure element (the production path)
///   2. Environment variable ANTHROPIC_API_KEY (dev host convenience)
///   3. Config key_file (plain text, one line)
///   4. ~/.brainos_api_key (dev host fallback)
///
/// Returns None if no key is found anywhere (fast+reflex tiers still work).
pub fn load_api_key(key_device: &str, key_file: &str) -> Option<String> {
    if !key_device.is_empty() && Path::new(key_device).exists() {
        match read_p3_key(Path::new(key_device)) {
            Ok(k) => {
                tracing::info!("[key] loaded from secure element {}", key_device);
                return Some(k);
            }
            Err(e) => tracing::warn!("[key] secure element: {e}"),
        }
    }
    if let Ok(k) = std::env::var("ANTHROPIC_API_KEY") {
        let k = k.trim().to_string();
        if looks_like_key(&k) {
            tracing::info!("[key] loaded from ANTHROPIC_API_KEY env");
            return Some(k);
        }
    }
    if !key_file.is_empty() {
        if let Some(k) = read_plaintext_key(Path::new(key_file)) {
            tracing::info!("[key] loaded from config key_file {}", key_file);
            return Some(k);
        }
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let dev_path = Path::new(&home).join(".brainos_api_key");
    if dev_path.exists() {
        if let Some(k) = read_plaintext_key(&dev_path) {
            tracing::info!("[key] loaded from ~/.brainos_api_key (dev fallback)");
            return Some(k);
        }
    }
    tracing::warn!("[key] no API key found — deliberate tier unavailable");
    None
}

fn looks_like_key(k: &str) -> bool {
    k.starts_with("sk-") && k.len() > 20
}

fn read_plaintext_key(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let k = text.lines().next()?.trim().to_string();
    if looks_like_key(&k) { Some(k) } else { None }
}

/// Parse the BSEC record sequence. Returns (type, value) pairs in order.
pub fn parse_records(raw: &[u8]) -> Vec<(u16, Vec<u8>)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + HDR <= raw.len() && &raw[i..i + 4] == SEC_MAGIC {
        let rtype = u16::from_le_bytes([raw[i + 4], raw[i + 5]]);
        let len = u32::from_le_bytes([raw[i + 8], raw[i + 9], raw[i + 10], raw[i + 11]]) as usize;
        let start = i + HDR;
        if start + len > raw.len() {
            break;
        }
        out.push((rtype, raw[start..start + len].to_vec()));
        i = start + len;
    }
    out
}

fn record(rtype: u16, value: &[u8]) -> Vec<u8> {
    let mut r = Vec::with_capacity(HDR + value.len());
    r.extend_from_slice(SEC_MAGIC);
    r.extend_from_slice(&rtype.to_le_bytes());
    r.extend_from_slice(&0u16.to_le_bytes());
    r.extend_from_slice(&(value.len() as u32).to_le_bytes());
    r.extend_from_slice(value);
    r
}

fn read_p3_raw(device: &Path) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut f = std::fs::File::open(device).context("opening secure element")?;
    let mut raw = vec![0u8; P3_SIZE];
    let mut got = 0;
    while got < P3_SIZE {
        let n = f.read(&mut raw[got..])?;
        if n == 0 {
            break;
        }
        got += n;
    }
    raw.truncate(got);
    Ok(raw)
}

fn read_p3_key(device: &Path) -> Result<String> {
    let raw = read_p3_raw(device)?;
    for (t, v) in parse_records(&raw) {
        if t == SEC_T_ANTHROPIC {
            let k = String::from_utf8_lossy(&v).trim().to_string();
            if looks_like_key(&k) {
                return Ok(k);
            }
        }
    }
    bail!("no ANTHROPIC record in the secure element")
}

/// Store (or replace) the ANTHROPIC record on p3, keeping every other record.
pub fn write_api_key_to_p3(device: &Path, key: &str) -> Result<()> {
    use std::io::{Seek, SeekFrom, Write};
    let raw = read_p3_raw(device).unwrap_or_default();
    let size = if raw.is_empty() { P3_SIZE } else { raw.len() };
    let mut out = Vec::new();
    for (t, v) in parse_records(&raw) {
        if t != SEC_T_ANTHROPIC {
            out.extend_from_slice(&record(t, &v));
        }
    }
    out.extend_from_slice(&record(SEC_T_ANTHROPIC, key.trim().as_bytes()));
    if out.len() > size {
        bail!("secure records exceed the secure element");
    }
    out.resize(size, 0);
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .open(device)
        .context("opening secure element for write")?;
    f.seek(SeekFrom::Start(0))?;
    f.write_all(&out)?;
    f.sync_all()?;
    tracing::info!("[key] stored in secure element {}", device.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_matches_make_key_format() {
        let dir = std::env::temp_dir().join(format!("brsec-{}", std::process::id()));
        std::fs::write(&dir, {
            let mut v = record(1, br#"{"pub":"abc"}"#);
            v.resize(64 * 1024, 0);
            v
        })
        .unwrap();
        write_api_key_to_p3(&dir, "sk-ant-test-0123456789abcdef").unwrap();
        let raw = std::fs::read(&dir).unwrap();
        assert_eq!(raw.len(), 64 * 1024);
        let recs = parse_records(&raw);
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[0].0, 1);
        assert_eq!(read_p3_key(&dir).unwrap(), "sk-ant-test-0123456789abcdef");
        // replacing keeps one ANTHROPIC record only
        write_api_key_to_p3(&dir, "sk-ant-test-fedcba9876543210").unwrap();
        let raw = std::fs::read(&dir).unwrap();
        assert_eq!(parse_records(&raw).len(), 2);
        assert_eq!(read_p3_key(&dir).unwrap(), "sk-ant-test-fedcba9876543210");
        std::fs::remove_file(&dir).unwrap();
    }
}
