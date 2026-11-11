//! BFS  LEVEL    Foundation - the wire (frames, sessions, links)  [crate brainos-transport]
//! BFS  PURPOSE  The on-wire message layout (magic, kind, counter, length, payload, 16-byte keyed checksum) and how to seal one and open one safely
//! BFS  DEFINES  MAGIC, HEADER_LEN, MAC_LEN, OVERHEAD, MAX_PAYLOAD, MAX_FRAME, Kind, FrameError, Frame
//! BFS  USES     brainos: crypto
//! BFS  USED BY  bodyd/protocol, bodyd/tests/bodyd_suite, host/bench, host/demo, host/fullstack, host/limits, host/tests/fullstack_suite, nostd/lib, +5 more
//!
//! Framing.
//!
//! ```text
//!   magic(2) | kind(1) | flags(1) | counter(8) | len(2) | payload | mac(16)
//! ```
//!
//! Fixed-width, little-endian, 31 bytes of overhead. The counter is in
//! the header rather than the payload because it is authenticated
//! separately from what it carries: a receiver checks freshness before
//! it looks at content, so a replayed frame costs one comparison rather
//! than a parse.
//!
//! The MAC is truncated to 16 bytes. That is 128 bits of forgery
//! resistance on a link carrying a few hundred frames a second, which is
//! not close to a concern, and it halves the overhead on a 20-byte servo
//! command.

use brainos_crypto::hmac::HmacSha256;
use brainos_crypto::sha256::Hash;

pub const MAGIC: [u8; 2] = *b"BZ";
pub const HEADER_LEN: usize = 2 + 1 + 1 + 8 + 2;
pub const MAC_LEN: usize = 16;
pub const OVERHEAD: usize = HEADER_LEN + MAC_LEN;
/// Sized so a whole frame fits in one 512-byte serial buffer with room
/// to spare, and so a hostile length field cannot ask for more.
pub const MAX_PAYLOAD: usize = 448;
pub const MAX_FRAME: usize = OVERHEAD + MAX_PAYLOAD;

/// What a frame is for. The set is small because a transport that grows
/// message types grows opinions.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Kind {
    /// the attestation exchange
    Hello = 0,
    /// the answer to a challenge
    Attest = 1,
    /// a command going down to the body
    Command = 2,
    /// telemetry coming up from the body
    Telemetry = 3,
    /// a capability schema
    Schema = 4,
    /// a calibration push
    Calibration = 5,
    /// keepalive; carries nothing
    Ping = 6,
    /// the answer to a ping
    Pong = 7,
    /// the body stopped itself and is saying why
    Stopped = 8,
    /// an orderly goodbye
    Bye = 9,
}

impl Kind {
    pub const ALL: [Kind; 10] = [
        Kind::Hello,
        Kind::Attest,
        Kind::Command,
        Kind::Telemetry,
        Kind::Schema,
        Kind::Calibration,
        Kind::Ping,
        Kind::Pong,
        Kind::Stopped,
        Kind::Bye,
    ];

    pub fn from_u8(v: u8) -> Option<Kind> {
        Kind::ALL.get(v as usize).copied()
    }

    pub const fn name(self) -> &'static str {
        match self {
            Kind::Hello => "hello",
            Kind::Attest => "attest",
            Kind::Command => "command",
            Kind::Telemetry => "telemetry",
            Kind::Schema => "schema",
            Kind::Calibration => "calibration",
            Kind::Ping => "ping",
            Kind::Pong => "pong",
            Kind::Stopped => "stopped",
            Kind::Bye => "bye",
        }
    }

    /// May this kind arrive before the session is attested?
    ///
    /// Only the handshake itself. Everything else is refused until both
    /// sides have proved who they are — which is what stops a device on
    /// the same bus from sending commands to a body that is listening.
    pub const fn pre_attestation(self) -> bool {
        matches!(self, Kind::Hello | Kind::Attest)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FrameError {
    /// fewer bytes than a frame can be
    Short,
    /// the magic is not ours
    NotAFrame,
    /// the declared length exceeds the buffer or the maximum
    BadLength,
    /// the kind byte is not one we know
    UnknownKind,
    /// the MAC did not verify
    Forged,
    /// the counter is not ahead of the last one seen
    Replayed { got: u64, expected_above: u64 },
    /// the payload is bigger than a frame may carry
    TooLarge,
}

/// One frame.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub kind: Kind,
    pub flags: u8,
    pub counter: u64,
    pub payload: alloc::vec::Vec<u8>,
}

