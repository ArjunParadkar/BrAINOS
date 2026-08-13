//! The body map — devices as regions you become (Architecture §8).
//!
//! This replaces device drivers. A device is not a tool the instance uses;
//! it is a region of the body the instance *is*, the way a brain learns it
//! has a hand. Every body the entity is bound to lives here, all present
//! in one body map at once. Today that is one machine; the cheetah, the
//! drone and the aircraft join as more regions, not as more programs.

use alloc::string::String;
use alloc::vec::Vec;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RegionClass {
    /// visual effector: framebuffer / text console
    Screen,
    /// tactile afferents: keystrokes arriving
    InputKeys,
    /// working substrate: how much room there is to think
    Ram,
    /// interoception: the body's sense of time
    Clock,
    /// the tethered line to Model M and the body daemon (§10.2)
    TelemetryLink,
    /// the key medium itself: where memory lives
    KeyMedium,
    /// auditory afferent: a microphone limb on the host body
    Microphone,
    /// vocal effector: a speaker limb on the host body
    Speaker,
    /// visual afferent: a lens the entity can look through (§9.3 — the
    /// human world perceived, not only conversed with)
    Camera,
    /// a host-side tool or sense offered over the tether
    HostTool,
    /// the machine's own execution substrate: code the entity runs itself
    Compute,
    /// a network interface the firmware can drive (§9.1: the internet as
    /// a navigational sense — dormant until a protocol limb exists)
    Network,
}

impl RegionClass {
    pub fn tag(self) -> &'static str {
        match self {
            RegionClass::Screen => "screen",
            RegionClass::InputKeys => "input.keys",
            RegionClass::Ram => "ram",
            RegionClass::Clock => "clock",
            RegionClass::TelemetryLink => "link.telemetry",
            RegionClass::KeyMedium => "key.medium",
            RegionClass::Microphone => "microphone",
            RegionClass::Speaker => "speaker",
            RegionClass::Camera => "camera",
            RegionClass::HostTool => "host.tool",
            RegionClass::Compute => "compute",
            RegionClass::Network => "network",
        }
    }

    /// Map a class name arriving in a limb offer to a region class.
    pub fn from_offer(tag: &str) -> RegionClass {
        match tag {
            "mic" | "microphone" => RegionClass::Microphone,
            "speaker" | "voice" => RegionClass::Speaker,
            "camera" | "eyes" | "lens" => RegionClass::Camera,
            "keys" | "input" => RegionClass::InputKeys,
            "screen" => RegionClass::Screen,
            _ => RegionClass::HostTool,
        }
    }
}

/// One region of the body. `capabilities` is what acting through this
/// region can do; `proprioception` is what the region reports back.
pub struct Region {
    pub id: String,
    pub class: RegionClass,
    pub capabilities: Vec<String>,
    pub proprioception: String,
}

/// Acquisition protocol state (§8): discovery → handshake → capability →
/// incorporation → calibration. Regions move through it explicitly so a
/// hot-plugged device later follows the same path.
pub enum Acquisition {
    Discovered,
    Handshaken,
    Incorporated,
}

pub struct BodyMap {
    regions: Vec<Region>,
}

impl BodyMap {
    pub fn new() -> BodyMap {
        BodyMap { regions: Vec::new() }
    }

    /// Step 4 of the acquisition protocol: the region joins the body.
    /// (Steps 1–3 — discovery, key handshake, capability publication —
    /// happen at the call site where the hardware is probed; step 5,
    /// calibration, is one Model M call once cognition is attached.)
    pub fn incorporate(&mut self, region: Region) {
        self.regions.push(region);
    }

    pub fn regions(&self) -> &[Region] {
        &self.regions
    }

    pub fn has(&self, class: RegionClass) -> bool {
        self.regions.iter().any(|r| r.class == class)
    }

    pub fn find(&self, class: RegionClass) -> Option<&Region> {
        self.regions.iter().find(|r| r.class == class)
    }

    /// The authorization question KIRA asks: does any region of the body
    /// actually advertise this capability? No region -> no limb -> the
    /// action cannot be authorized, no matter what cognition claims.
    pub fn find_capability(&self, verb: &str) -> Option<&Region> {
        self.regions
            .iter()
            .find(|r| r.capabilities.iter().any(|c| c == verb))
    }

    /// True if a region with this id is already incorporated (offers are
    /// re-sent on daemon reconnect; acquisition must be idempotent).
    pub fn knows(&self, id: &str) -> bool {
        self.regions.iter().any(|r| r.id == id)
    }
}
