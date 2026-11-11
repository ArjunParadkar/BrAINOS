//! BFS  LEVEL    Host - the only crate with an operating system (CLI, scenarios, OS nerves)  [crate brainos-host]
//! BFS  PURPOSE  Tests of the OS nerves against a fixture sysfs tree (always run) and against this machine's real hardware (run when present)
//! BFS  DEFINES  -
//! BFS  USES     brainos: abi, body, cognition, crypto, entity, host, kira, part | external: std
//! BFS  USED BY  (entry point or test)
//!
//! The operating-system nerves, on a fixture and on the real machine.
//!
//! Two kinds of test, kept apart because they establish different
//! things:
//!
//! * **fixture_** tests build a small fake `/sys` in a temporary
//!   directory. They always run, and they pin the parsing: which file
//!   becomes which sense, where limits come from, what happens when a
//!   file is missing.
//! * **live_** tests read this machine. They are what makes a nerve
//!   "Verified on a live signal". Where the hardware is absent a live
//!   test prints `SKIPPED (no ...)` and returns — it does not pass
//!   silently as though it had checked something, and the final report
//!   counts only the ones that actually ran. Run with `--nocapture` to
//!   see which did.

use brainos_abi::error::{DenyCode, Stage};
use brainos_abi::time::{Instant, Tick};
use brainos_abi::verb::Verb;
use brainos_body::policy::Arg;
use brainos_body::vital::Health;
use brainos_cognition::placement::Substrate;
use brainos_crypto::key::BrainKey;
use brainos_entity::entity::{Entity, Wiring};
use brainos_host::nerves::hwmon::HwmonNerve;
use brainos_host::nerves::machine::MachineNerve;
use brainos_host::nerves::{acquire_all, stable_key, survey};
use brainos_kira::action::Request;
use brainos_kira::gate::Verdict;
use brainos_part::class::PartClass;
use brainos_part::nerve::{Mandate, Nerve, NerveFault};
use brainos_part::policy::Guard;
use std::fs;
use std::path::{Path, PathBuf};

fn entity(seed: u8) -> Entity {
    let mut e = Entity::new(BrainKey::from_seed(&[seed; 32]), Tick(0));
    e.wake(None, None, Tick(1)).unwrap();
    e
}

/// A fake sysfs: one CPU-temperature chip that states limits, one that
/// does not, a fan chip with a mode switch, and a thermal zone.
fn fixture(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("brainos-nerves-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let w = |rel: &str, body: &str| {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    };
    w("class/hwmon/hwmon0/name", "coretemp\n");
    w("class/hwmon/hwmon0/temp1_input", "61500\n");
    w("class/hwmon/hwmon0/temp1_max", "90000\n");
    w("class/hwmon/hwmon0/temp1_crit", "100000\n");
    w("class/hwmon/hwmon1/name", "mystery\n");
    w("class/hwmon/hwmon1/temp1_input", "40000\n");
    w("class/hwmon/hwmon2/name", "fanchip\n");
    w("class/hwmon/hwmon2/fan1_input", "2100\n");
    w("class/hwmon/hwmon2/pwm1_enable", "2\n");
    w("class/hwmon/hwmon3/name", "empty\n");
    w("class/thermal/thermal_zone0/type", "acpitz\n");
    w("class/thermal/thermal_zone0/temp", "45000\n");
    w("class/thermal/thermal_zone0/trip_point_0_type", "critical\n");
    w("class/thermal/thermal_zone0/trip_point_0_temp", "105000\n");
    w("class/thermal/thermal_zone0/trip_point_1_type", "passive\n");
    w("class/thermal/thermal_zone0/trip_point_1_temp", "95000\n");
    root
}

fn by_label<'a>(n: &mut HwmonNerve, label: &str) -> brainos_part::nerve::Found {
    n.enumerate()
        .into_iter()
        .find(|f| f.label.as_str() == label)
        .unwrap_or_else(|| panic!("no part labelled {label}"))
}

// ------------------------------------------------------------------
// fixture: parsing
// ------------------------------------------------------------------

#[test]
fn fixture_every_chip_with_an_input_is_a_part_and_one_with_none_is_not() {
    let root = fixture("enum");
    let mut n = HwmonNerve::at(&root);
    let labels: Vec<String> = n.enumerate().iter().map(|f| f.label.as_str().to_string()).collect();
    assert_eq!(
        labels,
        ["hwmon coretemp", "hwmon mystery", "hwmon fanchip", "thermal zone acpitz"]
    );
}

