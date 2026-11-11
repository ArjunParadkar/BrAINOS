//! BFS  LEVEL    Operator (Stage 3-4 milestones) - the seat daemon on the Linux body  [crate brainos-operator]
//! BFS  PURPOSE  System verbs: volume, brightness, wifi, bluetooth, battery, power, lock, notifications, clipboard, time and date, via host command-line tools
//! BFS  DEFINES  volume, brightness, wifi, bluetooth, battery, power, lock, notify, clipboard, time_now, date_now
//! BFS  USES     this crate: HandCtx | external: chrono, serde-json, tokio
//! BFS  USED BY  -
//!
use serde_json::Value;
use super::{HandCtx, run_cmd};
use chrono::Local;

pub async fn volume(input: &Value, _ctx: &HandCtx) -> String {
    let action = input["action"].as_str()
        .or(input.as_str())
        .unwrap_or("").trim();
    match action {
        "up" => run_cmd(&["wpctl", "set-volume", "@DEFAULT_AUDIO_SINK@", "5%+"], 5).await,
        "down" => run_cmd(&["wpctl", "set-volume", "@DEFAULT_AUDIO_SINK@", "5%-"], 5).await,
        "mute" => run_cmd(&["wpctl", "set-mute", "@DEFAULT_AUDIO_SINK@", "toggle"], 5).await,
        pct if pct.chars().all(|c| c.is_ascii_digit()) => {
            run_cmd(&["wpctl", "set-volume", "@DEFAULT_AUDIO_SINK@", &format!("{pct}%")], 5).await
        }
        _ => format!("unknown volume action: {action}"),
    }
}

pub async fn brightness(input: &Value, _ctx: &HandCtx) -> String {
    let action = input["action"].as_str()
        .or(input.as_str())
        .unwrap_or("").trim();
    match action {
        "up" => run_cmd(&["brightnessctl", "set", "5%+"], 5).await,
        "down" => run_cmd(&["brightnessctl", "set", "5%-"], 5).await,
        pct if pct.chars().all(|c| c.is_ascii_digit()) => {
            run_cmd(&["brightnessctl", "set", &format!("{pct}%")], 5).await
        }
        _ => format!("unknown brightness action: {action}"),
    }
}

pub async fn wifi(input: &Value, _ctx: &HandCtx) -> String {
    let action = input["action"].as_str()
        .or(input.as_str())
        .unwrap_or("status").trim();
    match action {
        "on" => run_cmd(&["nmcli", "radio", "wifi", "on"], 10).await,
        "off" => run_cmd(&["nmcli", "radio", "wifi", "off"], 10).await,
        _ => run_cmd(&["nmcli", "radio", "wifi"], 5).await,
    }
}

pub async fn bluetooth(input: &Value, _ctx: &HandCtx) -> String {
    let action = input["action"].as_str()
        .or(input.as_str())
        .unwrap_or("status").trim();
    match action {
        "on" => run_cmd(&["bluetoothctl", "power", "on"], 10).await,
        "off" => run_cmd(&["bluetoothctl", "power", "off"], 10).await,
        _ => run_cmd(&["bluetoothctl", "show"], 5).await,
    }
}

pub async fn battery(_ctx: &HandCtx) -> String {
    run_cmd(&["upower", "-i",
        "/org/freedesktop/UPower/devices/battery_BAT0"], 5).await
}

pub async fn power(input: &Value, _ctx: &HandCtx) -> String {
    let action = input["action"].as_str()
        .or(input.as_str())
        .unwrap_or("").trim();
    match action {
        "sleep" => run_cmd(&["systemctl", "suspend"], 10).await,
        "shutdown" => run_cmd(&["systemctl", "poweroff"], 10).await,
        "reboot" => run_cmd(&["systemctl", "reboot"], 10).await,
        _ => format!("unknown power action: {action}"),
    }
}

pub async fn lock(_ctx: &HandCtx) -> String {
    // try hyprlock first, then swaylock
    if std::path::Path::new("/usr/bin/hyprlock").exists() {
        run_cmd(&["hyprlock", "--immediate"], 5).await
    } else {
        run_cmd(&["swaylock", "-f"], 5).await
    }
}

pub async fn notify(input: &Value, _ctx: &HandCtx) -> String {
    let title = input["title"].as_str().unwrap_or("BrAInOS").trim();
    let body = input["body"].as_str().unwrap_or("").trim();
    run_cmd(&["notify-send", title, body], 5).await
}

pub async fn clipboard(input: &Value, _ctx: &HandCtx) -> String {
    let text = input["text"].as_str().unwrap_or("").trim();
    if text.is_empty() {
        // read clipboard
        run_cmd(&["wl-paste"], 5).await
    } else {
        // write clipboard
        use tokio::process::Command;
        use tokio::io::AsyncWriteExt;
        let mut child = match Command::new("wl-copy").stdin(std::process::Stdio::piped()).spawn() {
            Ok(c) => c,
            Err(e) => return format!("wl-copy error: {e}"),
        };
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes()).await;
        }
        let _ = child.wait().await;
        "copied to clipboard".into()
    }
}

pub async fn time_now(_ctx: &HandCtx) -> String {
    Local::now().format("%-I:%M %p").to_string()
}

pub async fn date_now(_ctx: &HandCtx) -> String {
    Local::now().format("%A, %B %-d, %Y").to_string()
}
