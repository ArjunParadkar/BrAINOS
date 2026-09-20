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
const CORE_CRATES: [&str; 15] = [
    "abi",
    "crypto",
    "rt",
    "memory",
    "body",
    "kira",
    "cognition",
    "worlds",
    "entity",
    "transport",
    "adapters",
    "proof",
    "fleet",
    "export",
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
