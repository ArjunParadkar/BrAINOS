//! BFS  LEVEL    Operator (Stage 3-4 milestones) - the seat daemon on the Linux body  [crate brainos-operator]
//! BFS  PURPOSE  Push-to-talk voice: records the mic with pw-record, transcribes with whisper-cli, and speaks replies with piper and pw-play
//! BFS  DEFINES  Voice, Listen, clean_transcript, speakable
//! BFS  USES     this crate: config | external: std, tokio
//! BFS  USED BY  operator/hands/mod, operator/hands/voice_hand
//!
//! The voice loop — the entity's ears and mouth, inside the body.
//!
//! Push-to-talk from the face (`{"t":"ptt","on":true|false}`) drives this:
//!   ptt on  → `pw-record` starts capturing the default PipeWire source
//!   ptt off → capture stops (SIGINT), `whisper-cli` transcribes, the text runs
//!             through the ordinary tiers exactly as a typed utterance would,
//!             and the reply is spoken back with `piper` → `pw-play`.
//!
//! Nothing here is a test hook: the harness exercises this by pushing real
//! audio through the guest's PipeWire graph. Cancel kills capture and playback.

use crate::config::Config;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

const CAPTURE_WAV: &str = "/tmp/brainos-voice-capture.wav";
const TTS_WAV: &str = "/tmp/brainos-tts.wav";
/// 44-byte RIFF header + 0.25 s of 16 kHz s16 mono: anything shorter is a tap, not speech.
const MIN_CAPTURE_BYTES: u64 = 44 + 16000 * 2 / 4;

pub struct Voice {
    config: Arc<Config>,
    recorder: Mutex<Option<Child>>,
    player: Mutex<Option<Child>>,
}

pub enum Listen {
    /// Transcribed speech, cleaned.
    Heard(String),
    /// Capture worked but there was nothing intelligible in it.
    Silence,
    /// The ears themselves failed (no recorder, no model, no source). Human-readable.
    Failed(String),
}

impl Voice {
    pub fn new(config: Arc<Config>) -> Voice {
        Voice { config, recorder: Mutex::new(None), player: Mutex::new(None) }
    }

    pub fn has_ears(&self) -> bool {
        Path::new(&self.config.voice.whisper_bin).exists()
            && Path::new(&self.config.voice.whisper_model).exists()
            && (Path::new("/usr/bin/pw-record").exists() || Path::new("/usr/bin/arecord").exists())
    }

    pub fn has_mouth(&self) -> bool {
        Path::new(&self.config.voice.piper_bin).exists()
            && Path::new(&self.config.voice.piper_voice).exists()
            && (Path::new("/usr/bin/pw-play").exists() || Path::new("/usr/bin/aplay").exists())
    }

    /// Begin capturing. Idempotent: a second `start` while recording is a no-op.
    pub async fn start_recording(&self) -> Result<(), String> {
        let mut rec = self.recorder.lock().await;
        if rec.is_some() { return Ok(()); }
        let _ = std::fs::remove_file(CAPTURE_WAV);
        let max = self.config.voice.max_record_seconds.max(1);
        let child = if Path::new("/usr/bin/pw-record").exists() {
            // pw-record stops on SIGINT and finalises the RIFF header; `timeout`
            // bounds a ptt that never releases.
            Command::new("timeout")
                .args(["-s", "INT", &max.to_string(), "pw-record",
                       "--rate", "16000", "--channels", "1", "--format", "s16", CAPTURE_WAV])
                .kill_on_drop(true)
                .spawn()
        } else {
            Command::new("arecord")
                .args(["-q", "-r", "16000", "-c", "1", "-f", "S16_LE",
                       "-d", &max.to_string(), CAPTURE_WAV])
                .kill_on_drop(true)
                .spawn()
        };
        match child {
            Ok(c) => { *rec = Some(c); Ok(()) }
            Err(e) => Err(format!("recorder failed to start: {e}")),
        }
    }

