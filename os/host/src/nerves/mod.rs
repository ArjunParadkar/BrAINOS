//! BFS  LEVEL    Host - the only crate with an operating system (CLI, scenarios, OS nerves)  [crate brainos-host]
//! BFS  PURPOSE  Root of the operating-system nerves: shared helpers (stable part keys, the network switch) and the live survey of this machine
//! BFS  DEFINES  hwmon, machine, ros2, socketcan, nvml, net_allowed, stable_key, read_text, read_f64, Surveyed, acquire_all, survey
//! BFS  USES     this crate: clock | brainos: abi, body, cognition, crypto, entity, part | external: std
//! BFS  USED BY  host/tests/nerves_can, host/tests/nerves_live, host/tests/nerves_ros2
//!
//! The nerves that need an operating system.
//!
//! `brainos-part` defines the `Nerve` trait and is `no_std`. The nerves
//! here implement it by reading files, loading a vendor library or
//! opening a bus socket, so they live in the one crate that is allowed
//! an operating system.
//!
//! | module | nerve | parts it finds | status on this machine |
//! |---|---|---|---|
//! | `hwmon` | Linux hwmon + thermal zones (sysfs) | temperature chips, fans | read: live. Fan write: needs root |
//! | `machine` | `/proc`, `/sys/block`, `/sys/class/power_supply` | the CPU, each disk, the battery | live |
//! | `nvml` | NVIDIA NVML, loaded at run time | each NVIDIA GPU | live when the library and a GPU are present |
//! | `socketcan` | Linux SocketCAN + the BLUR CAN codec | BLUR motor nodes | needs a CAN interface |
//! | `ros2` | the `ros2` command-line tool | a running ROS 2 graph | needs ROS 2 installed |
//!
//! Details, status tags and runbooks: `docs/NERVES.md`.
//!
//! # Identity of an OS-level part
//!
//! §8 wants every part to hold a key and prove it. A fan behind sysfs
//! holds no key. `stable_key` derives one from the most stable
//! identifier the operating system offers (a disk's serial, a GPU's
//! UUID, a chip's device path) so the part gets the *same* id each
//! boot and a *different* id when the hardware is swapped.
//!
//! That is identity, **not attestation**: the kernel is vouching for
//! the part, the part is not proving itself. Hardware attestation of
//! on-board parts (SPDM over PCIe, for instance) is not built.
//!
//! # The network
//!
//! `net_allowed()` is true only when `BRAINOS_NET=on`. A nerve that
//! would open a network socket (ROS 2 / DDS) checks it first and
//! refuses otherwise. Sysfs, NVML and CAN are not network: they are
//! files, a local library and a local field bus.

pub mod hwmon;
pub mod machine;
pub mod ros2;
#[cfg(feature = "socketcan")]
mod can_ffi;
#[cfg(feature = "socketcan")]
pub mod socketcan;
#[cfg(feature = "nvml")]
pub mod nvml;
#[cfg(feature = "nvml")]
mod nvml_ffi;

use brainos_abi::time::Instant;
use brainos_body::vital::Health;
use brainos_crypto::key::BrainKey;
use brainos_crypto::sha256::Sha256;
use brainos_crypto::domain::Tag;
use brainos_entity::entity::{Entity, PartError, Wiring};
use brainos_part::nerve::{Found, Nerve};
use std::path::Path;

/// Is the network organ switched on for this session?
pub fn net_allowed() -> bool {
    std::env::var("BRAINOS_NET").map(|v| v == "on").unwrap_or(false)
}

/// A key for a part that cannot hold one, derived from its stable
/// identifier. Same hardware, same key; see the module note on why
/// this is identity and not attestation.
pub fn stable_key(nerve: &str, stable_id: &str) -> BrainKey {
    let mut h = Sha256::tagged(Tag::Subkey);
    h.field(b"os-part");
    h.field(nerve.as_bytes());
    h.field(stable_id.as_bytes());
    let seed: [u8; 32] = *h.finish().as_bytes();
    BrainKey::from_seed(&seed)
}

/// Read a sysfs-style file as trimmed text. `None` for missing,
/// unreadable or empty — all of which mean "this part does not say".
pub fn read_text(p: &Path) -> Option<String> {
    let s = std::fs::read_to_string(p).ok()?;
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_string())
}

pub fn read_f64(p: &Path) -> Option<f64> {
    read_text(p)?.parse::<f64>().ok()
}

