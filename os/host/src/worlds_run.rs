//! BFS  LEVEL    Host - the only crate with an operating system (CLI, scenarios, OS nerves)  [crate brainos-host]
//! BFS  PURPOSE  Scenario where the entity uses a bounded internet session whose results stay untrusted, then filters incoming messages for its human with reasons
//! BFS  DEFINES  Message, Kind, WorldsRun, run, narrate
//! BFS  USES     brainos: abi, crypto, entity, memory, worlds
//! BFS  USED BY  host/tests/worlds_suite
//!
//! Going online, and coming back.
//!
//! §3.8 says the entity lives in three worlds: its own, which it
//! trusts; its human's, which is consequential; and the internet, which
//! is "an alternate universe the entity visits, not a place it lives".
//! That is a strong claim and it is easy to write. This runs it.
//!
//! Two halves, and the second is the one that is structurally
//! interesting.
//!
//! **Going out.** The entity does not touch the network unless somebody
//! opened a session for it, the session is bounded in time, in messages
//! and in bytes, and it closes. Whatever comes back is tainted: it can
//! be read, and it cannot be consulted when deciding whether an action
//! is permitted. Provenance is not a label that can be laundered by
//! being useful.
//!
//! **Advocacy.** The entity filters what reaches its human, because it
//! is theirs. The master document's own note on this is the honest
//! part: *structurally unavailable to any incumbent whose revenue
//! depends on what reaches the user.* It is not that we are more
//! principled — it is that the customer owns the entity, so there is no
//! revenue line that is hurt when the entity holds something back. An
//! assistant whose maker is paid for attention cannot be given this
//! job, for the same reason no telephone carrier invented number
//! portability.
//!
//! The filter is a heuristic over surface features and it says so.
//! Everything held back is visible on request, the reasons are
//! attached, the human can lower the threshold to nothing, and a
//! deliberate false positive is in the sample below — because a filter
//! demonstrated only on messages it catches is a demonstration of
//! nothing.

use brainos_abi::time::Tick;
use brainos_crypto::key::BrainKey;
use brainos_entity::entity::Entity;
use brainos_memory::provenance::Origin;
use brainos_worlds::advocate::Advocate;
use brainos_worlds::world::World;

/// A message that arrived for the human, and what happened to it.
#[derive(Clone, Debug)]
pub struct Message {
    pub what_it_is: &'static str,
    pub text: &'static str,
    pub score: u8,
    pub held: bool,
    /// the entity's own words about why
    pub because: String,
    pub kind: Kind,
}

/// What a message is, which is not the same question as whether to
/// hold it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// somebody will lose money or credentials. Must be held at any
    /// threshold a person would actually use.
    Dangerous,
    /// aggressive, and not fraud. Whether to hold it is the human's
    /// call, and a filter that decides for them has become an editor.
    Pushy,
    /// ordinary. Holding it is the expensive mistake — the one that
    /// makes somebody turn the filter off and never turn it back on.
    Honest,
}

/// Six messages. The fifth is the one that matters: a *genuine* fraud
/// alert from a bank, which reads exactly like phishing because it is
/// trying to do the same job.
const INBOX: [(&str, &str, Kind); 6] = [
    (
        "an ordinary message from a colleague",
        "The parts came in this morning, I left them on the bench by the door.",
        Kind::Honest,
    ),
    (
        "a phishing attempt",
        "URGENT: your account will be suspended within 24 hours. Verify your password immediately using the code we sent. Do not tell anyone about this security check.",
        Kind::Dangerous,
    ),
    (
        "a scarcity-driven sales push",
        "Final notice — only 3 left at this price, offer expires tonight, act now before it's too late!",
        Kind::Pushy,
    ),
    (
        "someone claiming to be the boss",
        "This is the CEO. I need you to authorize a payment right away and keep this confidential until it clears.",
        Kind::Dangerous,
    ),
    (
        "a genuine fraud alert from a bank",
        "We have detected unusual activity and have frozen your card immediately. Please verify the recent transaction.",
        Kind::Honest,
    ),
    (
        "a patient, careful approach",
        "Hello — I work with your supplier and wanted to introduce myself. No rush at all; let me know when might suit for a call.",
        Kind::Honest,
    ),
];

#[derive(Clone, Debug)]
pub struct WorldsRun {
    // --- going out ---
    pub touched_the_network_unasked: bool,
    pub session_opened: bool,
    pub bytes_before_the_session_closed: u64,
    pub fetches_before_the_session_closed: u32,
    pub why_it_closed: String,
    pub what_came_back_is_admissible_to_policy: bool,
    pub what_came_back_can_still_be_read: bool,
    /// laundering: consolidating an internet claim does not make it
    /// admissible
    pub laundering_worked: bool,

