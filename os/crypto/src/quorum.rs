//! BFS  LEVEL    Domain 1 Silicon - the BrAIn Key, hashes, Merkle log, quorum  [crate brainos-crypto]
//! BFS  PURPOSE  Collects k distinct member signatures out of n before an irreversible operation (deactivate, revoke body, export memory, rotate key)
//! BFS  DEFINES  QuorumError, Operation, Share, Statement, Quorum
//! BFS  USES     this crate: domain, key, sha256 | brainos: abi
//! BFS  USED BY  crypto/lib, crypto/tests/crypto_suite, proof/adversary
//!
//! Quorum operations — the fifth anchor of the Key (§3.2, "deactivation").
//!
//! Some operations should not be possible for one party acting alone,
//! even the owner, even us: ending an entity, revoking its identity,
//! exporting its complete memory to a third party. The architecture
//! calls this "multi-party key operations can revoke an instance".
//!
//! This is k-of-n signature collection, not threshold cryptography. The
//! distinction is honest and deliberate: real threshold ed25519 requires
//! a distributed key generation ceremony and a scheme whose security
//! proof we would be taking on faith. Collecting k independent
//! signatures over the same tagged statement gives the same operational
//! property — no single holder can act — with arithmetic anyone can
//! check. What it does not give is a single aggregate signature, so a
//! quorum certificate is k signatures large. For an operation that
//! happens a handful of times in an entity's life, that is the right
//! trade.

use crate::domain::Tag;
use crate::key::{PublicKey, Signature};
use crate::sha256::Sha256;
use alloc::vec::Vec;
use brainos_abi::ids::EntityId;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum QuorumError {
    /// fewer valid shares than the threshold
    NotEnoughShares { have: usize, need: usize },
    /// a share was signed by a key that is not a member
    NotAMember,
    /// the same member signed twice; k-of-n means k *distinct* members
    DuplicateMember,
    /// a share's signature did not verify
    BadShare,
    /// the quorum was constructed with a threshold larger than n, or zero
    ImpossibleThreshold,
}

/// What a quorum is being asked to authorize. Closed list: each of these
/// is irreversible, and a new one is a governance decision.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Operation {
    /// end this entity: the Key is retired and no body may act as it again
    Deactivate = 1,
    /// revoke one body's binding, whatever that body currently believes
    RevokeBody = 2,
    /// authorize a complete memory export to a named recipient
    ExportMemory = 3,
    /// rotate the Key, carrying the state graph across to a new identity
    RotateKey = 4,
    /// clear a Level-1 rule that the entity itself cannot clear
    ClearRule = 5,
}

impl Operation {
    pub const fn name(self) -> &'static str {
        match self {
            Operation::Deactivate => "deactivate",
            Operation::RevokeBody => "revoke-body",
            Operation::ExportMemory => "export-memory",
            Operation::RotateKey => "rotate-key",
            Operation::ClearRule => "clear-rule",
        }
    }
}

/// One member's signature over a statement.
#[derive(Clone, Copy)]
pub struct Share {
    pub member: PublicKey,
    pub signature: Signature,
}

/// The statement a quorum signs. Binding the subject and a nonce in means
/// a signature collected to revoke body A can never be replayed to revoke
/// body B, nor re-used for a second revocation of A.
#[derive(Clone, Copy, Debug)]
pub struct Statement {
    pub operation: Operation,
    pub entity: EntityId,
    /// what the operation is about: a body id, a rule hash, a recipient
    pub subject: [u8; 32],
    pub nonce: [u8; 32],
}

impl Statement {
    pub fn transcript(&self) -> Vec<u8> {
        let mut h = Sha256::tagged(Tag::QuorumShare);
        h.field(&[self.operation as u8]);
        h.field(self.entity.as_bytes());
        h.field(&self.subject);
        h.field(&self.nonce);
        h.finish().as_bytes().to_vec()
    }
}

/// The members who may authorize, and how many must agree.
pub struct Quorum {
    members: Vec<PublicKey>,
    threshold: usize,
}

impl Quorum {
    pub fn new(members: Vec<PublicKey>, threshold: usize) -> Result<Quorum, QuorumError> {
        if threshold == 0 || threshold > members.len() {
            return Err(QuorumError::ImpossibleThreshold);
        }
        Ok(Quorum { members, threshold })
    }

    pub fn threshold(&self) -> usize {
        self.threshold
    }

    pub fn members(&self) -> &[PublicKey] {
        &self.members
    }

    fn is_member(&self, k: &PublicKey) -> bool {
        self.members.iter().any(|m| m.to_bytes() == k.to_bytes())
    }

    /// Check a collection of shares against a statement.
    ///
    /// Every failure mode is distinct on purpose: "you are one short" and
    /// "one of those signatures is forged" are different situations for
    /// the human holding the other hardware token, and collapsing them
    /// into `false` would make the second one invisible.
    pub fn authorize(&self, stmt: &Statement, shares: &[Share]) -> Result<(), QuorumError> {
        let transcript = stmt.transcript();
        let mut seen: Vec<[u8; 32]> = Vec::new();

        for share in shares {
            if !self.is_member(&share.member) {
                return Err(QuorumError::NotAMember);
            }
            let id = share.member.to_bytes();
            if seen.contains(&id) {
                return Err(QuorumError::DuplicateMember);
            }
            if !share
                .member
                .verify(Tag::QuorumShare, &transcript, &share.signature)
            {
                return Err(QuorumError::BadShare);
            }
            seen.push(id);
        }

        if seen.len() < self.threshold {
            return Err(QuorumError::NotEnoughShares {
                have: seen.len(),
                need: self.threshold,
            });
        }
        Ok(())
    }
}