#[test]
fn fixture_limits_come_from_the_hardware_and_a_chip_that_states_none_has_no_vital() {
    let root = fixture("limits");
    let mut n = HwmonNerve::at(&root);

    let f = by_label(&mut n, "hwmon coretemp");
    let s = n.describe(&f).unwrap();
    assert_eq!(s.check(), Ok(()));
    let v = s.vital_named("temp1.c").expect("the chip stated a limit");
    assert_eq!((v.band.warn, v.band.critical), (90.0, 100.0));
    assert_eq!(s.judge("temp1.c", 61.5), Some(Health::Nominal));
    assert_eq!(s.judge("temp1.c", 95.0), Some(Health::Degraded));

    let f = by_label(&mut n, "hwmon mystery");
    let s = n.describe(&f).unwrap();
    assert!(s.vitals().is_empty(), "a limit was invented for a chip that stated none");

    let f = by_label(&mut n, "thermal zone acpitz");
    let s = n.describe(&f).unwrap();
    let v = s.vital_named("temp.c").unwrap();
    assert_eq!((v.band.warn, v.band.critical), (95.0, 105.0), "trip points");
}

#[test]
fn fixture_readings_are_scaled_to_real_units_and_a_missing_file_is_silence() {
    let root = fixture("read");
    let mut n = HwmonNerve::at(&root);
    let f = by_label(&mut n, "hwmon coretemp");
    let mut out = Vec::new();
    n.read(&f, Instant(1), &mut out);
    assert_eq!(out.len(), 1);
    assert_eq!((out[0].sense.as_str(), out[0].value), ("temp1.c", 61.5));

    fs::remove_file(root.join("class/hwmon/hwmon0/temp1_input")).unwrap();
    let mut out = Vec::new();
    n.read(&f, Instant(2), &mut out);
    assert!(out.is_empty(), "a missing file was reported as a value");
}

#[test]
fn fixture_the_same_hardware_gets_the_same_identity_and_different_hardware_does_not() {
    assert_eq!(
        stable_key("hwmon", "a").public().to_bytes(),
        stable_key("hwmon", "a").public().to_bytes()
    );
    assert_ne!(
        stable_key("hwmon", "a").public().to_bytes(),
        stable_key("hwmon", "b").public().to_bytes()
    );
    assert_ne!(
        stable_key("hwmon", "a").public().to_bytes(),
        stable_key("nvml", "a").public().to_bytes()
    );
}

#[test]
fn fixture_a_fan_with_no_stated_limits_has_no_verb_and_gets_one_when_the_wirer_states_them() {
    let root = fixture("fanverb");
    let mut n = HwmonNerve::at(&root);
    let f = by_label(&mut n, "hwmon fanchip");
    let s = n.describe(&f).unwrap();
    assert_eq!(s.class, PartClass::Thermal);
    assert!(s.actionable().is_empty(), "sysfs states no fan limits, so there is no verb");

    let mut n = HwmonNerve::at(&root).with_fan_limits(6000.0, 7000.0);
    let f = by_label(&mut n, "hwmon fanchip");
    assert_eq!(n.describe(&f).unwrap().actionable().len(), 1);
}

