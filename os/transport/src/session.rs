//! BFS  LEVEL    Foundation - the wire (frames, sessions, links)  [crate brainos-transport]
//! BFS  PURPOSE  The handshake where entity and body prove who they are and derive a shared key; before it only handshake frames pass, after it no replays
//! BFS  DEFINES  SessionKey, WireError, Peer, Wire
//! BFS  USES     this crate: frame | brainos: abi, crypto
//! BFS  USED BY  host/demo, host/fullstack, proof/adversary, transport/lib, transport/tests/transport_suite
//!
//! Attested sessions.
//!
//! A session is a key and a counter. It exists after both sides have
//! proved who they are, and it dies when either stops answering. The
//! important property is the negative one: **nothing but the handshake
//! is accepted before the session exists**, so a device sharing a bus
//! with a body cannot send it a command by getting there first.
//!
//! The session key is derived, not exchanged. Both sides compute
//! `HKDF(Key, body-id ‖ session-nonce)` — the entity from its BrAIn Key,
//! the body from the subkey it was provisioned with — so nothing secret
//! crosses the wire at all. A recorded handshake replayed later produces
//! a different session nonce and therefore a key that decrypts nothing.

use crate::frame::{Frame, FrameError, Kind};
use alloc::vec::Vec;
use brainos_abi::ids::{BodyId, EntityId, SessionId};
use brainos_abi::time::Instant;
use brainos_crypto::hkdf::hkdf_sha256;
use brainos_crypto::key::{BrainKey, Challenge, ChallengePurpose, PublicKey, Signature};
use brainos_crypto::rng::Drbg;

pub type SessionKey = Vec<u8>;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WireError {
    /// a frame arrived that is not allowed before attestation
    NotAttested,
    /// the frame did not verify
    Frame(FrameError),
    /// the peer's attestation failed
    PeerUnattested,
    /// the peer refused ours
    RefusedByPeer,
    /// the session has been closed
    Closed,
    /// the handshake was replayed
    StaleHandshake,
}

/// The other end.
#[derive(Clone, Copy, Debug)]
pub struct Peer {
    pub entity: Option<EntityId>,
    pub body: Option<BodyId>,
    pub key: PublicKey,
}

/// One attested link between an entity and a body.
pub struct Wire {
    pub session: SessionId,
    key: SessionKey,
    /// the next counter we will send
    tx_counter: u64,
    /// the highest counter we have accepted
    rx_high_water: u64,
    attested: bool,
    closed: bool,
    pub peer: Option<Peer>,
    pub opened_at: Instant,
    pub frames_sent: u64,
    pub frames_received: u64,
    pub frames_refused: u64,
    pub replays_refused: u64,
}

impl Wire {
    /// Begin a session as the entity. The nonce is fresh, so the derived
    /// key is unique to this session and a recorded handshake is useless.
    pub fn initiate(
        key: &BrainKey,
        body: BodyId,
        rng: &mut Drbg,
        now: Instant,
    ) -> (Wire, [u8; 32]) {
        let nonce = rng.nonce32();
        let session_key = derive(key, &body, &nonce);
        let mut sid = [0u8; 32];
        sid.copy_from_slice(&nonce);
        (
            Wire {
                session: SessionId::from_bytes(sid),
                key: session_key,
                tx_counter: 1,
                rx_high_water: 0,
                attested: false,
                closed: false,
                peer: None,
                opened_at: now,
                frames_sent: 0,
                frames_received: 0,
                frames_refused: 0,
                replays_refused: 0,
            },
            nonce,
        )
    }

    /// Begin a session as the body, from the nonce the entity sent and
    /// the subkey this body was provisioned with.
    pub fn accept(subkey: &[u8], body: BodyId, nonce: [u8; 32], now: Instant) -> Wire {
        let session_key = derive_from_subkey(subkey, &body, &nonce);
        let mut sid = [0u8; 32];
        sid.copy_from_slice(&nonce);
        Wire {
            session: SessionId::from_bytes(sid),
            key: session_key,
            tx_counter: 1,
            rx_high_water: 0,
            attested: false,
            closed: false,
            peer: None,
            opened_at: now,
            frames_sent: 0,
            frames_received: 0,
            frames_refused: 0,
            replays_refused: 0,
        }
    }

    pub fn is_attested(&self) -> bool {
        self.attested && !self.closed
    }

    pub fn is_closed(&self) -> bool {
        self.closed
    }

    pub fn close(&mut self) {
        self.closed = true;
    }

    /// Mark the session attested. Called once, after the challenge
    /// exchange has succeeded in both directions.
    pub fn attested(&mut self, peer: Peer) {
        self.attested = true;
        self.peer = Some(peer);
    }

    /// Build the challenge this side sends.
    pub fn challenge(&self, asker: EntityId, rng: &mut Drbg) -> Challenge {
        Challenge::new(
            rng.nonce32(),
            ChallengePurpose::BodyHandshake,
            asker,
        )
    }

    /// Check an answer to a challenge.
    pub fn check_answer(c: &Challenge, peer: &PublicKey, sig: &Signature) -> bool {
        c.check(peer, sig)
    }

    /// Wrap a payload for sending.
    pub fn send(&mut self, kind: Kind, payload: &[u8]) -> Result<Vec<u8>, WireError> {
        if self.closed {
            return Err(WireError::Closed);
        }
        if !self.attested && !kind.pre_attestation() {
            return Err(WireError::NotAttested);
        }
        let f = Frame::new(kind, self.tx_counter, payload).map_err(WireError::Frame)?;
        self.tx_counter = self.tx_counter.saturating_add(1);
        self.frames_sent = self.frames_sent.saturating_add(1);
        Ok(f.seal(&self.key))
    }

    /// Verify and accept an arriving frame.
    ///
    /// Three refusals in order, and the order is the safety argument:
    /// closed, then forged, then replayed. Checking freshness before
    /// authenticity would let an unauthenticated party advance our
    /// counter and lock out the real one.
    pub fn receive(&mut self, raw: &[u8]) -> Result<Frame, WireError> {
        if self.closed {
            return Err(WireError::Closed);
        }
        let (frame, _) = Frame::open(raw, &self.key).map_err(|e| {
            self.frames_refused = self.frames_refused.saturating_add(1);
            WireError::Frame(e)
        })?;

        if frame.counter <= self.rx_high_water {
            self.replays_refused = self.replays_refused.saturating_add(1);
            self.frames_refused = self.frames_refused.saturating_add(1);
            return Err(WireError::Frame(FrameError::Replayed {
                got: frame.counter,
                expected_above: self.rx_high_water,
            }));
        }

        if !self.attested && !frame.kind.pre_attestation() {
            self.frames_refused = self.frames_refused.saturating_add(1);
            return Err(WireError::NotAttested);
        }

        self.rx_high_water = frame.counter;
        self.frames_received = self.frames_received.saturating_add(1);
        Ok(frame)
    }

    pub fn tx_counter(&self) -> u64 {
        self.tx_counter
    }

    pub fn rx_high_water(&self) -> u64 {
        self.rx_high_water
    }
}

fn derive(key: &BrainKey, body: &BodyId, nonce: &[u8; 32]) -> SessionKey {
    let subkey = key.derive_subkey(body, 32);
    derive_from_subkey(&subkey, body, nonce)
}

fn derive_from_subkey(subkey: &[u8], body: &BodyId, nonce: &[u8; 32]) -> SessionKey {
    let mut info = Vec::with_capacity(64);
    info.extend_from_slice(b"brainos/v1/wire");
    info.extend_from_slice(body.as_bytes());
    hkdf_sha256(nonce, subkey, &info, 32)
}
