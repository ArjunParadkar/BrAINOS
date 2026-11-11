//! BFS  LEVEL    Operator (Stage 3-4 milestones) - the seat daemon on the Linux body  [crate brainos-operator]
//! BFS  PURPOSE  Routes each verb to its handler (or to a remote body), defines the shared context handlers receive, and provides run_cmd with a timeout
//! BFS  DEFINES  app, code_hand, entity_hand, fs_hand, input_hand, notes, screen, shell, sys_hand, task_hand, ui_hand, voice_hand, +6 more
//! BFS  USES     this crate: config, kira_gate, pool, reach, skills, tasks, voice | external: serde-json, std, tokio
//! BFS  USED BY  operator/tasks
//!
//! Hand registry and dispatch.
//!
//! Every verb routes to a typed handler. The handler receives the raw
//! input JSON from the deliberate tier (or a synthetic JSON from the
//! reflex/fast tiers) and returns a result string that goes back to the
//! model as a tool_result.

pub mod app;
pub mod code_hand;
pub mod entity_hand;
pub mod fs_hand;
pub mod input_hand;
pub mod notes;
pub mod screen;
pub mod shell;
pub mod sys_hand;
pub mod task_hand;
pub mod ui_hand;
pub mod voice_hand;
pub mod web;
pub mod window;

use crate::config::Config;
use crate::kira_gate::SharedInstance;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::broadcast;

/// Context passed to every hand.
#[derive(Clone)]
pub struct HandCtx {
    pub config: Arc<Config>,
    pub instance: SharedInstance,
    pub ipc_tx: broadcast::Sender<String>,
    /// notes storage (brain_dir path)
    pub brain_dir: std::path::PathBuf,
    /// scratch dir for task/code
    pub scratch_dir: std::path::PathBuf,
    /// ears + mouth (shared with the push-to-talk loop so they never fight over the mic)
    pub voice: Arc<crate::voice::Voice>,
    /// Stage 4: the other bodies, the skill store, standing tasks, the model pool
    pub reach: Arc<crate::reach::ReachHub>,
    pub skills: Arc<std::sync::Mutex<crate::skills::SkillStore>>,
    pub tasks: Arc<crate::tasks::Tasks>,
    pub pool: Arc<crate::pool::Pool>,
    /// verbs the seat itself provides (for proprioception)
    pub local_verbs: Vec<String>,
}

/// Which body an input addresses. Missing/"seat"/"local"/"here" = the seat.
pub fn body_of(input: &Value) -> String {
    match input.get("body").and_then(|b| b.as_str()).map(str::trim) {
        None | Some("") | Some("seat") | Some("local") | Some("here") => crate::reach::SEAT.to_string(),
        Some(b) => b.to_string(),
    }
}

