//! BFS  LEVEL    Host - the only crate with an operating system (CLI, scenarios, OS nerves)  [crate brainos-host]
//! BFS  PURPOSE  The nerve for the machine the entity runs on: its CPU, each disk and its battery, read from /proc and /sys
//! BFS  DEFINES  MachineNerve
//! BFS  USES     this crate: read_f64 | brainos: abi, body, cognition, crypto, part | external: std
//! BFS  USED BY  host/tests/nerves_live
//!
//! The machine the entity is running on, as parts.
//!
//! ```text
//!   CPU      /proc/cpuinfo, /proc/stat, cpufreq        Compute / Cpu
//!            senses: cores, load, freq.hz
//!   disk     /sys/block/<dev>/{size,stat,device/*}     Storage
//!            senses: size.bytes, written.bytes, temp.c (if the drive has a hwmon chip)
//!   battery  /sys/class/power_supply/<bat>/*           Power
//!            senses: charge, cycles, capacity (full now / full when new)
//! ```
//!
//! None of these has a verb. They are felt, and the CPU is a placement
//! target because it is a Compute part.
//!
//! **What is not readable, said plainly:**
//! * *Disk wear.* NVMe "percentage used" and SATA SMART attributes need
//!   an admin ioctl, which needs root. Only what the kernel publishes
//!   without privilege is read: capacity, bytes written since boot, and
//!   the drive's temperature and its own temperature limits. A drive's
//!   temperature *is* a vital here, because the drive states its limit.
//! * *CPU temperature limit.* `k10temp` and `coretemp` report a
//!   temperature; on this machine neither states a critical value, so
//!   the CPU's temperature is not offered as a vital by this nerve. It
//!   is still read, as a sense of the corresponding hwmon part.
//! * *Battery thresholds.* The kernel's `alarm` level, when present,
//!   is used as the critical charge. No threshold is invented.

use super::{read_f64, read_text, stable_key};
use brainos_abi::text::Text;
use brainos_abi::time::Instant;
use brainos_body::vital::Band;
use brainos_cognition::placement::Substrate;
use brainos_crypto::key::{BrainKey, Challenge, Signature};
use brainos_part::class::PartClass;
use brainos_part::nerve::{Found, Mandate, Nerve, NerveFault, Sensed};
use brainos_part::schema::{PartSchema, Quantity, SenseSpec, VitalKind, VitalSpec};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
enum Kind {
    Cpu { model: String, cores: u32 },
    Disk { dev: String, dir: PathBuf, temp: Option<(PathBuf, Option<(f64, f64)>)> },
    Battery { dir: PathBuf, alarm_fraction: Option<f64> },
}

struct Part {
    kind: Kind,
    label: String,
    key: BrainKey,
}

pub struct MachineNerve {
    root_sys: PathBuf,
    root_proc: PathBuf,
    parts: Vec<Part>,
    /// the previous (busy, total) jiffies, to turn /proc/stat's
    /// counters into a load. The last sample, not a history.
    last_cpu: Option<(u64, u64)>,
}

impl MachineNerve {
    pub fn live() -> MachineNerve {
        MachineNerve::at(Path::new("/sys"), Path::new("/proc"))
    }

    pub fn at(sys: &Path, proc_: &Path) -> MachineNerve {
        MachineNerve {
            root_sys: sys.to_path_buf(),
            root_proc: proc_.to_path_buf(),
            parts: Vec::new(),
            last_cpu: None,
        }
    }

    fn cpu_times(&self) -> Option<(u64, u64)> {
        let stat = std::fs::read_to_string(self.root_proc.join("stat")).ok()?;
        let line = stat.lines().find(|l| l.starts_with("cpu "))?;
        let f: Vec<u64> = line
            .split_whitespace()
            .skip(1)
            .filter_map(|x| x.parse().ok())
            .collect();
        if f.len() < 5 {
            return None;
        }
        let total: u64 = f.iter().take(8).sum();
        // idle + iowait
        let idle = f[3] + f[4];
        Some((total - idle, total))
    }

    fn scan(&self) -> Vec<(Kind, String, String)> {
        let mut out = Vec::new();

        // --- the CPU ---
        if let Ok(info) = std::fs::read_to_string(self.root_proc.join("cpuinfo")) {
            let cores = info.lines().filter(|l| l.starts_with("processor")).count() as u32;
            let model = info
                .lines()
                .find(|l| l.starts_with("model name"))
                .and_then(|l| l.split(':').nth(1))
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|| "unknown cpu".into());
            if cores > 0 {
                let machine = read_text(Path::new("/etc/machine-id")).unwrap_or_default();
                out.push((
                    Kind::Cpu { model: model.clone(), cores },
                    format!("cpu {model}"),
                    format!("cpu:{model}:{cores}:{machine}"),
                ));
            }
        }