    pub async fn is_recording(&self) -> bool {
        self.recorder.lock().await.is_some()
    }

    /// Stop capturing and transcribe what was heard.
    pub async fn stop_and_transcribe(&self) -> Listen {
        let child = self.recorder.lock().await.take();
        let Some(child) = child else { return Listen::Failed("was not listening".into()); };
        if let Err(e) = stop_child(child, true).await {
            return Listen::Failed(format!("recorder did not stop cleanly: {e}"));
        }
        let size = std::fs::metadata(CAPTURE_WAV).map(|m| m.len()).unwrap_or(0);
        if size == 0 {
            return Listen::Failed("no audio was captured (is there a source?)".into());
        }
        if size < MIN_CAPTURE_BYTES { return Listen::Silence; }
        self.transcribe(Path::new(CAPTURE_WAV)).await
    }

    /// Abort capture without transcribing.
    pub async fn abort_recording(&self) {
        if let Some(child) = self.recorder.lock().await.take() {
            let _ = stop_child(child, false).await;
        }
    }

    pub async fn transcribe(&self, wav: &Path) -> Listen {
        let v = &self.config.voice;
        if !Path::new(&v.whisper_model).exists() {
            return Listen::Failed(format!("whisper model missing at {}", v.whisper_model));
        }
        let out = tokio::time::timeout(
            std::time::Duration::from_secs(90),
            Command::new(&v.whisper_bin)
                .args(["-m", &v.whisper_model, "-f", &wav.to_string_lossy(),
                       "-nt", "-np", "-l", "en", "-t", &v.whisper_threads.to_string()])
                .output(),
        ).await;
        match out {
            Ok(Ok(o)) if o.status.success() => {
                let text = clean_transcript(&String::from_utf8_lossy(&o.stdout));
                if text.is_empty() { Listen::Silence } else { Listen::Heard(text) }
            }
            Ok(Ok(o)) => Listen::Failed(format!("whisper exited {}: {}", o.status,
                String::from_utf8_lossy(&o.stderr).lines().last().unwrap_or("").trim())),
            Ok(Err(e)) => Listen::Failed(format!("whisper failed to run: {e}")),
            Err(_) => Listen::Failed("whisper timed out".into()),
        }
    }

    /// Speak `text` aloud. Cuts off anything already playing. Returns the
    /// synthesised wav path so a caller can inspect it, or an error string.
    pub async fn speak(&self, text: &str) -> Result<PathBuf, String> {
        let spoken = speakable(text);
        if spoken.is_empty() { return Err("nothing to say".into()); }
        let v = &self.config.voice;
        if !Path::new(&v.piper_voice).exists() {
            return Err(format!("piper voice missing at {}", v.piper_voice));
        }
        self.stop_speaking().await;
        let _ = std::fs::remove_file(TTS_WAV);

        let mut piper = Command::new(&v.piper_bin)
            .args(["--model", &v.piper_voice, "--output_file", TTS_WAV])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| format!("piper failed to start: {e}"))?;
        if let Some(mut stdin) = piper.stdin.take() {
            use tokio::io::AsyncWriteExt;
            let _ = stdin.write_all(spoken.as_bytes()).await;
            let _ = stdin.write_all(b"\n").await;
        }
        let st = tokio::time::timeout(std::time::Duration::from_secs(60), piper.wait()).await
            .map_err(|_| "piper timed out".to_string())?
            .map_err(|e| format!("piper: {e}"))?;
        if !st.success() || !Path::new(TTS_WAV).exists() {
            return Err(format!("piper exited {st}"));
        }

