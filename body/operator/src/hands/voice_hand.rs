//! Voice hand — pw-record/arecord + whisper-cli for listen; piper + pw-play for speak.

use serde_json::Value;
use super::{HandCtx, run_cmd};

pub async fn listen(input: &Value, ctx: &HandCtx) -> String {
    let secs = input["seconds"].as_u64().unwrap_or(ctx.config.voice.record_seconds as u64).min(30);
    let wav = "/tmp/brainos-voice-capture.wav";

    // try pw-record first, fall back to arecord
    let rec_r = if std::path::Path::new("/usr/bin/pw-record").exists() {
        run_cmd(&["pw-record", "--target", "0", "--rate", "16000",
            "--channels", "1", "--format", "s16", wav], secs + 2).await
    } else {
        run_cmd(&["arecord", "-r", "16000", "-c", "1", "-f", "S16_LE",
            "-d", &secs.to_string(), wav], secs + 2).await
    };

    if rec_r.contains("error") || !std::path::Path::new(wav).exists() {
        return format!("recording failed: {rec_r}");
    }

    // transcribe with whisper-cli
    let model = &ctx.config.voice.whisper_model;
    if !std::path::Path::new(model).exists() {
        return "whisper model not found; transcription unavailable".into();
    }
    run_cmd(&["whisper-cli", "-m", model, "-f", wav, "-nt"], 60).await
}

pub async fn speak(input: &Value, ctx: &HandCtx) -> String {
    let text = input["text"].as_str().unwrap_or("").trim();
    if text.is_empty() { return "error: no text".into(); }

    let voice = &ctx.config.voice.piper_voice;
    if !std::path::Path::new(voice).exists() {
        return "piper voice not found; TTS unavailable".into();
    }

    let wav = "/tmp/brainos-tts.wav";
    // piper reads stdin, writes wav
    let cmd = format!("echo {:?} | piper --model {:?} --output_file {:?}", text, voice, wav);
    let r = run_cmd(&["bash", "-c", &cmd], 30).await;
    if r.contains("error") { return format!("piper error: {r}"); }

    // play via pw-play or aplay
    if std::path::Path::new("/usr/bin/pw-play").exists() {
        run_cmd(&["pw-play", wav], 30).await
    } else {
        run_cmd(&["aplay", wav], 30).await
    }
}
