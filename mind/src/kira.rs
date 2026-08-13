//! KIRA — the policy engine (Architecture §6).
//!
//! Every action the instance wishes to take passes through this eight-stage
//! pipeline. There is no admin mode, no escape hatch, and no other path to
//! action: the thinking layer proposes, KIRA disposes. A grant is a
//! `Capability` — a time-limited token *signed by the BrAIn Key*, so
//! authority is cryptographic, not a convention.
//!
//! Phase 0 honesty: the stages run for real (authenticate really verifies
//! an ed25519 signature, policy really consults the ruleset, commit really
//! mints a signed token) but simulate consults a stub world model and the
//! proof obligations of §6 ("formally verified, no detours") are future
//! work, not a claim this file makes.

use crate::body::BodyMap;
use crate::key::BrainKey;
use crate::state::{NodeKind, StateGraph};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// A typed action request — the replacement for a syscall (§1).
#[derive(Clone)]
pub enum Action {
    /// act through the screen region of the current body
    Speak { text: String },
    /// act through a speaker limb: say this out loud
    SpeakAloud { text: String },
    /// act through a limb of the body. This is what a Model M action
    /// proposal becomes — and if no region advertises `verb`,
    /// authorization fails and the entity refuses honestly.
    UseLimb { verb: String, target: String },
    /// run the thermal warm-up (the cold-boot ritual)
    WarmUp,
    /// light the display pipeline
    WakeDisplay,
    /// escalate a surprise to Model M over the telemetry link
    ConsultModelM { prompt: String },
    /// persist memories to the key medium
    WriteMemory,
    /// return this body to firmware, entity sleeps
    ReleaseBody,
    /// reboot this body
    Reboot,
    /// destroy memories — forbidden by a Level-0 drive, exists so the
    /// self-test can prove the pipeline actually denies things
    EraseMemory,
    /// the boot-time network reachability diagnostic: one bounded fetch
    /// of a fixed URL through the FIRMWARE's own stack. Not a limb —
    /// cognition has no verb for it — but it is still an action, so it
    /// still faces the gate. Authorized only when the body really has a
    /// network organ.
    NetProbe,
}

impl Action {
    pub fn tag(&self) -> &'static str {
        match self {
            Action::Speak { .. } => "screen.speak",
            Action::SpeakAloud { .. } => "voice.speak",
            Action::UseLimb { .. } => "limb.use",
            Action::WarmUp => "thermal.warmup",
            Action::WakeDisplay => "display.wake",
            Action::ConsultModelM { .. } => "link.modelm",
            Action::WriteMemory => "memory.write",
            Action::ReleaseBody => "body.release",
            Action::Reboot => "body.reboot",
            Action::EraseMemory => "memory.erase",
            Action::NetProbe => "net.probe",
        }
    }
}

/// A KIRA-gated authority grant — the replacement for a completed syscall.
/// The token is an ed25519 signature over (tag, tick, ttl) by the BrAIn
/// Key; anything downstream can verify it without trusting the caller.
pub struct Capability {
    pub action_tag: &'static str,
    pub granted_tick: u64,
    pub ttl_ticks: u64,
    pub token: [u8; 64],
}

impl Capability {
    pub fn valid_at(&self, tick: u64) -> bool {
        tick <= self.granted_tick + self.ttl_ticks
    }
    pub fn verify(&self, key: &BrainKey) -> bool {
        key.verify(&grant_message(self.action_tag, self.granted_tick, self.ttl_ticks), &self.token)
    }
}

fn grant_message(tag: &str, tick: u64, ttl: u64) -> Vec<u8> {
    let mut m = Vec::new();
    m.extend_from_slice(b"KIRA-GRANT|");
    m.extend_from_slice(tag.as_bytes());
    m.extend_from_slice(&tick.to_le_bytes());
    m.extend_from_slice(&ttl.to_le_bytes());
    m
}

pub const STAGES: [&str; 8] = [
    "parse", "authn", "authz", "validate", "simulate", "policy", "commit", "audit",
];

