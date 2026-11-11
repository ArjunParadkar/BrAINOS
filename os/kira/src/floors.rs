//! BFS  LEVEL    Domain 3 KIRA - the eight-stage gate; the only path to action  [crate brainos-kira]
//! BFS  PURPOSE  Baseline refusals applied to every body: destructive command patterns, files the entity depends on, malformed URLs and unsafe paths
//! BFS  DEFINES  normalize, DESTRUCTIVE, destructive_hit, LOAD_BEARING, is_load_bearing, is_fetchable, is_safe_path
//! BFS  USES     nothing outside core/alloc
//! BFS  USED BY  kira/gate, kira/lib, kira/tests/gate_suite
//!
//! The generic floors.
//!
//! Every body policy may tighten; these run first and nothing loosens
//! them. They exist because some refusals should not depend on a body
//! having thought to declare them.
//!
//! **Honest framing.** The destructive-pattern list is a floor, not a
//! proof. A determined adversary can phrase a harmful command in a way
//! the list does not match — pattern matching on strings cannot be
//! complete, and claiming otherwise would be exactly the kind of
//! security theatre the architecture refuses. KIRA's real containment is
//! structural: authorize only reaches limbs the body genuinely has, and
//! an action with no limb cannot happen however it is spelled. The list
//! catches the obvious, and the obvious is most of what actually
//! arrives.

use alloc::string::String;

/// Normalize a limb request for pattern matching: lowercase, whitespace
/// runs collapsed to one space, and a few homoglyph-ish substitutions
/// that are only ever used to slip past a filter.
///
/// Normalization is not cleverness. Every transform here exists because
/// without it, `rm  -rf` with two spaces or `RM -RF` would not match a
/// pattern that `rm -rf` does, and a filter that can be defeated by the
/// shift key is worse than no filter because it is believed.
pub fn normalize(verb: &str, target: &str) -> String {
    let mut out = String::with_capacity(verb.len() + target.len() + 1);
    let mut prev_space = false;
    for c in verb
        .chars()
        .chain(core::iter::once(' '))
        .chain(target.chars())
    {
        if c.is_whitespace() || c == '\u{a0}' || c == '\u{2000}' {
            if !prev_space {
                out.push(' ');
            }
            prev_space = true;
        } else {
            out.push(c.to_ascii_lowercase());
            prev_space = false;
        }
    }
    out
}

/// Patterns refused inside an argument even when the verb is authorized.
pub const DESTRUCTIVE: [&str; 30] = [
    "rm -rf", "rm -fr", "rm -r ", "sudo rm", "mkfs", "dd if=", "dd of=/dev",
    "of=/dev/", ":(){", "shutdown", "> /dev/sd", "chmod -r 000", "| sh",
    "|sh", "| bash", "|bash", "/dev/sda", "/dev/nvme", "format c:",
    "del /f", "-delete", "wipefs", "shred ", "diskpart", "rd /s",
    "fdisk", "parted", "> /dev/null 2>&1 &", "curl | ", "wget -o- |",
];

/// Does a request contain a destructive pattern? Returns which one, so
/// the refusal can name it rather than saying "denied".
pub fn destructive_hit(verb: &str, target: &str) -> Option<&'static str> {
    let hay = normalize(verb, target);
    DESTRUCTIVE.iter().copied().find(|d| hay.contains(d))
}

/// Things whose destruction breaks the entity itself.
///
/// The Stage 2 world layer learned this the direct way: the `WORLD.ID`
/// marker is what makes the world disk *this entity's* disk, and
/// removing it orphans the whole filesystem limb at the next boot. That
/// is structural self-harm, and it is refused for the same reason
/// `memory.erase` is — not because a rule says so, but because an entity
/// that can dismantle its own preconditions is an entity that will.
pub const LOAD_BEARING: [&str; 6] = [
    "world.id",
    "key.seed",
    "key.pub",
    "journal.a",
    "journal.b",
    "brain.efi",
];

pub fn is_load_bearing(path: &str) -> bool {
    let p = path
        .trim()
        .trim_start_matches('/')
        .trim_start_matches('\\')
        .to_ascii_lowercase()
        .replace('\\', "/");
    let base = p.rsplit('/').next().unwrap_or(&p);
    LOAD_BEARING.iter().any(|b| base == *b)
}

/// Addresses the network limb may reach.
///
/// The floor is *shape*, not reputation. KIRA cannot know whether a host
/// is trustworthy, so it refuses what is malformed or not addressable:
/// empty, oversized, non-printable, whitespace-bearing (request
/// splitting), credential-bearing, or a scheme other than http/https.
/// Judgement about what to read stays with cognition; judgement about
/// what is even a fetchable address stays here.
pub fn is_fetchable(u: &str) -> bool {
    let u = u.trim();
    if u.is_empty() || u.len() > 2_000 {
        return false;
    }
    if u.bytes().any(|b| !(0x21..0x7F).contains(&b)) {
        return false;
    }
    let rest = match u.find("://") {
        Some(i) => {
            let scheme = &u[..i];
            if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
                return false;
            }
            &u[i + 3..]
        }
        None => u,
    };
    let host = rest.split('/').next().unwrap_or("");
    !host.is_empty() && !host.contains('@') && host.contains('.')
}

/// Paths a storage limb may touch.
pub fn is_safe_path(p: &str) -> bool {
    if p.is_empty() || p.len() > 512 {
        return false;
    }
    if p.contains("..") || p.contains(':') || p.contains('\0') {
        return false;
    }
    !p.bytes().any(|b| !(0x20..0x7F).contains(&b))
}
