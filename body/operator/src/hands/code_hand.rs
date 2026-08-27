//! code.run hand — executes Python3/bash/node in a scratch directory.

use serde_json::Value;
use super::{HandCtx, run_cmd};
use std::path::PathBuf;

pub async fn run(input: &Value, ctx: &HandCtx) -> String {
    let lang = input["lang"].as_str().unwrap_or("bash").trim();
    let src = input["src"].as_str().unwrap_or("").trim();
    if src.is_empty() { return "error: no source".into(); }

    let scratch = PathBuf::from(&ctx.scratch_dir);
    let _ = std::fs::create_dir_all(&scratch);

    let ext = match lang { "python3" | "python" => "py", "node" | "js" => "js", _ => "sh" };
    let script = scratch.join(format!("run_{}.{}", std::process::id(), ext));
    if std::fs::write(&script, src).is_err() {
        return "error: could not write script".into();
    }

    let r = match lang {
        "python3" | "python" => run_cmd(&["python3", script.to_str().unwrap_or("")], 30).await,
        "node" | "js" => run_cmd(&["node", script.to_str().unwrap_or("")], 30).await,
        _ => run_cmd(&["bash", script.to_str().unwrap_or("")], 30).await,
    };

    let _ = std::fs::remove_file(&script);
    r
}
