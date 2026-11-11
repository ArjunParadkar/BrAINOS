//! BFS  LEVEL    Discipline - the no_std canary and its checks  [crate brainos-nostd-check]
//! BFS  PURPOSE  Source-level checks: no std or unsafe below host, the external dependency list, the UEFI build settings, and that BFS cards are current
//! BFS  DEFINES  -
//! BFS  USES     external: std
//! BFS  USED BY  (entry point or test)
//!
//! The `no_std` discipline, checked mechanically.
//!
//! The strongest check is a bare-metal build, and it is one
//! `rustup target add` away — `nostd/src/lib.rs` documents the command
//! and `tools/check-no-std.sh` runs it when a target is present. On a
//! machine with no bare target installed, that command cannot run, and
//! pretending otherwise would be exactly the failure the "no larp"
//! principle names.
//!
//! What *can* run anywhere is this: a source-level check that no crate
//! below `host` mentions `std` at all. It is weaker than a build — a
//! dependency could still pull `std` in — but it is mechanical, it runs
//! on every `cargo test`, and it catches the way the discipline actually
//! breaks in practice, which is somebody reaching for `HashMap` at two
//! in the morning.

use std::fs;
use std::path::{Path, PathBuf};

/// Crates that must compile with no operating system beneath them.
const CORE_CRATES: [&str; 18] = [
    "abi",
    "crypto",
    "rt",
    "memory",
    "body",
    "part",
    "bpu",
    "kira",
    "cognition",
    "worlds",
    "entity",
    "transport",
    "adapters",
    "proof",
    "fleet",
    "export",
    "blur",
    "nostd",
];

/// The one crate that is allowed an operating system.
const HOST_CRATE: &str = "host";

fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is os/nostd
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("nostd has a parent")
        .to_path_buf()
}

fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return out;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            if p.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            out.extend(rust_files(&p));
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
    out
}

