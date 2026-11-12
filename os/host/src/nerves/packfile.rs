//! BFS  LEVEL    Host - the only crate with an operating system (CLI, scenarios, OS nerves)  [crate brainos-host]
//! BFS  PURPOSE  Reads an envelope pack from a small text file, so a person can state a part's limits and have the entity's Key sign them
//! BFS  DEFINES  parse_pack
//! BFS  USES     brainos: body, part
//! BFS  USED BY  host/tests/nerves_live
//!
//! An envelope pack, as a file a person can write.
//!
//! ```text
//!   # limits for the fans of an HP Omen 16
//!   label      hwmon hp
//!   source     human                      # or: vendor
//!   reference  measured at full speed, 2026-11-12
//!   limit      fan1.rpm  faults  rising   6000 7000
//!   limit      fan2.rpm  faults  rising   6000 7000
//! ```
//!
//! `limit <sense> <kind> <rising|falling> <warn> <critical>`, where
//! kind is one of thermal, wear, faults, charge, budget, load, and
//! `rising` means the value is bad when it goes up.
//!
//! The file is **unsigned text**. Parsing it produces an unsigned
//! pack, which nothing will accept. It becomes usable when a Key signs
//! it: `brainos parts --pack FILE` signs it with the session's Key, on
//! the reasoning that the person who typed the command is the owner
//! and is vouching for the numbers. A vendor's pack would arrive
//! already signed and need the owner's approval instead; a wire format
//! for signed packs is not built.
//!
//! A line that does not parse is an error, never a skipped line. A
//! typo in a limit must not quietly become no limit.

use brainos_body::vital::Band;
use brainos_part::pack::{EnvelopePack, Source};
use brainos_part::schema::VitalKind;

pub fn parse_pack(text: &str) -> Result<EnvelopePack, String> {
    let (mut label, mut source, mut reference) = (None, Source::Human, String::new());
    let mut limits = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let bad = |why: &str| format!("line {}: {why}: `{line}`", i + 1);
        let (key, rest) = line.split_once(char::is_whitespace).ok_or_else(|| bad("nothing after the keyword"))?;
        let rest = rest.trim();
        match key {
            "label" => label = Some(rest.to_string()),
            "reference" => reference = rest.to_string(),
            "source" => {
                source = match rest {
                    "human" => Source::Human,
                    "vendor" => Source::Vendor,
                    _ => return Err(bad("source must be `human` or `vendor`")),
                }
            }
            "limit" => {
                let f: Vec<&str> = rest.split_whitespace().collect();
                if f.len() != 5 {
                    return Err(bad("expected `limit sense kind rising|falling warn critical`"));
                }
                let kind = match f[1] {
                    "thermal" => VitalKind::Thermal,
                    "wear" => VitalKind::Wear,
                    "faults" => VitalKind::Faults,
                    "charge" => VitalKind::Charge,
                    "budget" => VitalKind::Budget,
                    "load" => VitalKind::Load,
                    _ => return Err(bad("unknown kind")),
                };
                let warn: f64 = f[3].parse().map_err(|_| bad("warn is not a number"))?;
                let crit: f64 = f[4].parse().map_err(|_| bad("critical is not a number"))?;
                let band = match f[2] {
                    "rising" => Band::rising(warn, crit),
                    "falling" => Band::falling(warn, crit),
                    _ => return Err(bad("direction must be `rising` or `falling`")),
                };
                if !band.is_sane() {
                    return Err(bad("warn must come before critical in the bad direction"));
                }
                limits.push((f[0].to_string(), kind, band));
            }
            _ => return Err(bad("unknown keyword")),
        }
    }
    let label = label.ok_or("no `label` line: which part is this for?")?;
    if limits.is_empty() {
        return Err("no `limit` lines".into());
    }
    let mut pack = EnvelopePack::for_label(&label, source, &reference);
    for (sense, kind, band) in limits {
        pack = pack.limit(&sense, kind, band);
    }
    Ok(pack)
}