/// Dispatch a verb+input to the appropriate hand.
/// Returns a result string (sent back as tool_result).
pub async fn dispatch(verb: &str, input: &Value, ctx: &HandCtx) -> String {
    // Stage 4: a verb aimed at another body travels over Reach. The input is
    // forwarded without the routing field; the digest comes back bounded.
    let body = body_of(input);
    if body != crate::reach::SEAT {
        let mut fwd = input.clone();
        if let Some(m) = fwd.as_object_mut() { m.remove("body"); }
        let (digest, is_err) = ctx.reach.call(&body, verb, &fwd).await;
        return if is_err { format!("error on {body}: {digest}") } else { digest };
    }
    match verb {
        // shell
        "shell.run" => shell::run(input, ctx).await,
        // app
        "app.open" => app::open(input, ctx).await,
        "app.list" => app::list(ctx).await,
        "app.close" => app::close(input, ctx).await,
        // window
        "window.list" => window::list(ctx).await,
        "window.focus" => window::focus(input, ctx).await,
        "window.move" => window::move_win(input, ctx).await,
        "window.resize" => window::resize(input, ctx).await,
        "window.workspace" => window::workspace(input, ctx).await,
        "window.close" => window::close(ctx).await,
        "window.fullscreen" => window::fullscreen(ctx).await,
        "window.tile" => window::tile(input, ctx).await,
        // input
        "input.type" => input_hand::type_text(input, ctx).await,
        "input.key" => input_hand::press_key(input, ctx).await,
        "input.click" => input_hand::click(input, ctx).await,
        "input.scroll" => input_hand::scroll(input, ctx).await,
        // screen
        "screen.look" => screen::look(input, ctx).await,
        // web (CDP)
        "web.open" => web::open(input, ctx).await,
        "web.read" => web::read(ctx).await,
        "web.find" => web::find(input, ctx).await,
        "web.click" => web::click(input, ctx).await,
        "web.type" => web::type_into(input, ctx).await,
        "web.search" => web::search(input, ctx).await,
        // fs
        "fs.list" => fs_hand::list(input, ctx).await,
        "fs.read" => fs_hand::read(input, ctx).await,
        "fs.write" => fs_hand::write(input, ctx).await,
        "fs.mkdir" => fs_hand::mkdir(input, ctx).await,
        "fs.delete" => fs_hand::delete(input, ctx).await,
        "fs.move" => fs_hand::move_file(input, ctx).await,
        "fs.stat" => fs_hand::stat(input, ctx).await,
        "fs.search" => fs_hand::search(input, ctx).await,
        // sys
        "sys.volume" => sys_hand::volume(input, ctx).await,
        "sys.brightness" => sys_hand::brightness(input, ctx).await,
        "sys.wifi" => sys_hand::wifi(input, ctx).await,
        "sys.bluetooth" => sys_hand::bluetooth(input, ctx).await,
        "sys.battery" => sys_hand::battery(ctx).await,
        "sys.power" => sys_hand::power(input, ctx).await,
        "sys.lock" => sys_hand::lock(ctx).await,
        "sys.notify" => sys_hand::notify(input, ctx).await,
        "sys.clipboard" => sys_hand::clipboard(input, ctx).await,
        "sys.time" => sys_hand::time_now(ctx).await,
        "sys.date" => sys_hand::date_now(ctx).await,
        // ui (emits IPC)
        "ui.build" => ui_hand::build(input, ctx).await,
        "ui.update" => ui_hand::update(input, ctx).await,
        "ui.close" => ui_hand::close_surface(input, ctx).await,
        "ui.teach" => ui_hand::teach(input, ctx).await,
        "ui.point" => ui_hand::point(input, ctx).await,
        "ui.clear" => ui_hand::clear(ctx).await,
        "ui.say" => ui_hand::say(input, ctx).await,
        // notes
        "notes.write" => notes::write(input, ctx).await,
        "notes.read" => notes::read(input, ctx).await,
        // task
        "task.plan" => task_hand::plan(input, ctx).await,
        "task.status" => task_hand::status(ctx).await,
        "task.cancel" => task_hand::cancel(ctx).await,
        "task.every" => entity_hand::task_every(input, ctx).await,
        "task.when" => entity_hand::task_when(input, ctx).await,
        "task.list" => entity_hand::task_list(ctx).await,
        "task.stop" => entity_hand::task_stop(input, ctx).await,
        // entity (Stage 4)
        "body.list" => entity_hand::body_list(ctx).await,
        "body.forget" => entity_hand::body_forget(input, ctx).await,
        "skill.list" => entity_hand::skill_list(ctx).await,
        "skill.forget" => entity_hand::skill_forget(input, ctx).await,
        "pool.status" => entity_hand::pool_status(ctx).await,
        // code
        "code.run" => code_hand::run(input, ctx).await,
        // voice
        "voice.listen" => voice_hand::listen(input, ctx).await,
        "voice.speak" => voice_hand::speak(input, ctx).await,
        _ => format!("unknown verb: {verb}"),
    }
}

/// Run a shell command with timeout, return (stdout+stderr, truncated to 4096 chars).
pub async fn run_cmd(args: &[&str], timeout_s: u64) -> String {
    use tokio::process::Command;
    use tokio::time::timeout;
    let dur = std::time::Duration::from_secs(timeout_s);
    match timeout(dur, Command::new(args[0]).args(&args[1..]).output()).await {
        Ok(Ok(out)) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            let combined = format!("{stdout}{stderr}");
            // bounded: first 4096 chars
            if combined.len() > 4096 {
                format!("{}…[truncated]", &combined[..4096])
            } else {
                combined
            }
        }
        Ok(Err(e)) => format!("exec error: {e}"),
        Err(_) => format!("timeout after {timeout_s}s"),
    }
}
