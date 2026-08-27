//! SSE event parser for the Anthropic streaming API.
//!
//! Parses a stream of `event: <type>\ndata: <json>\n\n` blocks.
//! Returns typed events for content_block_start/delta/stop, message_delta,
//! message_stop. All other event types are passed through as Unknown.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SseEvent {
    MessageStart {
        message: Value,
    },
    ContentBlockStart {
        index: usize,
        content_block: ContentBlock,
    },
    ContentBlockDelta {
        index: usize,
        delta: Delta,
    },
    ContentBlockStop {
        index: usize,
    },
    MessageDelta {
        delta: MessageDeltaData,
        usage: Option<Value>,
    },
    MessageStop,
    Ping,
    Error {
        error: Value,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text { text: String },
    ToolUse { id: String, name: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Delta {
    TextDelta { text: String },
    InputJsonDelta { partial_json: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MessageDeltaData {
    pub stop_reason: Option<String>,
    pub stop_sequence: Option<String>,
}

/// Parse an SSE buffer (potentially containing many events) into a list of
/// (event_type, data_json) pairs. The caller then tries to deserialize data
/// into `SseEvent`.
///
/// Lines starting with ':' are comments (heartbeats) and are skipped.
/// An empty line terminates an event block.
pub fn parse_raw_events(buf: &str) -> Vec<(String, String)> {
    let mut events = Vec::new();
    let mut current_event = String::new();
    let mut current_data = String::new();

    for line in buf.lines() {
        if line.is_empty() {
            // end of event block
            if !current_data.is_empty() {
                events.push((current_event.clone(), current_data.clone()));
            }
            current_event.clear();
            current_data.clear();
            continue;
        }
        if line.starts_with(':') {
            // comment / heartbeat — ignore
            continue;
        }
        if let Some(evt) = line.strip_prefix("event: ") {
            current_event = evt.trim().to_string();
        } else if let Some(data) = line.strip_prefix("data: ") {
            if !current_data.is_empty() {
                current_data.push('\n');
            }
            current_data.push_str(data.trim());
        }
    }
    // flush last event (no trailing blank line)
    if !current_data.is_empty() {
        events.push((current_event, current_data));
    }
    events
}

/// Parse a single (event_type, data_json) pair into a typed SseEvent.
pub fn parse_event(event_type: &str, data: &str) -> Result<SseEvent> {
    // For events we know, wrap the data in a discriminant if not already tagged
    let v: Value = serde_json::from_str(data)?;
    // the 'type' field in the data JSON overrides the event line
    let type_str = v.get("type").and_then(|t| t.as_str())
        .unwrap_or(event_type);
    match type_str {
        "message_start" => Ok(SseEvent::MessageStart { message: v }),
        "content_block_start" => {
            let index = v["index"].as_u64().unwrap_or(0) as usize;
            let cb = v["content_block"].clone();
            let block_type = cb["type"].as_str().unwrap_or("");
            let content_block = match block_type {
                "tool_use" => ContentBlock::ToolUse {
                    id: cb["id"].as_str().unwrap_or("").to_string(),
                    name: cb["name"].as_str().unwrap_or("").to_string(),
                },
                _ => ContentBlock::Text {
                    text: cb["text"].as_str().unwrap_or("").to_string(),
                },
            };
            Ok(SseEvent::ContentBlockStart { index, content_block })
        }
        "content_block_delta" => {
            let index = v["index"].as_u64().unwrap_or(0) as usize;
            let d = &v["delta"];
            let delta_type = d["type"].as_str().unwrap_or("");
            let delta = match delta_type {
                "input_json_delta" => Delta::InputJsonDelta {
                    partial_json: d["partial_json"].as_str().unwrap_or("").to_string(),
                },
                _ => Delta::TextDelta {
                    text: d["text"].as_str().unwrap_or("").to_string(),
                },
            };
            Ok(SseEvent::ContentBlockDelta { index, delta })
        }
        "content_block_stop" => {
            let index = v["index"].as_u64().unwrap_or(0) as usize;
            Ok(SseEvent::ContentBlockStop { index })
        }
        "message_delta" => {
            let delta_v = &v["delta"];
            let delta = MessageDeltaData {
                stop_reason: delta_v["stop_reason"].as_str().map(|s| s.to_string()),
                stop_sequence: delta_v["stop_sequence"].as_str().map(|s| s.to_string()),
            };
            Ok(SseEvent::MessageDelta { delta, usage: v.get("usage").cloned() })
        }
        "message_stop" => Ok(SseEvent::MessageStop),
        "ping" => Ok(SseEvent::Ping),
        "error" => Ok(SseEvent::Error { error: v["error"].clone() }),
        _ => {
            // Unknown event — treat as ping
            Ok(SseEvent::Ping)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CANNED_STREAM: &str = r#"event: message_start
data: {"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","content":[],"model":"claude-fable-5","stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":25,"output_tokens":1}}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}

event: ping
data: {"type": "ping"}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":", world!"}}

event: content_block_stop
data: {"type":"content_block_stop","index":0}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":10}}

event: message_stop
data: {"type":"message_stop"}

"#;

    const TOOL_USE_STREAM: &str = r#"event: content_block_start
data: {"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"tool_1","name":"fs.list"}}

event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":"}}

event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"\"/tmp\"}"}}

event: content_block_stop
data: {"type":"content_block_stop","index":1}

"#;

    #[test]
    fn test_parse_raw_events() {
        let events = parse_raw_events(CANNED_STREAM);
        assert!(!events.is_empty(), "expected events");
        let types: Vec<&str> = events.iter().map(|(t, _)| t.as_str()).collect();
        assert!(types.contains(&"message_start"), "missing message_start");
        assert!(types.contains(&"content_block_start"), "missing content_block_start");
        assert!(types.contains(&"content_block_delta"), "missing content_block_delta");
        assert!(types.contains(&"message_delta"), "missing message_delta");
        assert!(types.contains(&"message_stop"), "missing message_stop");
    }

    #[test]
    fn test_text_accumulation() {
        let events = parse_raw_events(CANNED_STREAM);
        let mut text = String::new();
        for (event_type, data) in &events {
            if let Ok(SseEvent::ContentBlockDelta { delta: Delta::TextDelta { text: t }, .. }) =
                parse_event(event_type, data)
            {
                text.push_str(&t);
            }
        }
        assert_eq!(text, "Hello, world!");
    }

    #[test]
    fn test_stop_reason() {
        let events = parse_raw_events(CANNED_STREAM);
        let mut stop_reason = None;
        for (event_type, data) in &events {
            if let Ok(SseEvent::MessageDelta { delta, .. }) = parse_event(event_type, data) {
                stop_reason = delta.stop_reason;
            }
        }
        assert_eq!(stop_reason.as_deref(), Some("end_turn"));
    }

    #[test]
    fn test_tool_use_block() {
        let events = parse_raw_events(TOOL_USE_STREAM);
        // find the content_block_start
        let start = events.iter().find(|(t, _)| t == "content_block_start").unwrap();
        let evt = parse_event(&start.0, &start.1).unwrap();
        match evt {
            SseEvent::ContentBlockStart { content_block: ContentBlock::ToolUse { name, id }, .. } => {
                assert_eq!(name, "fs.list");
                assert_eq!(id, "tool_1");
            }
            other => panic!("expected ToolUse, got {:?}", other),
        }

        // accumulate input_json_delta
        let mut partial = String::new();
        for (t, d) in &events {
            if let Ok(SseEvent::ContentBlockDelta { delta: Delta::InputJsonDelta { partial_json }, .. }) = parse_event(t, d) {
                partial.push_str(&partial_json);
            }
        }
        let parsed: serde_json::Value = serde_json::from_str(&partial).unwrap();
        assert_eq!(parsed["path"], "/tmp");
    }

    #[test]
    fn test_message_stop_event() {
        let events = parse_raw_events(CANNED_STREAM);
        let stop_evt = events.iter().find(|(t, _)| t == "message_stop").unwrap();
        let evt = parse_event(&stop_evt.0, &stop_evt.1).unwrap();
        assert!(matches!(evt, SseEvent::MessageStop));
    }

    #[test]
    fn test_ping_ignored() {
        let events = parse_raw_events(CANNED_STREAM);
        let ping = events.iter().find(|(t, _)| t == "ping").unwrap();
        let evt = parse_event(&ping.0, &ping.1).unwrap();
        assert!(matches!(evt, SseEvent::Ping));
    }
}
