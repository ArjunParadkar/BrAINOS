//! Probe the real body and build the BodyMap.
//!
//! Only limbs whose backing binary (or subsystem) actually exists on the
//! current host are registered. KIRA's authz stage then refuses verbs
//! whose region was never incorporated — the entity cannot claim a limb
//! it doesn't have.

use brainos_mind::body::{BodyMap, Region, RegionClass};

/// Which binaries back each hand group.
struct Group {
    id: &'static str,
    class: RegionClass,
    verbs: &'static [&'static str],
    prop: &'static str,
    /// at least one of these binaries must exist for the group to be registered
    requires: &'static [&'static str],
}

const GROUPS: &[Group] = &[
    Group {
        id: "body/shell",
        class: RegionClass::Compute,
        verbs: &["shell.run", "code.run"],
        prop: "executes shell commands and scripts in a bounded sandbox",
        requires: &["/bin/bash", "/usr/bin/bash"],
    },
    Group {
        id: "body/apps",
        class: RegionClass::HostTool,
        verbs: &["app.open", "app.list", "app.close"],
        prop: "launches and manages desktop applications via gtk-launch",
        requires: &["/usr/share/applications", "/usr/bin/gtk-launch"],
    },
    Group {
        id: "body/wm",
        class: RegionClass::HostTool,
        verbs: &[
            "window.list", "window.focus", "window.move", "window.resize",
            "window.workspace", "window.close", "window.fullscreen", "window.tile",
        ],
        prop: "controls windows via hyprctl or swaymsg",
        requires: &["/usr/bin/hyprctl", "/usr/bin/swaymsg"],
    },
    Group {
        id: "body/input",
        class: RegionClass::InputKeys,
        verbs: &["input.type", "input.key", "input.click", "input.scroll"],
        prop: "synthesises keyboard and pointer input",
        requires: &["/usr/bin/wtype", "/usr/bin/ydotool"],
    },
    Group {
        id: "body/screen",
        class: RegionClass::Screen,
        verbs: &["screen.look"],
        prop: "captures a screenshot with grim and reads it",
        requires: &["/usr/bin/grim"],
    },
    Group {
        id: "body/web",
        class: RegionClass::Network,
        verbs: &[
            "web.open", "web.read", "web.find", "web.click",
            "web.type", "web.search",
        ],
        prop: "controls Chromium via Chrome DevTools Protocol",
        // CDP is a runtime check (HTTP to 9222), but chromium binary presence is a good proxy
        requires: &["/usr/bin/chromium", "/usr/bin/chromium-browser", "/usr/bin/google-chrome"],
    },
    Group {
        id: "body/fs",
        class: RegionClass::HostTool,
        verbs: &[
            "fs.list", "fs.read", "fs.write", "fs.mkdir",
            "fs.delete", "fs.move", "fs.stat", "fs.search",
        ],
        prop: "reads and writes files under the human home directory",
        requires: &["/bin/sh"],  // always available
    },
    Group {
        id: "body/sys",
        class: RegionClass::HostTool,
        verbs: &[
            "sys.volume", "sys.brightness", "sys.wifi", "sys.bluetooth",
            "sys.battery", "sys.power", "sys.lock", "sys.notify", "sys.clipboard", "sys.time", "sys.date",
        ],
        prop: "system controls: audio, display, network, power",
        requires: &["/usr/bin/wpctl", "/usr/bin/brightnessctl"],
    },
    Group {
        id: "body/ui",
        class: RegionClass::Screen,
        verbs: &["ui.build", "ui.update", "ui.close", "ui.teach", "ui.point", "ui.clear", "ui.say"],
        prop: "builds and manages face surfaces and highlight overlays",
        requires: &["/bin/sh"],  // ui hands just emit IPC, always available
    },
    Group {
        id: "body/notes",
        class: RegionClass::KeyMedium,
        verbs: &["notes.write", "notes.read"],
        prop: "reads and writes the private notebook on the brain journal",
        requires: &["/bin/sh"],  // journal is always accessible
    },
    Group {
        id: "body/task",
        class: RegionClass::HostTool,
        verbs: &["task.plan", "task.status", "task.cancel", "task.every", "task.when", "task.list", "task.stop"],
        prop: "plans multi-step tasks and keeps standing orders (watchers, schedules)",
        requires: &["/bin/sh"],
    },
    Group {
        id: "body/entity",
        class: RegionClass::KeyMedium,
        verbs: &["body.list", "body.forget", "skill.list", "skill.forget", "pool.status"],
        prop: "proprioception of the whole entity: its bodies, its learned skills, its brains",
        requires: &["/bin/sh"],
    },
    Group {
        id: "body/voice",
        class: RegionClass::Microphone,
        verbs: &["voice.listen", "voice.speak"],
        prop: "records speech via pw-record/arecord, synthesises via piper",
        requires: &["/usr/bin/pw-record", "/usr/bin/arecord"],
    },
];

/// Build the BodyMap by probing which binaries exist.
pub fn build(brain_dir: &std::path::Path) -> (BodyMap, Vec<String>) {
    let _ = brain_dir; // future: probe brain dir for sub-features
    let mut body = BodyMap::new();
    let mut registered = Vec::new();

    for g in GROUPS {
        if !any_exists(g.requires) {
            tracing::debug!("[body] skip {} — no backing binary", g.id);
            continue;
        }
        let caps: Vec<String> = g.verbs.iter().map(|v| v.to_string()).collect();
        let region = Region {
            id: g.id.to_string(),
            class: g.class,
            capabilities: caps.clone(),
            proprioception: g.prop.to_string(),
        };
        body.incorporate(region);
        for v in &caps {
            registered.push(v.clone());
        }
        tracing::info!("[body] incorporated {} ({} verbs)", g.id, g.verbs.len());
    }

    (body, registered)
}

fn any_exists(paths: &[&str]) -> bool {
    paths.iter().any(|p| std::path::Path::new(p).exists())
}

/// Dynamically check if a specific verb is available on this host.
pub fn verb_available(body: &BodyMap, verb: &str) -> bool {
    body.find_capability(verb).is_some()
}