impl Frame {
    pub fn new(kind: Kind, counter: u64, payload: &[u8]) -> Result<Frame, FrameError> {
        if payload.len() > MAX_PAYLOAD {
            return Err(FrameError::TooLarge);
        }
        Ok(Frame {
            kind,
            flags: 0,
            counter,
            payload: payload.to_vec(),
        })
    }

    pub fn ping(counter: u64) -> Frame {
        Frame {
            kind: Kind::Ping,
            flags: 0,
            counter,
            payload: alloc::vec::Vec::new(),
        }
    }

    fn header(&self) -> [u8; HEADER_LEN] {
        let mut h = [0u8; HEADER_LEN];
        h[0..2].copy_from_slice(&MAGIC);
        h[2] = self.kind as u8;
        h[3] = self.flags;
        h[4..12].copy_from_slice(&self.counter.to_le_bytes());
        h[12..14].copy_from_slice(&(self.payload.len() as u16).to_le_bytes());
        h
    }

    fn mac(key: &[u8], header: &[u8], payload: &[u8]) -> [u8; MAC_LEN] {
        let mut h = HmacSha256::new(key);
        h.update(header);
        h.update(payload);
        let full: Hash = h.finish();
        let mut out = [0u8; MAC_LEN];
        out.copy_from_slice(&full.as_bytes()[..MAC_LEN]);
        out
    }

    /// Serialize and authenticate.
    pub fn seal(&self, key: &[u8]) -> alloc::vec::Vec<u8> {
        let header = self.header();
        let mac = Frame::mac(key, &header, &self.payload);
        let mut out = alloc::vec::Vec::with_capacity(OVERHEAD + self.payload.len());
        out.extend_from_slice(&header);
        out.extend_from_slice(&self.payload);
        out.extend_from_slice(&mac);
        out
    }

    /// Parse and verify. Returns the frame and how many bytes it used,
    /// so a stream reader can advance.
    ///
    /// The checks are in cost order — magic, length, MAC — because the
    /// cheap ones eliminate almost all garbage, and a receiver on a
    /// microcontroller that MACs every byte of noise on a floating serial
    /// line is a receiver that misses its deadline.
    pub fn open(raw: &[u8], key: &[u8]) -> Result<(Frame, usize), FrameError> {
        if raw.len() < OVERHEAD {
            return Err(FrameError::Short);
        }
        if raw[0..2] != MAGIC {
            return Err(FrameError::NotAFrame);
        }
        let kind = Kind::from_u8(raw[2]).ok_or(FrameError::UnknownKind)?;
        let flags = raw[3];
        let counter = u64::from_le_bytes([
            raw[4], raw[5], raw[6], raw[7], raw[8], raw[9], raw[10], raw[11],
        ]);
        let len = u16::from_le_bytes([raw[12], raw[13]]) as usize;
        if len > MAX_PAYLOAD {
            return Err(FrameError::BadLength);
        }
        let total = OVERHEAD + len;
        if raw.len() < total {
            return Err(FrameError::Short);
        }

        let header = &raw[..HEADER_LEN];
        let payload = &raw[HEADER_LEN..HEADER_LEN + len];
        let mac = &raw[HEADER_LEN + len..total];

        let expected = Frame::mac(key, header, payload);
        // constant time: a receiver that leaks how much of a MAC matched
        // is a receiver that can be searched
        let mut diff = 0u8;
        for i in 0..MAC_LEN {
            diff |= expected[i] ^ mac[i];
        }
        if diff != 0 {
            return Err(FrameError::Forged);
        }

        Ok((
            Frame {
                kind,
                flags,
                counter,
                payload: payload.to_vec(),
            },
            total,
        ))
    }

    /// Find the start of the next plausible frame in a buffer that has
    /// lost sync — a serial line that dropped bytes, which is the normal
    /// state of a serial line.
    pub fn resync(raw: &[u8]) -> Option<usize> {
        raw.windows(2).position(|w| w == MAGIC)
    }
}