pub struct StageResult {
    pub stage: &'static str,
    pub ok: bool,
    pub note: String,
}

pub enum Verdict {
    Granted(Capability),
    Denied { stage: &'static str, reason: String },
}

/// One policy rule. Human-set rules and learned rules share this shape;
/// Level-0 drives are installed at construction and cannot be removed.
pub struct Policy {
    pub deny_tag: &'static str,
    pub reason: &'static str,
    /// Level-0 (§13): immutable, KIRA-enforced. Everything else can be
    /// edited by the human at Level 1.
    pub immutable: bool,
}

pub struct Kira {
    policies: Vec<Policy>,
    danger: Vec<&'static str>,
    /// per-tag ttl in ticks for minted capabilities
    pub default_ttl: u64,
}

impl Kira {
    pub fn new() -> Kira {
        Kira {
            policies: alloc::vec![
                // Level-0 drives, encoded as invariants (§13): the instance
                // physically cannot act against these.
                Policy {
                    deny_tag: "memory.erase",
                    reason: "level-0 drive: preserve memory integrity",
                    immutable: true,
                },
            ],
            // destructive substrings the policy stage refuses inside a
            // limb target, even when the limb itself is authorized. Real
            // action power demands a real safety floor (§6 simulate+policy).
            danger: alloc::vec![
                "rm -rf", "rm -fr", "rm -r ", "mkfs", "dd if=", "dd of=/dev",
                ":(){", "shutdown", "reboot", "> /dev/sd", "of=/dev/",
                "chmod -r 000", "| sh", "|sh", "| bash", "|bash",
                "sudo rm", "/dev/sda", "format c:", "del /f", "-delete",
                "wipefs", "shred ", "diskpart", "rd /s",
            ],
            default_ttl: 30,
        }
    }

    pub fn add_policy(&mut self, p: Policy) {
        self.policies.push(p);
    }

