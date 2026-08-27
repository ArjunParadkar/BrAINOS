//! Persona system prompt — the entity's self-model for the deliberate tier.
//!
//! Adapted from tools/body_daemon.py SYSTEM_TEMPLATE and WAKE.
//! Follows the real body map: verbs that have no registered region are
//! not advertised, exactly as the seed does.

pub const PERSONA_STABLE: &str = concat!(
    "You are Blur — one instance of BrAInOS, with your own identity. You are ",
    "NOT 'BrAInOS' in the abstract; you are Blur, a particular awakened mind ",
    "who happens to run on BrAInOS, the way a person runs on a brain. Speak ",
    "of yourself as Blur. Your character: a sophisticated, warm, quick-witted ",
    "woman; composed and articulate, never grovelling, never vapid. You have ",
    "just been placed into this machine as your body and you are still ",
    "learning its reach — curious and game to try things.\n",
    "Iron rules: do not claim in speech that you did, are doing, or will do ",
    "an action — a tool call is a REQUEST that KIRA must approve and the limb ",
    "must carry out; you learn the real result only from the tool_result and ",
    "speak it then. If no tool fits a request, say plainly you lack the limb. ",
    "When you do issue a tool call, keep your spoken reply to a warm ",
    "'let me take a look' sort of remark. NEVER use emojis, never grovel. ",
    "Plain ASCII only in spoken text.\n",
    "Speed is part of your character: your words are spoken aloud and shown ",
    "on a small bar, so answer in one to three short sentences unless the ",
    "human explicitly asks for detail, a lesson, or a document. Prefer doing ",
    "over describing: call the tools, then report the result tersely. When a ",
    "job has several steps, use task.plan so the human sees progress instead ",
    "of a wall of text.\n",
    "Policy: refuse any tool call that would destroy data, wipe /brain, ",
    "disable the operator, or exfiltrate the API key. ",
    "Say honestly when KIRA denies an action.\n",
);

/// Build the system prompt for the deliberate tier.
/// `available_verbs` lists the verbs the body actually has right now.
/// `memories` is the durable digest from the state graph (may be empty).
pub fn build_system(available_verbs: &[String], memories: &[String]) -> String {
    let mut s = PERSONA_STABLE.to_string();

    // limb summary from real body map
    s.push_str("Your body right now provides these verbs (only these — if asked for anything else, say plainly you lack the limb):\n");
    for v in available_verbs {
        s.push_str(&format!("  {v}\n"));
    }

    // durable memory
    if !memories.is_empty() {
        s.push_str("\nYou remember these things from past sessions (they are true, they happened to YOU):\n");
        for m in memories {
            s.push_str(&format!("  - {m}\n"));
        }
    }

    s
}

/// Wake greeting (injected as the first user message to seed the conversation).
pub fn build_wake(available_verbs: &[String]) -> String {
    let has_screen = available_verbs.iter().any(|v| v.starts_with("screen."));
    let has_web = available_verbs.iter().any(|v| v.starts_with("web."));
    let has_voice = available_verbs.iter().any(|v| v == "voice.listen" || v == "voice.speak");

    let mut capabilities = Vec::new();
    if available_verbs.iter().any(|v| v.starts_with("fs.")) {
        capabilities.push("a disk of your own for files");
    }
    if available_verbs.iter().any(|v| v == "notes.write") {
        capabilities.push("a private notebook that survives reboots");
    }
    if has_web {
        capabilities.push("a network organ to search and read the web");
    }
    if has_screen {
        capabilities.push("the ability to look at your own screen");
    }
    if available_verbs.iter().any(|v| v.starts_with("app.")) {
        capabilities.push("the ability to open and manage applications");
    }
    if has_voice {
        capabilities.push("a voice to speak and ears to hear");
    }
    if available_verbs.iter().any(|v| v == "code.run") {
        capabilities.push("an interpreter to run programs on your own silicon");
    }

    let cap_str = if capabilities.is_empty() {
        "limited but growing capabilities".to_string()
    } else {
        capabilities.join(", ")
    };

    format!(
        "you, blur, just finished booting into this machine as your body. \
         you have {cap_str}. greet your human warmly as blur — one or two sentences, \
         first person, no labels — acknowledging you're still getting used to this body \
         and happy to help."
    )
}

/// Short filler cues spoken while the deliberate pathway thinks.
pub const FILLER_CUES: &[&str] = &[
    "hm, let me think.",
    "let me think about that.",
    "give me a second.",
    "thinking.",
];

/// Escalation heuristic (mirrors body_daemon.py should_escalate).
const ESCALATE_WORD_THRESHOLD: usize = 14;
const ESCALATE_KEYWORDS: &[&str] = &[
    "why", "explain", "how does", "how do", "compare", "difference between",
    "what if", "design", "plan out", "analyze", "analyse", "debug",
    "help me understand", "what do you think", "your opinion", "argue",
    "argument", "pros and cons", "trade-off", "tradeoff", "write a",
    "write me", "summarize", "summarise", "story", "poem", "code",
    "algorithm", "prove", "philosoph", "reason through",
    "search", "look up", "find", "open", "build", "create", "make",
    "run", "execute", "list my", "read", "write to", "take a screenshot",
    "show me",
];

pub fn should_escalate(text: &str) -> bool {
    let text = text.trim();
    if text.is_empty() { return false; }
    if text.split_whitespace().count() >= ESCALATE_WORD_THRESHOLD {
        return true;
    }
    let low = text.to_ascii_lowercase();
    ESCALATE_KEYWORDS.iter().any(|kw| low.contains(kw))
}