/// One part as the survey saw it.
#[derive(Clone, Debug)]
pub struct Surveyed {
    pub nerve: &'static str,
    pub label: String,
    pub class: &'static str,
    pub senses: usize,
    pub vitals: usize,
    pub verbs: usize,
    pub commandable: bool,
    pub health: Health,
    /// name, value, unit of each reading taken
    pub readings: Vec<(String, f64, &'static str)>,
    pub error: Option<String>,
}

/// Acquire every part a nerve finds into an entity, and take one round
/// of readings. This is §8 run against live hardware: enumerate,
/// challenge, describe, incorporate, then feel.
pub fn acquire_all(e: &mut Entity, n: &mut dyn Nerve, at: Instant) -> Vec<Surveyed> {
    let mut out = Vec::new();
    for found in n.enumerate() {
        let Some(schema) = n.describe(&found) else {
            continue;
        };
        let mut s = Surveyed {
            nerve: n.name(),
            label: schema.label().to_string(),
            class: schema.class.name(),
            senses: schema.senses().len(),
            vitals: schema.vitals().len(),
            verbs: schema.actionable().len(),
            commandable: false,
            health: Health::Nominal,
            readings: Vec::new(),
            error: None,
        };
        let key = found.key;
        let units: Vec<(String, &'static str)> = schema
            .senses()
            .iter()
            .map(|x| (x.name.as_str().to_string(), x.quantity.unit()))
            .collect();
        let acquired = acquire_one(e, n, &found, schema);
        match acquired {
            Ok(id) => {
                let mut sensed = Vec::new();
                n.read(&found, at, &mut sensed);
                e.feel(&id, &sensed, at);
                for r in &sensed {
                    let unit = units
                        .iter()
                        .find(|(nm, _)| nm == r.sense.as_str())
                        .map(|(_, u)| *u)
                        .unwrap_or("");
                    s.readings.push((r.sense.as_str().to_string(), r.value, unit));
                }
                if let Some(rec) = e.part(&id) {
                    s.commandable = rec.commandable;
                    s.health = rec.health();
                }
            }
            Err(err) => s.error = Some(format!("{err:?}")),
        }
        let _ = key;
        out.push(s);
    }
    out
}

fn acquire_one(
    e: &mut Entity,
    n: &mut dyn Nerve,
    found: &Found,
    schema: brainos_part::schema::PartSchema,
) -> Result<brainos_abi::ids::BodyId, PartError> {
    // the answer has to be produced inside the handshake, which holds
    // the entity; the nerve is a separate object, so this is fine
    // a Compute part is somewhere thinking can be placed, so it is
    // offered to the resource pool. One entry per kind of tissue.
    let compute = schema.substrate.map(|s| {
        let (name, cost) = match s {
            brainos_cognition::placement::Substrate::Gpu => ("local-gpu", 2),
            brainos_cognition::placement::Substrate::Npu => ("local-npu", 2),
            brainos_cognition::placement::Substrate::Bpu => ("local-bpu", 1),
            _ => ("local-cpu", 5),
        };
        brainos_entity::entity::ComputeOffer {
            name,
            tier: brainos_cognition::resource::Tier::Fast,
            site: brainos_cognition::placement::Site::OnBoard,
            scope: brainos_abi::verb::Namespace::Any,
            about: "compute on the machine the entity is running on",
            cost: Some(cost),
        }
    });
    e.acquire_part(
        found.key,
        |c| n.answer(found, c).unwrap_or_default(),
        schema,
        Wiring { compute, ..Wiring::default() },
    )
}

/// Survey this machine: every live nerve, every part, one round of
/// readings, all through §8 into a fresh entity.
///
/// The CPU load needs two samples, so the machine nerve is read twice
/// a short while apart.
pub fn survey(e: &mut Entity) -> Vec<Surveyed> {
    let at = Instant(crate::clock::now_micros());
    let mut out = Vec::new();

    let mut hw = hwmon::HwmonNerve::live();
    out.extend(acquire_all(e, &mut hw, at));

    let mut m = machine::MachineNerve::live();
    {
        use brainos_part::nerve::Nerve;
        // prime the CPU counters
        for f in m.enumerate() {
            let mut sink = Vec::new();
            m.read(&f, at, &mut sink);
        }
    }
    std::thread::sleep(std::time::Duration::from_millis(250));
    out.extend(acquire_all(e, &mut m, Instant(crate::clock::now_micros())));

    #[cfg(feature = "nvml")]
    if let Some(mut g) = nvml::NvmlNerve::live() {
        out.extend(acquire_all(e, &mut g, Instant(crate::clock::now_micros())));
    }
    out
}
