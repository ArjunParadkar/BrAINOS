//! The audit anchor: an append-only Merkle log.
//!
//! §3.3 says AUDIT always runs and the record is immutable. "Immutable"
//! in a file means "nobody edited it, we think". Here it means: the log
//! has a root hash, the root changes with every append, and any past
//! entry can be proven to be in the log with a path of `log2(n)` hashes.
//! Delete one decision from the middle and every root after it changes,
//! so the tampering is not merely detectable — it is detectable by
//! someone who only kept the 32-byte root.
//!
//! This is what makes EU Machinery Regulation conformity a feature
//! rather than a paperwork exercise (risk R1): "show the KIRA log" is a
//! proof, not a file.
//!
//! The construction is a standard binary Merkle tree with leaf/interior
//! domain separation, so a leaf's hash can never be presented as an
//! interior node's — the classic second-preimage attack on naive Merkle
//! trees. Odd nodes are promoted rather than duplicated, for the same
//! reason: duplicating the last node makes two different logs share a
//! root.

use crate::domain::Tag;
use crate::sha256::{Hash, Sha256};
use alloc::vec::Vec;

/// The root of an audit log at one moment. Small enough to sign, to
/// publish, and to keep in a register.
pub type Root = Hash;

pub fn leaf_hash(data: &[u8]) -> Hash {
    let mut h = Sha256::tagged(Tag::AuditLeaf);
    h.field(data);
    h.finish()
}

pub fn node_hash(left: &Hash, right: &Hash) -> Hash {
    let mut h = Sha256::tagged(Tag::AuditNode);
    h.update(left.as_bytes());
    h.update(right.as_bytes());
    h.finish()
}

/// An append-only log of leaf hashes.
///
/// The leaves themselves are kept because a robot's audit log is small
/// (one entry per gated action) and being able to replay it is worth
/// more than the bytes. A deployment that needs to prune keeps only the
/// roots it signed; the proofs it already issued stay valid.
#[derive(Default, Clone)]
pub struct MerkleLog {
    leaves: Vec<Hash>,
    /// The roots of the perfect subtrees covering the log so far, in
    /// decreasing height order.
    ///
    /// This is what makes `root()` cheap. Rebuilding the whole tree to
    /// answer it is O(n), which is invisible for a demo and a real
    /// problem for the thing this product is *for*: an entity that
    /// accumulates for years. A robot making ten gated decisions a
    /// second reaches a million in about a day, at which point every
    /// checkpoint cost 0.7 s on a laptop — and checkpoints are what
    /// make the audit log evidence rather than a diary, so making them
    /// expensive means taking fewer of them.
    ///
    /// The promote-the-odd-node tree turns out to be exactly the fold of
    /// the binary decomposition of `n` from the right, so a frontier
    /// works: appending merges equal-height neighbours, and the root is
    /// the frontier folded right to left. An exhaustive test checks the
    /// two agree on every prefix, because a clever root that disagrees
    /// with the obvious one would invalidate every proof ever issued.
    frontier: Vec<(u8, Hash)>,
}

