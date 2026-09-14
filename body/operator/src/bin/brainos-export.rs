//! `brainos-export` — entity export v1.0 (Execution Plan §11). The freedom
//! principle as a command: the owner takes their entity's complete state off
//! the key, and can put it on another.
//!
//!   brainos-export export --brain /brain/BRAIN --out ~/blur.brainos-export
//!   brainos-export verify --from ~/blur.brainos-export
//!   brainos-export import --from ~/blur.brainos-export --brain /brain/BRAIN [--adopt] [--force]

use brainos_operator_lib::export;
use std::path::PathBuf;

fn usage() -> ! {
    eprintln!("usage:\n  brainos-export export --brain DIR --out DIR\n  brainos-export verify --from DIR\n  brainos-export import --from DIR --brain DIR [--adopt] [--force]");
    std::process::exit(2)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut brain: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut from: Option<PathBuf> = None;
    let mut adopt = false;
    let mut force = false;
    let cmd = args.first().cloned().unwrap_or_default();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--brain" => { i += 1; brain = args.get(i).map(PathBuf::from); }
            "--out" => { i += 1; out = args.get(i).map(PathBuf::from); }
            "--from" => { i += 1; from = args.get(i).map(PathBuf::from); }
            "--adopt" => adopt = true,
            "--force" => force = true,
            _ => usage(),
        }
        i += 1;
    }
    let r = match cmd.as_str() {
        "export" => match (brain, out) {
            (Some(b), Some(o)) => export::export(&b, &o).map(|m| {
                format!(
                    "exported {}.. → {}\n  {} episodes, {} semantic, {} beliefs, {} skills, {} bytes of notes, calibrated bodies: {:?}\n  the private key was NOT exported; it is the owner's",
                    &m.pub_key[..8], o.display(), m.counts.episodes, m.counts.semantic, m.counts.beliefs, m.counts.skills, m.counts.notes_bytes, m.counts.bodies_calibrated
                )
            }),
            _ => usage(),
        },
        "verify" => match from {
            Some(f) => export::verify(&f).map(|m| format!("ok: entity {}.. exported {}, {} files hash-checked", &m.pub_key[..8], m.exported_at, m.sha256.len())),
            None => usage(),
        },
        "import" => match (from, brain) {
            (Some(f), Some(b)) => export::import(&f, &b, adopt, force).map(|r| {
                format!(
                    "imported into {}: {} episodes, {} semantic, {} skills{}{}",
                    b.display(), r.counts.episodes, r.counts.semantic, r.counts.skills,
                    if r.skills_resigned { " (re-signed under this key: adopted)" } else { "" },
                    if r.skills_rejected > 0 { format!(", {} skill record(s) rejected", r.skills_rejected) } else { String::new() }
                )
            }),
            _ => usage(),
        },
        _ => usage(),
    };
    match r {
        Ok(msg) => println!("{msg}"),
        Err(e) => {
            eprintln!("brainos-export: {e:#}");
            std::process::exit(1);
        }
    }
}