/// Strip line and block comments, and string literals, so that the word
/// "std" in prose or in a doc comment does not fail the check. Every
/// module in this workspace has a long doc comment and several of them
/// say "no_std" in it.
fn code_only(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let b: Vec<char> = src.chars().collect();
    let mut i = 0;
    let mut depth = 0usize; // block comment nesting
    while i < b.len() {
        if depth > 0 {
            if b[i] == '*' && i + 1 < b.len() && b[i + 1] == '/' {
                depth -= 1;
                i += 2;
                continue;
            }
            if b[i] == '/' && i + 1 < b.len() && b[i + 1] == '*' {
                depth += 1;
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }
        if b[i] == '/' && i + 1 < b.len() && b[i + 1] == '/' {
            while i < b.len() && b[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if b[i] == '/' && i + 1 < b.len() && b[i + 1] == '*' {
            depth = 1;
            i += 2;
            continue;
        }
        if b[i] == '"' {
            // skip a string literal, honouring escapes
            i += 1;
            while i < b.len() {
                if b[i] == '\\' {
                    i += 2;
                    continue;
                }
                if b[i] == '"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            out.push(' ');
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

#[test]
fn no_core_crate_mentions_std_in_its_source() {
    let root = workspace_root();
    let mut offences: Vec<String> = Vec::new();

    for krate in CORE_CRATES {
        let src = root.join(krate).join("src");
        for file in rust_files(&src) {
            let text = fs::read_to_string(&file).expect("readable source");
            let code = code_only(&text);
            for (n, line) in code.lines().enumerate() {
                let trimmed = line.trim();
                // `no_std`, `cfg_attr(not(test), no_std)` and
                // `alloc::string::ToString` are all fine; what is not is
                // reaching into `std` itself.
                let bad = trimmed.contains("use std::")
                    || trimmed.contains("extern crate std")
                    || trimmed.contains(" std::")
                    || trimmed.starts_with("std::");
                if bad {
                    offences.push(format!(
                        "{}:{}: {}",
                        file.strip_prefix(&root).unwrap_or(&file).display(),
                        n + 1,
                        trimmed
                    ));
                }
            }
        }
    }

    assert!(
        offences.is_empty(),
        "these crates must compile with no operating system beneath them:\n  {}",
        offences.join("\n  ")
    );
}

#[test]
fn every_core_crate_declares_no_std() {
    let root = workspace_root();
    for krate in CORE_CRATES {
        let lib = root.join(krate).join("src").join("lib.rs");
        let text = fs::read_to_string(&lib)
            .unwrap_or_else(|_| panic!("{krate} has no src/lib.rs"));
        assert!(
            text.contains("#![cfg_attr(not(test), no_std)]") || text.contains("#![no_std]"),
            "{krate} does not declare no_std"
        );
    }
}

#[test]
fn the_canary_is_unconditionally_no_std() {
    // A canary that becomes `std` under cfg(test) is not a canary.
    let root = workspace_root();
    let lib = fs::read_to_string(root.join("nostd").join("src").join("lib.rs")).unwrap();
    assert!(lib.contains("#![no_std]"));
    assert!(
        !lib.contains("cfg_attr(not(test), no_std)"),
        "the canary must not relax under test"
    );
}

#[test]
fn every_core_crate_forbids_unsafe() {
    let root = workspace_root();
    for krate in CORE_CRATES {
        let lib = root.join(krate).join("src").join("lib.rs");
        let text = fs::read_to_string(&lib).unwrap();
        assert!(
            text.contains("#![forbid(unsafe_code)]"),
            "{krate} does not forbid unsafe"
        );
    }
}

#[test]
fn the_host_crate_is_the_only_one_with_an_operating_system() {
    let root = workspace_root();
    let lib = fs::read_to_string(root.join(HOST_CRATE).join("src").join("lib.rs")).unwrap();
    assert!(
        !lib.contains("#![no_std]"),
        "host is deliberately the exception"
    );
    // and it says so
    assert!(lib.contains("only crate that is allowed"));
}

#[test]
fn the_dependency_list_is_still_two_crates_long() {
    // "Near-zero dependencies" is a claim in the README. This is the
    // check. ed25519-dalek is the one place where don't-be-clever-with-
    // crypto beats build-it-ourselves; libm is declared for float math
    // on targets without it.
    let root = workspace_root();
    let manifest = fs::read_to_string(root.join("Cargo.toml")).unwrap();
    let deps_section = manifest
        .split("[workspace.dependencies]")
        .nth(1)
        .expect("a workspace dependency table");

    let external: Vec<&str> = deps_section
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.trim().starts_with('#'))
        .take_while(|l| !l.trim().starts_with('['))
        .filter(|l| !l.contains("path = "))
        .map(|l| l.split('=').next().unwrap_or("").trim())
        .filter(|n| !n.is_empty())
        .collect();

    assert_eq!(
        external,
        vec!["ed25519-dalek", "libm"],
        "an external dependency appeared in the core"
    );
}

/// The workspace table is not the only place a dependency can be named:
/// a crate can declare one in its own manifest and the test above would
/// never see it. This one reads every member's manifest.
///
/// Exactly one such line is allowed, and it is deliberate. `sha2` is
/// named in `crypto/Cargo.toml` under the UEFI-only target table, with
/// `force-soft`, because that is the only way to select its portable
/// backend on a target with SSE switched off (x86_64-unknown-uefi). It
/// is not a new crate in the tree — ed25519-dalek already depends on it
/// — and the line must stay target-scoped, so hosted builds are
/// untouched. `tools/read-export` names ed25519-dalek directly because
/// it is forbidden from using the workspace's crates at all.
#[test]
fn no_crate_names_an_external_dependency_on_the_side() {
    let root = workspace_root();
    let mut members: Vec<PathBuf> = fs::read_dir(&root)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path().join("Cargo.toml"))
        .filter(|p| p.exists())
        .collect();
    members.push(root.join("tools/read-export/Cargo.toml"));
    members.sort();

    let mut found: Vec<String> = Vec::new();
    for manifest in &members {
        let text = fs::read_to_string(manifest).unwrap();
        let krate = manifest
            .parent()
            .unwrap()
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .to_string();
        let mut table = String::new();
        for line in text.lines() {
            let l = line.trim();
            if l.starts_with('[') {
                table = l.to_string();
                continue;
            }
            if l.is_empty() || l.starts_with('#') || !table.contains("dependencies") {
                continue;
            }
            if l.contains("workspace = true") || l.contains("path = ") {
                continue;
            }
            let name = l.split('=').next().unwrap_or("").trim();
            found.push(format!("{krate} {table} {name}"));
        }
    }

    assert_eq!(
        found,
        vec![
            "crypto [target.'cfg(target_os = \"uefi\")'.dependencies] sha2".to_string(),
            "tools/read-export [dependencies] ed25519-dalek".to_string(),
        ],
        "a crate named an external dependency outside the workspace table"
    );

    let crypto = fs::read_to_string(root.join("crypto/Cargo.toml")).unwrap();
    let sha2_line = crypto
        .lines()
        .find(|l| l.trim_start().starts_with("sha2"))
        .expect("the sha2 line");
    assert!(
        sha2_line.contains("force-soft") && sha2_line.contains("default-features = false"),
        "sha2 is named for one purpose only: its software backend on UEFI"
    );
}

/// The other half of the x86_64 UEFI build: curve25519-dalek must use
/// its serial backend there. If this file goes missing the debug build
/// for that target dies inside LLVM again, with an error that names
/// nothing of ours.
#[test]
fn the_x86_uefi_target_pins_the_serial_curve_backend() {
    let cfg = fs::read_to_string(workspace_root().join(".cargo/config.toml"))
        .expect("os/.cargo/config.toml");
    let code: String = cfg
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(code.contains("[target.x86_64-unknown-uefi]"));
    assert!(code.contains("curve25519_dalek_backend=\"serial\""));
}

/// The Memory-Integrity Law, for nerves, at the source level.
///
/// The types already stop a nerve writing memory: every mutating
/// method on the state graph needs a `MemoryHandle`, and nothing in
/// the `Nerve` trait hands one over (`part/src/nerve.rs` has the
/// compile-fail examples). What the types cannot stop is code that
/// *issues itself* a handle, because `MemoryHandle::issue` is public —
/// the entity lives in a different crate from the handle.
///
/// So this closes it by reading the source: no file that implements a
/// nerve, a part policy or a substrate may name the state graph or
/// issue a handle. The part and BPU crates must not depend on
/// `brainos-memory` at all outside their tests.
#[test]
fn no_nerve_part_or_substrate_can_reach_the_state_graph() {
    let root = workspace_root();
    let mut files = Vec::new();
    for dir in ["part/src", "bpu/src", "host/src/nerves"] {
        let d = root.join(dir);
        if d.exists() {
            files.extend(rust_files(&d));
        }
    }
    assert!(files.len() >= 10, "the nerve sources were not found: {}", files.len());
    for f in files {
        let code = code_only(&fs::read_to_string(&f).unwrap());
        for forbidden in ["MemoryHandle", "StateGraph", "brainos_memory"] {
            assert!(
                !code.contains(forbidden),
                "{} names `{forbidden}`: a nerve must not hold or write memory",
                f.display()
            );
        }
    }
    for krate in ["part", "bpu"] {
        let manifest = root.join(krate).join("Cargo.toml");
        if !manifest.exists() {
            continue;
        }
        let text = fs::read_to_string(manifest).unwrap();
        let deps = text
            .split("[dependencies]")
            .nth(1)
            .unwrap_or("")
            .split("\n[")
            .next()
            .unwrap_or("");
        assert!(
            !deps.contains("brainos-memory"),
            "{krate} depends on brainos-memory outside its tests"
        );
    }
}

/// Every Rust file opens with a BFS card (level, purpose, defines,
/// uses, used by) and `docs/CODE_MAP.md` is generated from them. The
/// computed lines are only worth having if they are true, so this runs
/// the generator in check mode: a file whose imports or public items
/// changed without the card being regenerated fails here.
///
/// Fix: `tools/bfs_cards.py` from the repository root.
#[test]
fn every_bfs_card_and_the_code_map_are_current() {
    let repo = workspace_root().parent().expect("os/ has a parent").to_path_buf();
    let tool = repo.join("tools/bfs_cards.py");
    assert!(tool.exists(), "tools/bfs_cards.py is missing");
    let out = match std::process::Command::new("python3")
        .arg(&tool)
        .arg("--check")
        .output()
    {
        Ok(o) => o,
        // no python on this machine: the check cannot run, and saying
        // so is better than pretending it passed
        Err(e) => panic!("python3 is needed to check the BFS cards: {e}"),
    };
    assert!(
        out.status.success(),
        "BFS cards are stale; run tools/bfs_cards.py\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn the_canary_touches_every_crate() {
    // A canary that only touches half the workspace checks half the
    // workspace.
    let root = workspace_root();
    let lib = fs::read_to_string(root.join("nostd").join("src").join("lib.rs")).unwrap();
    for krate in CORE_CRATES {
        if krate == "nostd" {
            continue;
        }
        let name = format!("brainos_{}", krate.replace('-', "_"));
        assert!(
            lib.contains(&name),
            "the canary never mentions {name}, so it proves nothing about it"
        );
    }
}
