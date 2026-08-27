use serde_json::Value;
use super::{HandCtx, run_cmd};

async fn hypr(args: &[&str]) -> String {
    // try hyprctl first, fallback swaymsg
    let r = run_cmd(&[&["hyprctl"][..], args].concat(), 5).await;
    if r.contains("hyprctl: not found") || r.contains("No such file") {
        run_cmd(&[&["swaymsg"][..], args].concat(), 5).await
    } else {
        r
    }
}

pub async fn list(_ctx: &HandCtx) -> String {
    run_cmd(&["hyprctl", "-j", "clients"], 5).await
}

pub async fn focus(input: &Value, _ctx: &HandCtx) -> String {
    let name = input["name"].as_str().unwrap_or("").trim();
    if name.is_empty() { return "error: no window name".into(); }
    hypr(&["dispatch", "focuswindow", &format!("class:{name}")]).await
}

pub async fn move_win(input: &Value, _ctx: &HandCtx) -> String {
    let x = input["x"].as_i64().unwrap_or(0);
    let y = input["y"].as_i64().unwrap_or(0);
    hypr(&["dispatch", "moveactive", &format!("{x} {y}")]).await
}

pub async fn resize(input: &Value, _ctx: &HandCtx) -> String {
    let w = input["w"].as_i64().unwrap_or(100);
    let h = input["h"].as_i64().unwrap_or(100);
    hypr(&["dispatch", "resizeactive", &format!("{w} {h}")]).await
}

pub async fn workspace(input: &Value, _ctx: &HandCtx) -> String {
    let target = input["target"].as_str()
        .or(input.as_str())
        .unwrap_or("").trim();
    match target {
        "next" => hypr(&["dispatch", "workspace", "e+1"]).await,
        "prev" | "previous" => hypr(&["dispatch", "workspace", "e-1"]).await,
        n => hypr(&["dispatch", "workspace", n]).await,
    }
}

pub async fn close(_ctx: &HandCtx) -> String {
    hypr(&["dispatch", "killactive"]).await
}

pub async fn fullscreen(_ctx: &HandCtx) -> String {
    hypr(&["dispatch", "fullscreen", "0"]).await
}

pub async fn tile(input: &Value, _ctx: &HandCtx) -> String {
    let _layout = input["layout"].as_str().unwrap_or("split");
    hypr(&["dispatch", "layoutmsg", "togglesplit"]).await
}
