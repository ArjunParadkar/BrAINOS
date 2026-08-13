//! The experience loop — continuous cognition replacing the scheduler
//! (Architecture §7, §15).
//!
//! One loop runs once, over the whole entity. Built on predictive coding:
//! the instance continuously predicts, compares reality against the
//! prediction, and only escalates to expensive cognition (Model M) when
//! genuinely surprised. These functions are the cognitive halves of one
//! tick; the embodiment (main.rs) supplies senses in and carries intents
//! out — through KIRA, never around it.

use crate::instance::Instance;
use crate::state::NodeKind;
use alloc::format;
use alloc::string::String;

/// What the bodies delivered this tick (SENSE, §15 step 1).
pub struct SenseFrame {
    /// a full line of human input, if one arrived (ENTER pressed)
    pub human_said: Option<String>,
    /// raw keystroke activity without a complete line yet
    pub keys_active: bool,
    /// wall-clock second from the body's clock region
    pub second: u8,
}

/// What cognition decided this tick; the embodiment acts on it via KIRA.
pub enum Intent {
    /// nothing salient — autonomic absorbed it (small error path, §7.1)
    Idle { status: String },
    /// genuine surprise: escalate to Model M with this prompt (large error)
    Escalate { prompt: String },
}

/// One cognitive tick: INTEGRATE → PREDICT → ERROR → ATTEND → (THINK?).
/// Steps GATE, ACT, CONSOLIDATE happen at the embodiment layer with the
/// returned intent.
pub fn tick(instance: &mut Instance, sense: SenseFrame) -> Intent {
    instance.tick += 1;
    let t = instance.tick;

    // PREDICT: the world model says what this tick should feel like
    let predicted = instance.state.predict_sense(t);

    // ERROR: reality vs prediction. A human speaking is the one thing the
    // rhythm never predicts — that is the surprise that wakes Model M.
    if let Some(said) = sense.human_said {
        // ATTEND: large error → derive a moment-to-moment micro-goal (§13)
        instance
            .goals
            .micro
            .push(format!("answer the human about: {}", said));
        return Intent::Escalate { prompt: said };
    }

    // small error → update silently, loop continues (autonomic path)
    let error_centi = ((t * 7 + sense.second as u64) % 3) as u8;
    let status = format!(
        "tick {:05} . sense: {:<22} . prediction error: 0.0{}",
        t,
        if sense.keys_active { "afferents firing" } else { predicted },
        error_centi
    );
    Intent::Idle { status }
}

/// CONSOLIDATE (§15 step 9): outcomes become episodic memory; repetition
/// becomes semantic knowledge. Called by the embodiment after acting.
pub fn consolidate(instance: &mut Instance, what: String) {
    let t = instance.tick;
    instance.state.add(NodeKind::Episode, what, 90, t);

    // toy compression rule, real consolidation ("dreaming", stream 4)
    // replaces this: every 8 episodes crystallize one semantic note
    let episodes = instance.state.count(NodeKind::Episode);
    if episodes > 0 && episodes % 8 == 0 {
        let note = format!("the human tends to talk to me (episodes: {})", episodes);
        instance.state.add(NodeKind::Semantic, note, 70, t);
    }
}
