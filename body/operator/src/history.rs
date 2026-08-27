//! Conversation history — shared between fast and deliberate tiers.
//! Capped at 60 turns (30 user + 30 assistant) to bound context growth.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

pub const MAX_TURNS: usize = 60;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Turn {
    pub role: String,
    pub content: serde_json::Value,
}

impl Turn {
    pub fn user(text: &str) -> Turn {
        Turn { role: "user".into(), content: serde_json::Value::String(text.into()) }
    }
    pub fn assistant(text: &str) -> Turn {
        Turn { role: "assistant".into(), content: serde_json::Value::String(text.into()) }
    }
}

pub struct History {
    turns: VecDeque<Turn>,
}

impl History {
    pub fn new() -> History {
        History { turns: VecDeque::new() }
    }

    pub fn push(&mut self, turn: Turn) {
        self.turns.push_back(turn);
        while self.turns.len() > MAX_TURNS {
            self.turns.pop_front();
        }
    }

    /// Returns turns as a Vec for API calls.
    pub fn as_vec(&self) -> Vec<Turn> {
        self.turns.iter().cloned().collect()
    }

    /// Returns turns as serde_json array for the Anthropic API.
    pub fn as_json(&self) -> Vec<serde_json::Value> {
        self.turns.iter().map(|t| {
            serde_json::json!({
                "role": t.role,
                "content": t.content,
            })
        }).collect()
    }

    pub fn len(&self) -> usize {
        self.turns.len()
    }

    pub fn clear(&mut self) {
        self.turns.clear();
    }
}

impl Default for History {
    fn default() -> Self {
        History::new()
    }
}
