use base64::Engine;
use serde_json::Value;
use super::{HandCtx, run_cmd};

/// Take a screenshot with grim, return path or base64 for vision.
pub async fn look(input: &Value, ctx: &HandCtx) -> String {
    let question = input["question"].as_str().unwrap_or("").to_string();
    let is_screenshot = input.get("_screenshot_only").and_then(|v| v.as_bool()).unwrap_or(false);

    // grim to a temp file
    let path = format!("/tmp/brainos-screen-{}.png",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0));

    let r = run_cmd(&["grim", &path], 10).await;
    if !r.is_empty() && r.contains("error") {
        return format!("screenshot failed: {r}");
    }

    if is_screenshot {
        return format!("screenshot saved to {path}");
    }

    // read the image for base64 (the deliberate tier will use vision)
    // For now, return the path and a short description via hyprctl clients
    let clients = run_cmd(&["hyprctl", "-j", "clients"], 3).await;
    let desc = if clients.len() > 10 {
        format!("screenshot at {path}; visible windows: {}", clients.chars().take(200).collect::<String>())
    } else {
        format!("screenshot saved to {path}")
    };

    if question.is_empty() { desc } else { format!("{desc} (asked: {question})") }
}
