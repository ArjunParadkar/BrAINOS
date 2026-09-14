//! Tier 1: compiled reflex intent table.
//!
//! A pure Rust match + small regex table. No model call.
//! Target: < 5 ms from utterance to action dispatch.
//! Returns None when the utterance doesn't match — caller escalates to fast tier.

use regex::Regex;
use std::sync::OnceLock;

/// A parsed reflex intent.
#[derive(Debug, Clone, PartialEq)]
pub struct ReflexIntent {
    pub verb: String,
    pub target: String,
}

impl ReflexIntent {
    fn new(verb: &str, target: &str) -> Self {
        ReflexIntent { verb: verb.into(), target: target.into() }
    }
}

struct Rule {
    pattern: &'static str,
    verb: &'static str,
    /// capture group 1 becomes target (if any)
    target_group: bool,
    target_literal: &'static str,
}

static RULES: &[Rule] = &[
    // ---- a walking body (Phase 0) — "on miniblur, walk forward 3 steps" ----
    Rule { pattern: r"(?i)^(?:please\s+)?(?:stand(?:\s+up)?|get\s+up|rise)[.!]?$", verb: "gait.stand", target_group: false, target_literal: "" },
    Rule { pattern: r"(?i)^(?:please\s+)?(?:sit(?:\s+down)?|lie\s+down|rest)[.!]?$", verb: "gait.sit", target_group: false, target_literal: "" },
    Rule { pattern: r"(?i)^(?:please\s+)?(?:stop|halt|freeze|stay)(?:\s+(?:moving|walking|now))?[.!]?$", verb: "gait.stop", target_group: false, target_literal: "" },
    Rule { pattern: r"(?i)^(?:please\s+)?(?:walk|go|move|step|take)\s+((?:forward|forwards|ahead|back|backward|backwards|reverse|left|right)(?:\s+\d+(?:\s+steps?)?)?)[.!]?$", verb: "gait.walk", target_group: true, target_literal: "" },
    Rule { pattern: r"(?i)^(?:please\s+)?(?:walk|go|move|step|take)\s+(\d+\s+steps?\s+(?:forward|forwards|ahead|back|backward|backwards|left|right))[.!]?$", verb: "gait.walk", target_group: true, target_literal: "" },
    Rule { pattern: r"(?i)^(?:please\s+)?turn\s+(left|right)(?:\s+\d+(?:\s+steps?)?)?[.!]?$", verb: "gait.walk", target_group: true, target_literal: "" },
    Rule { pattern: r"(?i)^(?:how\s+are\s+you\s+doing|telemetry|status|how\s+is\s+the\s+battery|battery\s+status)[?.!]?$", verb: "body.telemetry", target_group: false, target_literal: "" },
    Rule { pattern: r"(?i)^(?:begin|start)\s+calibrat(?:ion|ing)[.!]?$", verb: "body.calibrate", target_group: false, target_literal: "begin" },
    Rule { pattern: r"(?i)^(?:end|finish|stop)\s+calibrat(?:ion|ing)[.!]?$", verb: "body.calibrate", target_group: false, target_literal: "end" },
    Rule { pattern: r"(?i)^record\s+servo\s+(\d+\s+.+)$", verb: "body.calibrate", target_group: true, target_literal: "" },
    Rule { pattern: r"(?i)^(?:set\s+)?servo\s+(\d+)\s+(?:to\s+)?(\d+(?:\.\d+)?)(?:\s*(?:deg|degrees|°))?[.!]?$", verb: "servo.set", target_group: true, target_literal: "" },

    // ---- open/launch/start app ----
    Rule { pattern: r"(?i)^(?:open|launch|start|run)\s+(.+)$", verb: "app.open", target_group: true, target_literal: "" },

    // ---- close app ----
    Rule { pattern: r"(?i)^close\s+(.+)$", verb: "app.close", target_group: true, target_literal: "" },

    // ---- volume ----
    Rule { pattern: r"(?i)^(?:volume\s+)?(?:turn\s+)?up(?:\s+(?:the\s+)?volume)?$", verb: "sys.volume", target_group: false, target_literal: "up" },
    Rule { pattern: r"(?i)^(?:volume\s+)?(?:turn\s+)?down(?:\s+(?:the\s+)?volume)?$", verb: "sys.volume", target_group: false, target_literal: "down" },
    Rule { pattern: r"(?i)^(?:please\s+)?(?:turn|crank|bring|make|put)\s+(?:the\s+|my\s+)?(?:volume|sound|audio|music)\s+(?:up|louder|higher)(?:\s+(?:a\s+)?(?:bit|little|notch|touch))?(?:\s+please)?[.!]?$", verb: "sys.volume", target_group: false, target_literal: "up" },
    Rule { pattern: r"(?i)^(?:please\s+)?(?:turn|bring|make|put)\s+(?:the\s+|my\s+)?(?:volume|sound|audio|music)\s+(?:down|lower|quieter|softer)(?:\s+(?:a\s+)?(?:bit|little|notch|touch))?(?:\s+please)?[.!]?$", verb: "sys.volume", target_group: false, target_literal: "down" },
    Rule { pattern: r"(?i)^(?:please\s+)?(?:volume|sound|audio)\s+(?:up|louder)(?:\s+(?:a\s+)?(?:bit|little))?[.!]?$", verb: "sys.volume", target_group: false, target_literal: "up" },
    Rule { pattern: r"(?i)^(?:please\s+)?(?:volume|sound|audio)\s+(?:down|lower|quieter)(?:\s+(?:a\s+)?(?:bit|little))?[.!]?$", verb: "sys.volume", target_group: false, target_literal: "down" },
    Rule { pattern: r"(?i)^(?:please\s+)?louder(?:\s+please)?[.!]?$", verb: "sys.volume", target_group: false, target_literal: "up" },
    Rule { pattern: r"(?i)^(?:please\s+)?(?:quieter|softer)(?:\s+please)?[.!]?$", verb: "sys.volume", target_group: false, target_literal: "down" },
    Rule { pattern: r"(?i)^(?:mute|unmute)(?:\s+(?:the\s+)?(?:sound|audio|volume))?$", verb: "sys.volume", target_group: false, target_literal: "mute" },
    Rule { pattern: r"(?i)^(?:set\s+)?volume\s+(?:to\s+)?(\d+)%?$", verb: "sys.volume", target_group: true, target_literal: "" },
    Rule { pattern: r"(?i)^(\d+)%\s+volume$", verb: "sys.volume", target_group: true, target_literal: "" },

    // ---- brightness ----
    Rule { pattern: r"(?i)^brightness\s+up$", verb: "sys.brightness", target_group: false, target_literal: "up" },
    Rule { pattern: r"(?i)^brightness\s+down$", verb: "sys.brightness", target_group: false, target_literal: "down" },
    Rule { pattern: r"(?i)^(?:please\s+)?(?:turn|make|bring)\s+(?:the\s+|my\s+)?(?:screen|display|brightness)\s+(?:up|brighter)(?:\s+(?:a\s+)?(?:bit|little))?[.!]?$", verb: "sys.brightness", target_group: false, target_literal: "up" },
    Rule { pattern: r"(?i)^(?:please\s+)?(?:turn|make|bring|dim)\s+(?:the\s+|my\s+)?(?:screen|display|brightness)(?:\s+(?:down|darker|dimmer))?(?:\s+(?:a\s+)?(?:bit|little))?[.!]?$", verb: "sys.brightness", target_group: false, target_literal: "down" },
    Rule { pattern: r"(?i)^(?:set\s+)?brightness\s+(?:to\s+)?(\d+)%?$", verb: "sys.brightness", target_group: true, target_literal: "" },

    // ---- screenshot ----
    Rule { pattern: r"(?i)^(?:take\s+(?:a\s+)?)?screenshot$", verb: "screen.look", target_group: false, target_literal: "screenshot" },
    Rule { pattern: r"(?i)^(?:capture|grab)\s+(?:the\s+)?screen$", verb: "screen.look", target_group: false, target_literal: "screenshot" },

    // ---- lock ----
    Rule { pattern: r"(?i)^lock(?:\s+(?:the\s+)?screen)?$", verb: "sys.lock", target_group: false, target_literal: "" },
    Rule { pattern: r"(?i)^lock\s+screen$", verb: "sys.lock", target_group: false, target_literal: "" },

    // ---- sleep / shutdown / reboot (ask first unless "now") ----
    Rule { pattern: r"(?i)^(?:go\s+to\s+)?sleep(?:\s+now)?$", verb: "sys.power", target_group: false, target_literal: "sleep" },
    Rule { pattern: r"(?i)^shutdown(?:\s+now)?$", verb: "sys.power", target_group: false, target_literal: "shutdown" },
    Rule { pattern: r"(?i)^(?:power\s+off)(?:\s+now)?$", verb: "sys.power", target_group: false, target_literal: "shutdown" },
    Rule { pattern: r"(?i)^reboot(?:\s+now)?$", verb: "sys.power", target_group: false, target_literal: "reboot" },
    Rule { pattern: r"(?i)^restart(?:\s+now)?$", verb: "sys.power", target_group: false, target_literal: "reboot" },

    // ---- wifi ----
    Rule { pattern: r"(?i)^wifi\s+on$", verb: "sys.wifi", target_group: false, target_literal: "on" },
    Rule { pattern: r"(?i)^wifi\s+off$", verb: "sys.wifi", target_group: false, target_literal: "off" },
    Rule { pattern: r"(?i)^wifi\s+status$", verb: "sys.wifi", target_group: false, target_literal: "status" },
    Rule { pattern: r"(?i)^(?:turn\s+on\s+wifi|enable\s+wifi)$", verb: "sys.wifi", target_group: false, target_literal: "on" },
    Rule { pattern: r"(?i)^(?:turn\s+off\s+wifi|disable\s+wifi)$", verb: "sys.wifi", target_group: false, target_literal: "off" },

    // ---- bluetooth ----
    Rule { pattern: r"(?i)^bluetooth\s+on$", verb: "sys.bluetooth", target_group: false, target_literal: "on" },
    Rule { pattern: r"(?i)^bluetooth\s+off$", verb: "sys.bluetooth", target_group: false, target_literal: "off" },

    // ---- type text ----
    Rule { pattern: r#"(?i)^type\s+"(.+)"$"#, verb: "input.type", target_group: true, target_literal: "" },
    Rule { pattern: r"(?i)^type\s+(.+)$", verb: "input.type", target_group: true, target_literal: "" },

    // ---- press key ----
    Rule { pattern: r"(?i)^press\s+(.+)$", verb: "input.key", target_group: true, target_literal: "" },
    Rule { pattern: r"(?i)^(?:hit|tap)\s+(.+)$", verb: "input.key", target_group: true, target_literal: "" },

    // ---- search ----
    Rule { pattern: r"(?i)^search(?:\s+for)?\s+(.+)$", verb: "web.search", target_group: true, target_literal: "" },
    Rule { pattern: r"(?i)^(?:look\s+up|google)\s+(.+)$", verb: "web.search", target_group: true, target_literal: "" },

    // ---- go to url ----
    Rule { pattern: r"(?i)^(?:go\s+to|navigate\s+to|open)\s+(https?://\S+)$", verb: "web.open", target_group: true, target_literal: "" },
    Rule { pattern: r"(?i)^(?:go\s+to|navigate\s+to)\s+(\S+\.\S+)$", verb: "web.open", target_group: true, target_literal: "" },

    // ---- workspaces ----
    Rule { pattern: r"(?i)^(?:go\s+to\s+)?next\s+workspace$", verb: "window.workspace", target_group: false, target_literal: "next" },
    Rule { pattern: r"(?i)^(?:go\s+to\s+)?(?:previous|prev)\s+workspace$", verb: "window.workspace", target_group: false, target_literal: "prev" },
    Rule { pattern: r"(?i)^(?:go\s+to\s+)?workspace\s+(\d+)$", verb: "window.workspace", target_group: true, target_literal: "" },
    Rule { pattern: r"(?i)^(?:switch\s+to\s+)?workspace\s+(\d+)$", verb: "window.workspace", target_group: true, target_literal: "" },

    // ---- focus app ----
    Rule { pattern: r"(?i)^focus\s+(.+)$", verb: "window.focus", target_group: true, target_literal: "" },
    Rule { pattern: r"(?i)^switch\s+to\s+(.+)$", verb: "window.focus", target_group: true, target_literal: "" },

    // ---- fullscreen / tile ----
    Rule { pattern: r"(?i)^(?:make\s+)?fullscreen$", verb: "window.fullscreen", target_group: false, target_literal: "" },
    Rule { pattern: r"(?i)^(?:tile|split)\s+(?:the\s+)?(?:window|screen)?$", verb: "window.tile", target_group: false, target_literal: "split" },

    // ---- notes ----
    Rule { pattern: r"(?i)^(?:note|notes|remember|write\s+down|jot\s+down|keep)[:.]?\s+(.+)$", verb: "notes.write", target_group: true, target_literal: "" },
    Rule { pattern: r"(?i)^(?:read|show)\s+(?:my\s+)?notes$", verb: "notes.read", target_group: false, target_literal: "" },
    Rule { pattern: r"(?i)^what(?:'s|\s+is)\s+in\s+(?:my\s+)?notes$", verb: "notes.read", target_group: false, target_literal: "" },
    Rule { pattern: r"(?i)^read\s+(?:my\s+)?notes(?:\s+about\s+(.+))?$", verb: "notes.read", target_group: true, target_literal: "" },

    // ---- Stage 4: the entity about itself ----
    Rule { pattern: r"(?i)^forget\s+(?:that|it|this|the\s+last\s+one)$", verb: "skill.forget", target_group: false, target_literal: "that" },
    Rule { pattern: r"(?i)^forget\s+how\s+to\s+(.+)$", verb: "skill.forget", target_group: true, target_literal: "" },
    Rule { pattern: r"(?i)^(?:what\s+do\s+you\s+know\s+how\s+to\s+do|what\s+are\s+your\s+skills|list\s+(?:your\s+)?skills|what\s+can\s+you\s+do\s+from\s+memory)$", verb: "skill.list", target_group: false, target_literal: "" },
    Rule { pattern: r"(?i)^(?:what\s+are\s+you\s+watching|list\s+(?:standing\s+)?tasks|what\s+are\s+your\s+(?:standing\s+)?(?:tasks|orders)|show\s+(?:standing\s+)?tasks)$", verb: "task.list", target_group: false, target_literal: "" },
    Rule { pattern: r"(?i)^stop\s+(?:task|watcher)\s+#?(\d+)$", verb: "task.stop", target_group: true, target_literal: "" },
    Rule { pattern: r"(?i)^(?:(?:which|what)\s+(?:bodies|devices)\s+(?:are\s+you\s+in|do\s+you\s+have)|where\s+are\s+you|list\s+(?:your\s+)?bodies)$", verb: "body.list", target_group: false, target_literal: "" },
    Rule { pattern: r"(?i)^(?:which|what)\s+(?:brains|models)\s+do\s+you\s+have(?:\s+right\s+now)?$", verb: "pool.status", target_group: false, target_literal: "" },
    Rule { pattern: r"(?i)^(?:release|forget|drop)\s+(?:the\s+)?body\s+(.+)$", verb: "body.forget", target_group: true, target_literal: "" },

    // ---- time / date / battery ----
    Rule { pattern: r"(?i)^what(?:'s|\s+is)?\s+the\s+(?:current\s+)?time\??$", verb: "sys.time", target_group: false, target_literal: "" },
    Rule { pattern: r"(?i)^what\s+time\s+is\s+it\??$", verb: "sys.time", target_group: false, target_literal: "" },
    Rule { pattern: r"(?i)^what(?:'s|\s+is)?\s+(?:today's\s+)?(?:the\s+)?date\??$", verb: "sys.date", target_group: false, target_literal: "" },
    Rule { pattern: r"(?i)^what(?:'s|\s+is)?\s+(?:the\s+)?battery(?:\s+level)?\??$", verb: "sys.battery", target_group: false, target_literal: "" },
    Rule { pattern: r"(?i)^(?:how\s+(?:much\s+)?)?battery(?:\s+(?:do\s+I\s+have|left|remaining))?\??$", verb: "sys.battery", target_group: false, target_literal: "" },
];

/// Compiled rule set — built once on first call.
static COMPILED: OnceLock<Vec<(Regex, &'static Rule)>> = OnceLock::new();

fn compiled_rules() -> &'static Vec<(Regex, &'static Rule)> {
    COMPILED.get_or_init(|| {
        RULES.iter().map(|r| (Regex::new(r.pattern).expect("bad reflex regex"), r)).collect()
    })
}

/// Try to match `text` against the reflex table.
/// Returns `Some(ReflexIntent)` on match, `None` to escalate.
pub fn match_intent(text: &str) -> Option<ReflexIntent> {
    let text = normalize_utterance(text);
    let rules = compiled_rules();
    for (re, rule) in rules {
        if let Some(caps) = re.captures(&text) {
            let target = if rule.target_group {
                let raw = caps.get(1).map(|m| m.as_str()).unwrap_or_default();
                if rule.verb == "servo.set" {
                    // "<ch> <deg>": two captures
                    let deg = caps.get(2).map(|m| m.as_str()).unwrap_or("0");
                    return Some(ReflexIntent::new(rule.verb, &format!("{raw} {deg}")));
                }
                // typed text is payload, not a name: keep it verbatim
                if rule.verb == "input.type" { raw.to_string() } else { normalize_target(raw) }
            } else {
                rule.target_literal.to_string()
            };
            return Some(ReflexIntent::new(rule.verb, &target));
        }
    }
    None
}

/// Speech arrives punctuated and capitalised ("Open the terminal."); typed text
/// arrives however the human felt like typing it. Both must hit the same rule.
pub fn normalize_utterance(text: &str) -> String {
    let mut t: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    // a fully quoted utterance ("Lock the screen.") loses its quotes
    let quoted = |c: char| matches!(c, '"' | '\u{201c}' | '\u{201d}');
    if t.chars().next().map_or(false, quoted) && t.chars().last().map_or(false, quoted) && t.chars().count() > 2 {
        t = t.trim_start_matches(quoted).trim_end_matches(quoted).to_string();
    }
    let strip_end = |x: &str| x.trim_end_matches(|c: char| matches!(c, '.' | '!' | '?' | ',' | ';' | ':')).trim_end().to_string();
    t = strip_end(&t);
    // "please" at either end never changes the intent
    if t.len() > 7 && t[..7].eq_ignore_ascii_case("please ") { t = t[7..].trim_start_matches(',').trim().to_string(); }
    let lower = t.to_ascii_lowercase();
    if let Some(stem) = lower.strip_suffix("please") {
        if stem.is_empty() || stem.ends_with(' ') || stem.ends_with(',') {
            t = strip_end(&t[..stem.len()]);
        }
    }
    t.trim().to_string()
}

/// Captured targets lose their article: "the terminal" -> "terminal".
fn normalize_target(raw: &str) -> String {
    let t = raw.trim().trim_end_matches(|c: char| matches!(c, '.' | '!' | '?' | ','));
    let t = ["the ", "a ", "an ", "my ", "up "].iter()
        .find_map(|a| t.strip_prefix(a))
        .unwrap_or(t);
    t.trim().to_string()
}

/// Whether a power verb needs confirmation (unless "now" is in the utterance).
pub fn needs_confirm(intent: &ReflexIntent, original: &str) -> bool {
    matches!(intent.verb.as_str(), "sys.power")
        && !original.to_ascii_lowercase().contains("now")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(text: &str, verb: &str, target: &str) {
        let result = match_intent(text);
        assert!(
            result.is_some(),
            "Expected match for {:?} but got None", text
        );
        let r = result.unwrap();
        assert_eq!(r.verb, verb, "verb mismatch for {:?}: got {:?}", text, r.verb);
        if !target.is_empty() {
            assert_eq!(r.target, target, "target mismatch for {:?}: got {:?}", text, r.target);
        }
    }

    fn no_match(text: &str) {
        assert!(match_intent(text).is_none(), "Expected no match for {:?}", text);
    }

    #[test]
    fn test_spoken_forms() {
        // whisper output: capitalised, punctuated, articled
        check("Open the terminal.", "app.open", "terminal");
        check("What time is it?", "sys.time", "");
        check("Close Firefox, please.", "app.close", "Firefox");
        check("please open Firefox", "app.open", "Firefox");
        check(r#"type "Hello, World.""#, "input.type", "Hello, World.");
        check("  turn the volume down a bit!  ", "sys.volume", "down");
        assert_eq!(normalize_utterance("\u{201c}Lock the screen.\u{201d}"), "Lock the screen");
    }

    #[test]
    fn test_app_open() {
        check("open firefox", "app.open", "firefox");
        check("launch spotify", "app.open", "spotify");
        check("start terminal", "app.open", "terminal");
        check("run code", "app.open", "code");
    }

    #[test]
    fn test_app_close() {
        check("close firefox", "app.close", "firefox");
        check("close terminal", "app.close", "terminal");
    }

    #[test]
    fn test_volume() {
        check("turn the volume down a bit", "sys.volume", "down");
        check("crank the music up", "sys.volume", "up");
        check("quieter please", "sys.volume", "down");
        check("dim the screen a little", "sys.brightness", "down");
        check("volume up", "sys.volume", "up");
        check("up", "sys.volume", "up");
        check("volume down", "sys.volume", "down");
        check("mute", "sys.volume", "mute");
        check("unmute", "sys.volume", "mute");
        check("set volume to 50%", "sys.volume", "50");
        check("volume 80%", "sys.volume", "80");
        check("50% volume", "sys.volume", "50");
    }

    #[test]
    fn test_brightness() {
        check("brightness up", "sys.brightness", "up");
        check("brightness down", "sys.brightness", "down");
        check("set brightness to 75%", "sys.brightness", "75");
    }

    #[test]
    fn test_screenshot() {
        check("screenshot", "screen.look", "screenshot");
        check("take a screenshot", "screen.look", "screenshot");
        check("capture the screen", "screen.look", "screenshot");
    }

    #[test]
    fn test_lock() {
        check("lock", "sys.lock", "");
        check("lock screen", "sys.lock", "");
        check("lock the screen", "sys.lock", "");
    }

    #[test]
    fn test_power() {
        check("sleep", "sys.power", "sleep");
        check("shutdown", "sys.power", "shutdown");
        check("reboot", "sys.power", "reboot");
        check("restart", "sys.power", "reboot");
        check("power off", "sys.power", "shutdown");
        // confirm needed (no "now")
        assert!(needs_confirm(&match_intent("shutdown").unwrap(), "shutdown"));
        // no confirm needed (has "now")
        assert!(!needs_confirm(&match_intent("shutdown now").unwrap(), "shutdown now"));
    }

    #[test]
    fn test_wifi() {
        check("wifi on", "sys.wifi", "on");
        check("wifi off", "sys.wifi", "off");
        check("wifi status", "sys.wifi", "status");
        check("turn on wifi", "sys.wifi", "on");
        check("disable wifi", "sys.wifi", "off");
    }

    #[test]
    fn test_bluetooth() {
        check("bluetooth on", "sys.bluetooth", "on");
        check("bluetooth off", "sys.bluetooth", "off");
    }

    #[test]
    fn test_type() {
        check(r#"type "hello world""#, "input.type", "hello world");
        check("type hello", "input.type", "hello");
    }

    #[test]
    fn test_press() {
        check("press enter", "input.key", "enter");
        check("press ctrl+c", "input.key", "ctrl+c");
        check("hit tab", "input.key", "tab");
    }

    #[test]
    fn test_search() {
        check("search for rust programming", "web.search", "rust programming");
        check("look up tokio documentation", "web.search", "tokio documentation");
        check("google brainos", "web.search", "brainos");
    }

    #[test]
    fn test_navigate() {
        check("go to https://example.com", "web.open", "https://example.com");
        check("navigate to example.com", "web.open", "example.com");
    }

    #[test]
    fn test_workspace() {
        check("next workspace", "window.workspace", "next");
        check("previous workspace", "window.workspace", "prev");
        check("workspace 3", "window.workspace", "3");
        check("go to workspace 2", "window.workspace", "2");
        check("switch to workspace 5", "window.workspace", "5");
    }

    #[test]
    fn test_focus() {
        check("focus firefox", "window.focus", "firefox");
        check("switch to terminal", "window.focus", "terminal");
    }

    #[test]
    fn test_fullscreen_tile() {
        check("fullscreen", "window.fullscreen", "");
        check("tile window", "window.tile", "split");
    }

    #[test]
    fn test_notes() {
        check("note: buy milk", "notes.write", "buy milk");
        check("remember to call mom", "notes.write", "to call mom");
        check("read my notes", "notes.read", "");
        check("show my notes", "notes.read", "");
    }

    #[test]
    fn test_time_date_battery() {
        check("what time is it", "sys.time", "");
        check("what's the time", "sys.time", "");
        check("what's the date", "sys.date", "");
        check("what's the battery", "sys.battery", "");
        check("how much battery", "sys.battery", "");
    }

    #[test]
    fn test_entity_rules() {
        check("forget that", "skill.forget", "that");
        check("Forget how to list the files in /tmp.", "skill.forget", "list the files in /tmp");
        check("what do you know how to do", "skill.list", "");
        check("what are you watching", "task.list", "");
        check("stop task #3", "task.stop", "3");
        check("where are you", "body.list", "");
        check("which brains do you have right now", "pool.status", "");
        check("release the body pi", "body.forget", "pi");
    }

    #[test]
    fn test_no_match() {
        no_match("hello how are you");
        no_match("explain quantum computing");
        no_match("write me a poem about the sea");
    }
}