        // --- disks: real block devices only ---
        let block = self.root_sys.join("block");
        let mut devs: Vec<PathBuf> = std::fs::read_dir(&block)
            .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).collect())
            .unwrap_or_default();
        devs.sort();
        for dir in devs {
            let dev = dir.file_name().unwrap_or_default().to_string_lossy().to_string();
            if !dir.join("device").exists() {
                continue; // zram, loop, dm: not hardware
            }
            let model = read_text(&dir.join("device/model")).unwrap_or_else(|| dev.clone());
            let serial = read_text(&dir.join("device/serial")).unwrap_or_default();
            // a drive's own temperature chip sits under its device node
            let mut temp = None;
            for base in [dir.join("device"), dir.join("device/device")] {
                for h in std::fs::read_dir(&base)
                    .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).collect::<Vec<_>>())
                    .unwrap_or_default()
                {
                    let n = h.file_name().unwrap_or_default().to_string_lossy().to_string();
                    if !n.starts_with("hwmon") {
                        continue;
                    }
                    let input = h.join("temp1_input");
                    if input.exists() {
                        let crit = read_f64(&h.join("temp1_crit")).map(|v| v / 1000.0);
                        let max = read_f64(&h.join("temp1_max")).map(|v| v / 1000.0);
                        let limit = crit
                            .filter(|c| (20.0..=200.0).contains(c))
                            .map(|c| (max.filter(|m| *m > 0.0 && *m <= c).unwrap_or(c), c));
                        temp = Some((input, limit));
                    }
                }
            }
            out.push((
                Kind::Disk { dev: dev.clone(), dir, temp },
                format!("disk {model}"),
                format!("disk:{model}:{serial}"),
            ));
        }

        // --- batteries ---
        let ps = self.root_sys.join("class/power_supply");
        let mut sup: Vec<PathBuf> = std::fs::read_dir(&ps)
            .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).collect())
            .unwrap_or_default();
        sup.sort();
        for dir in sup {
            if read_text(&dir.join("type")).as_deref() != Some("Battery") {
                continue;
            }
            let name = dir.file_name().unwrap_or_default().to_string_lossy().to_string();
            let serial = read_text(&dir.join("serial_number")).unwrap_or_default();
            let model = read_text(&dir.join("model_name")).unwrap_or_default();
            let full = read_f64(&dir.join("energy_full")).or_else(|| read_f64(&dir.join("charge_full")));
            let alarm = read_f64(&dir.join("alarm"));
            let alarm_fraction = match (alarm, full) {
                (Some(a), Some(f)) if a > 0.0 && f > 0.0 && a < f => Some(a / f),
                _ => None,
            };
            out.push((
                Kind::Battery { dir, alarm_fraction },
                format!("battery {name}"),
                format!("battery:{name}:{model}:{serial}"),
            ));
        }
        out
    }
}