        let child = if Path::new("/usr/bin/pw-play").exists() {
            Command::new("pw-play").arg(TTS_WAV).kill_on_drop(true).spawn()
        } else {
            Command::new("aplay").args(["-q", TTS_WAV]).kill_on_drop(true).spawn()
        }.map_err(|e| format!("player failed to start: {e}"))?;
        *self.player.lock().await = Some(child);
        Ok(PathBuf::from(TTS_WAV))
    }

    /// Block until the current utterance has finished playing (or 60 s).
    pub async fn wait_spoken(&self) {
        let child = self.player.lock().await.take();
        if let Some(mut c) = child {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(60), c.wait()).await;
        }
    }

    pub async fn stop_speaking(&self) {
        if let Some(child) = self.player.lock().await.take() {
            let _ = stop_child(child, false).await;
        }
    }
}

/// Stop a child. `gentle` sends SIGINT first (pw-record needs it to finalise the
/// wav header) and waits up to 3 s before killing.
async fn stop_child(mut child: Child, gentle: bool) -> Result<(), String> {
    if let Some(pid) = child.id() {
        if gentle {
            let _ = Command::new("kill").args(["-INT", &pid.to_string()]).status().await;
            if tokio::time::timeout(std::time::Duration::from_secs(3), child.wait()).await.is_ok() {
                return Ok(());
            }
        }
    }
    child.kill().await.map_err(|e| e.to_string())?;
    let _ = child.wait().await;
    Ok(())
}

/// whisper prints one line per segment; drop its non-speech markers.
pub fn clean_transcript(raw: &str) -> String {
    let mut out = String::new();
    for line in raw.lines() {
        let mut s = line.trim();
        // timestamps if -nt was ignored: "[00:00.000 --> 00:02.000]  text"
        if s.starts_with('[') {
            if let Some(i) = s.find(']') {
                if s[..i].contains("-->") { s = s[i + 1..].trim(); }
            }
        }
        let s = strip_brackets(s);
        let s = s.trim();
        if s.is_empty() { continue; }
        if !out.is_empty() { out.push(' '); }
        out.push_str(s);
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Remove `[BLANK_AUDIO]`, `(wind blowing)`, `*music*` style annotations.
fn strip_brackets(s: &str) -> String {
    let mut out = String::new();
    let mut depth = 0i32;
    for ch in s.chars() {
        match ch {
            '[' | '(' => depth += 1,
            ']' | ')' => depth = (depth - 1).max(0),
            _ if depth == 0 => out.push(ch),
            _ => {}
        }
    }
    out
}

/// What the mouth gets: markdown stripped, code fences dropped, length bounded.
pub fn speakable(text: &str) -> String {
    let mut out = String::new();
    let mut in_fence = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with("```") { in_fence = !in_fence; continue; }
        if in_fence { continue; }
        let t = t.trim_start_matches(|c| c == '#' || c == '>' || c == '-' || c == '*').trim();
        if t.is_empty() { continue; }
        if !out.is_empty() { out.push(' '); }
        out.push_str(t);
    }
    let out: String = out.chars().filter(|c| !matches!(c, '*' | '`' | '_' | '~' | '#')).collect();
    let out = out.split_whitespace().collect::<Vec<_>>().join(" ");
    if out.chars().count() > 600 {
        let cut: String = out.chars().take(600).collect();
        match cut.rfind(['.', '!', '?']) {
            Some(i) => cut[..=i].to_string(),
            None => cut,
        }
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcript_is_cleaned() {
        assert_eq!(clean_transcript(" What time is it?\n"), "What time is it?");
        assert_eq!(clean_transcript("[BLANK_AUDIO]\n"), "");
        assert_eq!(clean_transcript("[00:00.000 --> 00:02.000]   open the terminal\n (wind)\n"), "open the terminal");
        assert_eq!(clean_transcript("hello\n\n  world  "), "hello world");
    }

    #[test]
    fn speech_drops_markup() {
        assert_eq!(speakable("**Sure** — I'll open `foot`.\n```\nls -la\n```\n- done"), "Sure — I'll open foot. done");
        assert_eq!(speakable(""), "");
        let long = "word ".repeat(300);
        assert!(speakable(&long).chars().count() <= 600);
    }
}
