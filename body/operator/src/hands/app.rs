//! BFS  LEVEL    Operator (Stage 3-4 milestones) - the seat daemon on the Linux body  [crate brainos-operator]
//! BFS  PURPOSE  App verbs: open a desktop app by name or role (terminal, browser...) without waiting on it, list installed apps, and close one
//! BFS  DEFINES  open, list, close
//! BFS  USES     this crate: HandCtx | external: serde-json, std
//! BFS  USED BY  -
//!
use serde_json::Value;
use super::{HandCtx, run_cmd};

pub async fn open(input: &Value, ctx: &HandCtx) -> String {
    let name = input["name"].as_str()
        .or(input.as_str())
        .unwrap_or("").trim().to_string();
    if name.is_empty() { return "error: no app name".into(); }
    let _ = ctx;

    // Resolve a .desktop entry first (by id or case-insensitive Name=), then a
    // bare binary. The app is spawned DETACHED and never awaited: opening an
    // app must cost the operator milliseconds, not the app's lifetime.
    let norm = |x: &str| x.to_ascii_lowercase().chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>();
    // generic requests ("the terminal", "a browser") name a role, not an app;
    // the role resolves through GenericName= or, failing that, the body's own default
    let (name, role_default) = match norm(&name).as_str() {
        "terminal" | "shell" | "console" => ("terminal".to_string(), Some("foot")),
        "browser" | "web" | "internet" => ("browser".to_string(), Some("chromium")),
        "files" | "filemanager" | "explorer" => ("file manager".to_string(), None),
        "editor" | "texteditor" => ("editor".to_string(), None),
        _ => (name, None),
    };
    let lower = name.to_ascii_lowercase();
    let want = norm(&name);
    let want_nognome = want.trim_start_matches("gnome").to_string();
    let mut cmd: Option<Vec<String>> = None;
    for dir in ["/usr/share/applications", "/usr/local/share/applications",
                &format!("{}/.local/share/applications", std::env::var("HOME").unwrap_or_default())] {
        let Ok(rd) = std::fs::read_dir(dir) else { continue };
        for e in rd.flatten() {
            let path = e.path();
            let fname = e.file_name().to_string_lossy().to_ascii_lowercase();
            if !fname.ends_with(".desktop") { continue; }
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            let stem = fname.trim_end_matches(".desktop");
            let id_norm = norm(stem);
            let id_match = fname == format!("{lower}.desktop") || fname.ends_with(&format!(".{lower}.desktop"))
                || id_norm == want || id_norm.ends_with(&want) || id_norm.ends_with(&want_nognome);
            let name_match = text.lines().any(|l| l.strip_prefix("Name=").map(|n| {
                let nn = norm(n);
                nn == want || nn == want_nognome || (want.len() >= 4 && nn.contains(&want))
            }).unwrap_or(false));
            // role match: GenericName=Terminal / Web Browser / File Manager …
            let name_match = name_match || (role_default.is_some() || want.len() >= 4)
                && text.lines().any(|l| l.strip_prefix("GenericName=").map(|g| norm(g).contains(&want)).unwrap_or(false));
            if id_match || name_match {
                if let Some(exec) = text.lines().find_map(|l| l.strip_prefix("Exec=")) {
                    let argv: Vec<String> = exec.split_whitespace()
                        .filter(|a| !a.starts_with('%'))
                        .map(|a| a.to_string()).collect();
                    if !argv.is_empty() { cmd = Some(argv); }
                }
                if cmd.is_some() { break; }
            }
        }
        if cmd.is_some() { break; }
    }
    let argv = cmd.unwrap_or_else(|| vec![role_default.map(str::to_string).unwrap_or_else(|| name.clone())]);
    let mut c = std::process::Command::new(&argv[0]);
    c.args(&argv[1..])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        c.process_group(0);
    }
    match c.spawn() {
        Ok(mut child) => {
            // give it a moment to fail fast (missing binary, bad args)
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            match child.try_wait() {
                Ok(Some(status)) if !status.success() => format!("error: {name} exited immediately ({status})"),
                _ => { std::mem::forget(child); format!("opened {name}") }
            }
        }
        Err(e) => format!("error: cannot launch {name}: {e}"),
    }
}

pub async fn list(_ctx: &HandCtx) -> String {
    // list .desktop files
    let out = run_cmd(&["bash", "-c",
        "find /usr/share/applications ~/.local/share/applications -name '*.desktop' \
         -exec grep -l '^Name=' {} ';' 2>/dev/null | head -50"], 10).await;
    if out.trim().is_empty() { "no applications found".into() } else { out }
}

pub async fn close(input: &Value, _ctx: &HandCtx) -> String {
    let name = input["name"].as_str().unwrap_or("").trim();
    if name.is_empty() { return "error: no app name".into(); }
    // try hyprctl dispatch closewindow
    let r = run_cmd(&["hyprctl", "dispatch", "closewindow", &format!("class:{name}")], 5).await;
    if r.trim().is_empty() || r.contains("ok") {
        format!("closed {name}")
    } else {
        // fallback: pkill
        run_cmd(&["pkill", "-f", name], 5).await
    }
}
