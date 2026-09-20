//! The freedom principle, as a test.
//!
//! `fixture.brainex` is a real export, produced by `brainos export` and
//! committed. It is here for two reasons.
//!
//! The first is independence: this suite reads it with a program that
//! depends on no BrAInOS crate, so the claim "the customer owns their
//! entity permanently" has something behind it other than a sentence in
//! a document.
//!
//! The second is that a committed fixture pins the format. If somebody
//! changes the encoding, this breaks — which is the correct outcome for
//! a format that customers' files are written in. Changing it should
//! require deciding to, and bumping `FORMAT_VERSION`.
//!
//! To regenerate it deliberately, which is the only way it should ever
//! happen:
//!
//! ```sh
//! cargo run --release -p brainos-host -- export tools/read-export/tests/fixture.brainex
//! ```

use std::path::PathBuf;
use std::process::Command;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixture.brainex")
}

fn reader() -> Command {
    Command::new(env!("CARGO_BIN_EXE_read-export"))
}

fn read(path: &std::path::Path) -> (bool, String) {
    let out = reader().arg(path).output().expect("reader ran");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).to_string()
            + &String::from_utf8_lossy(&out.stderr),
    )
}

#[test]
fn an_export_is_readable_without_any_brainos_code() {
    let (ok, text) = read(&fixture());
    assert!(ok, "the reader rejected a valid export:\n{text}");
    assert!(text.contains("verifies under the entity's own key"));
    assert!(text.contains("matches the sections present"));
    // it did not merely parse the header: it walked the memories
    assert!(text.contains("50 memories"));
    assert!(text.contains("episode"));
}

/// The dependency list *is* the claim, so assert on it.
///
/// If reading an entity export ever required our code, the customer
/// would not own their entity, they would rent it — and the first sign
/// of that happening would be a `brainos-*` line appearing here.
#[test]
fn the_reader_depends_on_nothing_of_ours() {
    let manifest = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"),
    )
    .unwrap();
    let deps = manifest
        .split("[dependencies]")
        .nth(1)
        .expect("a dependencies section");
    assert!(
        !deps.contains("brainos"),
        "the standalone reader has grown a BrAInOS dependency:\n{deps}"
    );
    assert!(
        !deps.contains("miniblur"),
        "the standalone reader has grown a body dependency:\n{deps}"
    );
    // one line, and it is the signature scheme
    let lines: Vec<&str> = deps
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with('['))
        .collect();
    assert_eq!(lines.len(), 1, "expected exactly one dependency: {lines:?}");
    assert!(lines[0].starts_with("ed25519-dalek"));
}

/// Being readable by anyone is not the same as being forgeable by
/// anyone. Every single-byte change to the file must be caught, by the
/// Merkle root or by the signature.
#[test]
fn one_flipped_byte_anywhere_is_caught() {
    let raw = std::fs::read(fixture()).unwrap();
    let dir = std::env::temp_dir().join("brainex-tamper");
    std::fs::create_dir_all(&dir).unwrap();

    // Sample across the whole file rather than all 12 kB of it: the
    // header, the manifest, every section, the root and the signature.
    let mut checked = 0;
    for i in (0..raw.len()).step_by(37) {
        let mut bad = raw.clone();
        bad[i] ^= 0x01;
        if bad == raw {
            continue;
        }
        let path = dir.join(format!("tampered-{i}.brainex"));
        std::fs::write(&path, &bad).unwrap();
        let (ok, text) = read(&path);
        assert!(
            !ok,
            "a flipped bit at byte {i} was accepted as a valid export:\n{text}"
        );
        std::fs::remove_file(&path).ok();
        checked += 1;
    }
    assert!(checked > 100, "only {checked} positions were checked");
}

/// A truncated file is a common real failure — half a download, a full
/// disk — and must read as broken rather than as a smaller entity.
#[test]
fn a_truncated_export_is_not_a_smaller_entity() {
    let raw = std::fs::read(fixture()).unwrap();
    let dir = std::env::temp_dir().join("brainex-truncate");
    std::fs::create_dir_all(&dir).unwrap();
    for cut in [1usize, 9, 64, 512, 4096, raw.len() - 1] {
        let path = dir.join(format!("short-{cut}.brainex"));
        std::fs::write(&path, &raw[..cut]).unwrap();
        let (ok, text) = read(&path);
        assert!(!ok, "a {cut}-byte prefix was accepted:\n{text}");
        std::fs::remove_file(&path).ok();
    }
}

/// Appending to an export must not pass either. A reader that stops at
/// the last byte it understands would let anything be smuggled after
/// the signature.
#[test]
fn trailing_bytes_are_refused() {
    let mut raw = std::fs::read(fixture()).unwrap();
    raw.push(0);
    let path = std::env::temp_dir().join("brainex-trailing.brainex");
    std::fs::write(&path, &raw).unwrap();
    let (ok, text) = read(&path);
    assert!(!ok, "a byte appended after the signature was accepted:\n{text}");
    std::fs::remove_file(&path).ok();
}