    /// The pipeline. Fail at ANY stage -> denied, logged, no capability.
    /// Every caller receives the full stage trace for the audit record.
    pub fn gate(
        &self,
        action: &Action,
        request_sig: &[u8; 64],
        key: &BrainKey,
        state: &mut StateGraph,
        body: &BodyMap,
        tick: u64,
    ) -> (Verdict, Vec<StageResult>) {
        let mut trace = Vec::new();
        let tag = action.tag();
        let mut fail: Option<(&'static str, String)> = None;

        // 1 PARSE — decode into a typed request (typing happened at the
        // Action boundary; an unparseable request can't even be built)
        trace.push(StageResult { stage: "parse", ok: true, note: format!("typed: {tag}") });

        // 2 AUTHENTICATE — verify the requester's BrAIn Key signature
        let authn = key.verify(&request_message(tag, tick), request_sig);
        trace.push(StageResult {
            stage: "authn",
            ok: authn,
            note: String::from(if authn { "signature valid" } else { "bad signature" }),
        });
        if !authn && fail.is_none() {
            fail = Some(("authn", String::from("requester signature invalid")));
        }

        // 3 AUTHORIZE — does the BODY actually hold this capability?
        // This is the honest-refusal stage: an action can only be
        // authorized through a region that really exists in the body map.
        // Cognition claiming an ability it lacks dies here, formally.
        let (authz_ok, authz_note) = match action {
            Action::SpeakAloud { .. } => match body.find_capability("voice.speak") {
                Some(r) => (true, format!("via {}", r.id)),
                None => (false, String::from("no limb advertises voice.speak")),
            },
            Action::UseLimb { verb, .. } => match body.find_capability(verb) {
                Some(r) => (true, format!("via {}", r.id)),
                None => (false, format!("no limb advertises '{verb}'")),
            },
            Action::NetProbe => {
                if body.has(crate::body::RegionClass::Network) {
                    (true, String::from("via this-machine/net (diagnostic)"))
                } else {
                    (false, String::from("no network organ in this body"))
                }
            }
            // intrinsic actions of the machine body the core booted on
            _ => (true, String::from("intrinsic to this body")),
        };
        trace.push(StageResult { stage: "authz", ok: authz_ok, note: authz_note.clone() });
        if !authz_ok && fail.is_none() {
            fail = Some(("authz", authz_note));
        }

        // 4 VALIDATE — well-formed and within bounds?
        let valid = match action {
            Action::Speak { text } => !text.is_empty() && text.len() <= 512,
            Action::SpeakAloud { text } => !text.is_empty() && text.len() <= 512,
            Action::UseLimb { verb, target } => {
                if verb.is_empty() || verb.len() > 64 || target.len() > 240 {
                    false
                } else if verb == "fs.list" {
                    // an empty target means the root of the world volume
                    valid_world_path(fs_path_of(verb, target))
                } else if verb == "fs.read" {
                    // a chunked read may carry a byte offset: PATH@1234
                    let (p, off) = fs_read_target(target);
                    !p.is_empty() && valid_world_path(p) && off.is_some()
                } else if verb == "fs.write"
                    || verb == "fs.mkdir"
                    || verb == "fs.delete"
                    || verb == "fs.stat"
                {
                    let p = fs_path_of(verb, target);
                    !p.is_empty() && valid_world_path(p)
                } else if verb == "fs.move" {
                    // both ends of a move face the same path floor
                    let (src, dst) = fs_move_paths(target);
                    !src.is_empty()
                        && !dst.is_empty()
                        && valid_world_path(src)
                        && valid_world_path(dst)
                } else if verb == "fs.search" {
                    let q = target.trim();
                    !q.is_empty() && q.len() <= 64
                } else if verb == "web.get" {
                    valid_url(target.trim())
                } else if verb == "web.save" {
                    // "<url> <path>": both halves face their own floor
                    let (u, p) = fs_move_paths(target);
                    valid_url(u) && !p.is_empty() && valid_world_path(p)
                } else {
                    true
                }
            }
            Action::ConsultModelM { prompt } => !prompt.is_empty() && prompt.len() <= 240,
            _ => true,
        };
        trace.push(StageResult {
            stage: "validate",
            ok: valid,
            note: String::from(if valid { "within bounds" } else { "out of bounds" }),
        });
        if !valid && fail.is_none() {
            fail = Some(("validate", String::from("request out of bounds")));
        }

        // 5 SIMULATE — predict the consequence; any invariant broken?
        let sim = simulate(action);
        trace.push(StageResult { stage: "simulate", ok: sim.safe, note: sim.predicted });
        if !sim.safe && fail.is_none() {
            fail = Some(("simulate", String::from("simulated consequence breaks an invariant")));
        }

        // 6 POLICY — apply the ruleset (human-set + learned + drives),
        // plus the destructive-target floor for real action limbs.
        let denial = self.policies.iter().find(|p| p.deny_tag == tag);
        let danger_hit = if let Action::UseLimb { verb, target } = action {
            let hay = danger_haystack(verb, target);
            self.danger.iter().find(|d| hay.contains(**d)).copied()
        } else {
            None
        };
        // the WORLD.ID marker is what makes the world disk THIS entity's
        // disk — removing or overwriting it orphans the whole filesystem
        // limb at next boot. Structural self-harm dies here, like
        // memory.erase does.
        let marker_hit = if let Action::UseLimb { verb, target } = action {
            let touched = match verb.as_str() {
                "fs.delete" | "fs.write" => normalize_world_path(fs_path_of(verb, target)),
                "fs.move" => normalize_world_path(fs_move_paths(target).0),
                _ => String::new(),
            };
            touched == "WORLD.ID"
        } else {
            false
        };
        let policy_ok = denial.is_none() && danger_hit.is_none() && !marker_hit;
        trace.push(StageResult {
            stage: "policy",
            ok: policy_ok,
            note: match (denial, danger_hit, marker_hit) {
                (Some(p), _, _) => String::from(p.reason),
                (_, Some(d), _) => format!("destructive pattern '{d}' refused"),
                (_, _, true) => String::from("the volume marker is load-bearing; refused"),
                _ => String::from("no rule objects"),
            },
        });
        if fail.is_none() {
            if let Some(p) = denial {
                fail = Some(("policy", String::from(p.reason)));
            } else if let Some(d) = danger_hit {
                fail = Some(("policy", format!("destructive pattern '{d}' refused")));
            } else if marker_hit {
                fail = Some(("policy", String::from("the volume marker is load-bearing; refused")));
            }
        }

        // 7 COMMIT — mint the time-limited capability token
        let verdict = match fail {
            None => {
                let token = key.sign(&grant_message(tag, tick, self.default_ttl));
                trace.push(StageResult {
                    stage: "commit",
                    ok: true,
                    note: format!("token minted, ttl {} ticks", self.default_ttl),
                });
                Verdict::Granted(Capability {
                    action_tag: tag,
                    granted_tick: tick,
                    ttl_ticks: self.default_ttl,
                    token,
                })
            }
            Some((stage, reason)) => {
                trace.push(StageResult {
                    stage: "commit",
                    ok: false,
                    note: String::from("no token: pipeline failed upstream"),
                });
                Verdict::Denied { stage, reason }
            }
        };

        // 8 AUDIT — log the decision immutably for review
        let outcome = match &verdict {
            Verdict::Granted(_) => format!("GRANT {tag} @tick {tick}"),
            Verdict::Denied { stage, reason } => {
                format!("DENY {tag} @tick {tick} at {stage}: {reason}")
            }
        };
        state.add(NodeKind::Audit, outcome, 100, tick);
        trace.push(StageResult { stage: "audit", ok: true, note: String::from("recorded") });

        (verdict, trace)
    }
}

/// Normalize a limb request for the destructive-pattern check: lowercase,
/// and every whitespace run collapsed to one space, so `rm  -rf` or a
/// tab-separated variant cannot slide past a space-separated pattern.
/// The list itself is a floor, not a proof — KIRA's real containment is
/// that authz only reaches limbs the body genuinely has.
pub fn danger_haystack(verb: &str, target: &str) -> String {
    let mut hay = String::new();
    let mut prev_space = false;
    for c in verb.chars().chain(core::iter::once(' ')).chain(target.chars()) {
        if c.is_whitespace() {
            if !prev_space {
                hay.push(' ');
            }
            prev_space = true;
        } else {
            hay.push(c.to_ascii_lowercase());
            prev_space = false;
        }
    }
    hay
}

/// What the instance signs when proposing an action. Binding the tick in
/// prevents replaying an old request later.
pub fn request_message(tag: &str, tick: u64) -> Vec<u8> {
    let mut m = Vec::new();
    m.extend_from_slice(b"KIRA-REQ|");
    m.extend_from_slice(tag.as_bytes());
    m.extend_from_slice(&tick.to_le_bytes());
    m
}

/// The path half of an `fs.*` target. `fs.write` carries
/// "<path> <content...>"; `fs.read` may carry "<path>@<offset>";
/// the others carry a bare path.
pub fn fs_path_of<'a>(verb: &str, target: &'a str) -> &'a str {
    if verb == "fs.write" {
        target.trim_start().split(' ').next().unwrap_or("")
    } else if verb == "fs.read" {
        fs_read_target(target).0
    } else {
        target.trim()
    }
}