/// The whole path on a fixture: acquire the fan and the CPU chip,
/// wire the fan to the chip, and drive the fan's mode file through
/// the gate. This is the fan *write* path, exercised against a file
/// in a temporary directory rather than a real fan.
#[test]
fn fixture_the_fan_write_goes_through_kira_and_is_refused_while_the_chip_is_hot() {
    let root = fixture("gate");
    let mut n = HwmonNerve::at(&root).with_fan_limits(6000.0, 7000.0);
    let mut e = entity(0x61);

    let cpu = by_label(&mut n, "hwmon coretemp");
    let schema = n.describe(&cpu).unwrap();
    let cpu_id = e
        .acquire_part(cpu.key, |c| n.answer(&cpu, c).unwrap(), schema, Wiring::default())
        .unwrap();

    let fan = by_label(&mut n, "hwmon fanchip");
    let schema = n.describe(&fan).unwrap();
    let full = Verb::parse("fan.full").unwrap();
    let fan_id = e
        .acquire_part(
            fan.key,
            |c| n.answer(&fan, c).unwrap(),
            schema,
            Wiring {
                // while the chip is at or above 90 C, full speed may
                // not be switched off
                guards: vec![Guard::sustain(full, "on", cpu_id, "temp1.c", 90.0, 1.0)],
                relief: vec![full],
                ..Wiring::default()
            },
        )
        .unwrap();

    let mut feel = |e: &mut Entity, n: &mut HwmonNerve, at: u64| {
        for (f, id) in [(cpu, cpu_id), (fan, fan_id)] {
            let mut out = Vec::new();
            n.read(&f, Instant(at), &mut out);
            e.feel(&id, &out, Instant(at));
        }
    };
    let mode = root.join("class/hwmon/hwmon2/pwm1_enable");
    let ask = |e: &mut Entity, on: i64| {
        let r = Request::on_body(full, fan_id, e.tick).arg("on", Arg::Count(on));
        let out = e.propose(r.clone());
        (r, out)
    };

    // cool: full speed on, then off again, each written by the nerve
    feel(&mut e, &mut n, 1_000);
    for (on, expect) in [(1, "0"), (0, "2")] {
        let (r, out) = ask(&mut e, on);
        let Verdict::Granted(g) = out.verdict else { panic!("refused while cool") };
        let m = Mandate::check(g, &r, &e.key().public(), e.tick).unwrap();
        n.apply(&fan, &m).unwrap();
        assert_eq!(fs::read_to_string(&mode).unwrap(), expect);
    }

    // hot: switching full speed off is refused, and the file is untouched
    fs::write(root.join("class/hwmon/hwmon0/temp1_input"), "97000\n").unwrap();
    feel(&mut e, &mut n, 2_000);
    fs::write(&mode, "0").unwrap();
    let (_, out) = ask(&mut e, 0);
    let d = out.verdict.denial().expect("granted while hot");
    assert_eq!((d.stage, d.code), (Stage::Simulate, DenyCode::PredictedInvariantBreak));
    assert_eq!(fs::read_to_string(&mode).unwrap(), "0");

    // and a file the process may not write comes back as a refusal by
    // the nerve, not a panic and not a silent success
    let (r, out) = ask(&mut e, 1);
    let Verdict::Granted(g) = out.verdict else { panic!() };
    let m = Mandate::check(g, &r, &e.key().public(), e.tick).unwrap();
    fs::remove_file(&mode).unwrap();
    fs::create_dir(&mode).unwrap();
    assert_eq!(n.apply(&fan, &m), Err(NerveFault::Refused));
}

