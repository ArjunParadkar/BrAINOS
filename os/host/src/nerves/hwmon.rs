//! BFS  LEVEL    Host - the only crate with an operating system (CLI, scenarios, OS nerves)  [crate brainos-host]
//! BFS  PURPOSE  The Linux hwmon and thermal-zone nerve: temperature chips and fans read from sysfs, and the fan full-speed switch as a gated verb
//! BFS  DEFINES  HwmonNerve
//! BFS  USES     this crate: read_f64 | brainos: abi, body, crypto, part | external: std
//! BFS  USED BY  host/tests/nerves_live
//!
//! Linux hwmon and thermal zones, over sysfs.
//!
//! ```text
//!   /sys/class/hwmon/hwmonN/          one chip
//!       name                          "k10temp", "nvme", "hp", ...
//!       tempK_input                   millidegrees Celsius
//!       tempK_max, tempK_crit         the chip's own limits, when it states them
//!       fanK_input                    rpm
//!       pwm1_enable                   fan mode, on chips that expose one
//!   /sys/class/thermal/thermal_zoneN/
//!       type, temp                    millidegrees Celsius
//!       trip_point_J_type, _temp      "critical", "hot", "passive", ...
//! ```
//!
//! Every chip and every zone is one part.
//!
//! **Limits come from the hardware or not at all.** A temperature is a
//! vital only when the chip states a limit for it (`tempK_crit`, with
//! `tempK_max` as the warning if present), or the zone has a `critical`
//! trip point. This nerve never supplies a threshold of its own: a
//! number invented here would be enforced by the gate as if the
//! hardware had said it.
//!
//! **Reading needs nothing. Writing needs root.** The one verb is
//! `fan.full(on)`: `pwm1_enable` 0 is "full speed" and 2 is "automatic"
//! on the chips that have it (documented for hp-wmi; other drivers
//! differ, see `docs/NERVES.md`). The nerve always *declares* it, and
//! on its own that gets the entity nothing: sysfs states no limit for
//! a fan, and a part with no limits gets no verbs. The verb becomes
//! usable when the part is acquired with an envelope pack
//! (`brainos_part::pack`) that states the fan's limits and is signed
//! by the entity's Key. The write is
//! attempted only under a `Mandate`, and a permission error comes back
//! as `NerveFault::Refused`.

use super::{read_f64, read_text, stable_key};
use brainos_abi::text::Text;
use brainos_abi::time::Instant;
use brainos_abi::verb::Verb;
use brainos_body::region::RegionClass;
use brainos_body::schema::{ParamSpec, VerbSpec};
use brainos_body::vital::Band;
use brainos_crypto::key::{BrainKey, Challenge, Signature};
use brainos_part::class::PartClass;
use brainos_part::homeostat::ReflexAct;
use brainos_part::nerve::{Found, Mandate, Nerve, NerveFault, Sensed};
use brainos_part::schema::{PartSchema, Quantity, SenseSpec, VitalKind, VitalSpec};
use std::path::{Path, PathBuf};

/// What one sysfs input file is.
#[derive(Clone, Debug)]
struct Input {
    sense: String,
    path: PathBuf,
    /// divide the file's integer by this
    scale: f64,
    quantity: Quantity,
    /// (warn, critical) in the sense's own unit, if the hardware says
    limit: Option<(f64, f64)>,
}

#[derive(Clone, Debug)]
struct Chip {
    label: String,
    stable_id: String,
    inputs: Vec<Input>,
    /// the fan-mode file, if this chip has one
    fan_mode: Option<PathBuf>,
    has_fans: bool,
}

/// The nerve.
pub struct HwmonNerve {
    root: PathBuf,
    chips: Vec<(Chip, BrainKey)>,
}

impl HwmonNerve {
    /// The real machine.
    pub fn live() -> HwmonNerve {
        HwmonNerve::at(Path::new("/sys"))
    }

    /// A sysfs tree rooted somewhere else, for tests.
    pub fn at(root: &Path) -> HwmonNerve {
        HwmonNerve {
            root: root.to_path_buf(),
            chips: Vec::new(),
        }
    }