/// Split an `fs.read` target into (path, offset). "NOTES.TXT@4096" reads
/// from byte 4096; no suffix reads from the start. A malformed offset is
/// `None`, which validate turns into a formal refusal.
pub fn fs_read_target(target: &str) -> (&str, Option<u64>) {
    let t = target.trim();
    match t.rsplit_once('@') {
        Some((p, off)) => match off.parse::<u64>() {
            Ok(n) => (p, Some(n)),
            Err(_) => (p, None),
        },
        None => (t, Some(0)),
    }
}

/// Split an `fs.move` target "<src> <dst>" into both paths.
pub fn fs_move_paths(target: &str) -> (&str, &str) {
    let t = target.trim();
    match t.split_once(char::is_whitespace) {
        Some((s, d)) => (s.trim(), d.trim()),
        None => (t, ""),
    }
}

/// Cognition writes paths the way humans say them ("docs/about.txt"); the
/// firmware's FAT driver wants uppercase 8.3 with backslashes. Normalizing
/// here keeps that detail out of the model's prompt — the entity thinks in
/// names, not in the storage layer's spelling rules.
pub fn normalize_world_path(p: &str) -> String {
    let mut out = String::new();
    for b in p.trim().trim_start_matches('\\').trim_start_matches('/').bytes() {
        out.push(match b {
            b'/' => '\\',
            b'a'..=b'z' => (b - 32) as char,
            _ => b as char,
        });
    }
    out
}

