//! BFS  LEVEL    Domain 1 Silicon - the BrAIn Key, hashes, Merkle log, quorum  [crate brainos-crypto]
//! BFS  PURPOSE  The closed list of labels prefixed to everything signed or hashed, so a signature made for one kind of message never verifies as another
//! BFS  DEFINES  Tag, tagged, tagged_into
//! BFS  USES     nothing outside core/alloc
//! BFS  USED BY  blur/acquire, blur/census, blur/firmware, body/acquire, body/schema, bpu/device, crypto/key, crypto/lib, +22 more
//!
//! Domain separation tags.
//!
//! Every signature and structural hash in BrAInOS is made over
//! `tag ‖ 0x00 ‖ payload`, where the tag comes from this closed list. The
//! reason is a specific attack, not hygiene: if a KIRA grant and an
//! attestation response were both "an ed25519 signature over 64 bytes",
//! then a body that can get the entity to attest to a chosen challenge
//! could harvest a signature that also happens to verify as a grant.
//!
//! Adding a tag is adding a new kind of signed thing, which is a change
//! to the trust surface, so the list is closed and the proof harness
//! enumerates it.

/// Every kind of thing BrAInOS signs or structurally hashes.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, PartialOrd, Ord)]
pub enum Tag {
    /// a KIRA action request, signed by the entity before the gate runs
    Request,
    /// a minted Granted token — the only thing that authorizes action
    Grant,
    /// a capability delegated to a body or a peer, with caveats
    Delegation,
    /// an attestation challenge response
    Attestation,
    /// the identity of a body: attestation key ‖ schema digest
    BodyIdentity,
    /// a capability schema, hashed so a changed schema is a changed body
    Schema,
    /// one leaf of the audit log
    AuditLeaf,
    /// an interior node of the audit Merkle tree
    AuditNode,
    /// a sealed state-graph journal record
    Journal,
    /// a complete entity export
    Export,
    /// a transport session handshake transcript
    Session,
    /// one share of a quorum operation (revocation, deactivation)
    QuorumShare,
    /// a marketplace capability package
    Package,
    /// key derivation for a per-body subkey
    Subkey,
    /// a fleet registry record
    Registry,
}

impl Tag {
    pub const fn as_str(self) -> &'static str {
        match self {
            Tag::Request => "brainos/v1/request",
            Tag::Grant => "brainos/v1/grant",
            Tag::Delegation => "brainos/v1/delegation",
            Tag::Attestation => "brainos/v1/attestation",
            Tag::BodyIdentity => "brainos/v1/body-identity",
            Tag::Schema => "brainos/v1/schema",
            Tag::AuditLeaf => "brainos/v1/audit-leaf",
            Tag::AuditNode => "brainos/v1/audit-node",
            Tag::Journal => "brainos/v1/journal",
            Tag::Export => "brainos/v1/export",
            Tag::Session => "brainos/v1/session",
            Tag::QuorumShare => "brainos/v1/quorum-share",
            Tag::Package => "brainos/v1/package",
            Tag::Subkey => "brainos/v1/subkey",
            Tag::Registry => "brainos/v1/registry",
        }
    }

    pub const ALL: [Tag; 15] = [
        Tag::Request,
        Tag::Grant,
        Tag::Delegation,
        Tag::Attestation,
        Tag::BodyIdentity,
        Tag::Schema,
        Tag::AuditLeaf,
        Tag::AuditNode,
        Tag::Journal,
        Tag::Export,
        Tag::Session,
        Tag::QuorumShare,
        Tag::Package,
        Tag::Subkey,
        Tag::Registry,
    ];
}

/// Build a tagged message: `tag ‖ 0x00 ‖ payload`. This is what goes
/// under a signature; the hash equivalent is `Sha256::tagged`.
pub fn tagged(tag: Tag, payload: &[u8]) -> alloc::vec::Vec<u8> {
    let t = tag.as_str().as_bytes();
    let mut m = alloc::vec::Vec::with_capacity(t.len() + 1 + payload.len());
    m.extend_from_slice(t);
    m.push(0x00);
    m.extend_from_slice(payload);
    m
}

/// Same, into a caller-provided buffer, for the no-allocator paths.
/// Returns the number of bytes written, or `None` if it would not fit —
/// never a truncated message, because a truncated signed message is a
/// signature over something other than what the caller meant.
pub fn tagged_into(tag: Tag, payload: &[u8], out: &mut [u8]) -> Option<usize> {
    let t = tag.as_str().as_bytes();
    let n = t.len() + 1 + payload.len();
    if out.len() < n {
        return None;
    }
    out[..t.len()].copy_from_slice(t);
    out[t.len()] = 0x00;
    out[t.len() + 1..n].copy_from_slice(payload);
    Some(n)
}