    fn scan(&self) -> Vec<Chip> {
        let mut chips = Vec::new();

        let hw = self.root.join("class/hwmon");
        for dir in sorted_dirs(&hw) {
            let name = read_text(&dir.join("name")).unwrap_or_else(|| "unnamed".into());
            let device = std::fs::canonicalize(dir.join("device"))
                .ok()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();
            let mut inputs = Vec::new();
            let mut has_fans = false;
            for k in 1..=16 {
                let t = dir.join(format!("temp{k}_input"));
                if t.exists() {
                    let crit = read_f64(&dir.join(format!("temp{k}_crit"))).map(|v| v / 1000.0);
                    let max = read_f64(&dir.join(format!("temp{k}_max"))).map(|v| v / 1000.0);
                    // a limit is only a limit if it is one: some chips
                    // report 0 or absurd values for "not set"
                    let limit = crit.filter(|c| (20.0..=200.0).contains(c)).map(|c| {
                        let warn = max.filter(|m| *m > 0.0 && *m <= c).unwrap_or(c);
                        (warn, c)
                    });
                    inputs.push(Input {
                        sense: format!("temp{k}.c"),
                        path: t,
                        scale: 1000.0,
                        quantity: Quantity::Temperature,
                        limit,
                    });
                }
                let f = dir.join(format!("fan{k}_input"));
                if f.exists() {
                    has_fans = true;
                    inputs.push(Input {
                        sense: format!("fan{k}.rpm"),
                        path: f,
                        scale: 1.0,
                        quantity: Quantity::Rpm,
                        limit: None,
                    });
                }
            }
            if inputs.is_empty() {
                continue;
            }
            let mode = dir.join("pwm1_enable");
            chips.push(Chip {
                label: format!("hwmon {name}"),
                stable_id: format!("hwmon:{name}:{device}"),
                inputs,
                fan_mode: (has_fans && mode.exists()).then_some(mode),
                has_fans,
            });
        }

        let tz = self.root.join("class/thermal");
        for dir in sorted_dirs(&tz) {
            let base = dir.file_name().unwrap_or_default().to_string_lossy().to_string();
            if !base.starts_with("thermal_zone") {
                continue;
            }
            let temp = dir.join("temp");
            if !temp.exists() {
                continue;
            }
            let kind = read_text(&dir.join("type")).unwrap_or_else(|| "zone".into());
            let mut critical = None;
            let mut warn = None;
            for j in 0..12 {
                let ty = read_text(&dir.join(format!("trip_point_{j}_type")));
                let tv = read_f64(&dir.join(format!("trip_point_{j}_temp"))).map(|v| v / 1000.0);
                match (ty.as_deref(), tv) {
                    (Some("critical"), Some(v)) => critical = Some(v),
                    (Some("hot") | Some("passive"), Some(v)) => {
                        warn = Some(warn.map_or(v, |w: f64| w.min(v)))
                    }
                    _ => {}
                }
            }
            let limit = critical
                .filter(|c| (20.0..=200.0).contains(c))
                .map(|c| (warn.filter(|w| *w <= c).unwrap_or(c), c));
            chips.push(Chip {
                label: format!("thermal zone {kind}"),
                stable_id: format!("thermal:{base}:{kind}"),
                inputs: vec![Input {
                    sense: "temp.c".into(),
                    path: temp,
                    scale: 1000.0,
                    quantity: Quantity::Temperature,
                    limit,
                }],
                fan_mode: None,
                has_fans: false,
            });
        }
        chips
    }

    fn chip(&self, f: &Found) -> Option<&(Chip, BrainKey)> {
        self.chips.get(f.handle as usize)
    }
}

fn sorted_dirs(p: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(p)
        .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).collect())
        .unwrap_or_default();
    v.sort();
    v
}

impl Nerve for HwmonNerve {
    fn name(&self) -> &'static str {
        "hwmon"
    }

    fn enumerate(&mut self) -> Vec<Found> {
        self.chips = self
            .scan()
            .into_iter()
            .map(|c| {
                let k = stable_key("hwmon", &c.stable_id);
                (c, k)
            })
            .collect();
        self.chips
            .iter()
            .enumerate()
            .map(|(i, (c, k))| Found {
                handle: i as u32,
                key: k.public(),
                label: Text::from_str_truncating(&c.label),
            })
            .collect()
    }

    fn answer(&mut self, part: &Found, c: &Challenge) -> Option<Signature> {
        self.chip(part).map(|(_, k)| k.answer(c))
    }

    fn describe(&mut self, part: &Found) -> Option<PartSchema> {
        let (chip, _) = self.chip(part)?;
        let class = if chip.has_fans {
            PartClass::Thermal
        } else {
            PartClass::Sensor
        };
        let mut s = PartSchema::new(class, &chip.label, "hwmon");
        for i in &chip.inputs {
            let (lo, hi) = match i.quantity {
                Quantity::Temperature => (-60.0, 250.0),
                _ => (0.0, 60_000.0),
            };
            s = s.sense(SenseSpec::new(&i.sense, i.quantity, lo, hi));
            if let Some((warn, crit)) = i.limit {
                s = s.vital(VitalSpec::new(&i.sense, VitalKind::Thermal, Band::rising(warn, crit)));
            }
        }
        if chip.fan_mode.is_some() {
            s = s.verb(
                VerbSpec::new(
                    Verb::literal("fan.full"),
                    RegionClass::Effector,
                    "force the fans to full speed (1) or return them to automatic (0)",
                )
                .with(ParamSpec::count("on", 0, 1)),
            );
        }
        Some(s)
    }

    fn read(&mut self, part: &Found, _at: Instant, out: &mut Vec<Sensed>) {
        let Some((chip, _)) = self.chip(part) else {
            return;
        };
        for i in &chip.inputs {
            if let Some(raw) = read_f64(&i.path) {
                out.push(Sensed::new(&i.sense, raw / i.scale));
            }
        }
    }

    fn apply(&mut self, part: &Found, m: &Mandate) -> Result<(), NerveFault> {
        let (chip, _) = self.chip(part).ok_or(NerveFault::NoSuchPart)?;
        if m.verb().as_str() != "fan.full" {
            return Err(NerveFault::NotSupported);
        }
        let path = chip.fan_mode.as_ref().ok_or(NerveFault::NotSupported)?;
        let on = m.scalar("on").ok_or(NerveFault::NotSupported)? >= 1.0;
        std::fs::write(path, if on { "0" } else { "2" }).map_err(|_| NerveFault::Refused)
    }

    fn reflex(&mut self, part: &Found, act: &ReflexAct) -> Result<(), NerveFault> {
        let (chip, _) = self.chip(part).ok_or(NerveFault::NoSuchPart)?;
        let path = chip.fan_mode.as_ref().ok_or(NerveFault::NotSupported)?;
        if act.param() != "on" {
            return Err(NerveFault::NotSupported);
        }
        std::fs::write(path, if act.value() >= 0.5 { "0" } else { "2" })
            .map_err(|_| NerveFault::Refused)
    }

    /// Full cooling on every chip that has a mode switch. Without root
    /// this does nothing, and the firmware's own fan control — which
    /// was in charge all along — stays in charge.
    fn safe(&mut self) {
        for (chip, _) in &self.chips {
            if let Some(p) = &chip.fan_mode {
                let _ = std::fs::write(p, "0");
            }
        }
    }
}