impl MerkleLog {
    pub fn new() -> MerkleLog {
        MerkleLog {
            leaves: Vec::new(),
            frontier: Vec::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.leaves.len()
    }

    pub fn is_empty(&self) -> bool {
        self.leaves.is_empty()
    }

    /// Append one record. Returns its index, which is what a `Proof`
    /// later refers to.
    pub fn append(&mut self, data: &[u8]) -> usize {
        self.append_hash(leaf_hash(data))
    }

    pub fn append_hash(&mut self, h: Hash) -> usize {
        self.leaves.push(h);
        self.absorb(h);
        self.leaves.len() - 1
    }

    /// Fold a new leaf into the frontier: merge with the neighbour above
    /// while the heights match, which is the carry in binary counting.
    fn absorb(&mut self, h: Hash) {
        let mut node = (0u8, h);
        while let Some(&(height, top)) = self.frontier.last() {
            if height != node.0 {
                break;
            }
            self.frontier.pop();
            node = (height.saturating_add(1), node_hash(&top, &node.1));
        }
        self.frontier.push(node);
    }

    pub fn leaf(&self, i: usize) -> Option<Hash> {
        self.leaves.get(i).copied()
    }

    /// The root over the whole log. An empty log's root is the zero
    /// hash — distinct from any real root, because `leaf_hash` of
    /// anything is tagged and therefore never zero in practice.
    pub fn root(&self) -> Root {
        let Some((_, last)) = self.frontier.last() else {
            return Hash::ZERO;
        };
        let mut acc = *last;
        for (_, h) in self.frontier.iter().rev().skip(1) {
            acc = node_hash(h, &acc);
        }
        acc
    }

    /// The root, computed the obvious way: build every level.
    ///
    /// Kept because `root()` is now clever, and a clever answer is only
    /// trustworthy next to the obvious one it has to agree with. The
    /// test suite checks they agree on every prefix up to a few
    /// thousand leaves; this is O(n) and is not on any path that runs
    /// more than once.
    pub fn root_the_slow_way(&self) -> Root {
        if self.leaves.is_empty() {
            return Hash::ZERO;
        }
        let mut level: Vec<Hash> = self.leaves.clone();
        while level.len() > 1 {
            let mut next = Vec::with_capacity(level.len().div_ceil(2));
            let mut i = 0;
            while i + 1 < level.len() {
                next.push(node_hash(&level[i], &level[i + 1]));
                i += 2;
            }
            if i < level.len() {
                // promote the odd one rather than pairing it with itself
                next.push(level[i]);
            }
            level = next;
        }
        level[0]
    }

    /// How many perfect subtrees the frontier currently holds — the
    /// population count of the log's length, and the cost of a root.
    pub fn frontier_len(&self) -> usize {
        self.frontier.len()
    }

    /// An inclusion proof for one entry.
    pub fn prove(&self, index: usize) -> Option<Proof> {
        if index >= self.leaves.len() {
            return None;
        }
        let mut path = Vec::new();
        let mut level: Vec<Hash> = self.leaves.clone();
        let mut idx = index;
        while level.len() > 1 {
            let mut next = Vec::with_capacity(level.len().div_ceil(2));
            let mut i = 0;
            while i + 1 < level.len() {
                if i == idx & !1 {
                    // our pair: record the sibling and which side it is on
                    if idx % 2 == 0 {
                        path.push((level[i + 1], Side::Right));
                    } else {
                        path.push((level[i], Side::Left));
                    }
                }
                next.push(node_hash(&level[i], &level[i + 1]));
                i += 2;
            }
            if i < level.len() {
                next.push(level[i]);
                // a promoted node has no sibling at this level; nothing
                // is recorded, which is exactly how verification replays it
            }
            idx /= 2;
            level = next;
        }
        Some(Proof {
            index,
            leaf: self.leaves[index],
            path,
            size: self.leaves.len(),
        })
    }

    /// Every root the log has had, oldest first. A deployment signs these
    /// periodically; a signed root is a commitment that the log up to
    /// that point said what it said.
    pub fn roots_through(&self) -> Vec<Root> {
        let mut out = Vec::with_capacity(self.leaves.len());
        let mut partial = MerkleLog::new();
        for leaf in &self.leaves {
            partial.append_hash(*leaf);
            out.push(partial.root());
        }
        out
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    Left,
    Right,
}

/// Proof that one record is in a log with a given root.
#[derive(Clone, Debug)]
pub struct Proof {
    pub index: usize,
    pub leaf: Hash,
    pub size: usize,
    path: Vec<(Hash, Side)>,
}

impl Proof {
    pub fn path_len(&self) -> usize {
        self.path.len()
    }

    /// Recompute the root from the leaf and the path. Verification needs
    /// nothing but the proof and the 32-byte root it is checked against —
    /// not the log, not the other records, not the entity.
    pub fn recompute(&self) -> Hash {
        let mut acc = self.leaf;
        for (sibling, side) in &self.path {
            acc = match side {
                Side::Right => node_hash(&acc, sibling),
                Side::Left => node_hash(sibling, &acc),
            };
        }
        acc
    }

    pub fn verify(&self, root: &Root) -> bool {
        self.recompute().ct_eq(root)
    }

    /// Verify that this proof is for a specific record's bytes, not just
    /// for *some* leaf. Without this a proof could be presented alongside
    /// a different record and look valid.
    pub fn verify_record(&self, record: &[u8], root: &Root) -> bool {
        leaf_hash(record).ct_eq(&self.leaf) && self.verify(root)
    }
}