impl Nerve for MachineNerve {
    fn name(&self) -> &'static str {
        "machine"
    }

    fn enumerate(&mut self) -> Vec<Found> {
        self.parts = self
            .scan()
            .into_iter()
            .map(|(kind, label, id)| Part { kind, label, key: stable_key("machine", &id) })
            .collect();
        self.parts
            .iter()
            .enumerate()
            .map(|(i, p)| Found {
                handle: i as u32,
                key: p.key.public(),
                label: Text::from_str_truncating(&p.label),
            })
            .collect()
    }

    fn answer(&mut self, part: &Found, c: &Challenge) -> Option<Signature> {
        self.parts.get(part.handle as usize).map(|p| p.key.answer(c))
    }

    fn describe(&mut self, part: &Found) -> Option<PartSchema> {
        let p = self.parts.get(part.handle as usize)?;
        Some(match &p.kind {
            Kind::Cpu { .. } => PartSchema::new(PartClass::Compute, &p.label, "machine")
                .made_of(Substrate::Cpu)
                .sense(SenseSpec::new("cores", Quantity::Count, 1.0, 4096.0))
                .sense(SenseSpec::new("load", Quantity::Fraction, 0.0, 1.0))
                .sense(SenseSpec::new("freq.hz", Quantity::Frequency, 0.0, 1e11)),
            Kind::Disk { temp, .. } => {
                let mut s = PartSchema::new(PartClass::Storage, &p.label, "machine")
                    .sense(SenseSpec::new("size.bytes", Quantity::Bytes, 0.0, 1e18))
                    .sense(SenseSpec::new("written.bytes", Quantity::Bytes, 0.0, 1e20));
                if let Some((_, limit)) = temp {
                    s = s.sense(SenseSpec::new("temp.c", Quantity::Temperature, -60.0, 250.0));
                    if let Some((warn, crit)) = limit {
                        s = s.vital(VitalSpec::new(
                            "temp.c",
                            VitalKind::Thermal,
                            Band::rising(*warn, *crit),
                        ));
                    }
                }
                s
            }
            Kind::Battery { alarm_fraction, .. } => {
                let mut s = PartSchema::new(PartClass::Power, &p.label, "machine")
                    .sense(SenseSpec::new("charge", Quantity::Fraction, 0.0, 1.0))
                    .sense(SenseSpec::new("cycles", Quantity::Count, 0.0, 1e6))
                    .sense(SenseSpec::new("capacity", Quantity::Fraction, 0.0, 1.5));
                if let Some(a) = alarm_fraction {
                    s = s.vital(VitalSpec::new("charge", VitalKind::Charge, Band::falling(*a, *a)));
                }
                s
            }
        })
    }

    fn read(&mut self, part: &Found, _at: Instant, out: &mut Vec<Sensed>) {
        let Some(p) = self.parts.get(part.handle as usize) else {
            return;
        };
        match &p.kind {
            Kind::Cpu { cores, .. } => {
                out.push(Sensed::new("cores", *cores as f64));
                if let Some((busy, total)) = self.cpu_times() {
                    if let Some((b0, t0)) = self.last_cpu {
                        if total > t0 {
                            let load = (busy.saturating_sub(b0)) as f64 / (total - t0) as f64;
                            out.push(Sensed::new("load", load.clamp(0.0, 1.0)));
                        }
                    }
                    self.last_cpu = Some((busy, total));
                }
                let f = self
                    .root_sys
                    .join("devices/system/cpu/cpu0/cpufreq/scaling_cur_freq");
                if let Some(khz) = read_f64(&f) {
                    out.push(Sensed::new("freq.hz", khz * 1000.0));
                }
            }
            Kind::Disk { dir, temp, .. } => {
                if let Some(sectors) = read_f64(&dir.join("size")) {
                    out.push(Sensed::new("size.bytes", sectors * 512.0));
                }
                // field 7 of the block stat file: sectors written
                if let Some(stat) = read_text(&dir.join("stat")) {
                    if let Some(w) = stat.split_whitespace().nth(6).and_then(|x| x.parse::<f64>().ok()) {
                        out.push(Sensed::new("written.bytes", w * 512.0));
                    }
                }
                if let Some((input, _)) = temp {
                    if let Some(m) = read_f64(input) {
                        out.push(Sensed::new("temp.c", m / 1000.0));
                    }
                }
            }
            Kind::Battery { dir, .. } => {
                if let Some(c) = read_f64(&dir.join("capacity")) {
                    out.push(Sensed::new("charge", c / 100.0));
                }
                if let Some(n) = read_f64(&dir.join("cycle_count")) {
                    out.push(Sensed::new("cycles", n));
                }
                let full = read_f64(&dir.join("energy_full")).or_else(|| read_f64(&dir.join("charge_full")));
                let design = read_f64(&dir.join("energy_full_design"))
                    .or_else(|| read_f64(&dir.join("charge_full_design")));
                if let (Some(f), Some(d)) = (full, design) {
                    if d > 0.0 {
                        out.push(Sensed::new("capacity", f / d));
                    }
                }
            }
        }
    }

    fn apply(&mut self, _part: &Found, _m: &Mandate) -> Result<(), NerveFault> {
        Err(NerveFault::NotSupported)
    }

    fn safe(&mut self) {}
}

impl MachineNerve {
    /// The disk device names found, for the survey.
    pub fn disks(&self) -> Vec<String> {
        self.parts
            .iter()
            .filter_map(|p| match &p.kind {
                Kind::Disk { dev, .. } => Some(dev.clone()),
                _ => None,
            })
            .collect()
    }

    pub fn cpu_model(&self) -> Option<String> {
        self.parts.iter().find_map(|p| match &p.kind {
            Kind::Cpu { model, .. } => Some(model.clone()),
            _ => None,
        })
    }
}
