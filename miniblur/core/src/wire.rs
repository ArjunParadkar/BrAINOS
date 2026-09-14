//! The wire protocol — exactly what `reach/brainos-reach.py` speaks, so the
//! seat's `reach.rs` incorporates the hexapod with no special case, and the
//! simulator and the real board are interchangeable to the seat.
//!
//! ```text
//! body → {"t":"hello","name":"miniblur","nonce":"<64 hex>","os":"esp32s3","hands":[..],"schema":{..}}
//! seat → {"t":"attest","pub":"<64 hex>","sig":"<128 hex>"}     sig = ed25519(nonce + "|" + name)
//! body → {"t":"ready"}  |  {"t":"refused","reason":".."}
//! seat → {"t":"cmd","id":N,"verb":"gait.walk","input":{"dir":"fwd","steps":3,"speed":0.5}}
//! body → {"t":"result","id":N,"ok":true,"digest":".."}
//! either → {"t":"ping"} / {"t":"pong"}
//! ```
//! Newline-framed JSON, one frame per line, over TCP or a serial line.

use crate::json::{self, push_str};
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;

/// A frame arriving at the body from the seat.
#[derive(Clone, Debug, PartialEq)]
pub enum FromSeat {
    Attest { pub_hex: String, sig_hex: String },
    /// `input` is the raw JSON object slice, scanned by the handler.
    Cmd { id: u64, verb: String, input: String },
    Ping,
    Pong,
    Bye { reason: String },
    Unknown(String),
}

/// Parse one line from the seat.
pub fn parse(line: &str) -> FromSeat {
    let l = line.trim();
    match json::get_str(l, "t").as_deref() {
        Some("attest") => FromSeat::Attest {
            pub_hex: json::get_str(l, "pub").unwrap_or_default(),
            sig_hex: json::get_str(l, "sig").unwrap_or_default(),
        },
        Some("cmd") => FromSeat::Cmd {
            id: json::get_u64(l, "id").unwrap_or(0),
            verb: json::get_str(l, "verb").unwrap_or_default(),
            input: json::get_obj(l, "input").map(String::from).unwrap_or_else(|| String::from("{}")),
        },
        Some("ping") => FromSeat::Ping,
        Some("pong") => FromSeat::Pong,
        Some("bye") => FromSeat::Bye { reason: json::get_str(l, "reason").unwrap_or_default() },
        other => FromSeat::Unknown(String::from(other.unwrap_or("?"))),
    }
}

/// The challenge the seat signs. Mirrors `reach.rs::challenge_bytes` and
/// brainos-reach.py; a change here is a protocol change.
pub fn challenge_bytes(nonce_hex: &str, name: &str) -> Vec<u8> {
    let mut v = Vec::with_capacity(nonce_hex.len() + 1 + name.len());
    v.extend_from_slice(nonce_hex.as_bytes());
    v.push(b'|');
    v.extend_from_slice(name.as_bytes());
    v
}

pub fn hello(name: &str, nonce_hex: &str, os: &str, hands: &[&str], schema_json: &str) -> String {
    let mut o = String::from("{\"t\":\"hello\",\"name\":");
    push_str(&mut o, name);
    o.push_str(",\"nonce\":");
    push_str(&mut o, nonce_hex);
    o.push_str(",\"os\":");
    push_str(&mut o, os);
    o.push_str(",\"hands\":[");
    for (i, h) in hands.iter().enumerate() {
        if i > 0 {
            o.push(',');
        }
        push_str(&mut o, h);
    }
    o.push_str("],\"schema\":");
    o.push_str(schema_json);
    o.push_str("}\n");
    o
}

pub fn ready() -> &'static str {
    "{\"t\":\"ready\"}\n"
}

pub fn refused(reason: &str) -> String {
    let mut o = String::from("{\"t\":\"refused\",\"reason\":");
    push_str(&mut o, reason);
    o.push_str("}\n");
    o
}

pub fn result(id: u64, ok: bool, digest: &str) -> String {
    let mut o = String::new();
    let _ = write!(o, "{{\"t\":\"result\",\"id\":{id},\"ok\":{ok},\"digest\":");
    push_str(&mut o, digest);
    o.push_str("}\n");
    o
}

pub fn ping() -> &'static str {
    "{\"t\":\"ping\"}\n"
}

pub fn pong() -> &'static str {
    "{\"t\":\"pong\"}\n"
}

/// Lower-case hex → bytes. None on odd length or a non-hex digit.
pub fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let hi = (b[i] as char).to_digit(16)?;
        let lo = (b[i + 1] as char).to_digit(16)?;
        out.push(((hi << 4) | lo) as u8);
        i += 2;
    }
    Some(out)
}

pub fn hex_encode(b: &[u8]) -> String {
    let mut o = String::with_capacity(b.len() * 2);
    for x in b {
        let _ = write!(o, "{:02x}", x);
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_seat_frame() {
        assert_eq!(parse("{\"t\":\"ping\"}"), FromSeat::Ping);
        assert_eq!(parse("{\"t\":\"pong\"}\n"), FromSeat::Pong);
        match parse(r#"{"t":"attest","pub":"ab","sig":"cd"}"#) {
            FromSeat::Attest { pub_hex, sig_hex } => {
                assert_eq!(pub_hex, "ab");
                assert_eq!(sig_hex, "cd");
            }
            o => panic!("{o:?}"),
        }
        match parse(r#"{"t":"cmd","id":7,"verb":"gait.walk","input":{"dir":"fwd","steps":3}}"#) {
            FromSeat::Cmd { id, verb, input } => {
                assert_eq!(id, 7);
                assert_eq!(verb, "gait.walk");
                assert_eq!(json::get_str(&input, "dir").as_deref(), Some("fwd"));
            }
            o => panic!("{o:?}"),
        }
        assert!(matches!(parse("{\"t\":\"nope\"}"), FromSeat::Unknown(_)));
        assert!(matches!(parse("garbage"), FromSeat::Unknown(_)));
    }

    #[test]
    fn hello_is_one_line_the_seat_can_scan() {
        let h = hello("miniblur", "00ff", "esp32s3", &["gait.walk", "gait.stop"], "{\"v\":1}");
        assert!(h.ends_with('\n'));
        assert_eq!(h.matches('\n').count(), 1);
        let l = h.trim();
        assert_eq!(json::get_str(l, "t").as_deref(), Some("hello"));
        assert_eq!(json::get_str(l, "name").as_deref(), Some("miniblur"));
        assert_eq!(json::get_obj(l, "hands"), Some("[\"gait.walk\",\"gait.stop\"]"));
        assert_eq!(json::get_obj(l, "schema"), Some("{\"v\":1}"));
    }

    #[test]
    fn challenge_matches_the_seat() {
        assert_eq!(challenge_bytes("ab12", "pi"), b"ab12|pi".to_vec());
    }

    #[test]
    fn hex_round_trips_and_rejects_junk() {
        assert_eq!(hex_decode("00ff10"), Some(alloc::vec![0, 255, 16]));
        assert_eq!(hex_decode("0"), None);
        assert_eq!(hex_decode("zz"), None);
        assert_eq!(hex_encode(&[0, 255, 16]), "00ff10");
    }

    #[test]
    fn results_escape_digests() {
        let r = result(3, true, "walked \"3\"\nsteps");
        assert_eq!(json::get_u64(r.trim(), "id"), Some(3));
        assert_eq!(json::get_bool(r.trim(), "ok"), Some(true));
        assert_eq!(json::get_str(r.trim(), "digest").as_deref(), Some("walked \"3\"\nsteps"));
    }
}