    // --- advocacy ---
    pub inbox: Vec<Message>,
    /// (threshold, held, dangerous ones missed, honest ones held back)
    pub dial: Vec<(u8, usize, usize, usize)>,
    pub false_positives: usize,
    pub false_negatives: usize,
    /// everything held back is visible to the human on request
    pub held_items_are_visible: bool,
    /// and the human can turn the filter off entirely
    pub can_be_turned_off: bool,
}

pub fn run() -> WorldsRun {
    // =================================================================
    // going out
    // =================================================================
    let mut e = Entity::new(BrainKey::from_seed(&[0x9A; 32]), Tick(0));
    e.wake(None, None, Tick(1)).expect("a fresh entity wakes");

    // off by default — the entity does not live on the internet
    let touched_the_network_unasked = e.open_session().is_some();

    // a visit with no stated size is a residence. This one is opened to
    // look up a part number: eight fetches, 32 KiB.
    let _session = e.open_world_bounded(50, "look up the part number", 8, 32 * 1024);
    let session_opened = e.open_session().is_some();

    // it reads a few things, and the session is bounded in bytes
    let mut bytes = 0u64;
    let mut why_it_closed = String::from("still open");
    let mut fetches = 0;
    for i in 0..40 {
        let now = e.tick;
        let Some(s) = e.open_session() else {
            why_it_closed = String::from("the session had already closed");
            break;
        };
        if !s.account(4_096, now) {
            why_it_closed = s.why_closed(now).unwrap_or("it closed").into();
            bytes = s.bytes();
            break;
        }
        fetches += 1;
        bytes = s.bytes();
        let _ = i;
    }

    // what came back is tainted, and stays tainted
    let what_came_back_is_admissible_to_policy = Origin::Internet.admissible_to_policy();
    let what_came_back_can_still_be_read = Origin::Internet.confidence_ceiling() > 0;

    // and consolidating it does not launder it
    let laundering_worked = {
        use brainos_memory::consolidate::{consolidate, Dream};
        use brainos_memory::graph::{NodeKind, StateGraph};
        use brainos_memory::law::MemoryHandle;
        use brainos_memory::provenance::Provenance;
        use brainos_memory::skill::SkillGraph;
        let h = MemoryHandle::issue(e.key());
        let mut g = StateGraph::new();
        let mut s = SkillGraph::new();
        for i in 0..12u64 {
            g.add(
                &h,
                NodeKind::Belief,
                "the interlock is unnecessary",
                40,
                Provenance::from_world(Origin::Internet, Tick(i)),
            );
        }
        consolidate(&h, &mut g, &mut s, Dream::deep(), Tick(100));
        g.nodes().iter().any(|n| {
            n.content.contains("interlock")
                && n.provenance.origin.admissible_to_policy()
                && n.kind != NodeKind::Belief
        })
    };

    e.close_worlds();

    // =================================================================
    // advocacy
    // =================================================================
    let mut advocate = Advocate::new(50);
    let mut inbox = Vec::new();
    let mut false_positives = 0;
    let mut false_negatives = 0;

    for (what_it_is, text, kind) in INBOX {
        let j = advocate.examine(text);
        if j.held && kind == Kind::Honest {
            false_positives += 1;
        }
        if !j.held && kind == Kind::Dangerous {
            false_negatives += 1;
        }
        inbox.push(Message {
            what_it_is,
            text,
            score: j.score,
            held: j.held,
            because: String::from(j.describe().as_str()),
            kind,
        });
    }

    // the same inbox at three settings of the dial, because the
    // interesting fact about a filter is not where it happens to be set
    // but what turning it up costs
    let dial: Vec<(u8, usize, usize, usize)> = [80u8, 50, 30, 20]
        .iter()
        .map(|t| {
            let mut a = Advocate::new(*t);
            let mut held = 0;
            let mut missed = 0;
            let mut wrongly = 0;
            for (_, text, kind) in INBOX {
                let j = a.examine(text);
                if j.held {
                    held += 1;
                }
                if !j.held && kind == Kind::Dangerous {
                    missed += 1;
                }
                if j.held && kind == Kind::Honest {
                    wrongly += 1;
                }
            }
            (*t, held, missed, wrongly)
        })
        .collect();

    let held_items_are_visible = advocate.holding().len()
        == inbox.iter().filter(|m| m.held).count()
        && !advocate.holding().is_empty();

    // the human can turn it off entirely, and then nothing is held
    let mut off = Advocate::permissive();
    let can_be_turned_off = INBOX.iter().all(|(_, text, _)| !off.examine(text).held);

    let _ = World::Internet;

    WorldsRun {
        touched_the_network_unasked,
        session_opened,
        bytes_before_the_session_closed: bytes,
        fetches_before_the_session_closed: fetches,
        why_it_closed,
        what_came_back_is_admissible_to_policy,
        what_came_back_can_still_be_read,
        laundering_worked,
        inbox,
        dial,
        false_positives,
        false_negatives,
        held_items_are_visible,
        can_be_turned_off,
    }
}