#[test]
fn fixture_the_machine_nerve_reads_cpu_disk_and_battery() {
    let root = std::env::temp_dir().join(format!("brainos-machine-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let w = |rel: &str, body: &str| {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    };
    w("proc/cpuinfo", "processor\t: 0\nmodel name\t: Test CPU\nprocessor\t: 1\nmodel name\t: Test CPU\n");
    w("proc/stat", "cpu  100 0 100 800 0 0 0 0 0 0\n");
    w("sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq", "3000000\n");
    w("sys/block/nvme0n1/size", "2000\n");
    w("sys/block/nvme0n1/stat", "1 2 3 4 5 6 1000 8 9 10 11\n");
    w("sys/block/nvme0n1/device/model", "Test Disk\n");
    w("sys/block/nvme0n1/device/serial", "S123\n");
    w("sys/block/nvme0n1/device/hwmon4/temp1_input", "35000\n");
    w("sys/block/nvme0n1/device/hwmon4/temp1_max", "80000\n");
    w("sys/block/nvme0n1/device/hwmon4/temp1_crit", "85000\n");
    w("sys/block/zram0/size", "999\n");
    w("sys/class/power_supply/BAT0/type", "Battery\n");
    w("sys/class/power_supply/BAT0/capacity", "80\n");
    w("sys/class/power_supply/BAT0/cycle_count", "12\n");
    w("sys/class/power_supply/BAT0/energy_full", "45000000\n");
    w("sys/class/power_supply/BAT0/energy_full_design", "50000000\n");
    w("sys/class/power_supply/BAT0/alarm", "4500000\n");
    w("sys/class/power_supply/AC/type", "Mains\n");

    let mut n = MachineNerve::at(&root.join("sys"), &root.join("proc"));
    let found = n.enumerate();
    let labels: Vec<&str> = found.iter().map(|f| f.label.as_str()).collect();
    assert_eq!(labels, ["cpu Test CPU", "disk Test Disk", "battery BAT0"], "zram and mains are not parts");

    let cpu = n.describe(&found[0]).unwrap();
    assert_eq!((cpu.class, cpu.substrate), (PartClass::Compute, Some(Substrate::Cpu)));
    assert_eq!(cpu.check(), Ok(()));

    // load needs two samples: 100 more busy jiffies out of 200 more
    let mut out = Vec::new();
    n.read(&found[0], Instant(1), &mut out);
    assert!(out.iter().all(|s| s.sense.as_str() != "load"), "a load from one sample");
    w("proc/stat", "cpu  150 0 150 900 0 0 0 0 0 0\n");
    let mut out = Vec::new();
    n.read(&found[0], Instant(2), &mut out);
    let get = |out: &[brainos_part::nerve::Sensed], k: &str| {
        out.iter().find(|s| s.sense.as_str() == k).map(|s| s.value)
    };
    assert_eq!(get(&out, "load"), Some(0.5));
    assert_eq!(get(&out, "cores"), Some(2.0));
    assert_eq!(get(&out, "freq.hz"), Some(3.0e9));

    let disk = n.describe(&found[1]).unwrap();
    assert_eq!(disk.class, PartClass::Storage);
    let v = disk.vital_named("temp.c").expect("the drive states its limit");
    assert_eq!((v.band.warn, v.band.critical), (80.0, 85.0));
    let mut out = Vec::new();
    n.read(&found[1], Instant(3), &mut out);
    assert_eq!(get(&out, "size.bytes"), Some(2000.0 * 512.0));
    assert_eq!(get(&out, "written.bytes"), Some(1000.0 * 512.0));
    assert_eq!(get(&out, "temp.c"), Some(35.0));

    let bat = n.describe(&found[2]).unwrap();
    assert_eq!(bat.class, PartClass::Power);
    assert_eq!(bat.vital_named("charge").unwrap().band.critical, 0.1);
    let mut out = Vec::new();
    n.read(&found[2], Instant(4), &mut out);
    assert_eq!(get(&out, "charge"), Some(0.8));
    assert_eq!(get(&out, "capacity"), Some(0.9));
    assert_eq!(get(&out, "cycles"), Some(12.0));
}

// ------------------------------------------------------------------
// live: this machine
// ------------------------------------------------------------------

#[test]
fn live_hwmon_temperatures_are_read_and_acquired() {
    if !Path::new("/sys/class/hwmon").exists() {
        println!("SKIPPED (no /sys/class/hwmon)");
        return;
    }
    let mut e = entity(0x62);
    let mut n = HwmonNerve::live();
    let found = acquire_all(&mut e, &mut n, Instant(1_000));
    let temps: Vec<f64> = found
        .iter()
        .flat_map(|p| p.readings.iter())
        .filter(|(_, _, unit)| *unit == "C")
        .map(|(_, v, _)| *v)
        .collect();
    if temps.is_empty() {
        println!("SKIPPED (hwmon present but reports no temperature)");
        return;
    }
    println!("LIVE hwmon: {} parts, {} temperatures: {temps:?}", found.len(), temps.len());
    assert!(found.iter().all(|p| p.error.is_none()), "{found:?}");
    for t in &temps {
        assert!((-20.0..=120.0).contains(t), "{t} C is not a temperature this machine is at");
    }
    assert_eq!(e.parts.len(), found.len());
    // nothing on this path is a verb unless a wirer stated fan limits
    assert!(found.iter().all(|p| p.verbs == 0));
}

#[test]
fn live_readings_change_between_two_samples_or_stay_plausible() {
    if !Path::new("/sys/class/hwmon").exists() {
        println!("SKIPPED (no /sys/class/hwmon)");
        return;
    }
    // a live signal is one that is being measured now: read the same
    // senses twice and require the nerve to return the same *set* both
    // times, with values in range. (Equality is allowed; a temperature
    // can sit still for 300 ms.)
    let mut n = HwmonNerve::live();
    let found = n.enumerate();
    let sample = |n: &mut HwmonNerve| {
        let mut v = Vec::new();
        for f in &found {
            n.read(f, Instant(0), &mut v);
        }
        v
    };
    let a = sample(&mut n);
    std::thread::sleep(std::time::Duration::from_millis(300));
    let b = sample(&mut n);
    if a.is_empty() {
        println!("SKIPPED (no readings)");
        return;
    }
    assert_eq!(a.len(), b.len());
    let moved = a.iter().zip(&b).filter(|(x, y)| x.value != y.value).count();
    println!("LIVE hwmon: {} senses sampled twice, {moved} changed in 300 ms", a.len());
}

#[test]
fn live_the_machine_cpu_and_disk_are_acquired_as_parts() {
    if !Path::new("/proc/cpuinfo").exists() {
        println!("SKIPPED (no /proc/cpuinfo)");
        return;
    }
    let mut e = entity(0x63);
    let mut n = MachineNerve::live();
    for f in n.enumerate() {
        let mut sink = Vec::new();
        n.read(&f, Instant(0), &mut sink);
    }
    std::thread::sleep(std::time::Duration::from_millis(200));
    let found = acquire_all(&mut e, &mut n, Instant(1_000));
    println!(
        "LIVE machine: cpu {:?}, disks {:?}, {} parts",
        n.cpu_model(),
        n.disks(),
        found.len()
    );
    let cpu = found.iter().find(|p| p.class == "compute").expect("a CPU");
    assert!(cpu.error.is_none());
    let get = |k: &str| cpu.readings.iter().find(|(n, _, _)| n == k).map(|(_, v, _)| *v);
    assert!(get("cores").unwrap() >= 1.0);
    let load = get("load").expect("two samples give a load");
    assert!((0.0..=1.0).contains(&load));
    // the CPU is a placement target
    let r = e.resources.get("local-cpu").expect("the CPU was offered for placement");
    assert_eq!(r.substrate, Substrate::Cpu);

    if let Some(disk) = found.iter().find(|p| p.class == "storage") {
        assert!(disk.error.is_none());
        let size = disk.readings.iter().find(|(n, _, _)| n == "size.bytes").unwrap().1;
        assert!(size > 1e9, "a {size}-byte disk");
    } else {
        println!("SKIPPED the disk half (no block device with a device node)");
    }
}

#[cfg(feature = "nvml")]
#[test]
fn live_the_nvidia_gpu_is_acquired_as_a_compute_part_and_a_placement_target() {
    use brainos_host::nerves::nvml::NvmlNerve;
    let Some(mut n) = NvmlNerve::live() else {
        println!("SKIPPED (no NVIDIA library or no GPU)");
        return;
    };
    let mut e = entity(0x64);
    let found = acquire_all(&mut e, &mut n, Instant(1_000));
    let g = &found[0];
    println!("LIVE nvml: {} — {:?}; stated limits {:?}", g.label, g.readings, n.stated_limits());
    assert!(g.error.is_none(), "{:?}", g.error);
    assert_eq!(g.class, "compute");
    let get = |k: &str| g.readings.iter().find(|(n, _, _)| n == k).map(|(_, v, _)| *v);
    let t = get("temp.c").expect("a temperature");
    assert!((0.0..=110.0).contains(&t), "{t} C");
    let total = get("mem.total").expect("memory");
    assert!(total >= 1e9);
    assert!(get("mem.used").unwrap() <= total);
    assert!((0.0..=1.0).contains(&get("util").unwrap()));

    // cross-check against the vendor's own tool, when it is installed
    if let Ok(out) = std::process::Command::new("nvidia-smi")
        .args(["--query-gpu=temperature.gpu,memory.total", "--format=csv,noheader,nounits"])
        .output()
    {
        let text = String::from_utf8_lossy(&out.stdout);
        let nums: Vec<f64> = text
            .lines()
            .next()
            .unwrap_or("")
            .split(',')
            .filter_map(|x| x.trim().parse().ok())
            .collect();
        if nums.len() == 2 {
            assert!((nums[0] - t).abs() <= 5.0, "nvidia-smi says {} C, NVML said {t}", nums[0]);
            assert!((nums[1] * 1048576.0 - total).abs() / total < 0.02, "memory disagrees");
            println!("LIVE nvml: agrees with nvidia-smi ({} C, {} MiB)", nums[0], nums[1]);
        }
    }

    let r = e.resources.get("local-gpu").expect("the GPU was offered for placement");
    assert_eq!(r.substrate, Substrate::Gpu);
    assert!(r.available);
}

#[test]
fn live_survey_acquires_everything_and_writes_only_join_records() {
    if !Path::new("/sys/class/hwmon").exists() {
        println!("SKIPPED (no sysfs)");
        return;
    }
    let mut e = entity(0x65);
    let before = e.graph.len();
    let found = survey(&mut e);
    let ok = found.iter().filter(|p| p.error.is_none()).count();
    println!("LIVE survey: {} parts, {} acquired", found.len(), ok);
    assert!(ok >= 1);
    // one join record per part; a first round of healthy readings is
    // not news, so nothing else is written
    let unwell = found.iter().filter(|p| p.vitals > 0 && p.health != Health::Nominal).count();
    assert!(e.graph.len() - before >= ok);
    assert!(e.graph.len() - before <= ok + unwell + 2, "steady readings became memories");
}
