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
    let text = text.trim();
    let rules = compiled_rules();
    for (re, rule) in rules {
        if let Some(caps) = re.captures(text) {
            let target = if rule.target_group {
                caps.get(1).map(|m| m.as_str().trim().to_string()).unwrap_or_default()
            } else {
                rule.target_literal.to_string()
            };
            return Some(ReflexIntent::new(rule.verb, &target));
        }
    }
    None
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
    fn test_no_match() {
        no_match("hello how are you");
        no_match("explain quantum computing");
        no_match("write me a poem about the sea");
    }
}
