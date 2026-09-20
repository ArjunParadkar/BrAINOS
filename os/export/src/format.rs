//! The format specification, emitted by the code that implements it.
//!
//! This text ships *inside* every archive, as a section. That is the
//! point: a customer who exports their entity and disappears can still
//! write a reader ten years later without us, because the specification
//! is in the file. A format documented only on a website is a format
//! that stops being documented when the website does.
//!
//! It lives next to the encoder so that changing one without the other
//! is a diff somebody has to look at.

use alloc::string::String;

pub fn describe_format() -> String {
    alloc::string::ToString::to_string(
        r#"BrAInOS entity export — format v1
=================================

This file is a complete, portable copy of one entity: its memories, its
skills, its beliefs, its calibration, and the record of its life. It
belongs to whoever holds it. Nothing about reading it requires BrAIn
Corp to exist.

LAYOUT
------
All integers are little-endian. All lengths are u32. There is no
padding and no alignment requirement.

  offset  size  field
  0       8     magic, the ASCII bytes "BRAINEX1"
  8       2     format version, u16. This document describes 1.
  10      ...   manifest (below)
  ...     4     section count, u32
  ...     ...   sections, each: kind u8, length u32, bytes
  ...     32    merkle root over the sections
  ...     64    ed25519 signature

MANIFEST
--------
  entity id        32 bytes, the ed25519 public key. This IS the entity.
  exported at      u64, the entity's own tick counter (not wall time)
  format version   u16
  produced by      length-prefixed UTF-8
  node count       u32
  skill count      u32
  body count       u32
  wakes            u32
  age in ticks     u64
  omissions        u32 count, then that many length-prefixed strings

SECTIONS
--------
  0  memory   the durable state graph
  1  life     the life record
  2  readme   this text

A section's hash is SHA-256 over:
    "brainos/v1/export" | 0x00 | u64(1) | kind_byte | u64(len) | bytes
The merkle root is a binary tree over those hashes, leaves hashed with
the tag "brainos/v1/audit-leaf" and interior nodes with
"brainos/v1/audit-node", odd nodes promoted rather than duplicated.

The signature is over:
    "brainos/v1/export" | 0x00 | SHA-256( tagged(manifest) | root )
and verifies under the entity id in the manifest.

WHAT IS NOT IN THIS FILE, AND WHY
---------------------------------
  * The private half of the BrAIn Key. It never leaves the secure
    element it was minted in. Without it this archive is a complete
    record of an entity that cannot act — which is the correct thing to
    be able to hand to an auditor, a court, or a new owner who will
    provision their own key.
  * The KIRA audit log. It belongs to the machine of record. An entity
    that carried its own audit log between owners could edit it in
    transit, and an audit log that can be edited is not one.

Both absences are listed in the manifest's `omissions` field, so a
reader does not have to know this document to know they are missing.

MEMORY SECTION
--------------
  u64   next node id
  u32   node count
  then, per node:
    u64   id
    u8    kind: 0 episode, 1 semantic, 2 belief, 3 skill,
                4 calibration, 5 audit, 6 body, 7 goal, 8 known
    str   content
    u8    confidence, 0-100
    u8    salience, 0-100
    prov  provenance: u8 origin, opt body id, opt peer id, u64 tick,
          u16 times reinforced
    u32   edge count, then per edge: u64 target, u8 kind
    u32   retrievals

  origin: 0 self, 1 body, 2 human, 3 internet, 4 peer, 5 model,
          6 consolidation
  edge:   0 follows, 1 about, 2 caused, 3 supports, 4 contradicts,
          5 derived-from, 6 requires, 7 produces, 8 uses, 9 on-body

An `opt` is one byte, 0 or 1, followed by the value when 1.

LIFE SECTION
------------
  u32   wakes
  u64   born at (tick)
  u32   count, then that many 32-byte body ids, in order of first
        acquisition
  u32   count, then that many spans:
          u64 woke at, u64 slept at, u16 peak bodies,
          u32 granted, u32 denied, u32 stops, u16 dreams, u8 clean
  u64   total granted
  u64   total denied
  u64   total stops
  u64   total dreams
  u32   unclean wakes

READING THIS WITHOUT US
-----------------------
Everything above is plain byte layout and SHA-256/ed25519, both of which
have public specifications and implementations in every language. A
reader is a few hundred lines. If you want to check that this file is
genuinely the entity it claims to be and has not been altered, you need
the entity id (which is in the file), the section bytes (which are in
the file), and a standard ed25519 verify. You do not need us, and you
will not need us later.
"#,
    )
}
