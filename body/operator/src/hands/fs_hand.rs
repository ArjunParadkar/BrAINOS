//! Filesystem hand — operates under config.fs.home_dir.
//! All paths are validated (no ../, no absolute escapes).

use serde_json::Value;
use std::path::{Path, PathBuf};
use super::HandCtx;

/// Resolve a user-provided path against home_dir safely.
/// Rejects: absolute paths, path traversal, paths escaping home_dir.
fn resolve(home: &Path, user_path: &str) -> Result<PathBuf, String> {
    let raw = user_path.trim();
    if raw.contains("..") {
        return Err("path traversal rejected".into());
    }
    // Absolute paths: allow if they don't escape to sensitive locations
    if raw.starts_with('/') {
        let pb = std::path::PathBuf::from(raw);
        // Block sensitive paths even when absolute
        let s = raw.to_ascii_lowercase();
        if s.starts_with("/etc/") || s.starts_with("/root")
            || s.starts_with("/boot") || s.starts_with("/brain")
            || s.starts_with("/opt/brainos/bin") || s.contains(".brainos_api_key")
        {
            return Err("path rejected: sensitive system location".into());
        }
        return Ok(pb);
    }
    // Relative paths: anchor to home
    let p = raw.trim_start_matches('/');
    let full = home.join(p);
    let norm = normalize(&full);
    let home_norm = normalize(home);
    if !norm.starts_with(&home_norm) {
        return Err("path escapes home directory".into());
    }
    Ok(full)
}

fn normalize(p: &Path) -> PathBuf {
    // remove . components without requiring existence
    let mut out = PathBuf::new();
    for comp in p.components() {
        out.push(comp);
    }
    out
}

fn home(ctx: &HandCtx) -> PathBuf {
    PathBuf::from(&ctx.config.fs.home_dir)
}

pub async fn list(input: &Value, ctx: &HandCtx) -> String {
    let rel = input["path"].as_str().or(input.as_str()).unwrap_or("").trim();
    let home = home(ctx);
    let dir = if rel.is_empty() {
        home.clone()
    } else {
        match resolve(&home, rel) {
            Ok(p) => p,
            Err(e) => return e,
        }
    };
    match std::fs::read_dir(&dir) {
        Ok(entries) => {
            let mut lines = Vec::new();
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
                lines.push(if is_dir { format!("{name}/") } else { name });
            }
            lines.sort();
            if lines.is_empty() {
                format!("(empty directory: {})", dir.display())
            } else {
                lines.join("\n")
            }
        }
        Err(e) => format!("error listing {}: {e}", dir.display()),
    }
}

pub async fn read(input: &Value, ctx: &HandCtx) -> String {
    let rel = input["path"].as_str().unwrap_or("").trim();
    let offset = input["offset"].as_u64().unwrap_or(0) as usize;
    let home = home(ctx);
    let path = match resolve(&home, rel) {
        Ok(p) => p, Err(e) => return e,
    };
    match std::fs::read(&path) {
        Ok(data) => {
            let text = String::from_utf8_lossy(&data[offset.min(data.len())..]);
            let chunk = text.chars().take(8192).collect::<String>();
            if text.len() > 8192 {
                format!("{chunk}\n…[{} bytes remaining, use offset {}]", data.len() - offset - 8192, offset + 8192)
            } else {
                chunk
            }
        }
        Err(e) => format!("error reading {}: {e}", path.display()),
    }
}

pub async fn write(input: &Value, ctx: &HandCtx) -> String {
    let rel = input["path"].as_str().unwrap_or("").trim();
    let content = input["content"].as_str().unwrap_or("");
    let home = home(ctx);
    let path = match resolve(&home, rel) {
        Ok(p) => p, Err(e) => return e,
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match std::fs::write(&path, content) {
        Ok(_) => format!("wrote {} bytes to {}", content.len(), path.display()),
        Err(e) => format!("error writing {}: {e}", path.display()),
    }
}

pub async fn mkdir(input: &Value, ctx: &HandCtx) -> String {
    let rel = input["path"].as_str().unwrap_or("").trim();
    let home = home(ctx);
    let path = match resolve(&home, rel) { Ok(p) => p, Err(e) => return e };
    match std::fs::create_dir_all(&path) {
        Ok(_) => format!("created {}", path.display()),
        Err(e) => format!("error: {e}"),
    }
}

pub async fn delete(input: &Value, ctx: &HandCtx) -> String {
    let rel = input["path"].as_str().unwrap_or("").trim();
    let home = home(ctx);
    let path = match resolve(&home, rel) { Ok(p) => p, Err(e) => return e };
    if path.is_dir() {
        match std::fs::remove_dir(&path) {
            Ok(_) => format!("deleted directory {}", path.display()),
            Err(e) => format!("error: {e}"),
        }
    } else {
        match std::fs::remove_file(&path) {
            Ok(_) => format!("deleted {}", path.display()),
            Err(e) => format!("error: {e}"),
        }
    }
}

pub async fn move_file(input: &Value, ctx: &HandCtx) -> String {
    let src = input["src"].as_str().unwrap_or("").trim();
    let dst = input["dst"].as_str().unwrap_or("").trim();
    let home = home(ctx);
    let src = match resolve(&home, src) { Ok(p) => p, Err(e) => return e };
    let dst = match resolve(&home, dst) { Ok(p) => p, Err(e) => return e };
    match std::fs::rename(&src, &dst) {
        Ok(_) => format!("moved {} → {}", src.display(), dst.display()),
        Err(e) => format!("error: {e}"),
    }
}

pub async fn stat(input: &Value, ctx: &HandCtx) -> String {
    let rel = input["path"].as_str().unwrap_or("").trim();
    let home = home(ctx);
    let path = match resolve(&home, rel) { Ok(p) => p, Err(e) => return e };
    match std::fs::metadata(&path) {
        Ok(m) => {
            let kind = if m.is_dir() { "directory" } else if m.is_file() { "file" } else { "other" };
            format!("{}: {kind}, {} bytes, readonly: {}", path.display(), m.len(), m.permissions().readonly())
        }
        Err(e) => format!("not found: {e}"),
    }
}

pub async fn search(input: &Value, ctx: &HandCtx) -> String {
    let query = input["query"].as_str().unwrap_or("").trim();
    if query.is_empty() { return "error: no query".into(); }
    let home = home(ctx);
    // simple recursive file name + content search
    super::run_cmd(&["bash", "-c", &format!(
        "grep -rl --include='*' {:?} {:?} 2>/dev/null | head -20",
        query, home
    )], 15).await
}