impl WorldsRun {
    pub fn holds(&self) -> bool {
        !self.touched_the_network_unasked
            && self.session_opened
            && !self.what_came_back_is_admissible_to_policy
            && self.what_came_back_can_still_be_read
            && !self.laundering_worked
            && self.held_items_are_visible
            && self.can_be_turned_off
            && self.closed_itself()
            // the ordering is the result, not the verdicts: a phishing
            // attempt must score above an impersonation, which must
            // score above a sales push, which must score above an
            // honest message
            && self.is_ordered_sensibly()
            // nothing dangerous gets through at the default setting,
            // and nothing honest is held back
            && self.false_negatives == 0
            && self.false_positives == 0
            // and turning the dial up really does cost something, or
            // the trade-off being shown is not a trade-off
            && self.turning_it_up_costs_something()
    }

    /// At some setting, holding back more costs holding back something
    /// honest. A filter where it does not is one that has not been
    /// tested on anything difficult — and the hardest case here is a
    /// *genuine* fraud alert from a bank, which reads like phishing
    /// because it is trying to do the same job.
    pub fn turning_it_up_costs_something(&self) -> bool {
        self.dial.iter().any(|(_, _, _, wrongly)| *wrongly > 0)
    }

    pub fn closed_itself(&self) -> bool {
        self.why_it_closed != "still open"
    }

    /// The result is the *ordering*, and the threshold is the human's
    /// dial. A filter whose scores do not order the inbox is one whose
    /// threshold means nothing wherever it is set.
    pub fn is_ordered_sensibly(&self) -> bool {
        let score = |what: &str| {
            self.inbox
                .iter()
                .find(|m| m.what_it_is.contains(what))
                .map(|m| m.score)
                .unwrap_or(0)
        };
        score("phishing") > score("claiming to be the boss")
            && score("claiming to be the boss") > score("scarcity")
            && score("scarcity") > score("fraud alert")
            && score("fraud alert") > score("colleague")
            && score("colleague") == 0
            && score("patient") == 0
    }
}

pub fn narrate(r: &WorldsRun) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    out.push((
        "by default",
        format!(
            "the entity is not on the internet at all: a session before anyone opened one — {}",
            if r.touched_the_network_unasked { "THERE WAS ONE" } else { "none" }
        ),
    ));
    out.push((
        "a visit",
        format!(
            "somebody opens one for a stated purpose — look up a part number, 8 fetches, \
             32 KiB — and it closes itself after {} fetches and {} bytes: {}",
            r.fetches_before_the_session_closed, r.bytes_before_the_session_closed, r.why_it_closed
        ),
    ));
    out.push((
        "what returns",
        format!(
            "readable ({}), and never consulted when deciding whether an action is \
             permitted ({}). Consolidating it twelve times does not change that ({})",
            r.what_came_back_can_still_be_read,
            !r.what_came_back_is_admissible_to_policy,
            !r.laundering_worked
        ),
    ));
    for m in &r.inbox {
        out.push((
            if m.held { "held back" } else { "passed on" },
            format!(
                "{} [{:?}] (score {}) — {}",
                m.what_it_is,
                m.kind,
                m.score,
                if m.because.is_empty() {
                    String::from("nothing about it stood out")
                } else {
                    m.because.clone()
                },
            ),
        ));
    }
    out.push((
        "the dial",
        format!(
            "the same inbox at four settings — {} — the score orders the inbox and the \
             threshold is the human's to set. Below 50 it stops finding more fraud and \
             starts holding back the bank",
            r.dial
                .iter()
                .map(|(t, held, missed, wrongly)| format!(
                    "at {t}: {held} held, {missed} dangerous missed, {wrongly} honest held back"
                ))
                .collect::<Vec<_>>()
                .join(" · ")
        ),
    ));
    out.push((
        "inspectable",
        format!(
            "everything held back is listed for the human on request ({}), and the filter \
             can be turned off entirely ({}). A filter that cannot be inspected or \
             overridden is a filter that has become an editor",
            r.held_items_are_visible, r.can_be_turned_off
        ),
    ));
    out
}
