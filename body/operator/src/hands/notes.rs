//! Notes hand — reads/writes the private notebook via journal_io.

use serde_json::Value;
use super::HandCtx;
use crate::journal_io::JournalIO;

pub async fn write(input: &Value, ctx: &HandCtx) -> String {
    let content = input["content"].as_str()
        .or(input.as_str())
        .unwrap_or("").trim();
    if content.is_empty() { return "error: nothing to note".into(); }

    let io = JournalIO::new(&ctx.brain_dir);
    let mut existing = io.load_notes();
    // append with timestamp
    let ts = chrono::Local::now().format("%Y-%m-%d %H:%M").to_string();
    existing.push_str(&format!("[{ts}] {content}\n"));
    match io.save_notes(&existing) {
        Ok(_) => format!("noted: {content}"),
        Err(e) => format!("error saving note: {e}"),
    }
}

pub async fn read(input: &Value, ctx: &HandCtx) -> String {
    let topic = input["topic"].as_str().or(input.as_str()).unwrap_or("").trim();
    let io = JournalIO::new(&ctx.brain_dir);
    let notes = io.load_notes();
    if notes.is_empty() {
        return "notebook is empty".into();
    }
    if topic.is_empty() {
        // return last 2000 chars
        let s = if notes.len() > 2000 {
            format!("…{}", &notes[notes.len() - 2000..])
        } else {
            notes
        };
        s
    } else {
        // filter lines containing the topic
        let matching: Vec<&str> = notes.lines()
            .filter(|l| l.to_ascii_lowercase().contains(&topic.to_ascii_lowercase()))
            .collect();
        if matching.is_empty() {
            format!("no notes about '{topic}'")
        } else {
            matching.join("\n")
        }
    }
}
