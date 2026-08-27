use serde_json::Value;
use super::{HandCtx, run_cmd};

pub async fn type_text(input: &Value, _ctx: &HandCtx) -> String {
    let text = input["text"].as_str()
        .or(input.as_str())
        .unwrap_or("").to_string();
    if text.is_empty() { return "error: no text".into(); }
    // wtype for wayland
    run_cmd(&["wtype", &text], 10).await
}

pub async fn press_key(input: &Value, _ctx: &HandCtx) -> String {
    let combo = input["combo"].as_str()
        .or(input["key"].as_str())
        .or(input.as_str())
        .unwrap_or("").to_string();
    if combo.is_empty() { return "error: no key".into(); }
    // wtype -k for key names, or ydotool key
    if std::path::Path::new("/usr/bin/wtype").exists() {
        run_cmd(&["wtype", "-k", &combo], 5).await
    } else {
        run_cmd(&["ydotool", "key", &combo], 5).await
    }
}

pub async fn click(input: &Value, _ctx: &HandCtx) -> String {
    let x = input["x"].as_i64().unwrap_or(0);
    let y = input["y"].as_i64().unwrap_or(0);
    run_cmd(&["ydotool", "click", "--next-delay", "100",
        "-x", &x.to_string(), "-y", &y.to_string()], 5).await
}

pub async fn scroll(input: &Value, _ctx: &HandCtx) -> String {
    let dx = input["dx"].as_i64().unwrap_or(0);
    let dy = input["dy"].as_i64().unwrap_or(0);
    run_cmd(&["ydotool", "mousemove", "--wheel",
        &dx.to_string(), &dy.to_string()], 5).await
}