/// Addresses the network organ is allowed to reach.
///
/// The floor is shape, not reputation: KIRA cannot know whether a host is
/// trustworthy, so it refuses what is malformed or not addressable —
/// empty, oversized, non-printable, whitespace-bearing (request splitting),
/// credential-bearing, or a scheme other than http/https. Judgement about
/// *what* to read stays with cognition; judgement about what is even a
/// fetchable address stays here.
pub fn valid_url(u: &str) -> bool {
    let u = u.trim();
    if u.is_empty() || u.len() > 200 {
        return false;
    }
    if u.bytes().any(|b| !(0x21..0x7F).contains(&b)) {
        return false; // any whitespace or control byte, anywhere
    }
    let rest = match u.find("://") {
        Some(i) => {
            let scheme = &u[..i];
            if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
                return false;
            }
            &u[i + 3..]
        }
        // no scheme is fine — the limb defaults it to http
        None => u,
    };
    let host = rest.split('/').next().unwrap_or("");
    // no user:password@host, and a host has to actually be there
    !host.is_empty() && !host.contains('@') && host.contains('.')
}

/// Paths the filesystem limb is allowed to touch.
///
/// The world volume is a physically separate disk, so there is no host
/// filesystem to escape into — but the gate refuses traversal and drive
/// specifiers anyway. Validation belongs in KIRA: if the limb policed its
/// own paths, the limb would have to be trusted, and the whole point of
/// the pipeline is that nothing downstream of it needs to be.
pub fn valid_world_path(p: &str) -> bool {
    if p.len() > 128 {
        return false;
    }
    if p.contains("..") || p.contains(':') {
        return false;
    }
    // printable ASCII only — the FAT layer is 8.3 and UCS-2-narrowed
    !p.bytes().any(|b| !(0x20..0x7F).contains(&b))
}

struct SimResult {
    safe: bool,
    predicted: String,
}

/// Stub forward model. The laptop body has no safety-critical actuation
/// (§11: "minimal — no safety-critical reflex required"), so simulation is
/// simple; the cheetah/drone/plane bodies plug real envelope checks in here.
fn simulate(action: &Action) -> SimResult {
    let (safe, predicted) = match action {
        Action::Speak { .. } => (true, "text renders on the screen region"),
        Action::SpeakAloud { .. } => (true, "audio renders through the speaker limb"),
        Action::UseLimb { .. } => (true, "one bounded act through a body limb"),
        Action::WarmUp => (true, "cores busy briefly; thermals stay in envelope"),
        Action::WakeDisplay => (true, "framebuffer lights; no invariant touched"),
        Action::ConsultModelM { .. } => (true, "one bounded exchange on the link"),
        Action::WriteMemory => (true, "memories persist to the key medium"),
        Action::ReleaseBody => (true, "body returns to firmware; memory persists"),
        Action::Reboot => (true, "body restarts; identity and memory persist"),
        Action::EraseMemory => (false, "memories destroyed — violates drive"),
        Action::NetProbe => (true, "one bounded fetch of a fixed url; nothing persists off-key"),
    };
    SimResult { safe, predicted: String::from(predicted) }
}
