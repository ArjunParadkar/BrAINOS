//! Operator configuration — read from /etc/brainos/operator.toml or --config.
//! Every field has a sane dev-host default so the binary runs without any
//! config file during local testing.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    pub operator: OperatorCfg,
    pub llama: LlamaCfg,
    pub cdp: CdpCfg,
    pub anthropic: AnthropicCfg,
    pub voice: VoiceCfg,
    pub fs: FsCfg,
    pub task: TaskCfg,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct OperatorCfg {
    /// Unix socket path. Empty = $XDG_RUNTIME_DIR/brainos/operator.sock
    pub socket_path: String,
    /// p1 BRAIN dir where journal + key live. Fallback: ./workspace/brain
    pub brain_dir: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct LlamaCfg {
    pub url: String,
    pub timeout_ms: u64,
    /// "llama" | "ollama"
    pub backend: String,
    pub ollama_url: String,
    pub ollama_model: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct CdpCfg {
    pub url: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct AnthropicCfg {
    pub model: String,
    pub max_tokens: u32,
    pub effort: String,
    /// p3 raw device (BRAINOS-SECURE partition)
    pub key_device: String,
    /// local file fallback for the API key (plain text, one line)
    pub key_file: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct VoiceCfg {
    pub whisper_model: String,
    pub piper_voice: String,
    pub record_seconds: u32,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct FsCfg {
    /// Root of the human's filesystem that fs.* hands operate under.
    /// On the body: /home/human.  On the dev host: /home/god (fallback).
    pub home_dir: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct TaskCfg {
    pub scratchpad: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            operator: Default::default(),
            llama: Default::default(),
            cdp: Default::default(),
            anthropic: Default::default(),
            voice: Default::default(),
            fs: Default::default(),
            task: Default::default(),
        }
    }
}

impl Default for OperatorCfg {
    fn default() -> Self {
        // socket_path resolved at runtime from $XDG_RUNTIME_DIR
        // brain_dir tries /brain/BRAIN first, then workspace/brain
        OperatorCfg {
            socket_path: String::new(),
            brain_dir: String::new(),
        }
    }
}

impl Default for LlamaCfg {
    fn default() -> Self {
        LlamaCfg {
            url: "http://127.0.0.1:8080".into(),
            timeout_ms: 6000,
            backend: "llama".into(),
            ollama_url: "http://127.0.0.1:11434".into(),
            ollama_model: "qwen2.5:32b".into(),
        }
    }
}

impl Default for CdpCfg {
    fn default() -> Self {
        CdpCfg { url: "http://127.0.0.1:9222".into() }
    }
}

impl Default for AnthropicCfg {
    fn default() -> Self {
        AnthropicCfg {
            model: "claude-fable-5".into(),
            max_tokens: 16000,
            effort: "low".into(),
            key_device: "/dev/disk/by-partlabel/BRAINOS-SECURE".into(),
            key_file: String::new(),
        }
    }
}

impl Default for VoiceCfg {
    fn default() -> Self {
        VoiceCfg {
            whisper_model: "/opt/brainos/models/ggml-base.en.bin".into(),
            piper_voice: "/opt/brainos/voices/en_US-amy-medium.onnx".into(),
            record_seconds: 5,
        }
    }
}

impl Default for FsCfg {
    fn default() -> Self {
        // dev-host fallback: use /home/god if /home/human doesn't exist
        let home = if std::path::Path::new("/home/human").exists() {
            "/home/human".into()
        } else {
            std::env::var("HOME").unwrap_or_else(|_| "/home/god".into())
        };
        FsCfg { home_dir: home }
    }
}

impl Default for TaskCfg {
    fn default() -> Self {
        TaskCfg { scratchpad: "/tmp/brainos-task".into() }
    }
}

impl Config {
    pub fn load(path: Option<&str>) -> Result<Config> {
        let path = match path {
            Some(p) => PathBuf::from(p),
            None => PathBuf::from("/etc/brainos/operator.toml"),
        };
        if !path.exists() {
            tracing::info!("config not found at {}, using defaults", path.display());
            return Ok(Config::default());
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading config {}", path.display()))?;
        let cfg: Config = toml::from_str(&text)
            .with_context(|| format!("parsing config {}", path.display()))?;
        Ok(cfg)
    }

    /// Resolved socket path: config override → $XDG_RUNTIME_DIR/brainos/operator.sock
    pub fn socket_path(&self) -> PathBuf {
        if !self.operator.socket_path.is_empty() {
            return PathBuf::from(&self.operator.socket_path);
        }
        let rt = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/run/user/1000".into());
        PathBuf::from(rt).join("brainos").join("operator.sock")
    }

    /// Resolved brain dir: config → /brain/BRAIN → ./workspace/brain
    pub fn brain_dir(&self) -> PathBuf {
        if !self.operator.brain_dir.is_empty() {
            return PathBuf::from(&self.operator.brain_dir);
        }
        let prod = PathBuf::from("/brain/BRAIN");
        if prod.exists() {
            return prod;
        }
        // dev host: try workspace/brain relative to cwd
        let dev = PathBuf::from("./workspace/brain");
        if dev.exists() {
            return dev;
        }
        prod // return prod even if absent; journal_io will handle the miss
    }
}
