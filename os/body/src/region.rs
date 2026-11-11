//! BFS  LEVEL    Section 8 Acquisition - a device becomes a region of the body  [crate brainos-body]
//! BFS  PURPOSE  Defines regions (parts of a body, each advertising verbs) and the BodyMap of all parts the entity currently has, used to refuse verbs no part offers
//! BFS  DEFINES  RegionClass, Region, BodyMap
//! BFS  USES     this crate: schema | brainos: abi
//! BFS  USED BY  adapters/mhs, adapters/native, adapters/ros2, adapters/tests/adapters_suite, blur/acquire, blur/schema, body/acquire, body/lib, +19 more
//!
//! Regions — the parts of the body, and the body map.
//!
//! §8: a device is a region of the body, not a program the entity runs.
//! The body map is the set of regions the entity currently *is*. It is
//! what the authorize stage consults, and its most important property is
//! negative: a verb no region advertises cannot be authorized, no matter
//! how confidently cognition asks for it. That is the honest-refusal
//! stage, and it is why the entity says "I have no hand for that" rather
//! than describing an action it cannot take.

use crate::schema::CapabilitySchema;
use alloc::string::String;
use alloc::vec::Vec;
use brainos_abi::error::{Error, Result};
use brainos_abi::ids::{BodyId, RegionId};
use brainos_abi::verb::Verb;

/// What kind of part of a body this is.
#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord)]
#[repr(u8)]
pub enum RegionClass {
    /// visual effector: a screen or a console
    Screen = 0,
    /// tactile afferent: keys, buttons, touch
    Input = 1,
    /// working substrate: how much room there is to think
    Ram = 2,
    /// interoception: the body's sense of time
    Clock = 3,
    /// the tethered line to a cognitive resource
    Link = 4,
    /// where memory is persisted
    Storage = 5,
    /// auditory afferent
    Microphone = 6,
    /// vocal effector
    Speaker = 7,
    /// visual afferent
    Camera = 8,
    /// a tool or sense offered over a tether
    Tool = 9,
    /// the entity's own execution substrate
    Compute = 10,
    /// a network interface
    Network = 11,
    /// legs: a walking body. Consequential; always has an envelope.
    Legs = 12,
    /// an arm or manipulator. Consequential.
    Arm = 13,
    /// a gripper or end effector. Consequential.
    Gripper = 14,
    /// wheels or tracks. Consequential.
    Drive = 15,
    /// a flight surface or rotor. Consequential, and irreversible in a
    /// way nothing else on this list is.
    Flight = 16,
    /// proprioception: joint encoders, IMU, current sensing
    Proprioception = 17,
    /// a heater, a pump, a valve — something that changes the world and
    /// is not locomotion
    Effector = 18,
}

impl RegionClass {
    pub const ALL: [RegionClass; 19] = [
        RegionClass::Screen,
        RegionClass::Input,
        RegionClass::Ram,
        RegionClass::Clock,
        RegionClass::Link,
        RegionClass::Storage,
        RegionClass::Microphone,
        RegionClass::Speaker,
        RegionClass::Camera,
        RegionClass::Tool,
        RegionClass::Compute,
        RegionClass::Network,
        RegionClass::Legs,
        RegionClass::Arm,
        RegionClass::Gripper,
        RegionClass::Drive,
        RegionClass::Flight,
        RegionClass::Proprioception,
        RegionClass::Effector,
    ];

    pub fn from_u8(v: u8) -> Option<RegionClass> {
        RegionClass::ALL.get(v as usize).copied()
    }

    pub const fn name(self) -> &'static str {
        match self {
            RegionClass::Screen => "screen",
            RegionClass::Input => "input",
            RegionClass::Ram => "ram",
            RegionClass::Clock => "clock",
            RegionClass::Link => "link",
            RegionClass::Storage => "storage",
            RegionClass::Microphone => "microphone",
            RegionClass::Speaker => "speaker",
            RegionClass::Camera => "camera",
            RegionClass::Tool => "tool",
            RegionClass::Compute => "compute",
            RegionClass::Network => "network",
            RegionClass::Legs => "legs",
            RegionClass::Arm => "arm",
            RegionClass::Gripper => "gripper",
            RegionClass::Drive => "drive",
            RegionClass::Flight => "flight",
            RegionClass::Proprioception => "proprioception",
            RegionClass::Effector => "effector",
        }
    }

    /// Does acting through this region move something in the physical
    /// world? Consequential regions must have an envelope bound before
    /// incorporation completes; the rest need only a policy.
    ///
    /// This is the distinction §1.3 is about: on a PC a bad action
    /// deletes a file and undo exists; on one of these it is a fall.
    pub const fn is_consequential(self) -> bool {
        matches!(
            self,
            RegionClass::Legs
                | RegionClass::Arm
                | RegionClass::Gripper
                | RegionClass::Drive
                | RegionClass::Flight
                | RegionClass::Effector
        )
    }

    /// Does this region only sense? A purely afferent region cannot be
    /// commanded, so a verb declared on one is a schema error.
    pub const fn is_afferent_only(self) -> bool {
        matches!(
            self,
            RegionClass::Input
                | RegionClass::Microphone
                | RegionClass::Camera
                | RegionClass::Proprioception
                | RegionClass::Clock
                | RegionClass::Ram
        )
    }

    /// Map an adapter's class name onto a region class. Anything not
    /// recognized becomes `Tool`: useful, non-consequential, and gated
    /// like everything else. Guessing `Legs` from an unknown string
    /// would be guessing that something can walk.
    pub fn from_offer(tag: &str) -> RegionClass {
        match tag {
            "mic" | "microphone" | "audio_in" => RegionClass::Microphone,
            "speaker" | "voice" | "audio_out" => RegionClass::Speaker,
            "camera" | "eyes" | "lens" | "image" => RegionClass::Camera,
            "keys" | "input" | "keyboard" => RegionClass::Input,
            "screen" | "display" | "console" => RegionClass::Screen,
            "legs" | "hexapod" | "quadruped" | "biped" => RegionClass::Legs,
            "arm" | "manipulator" | "joint_group" => RegionClass::Arm,
            "gripper" | "hand" | "end_effector" => RegionClass::Gripper,
            "drive" | "wheels" | "base" | "tracks" => RegionClass::Drive,
            "flight" | "rotor" | "surface" => RegionClass::Flight,
            "imu" | "encoder" | "proprioception" => RegionClass::Proprioception,
            "net" | "network" => RegionClass::Network,
            "storage" | "disk" => RegionClass::Storage,
            "compute" | "cpu" => RegionClass::Compute,
            "clock" | "time" => RegionClass::Clock,
            "heater" | "pump" | "valve" | "effector" => RegionClass::Effector,
            _ => RegionClass::Tool,
        }
    }
}

/// One region of the body.
#[derive(Clone, Debug)]
pub struct Region {
    pub id: RegionId,
    /// which body this region belongs to — a region always knows, so
    /// "act through the hexapod's legs" is unambiguous when two hexapods
    /// are bound at once
    pub body: BodyId,
    pub class: RegionClass,
    /// a stable, human-facing name
    pub label: String,
    /// the verbs this region advertises, sorted
    capabilities: Vec<Verb>,
    /// what the region reports back
    pub proprioception: String,
    /// is this region currently answering?
    pub alive: bool,
}

impl Region {
    pub fn new(id: RegionId, body: BodyId, class: RegionClass, label: &str) -> Region {
        Region {
            id,
            body,
            class,
            label: alloc::string::ToString::to_string(label),
            capabilities: Vec::new(),
            proprioception: String::new(),
            alive: true,
        }
    }

    pub fn advertise(&mut self, verb: Verb) {
        if !self.capabilities.contains(&verb) {
            self.capabilities.push(verb);
            self.capabilities
                .sort_by(|a, b| a.as_str().cmp(b.as_str()));
        }
    }

    pub fn capabilities(&self) -> &[Verb] {
        &self.capabilities
    }

    pub fn advertises(&self, verb: &Verb) -> bool {
        self.capabilities.contains(verb)
    }
}

/// Every region of every body the entity is currently bound to.
///
/// One map, all bodies. That is the Stage 4 claim — "one self with
/// several bodies", not several selves synced — and it falls out of the
/// data structure rather than being maintained by synchronization.
#[derive(Default)]
pub struct BodyMap {
    regions: Vec<Region>,
    bodies: Vec<(BodyId, String, CapabilitySchema)>,
}

impl BodyMap {
    pub fn new() -> BodyMap {
        BodyMap {
            regions: Vec::new(),
            bodies: Vec::new(),
        }
    }

    pub fn regions(&self) -> &[Region] {
        &self.regions
    }

    pub fn bodies(&self) -> impl Iterator<Item = (&BodyId, &str, &CapabilitySchema)> {
        self.bodies.iter().map(|(a, b, c)| (a, b.as_str(), c))
    }

    pub fn body_count(&self) -> usize {
        self.bodies.len()
    }

    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }

    pub fn knows_body(&self, body: &BodyId) -> bool {
        self.bodies.iter().any(|(b, _, _)| b == body)
    }

    pub fn schema_for(&self, body: &BodyId) -> Option<&CapabilitySchema> {
        self.bodies
            .iter()
            .find(|(b, _, _)| b == body)
            .map(|(_, _, s)| s)
    }

    /// Step 4: the body joins. Idempotent — offers are re-sent when a
    /// link reconnects, and acquisition that is not idempotent produces
    /// a second copy of a leg.
    pub fn incorporate(
        &mut self,
        body: BodyId,
        label: &str,
        schema: CapabilitySchema,
        regions: Vec<Region>,
    ) -> Result<()> {
        if self.knows_body(&body) {
            return Err(Error::Exists("body already incorporated"));
        }
        for r in &regions {
            if r.body != body {
                return Err(Error::State("region belongs to a different body"));
            }
        }
        self.bodies.push((
            body,
            alloc::string::ToString::to_string(label),
            schema,
        ));
        self.regions.extend(regions);
        Ok(())
    }

    /// The authorization question: does any region actually advertise
    /// this? `None` means no limb, which means no authorization, which
    /// means an honest refusal rather than a described action.
    pub fn find_capability(&self, verb: &Verb) -> Option<&Region> {
        self.regions
            .iter()
            .find(|r| r.alive && r.advertises(verb))
    }

    /// The same question, restricted to one named body. Used when
    /// several bodies advertise the same verb and the caller must say
    /// which one it means — "walk" with two hexapods bound is ambiguous,
    /// and resolving ambiguity by picking the first is how the wrong
    /// robot moves.
    pub fn find_capability_on(&self, body: &BodyId, verb: &Verb) -> Option<&Region> {
        self.regions
            .iter()
            .find(|r| r.alive && r.body == *body && r.advertises(verb))
    }

    /// Every body that can do this. The entity uses it to *notice* the
    /// ambiguity rather than to resolve it silently.
    pub fn bodies_offering(&self, verb: &Verb) -> Vec<BodyId> {
        let mut out: Vec<BodyId> = self
            .regions
            .iter()
            .filter(|r| r.alive && r.advertises(verb))
            .map(|r| r.body)
            .collect();
        out.sort();
        out.dedup();
        out
    }

    pub fn region(&self, id: &RegionId) -> Option<&Region> {
        self.regions.iter().find(|r| r.id == *id)
    }

    pub fn region_mut(&mut self, id: &RegionId) -> Option<&mut Region> {
        self.regions.iter_mut().find(|r| r.id == *id)
    }

    pub fn has_class(&self, class: RegionClass) -> bool {
        self.regions.iter().any(|r| r.alive && r.class == class)
    }

    pub fn of_class(&self, class: RegionClass) -> impl Iterator<Item = &Region> {
        self.regions
            .iter()
            .filter(move |r| r.alive && r.class == class)
    }

    /// A region stopped answering. Its verbs vanish with it, so the next
    /// proposal through it is refused at authorize — the entity cannot
    /// keep claiming a hand it no longer has.
    pub fn mark_dead(&mut self, id: &RegionId) -> bool {
        match self.region_mut(id) {
            Some(r) => {
                r.alive = false;
                true
            }
            None => false,
        }
    }

    pub fn mark_alive(&mut self, id: &RegionId) -> bool {
        match self.region_mut(id) {
            Some(r) => {
                r.alive = true;
                true
            }
            None => false,
        }
    }

    /// A body leaves entirely. Its regions and its schema go with it; the
    /// entity does not.
    pub fn release(&mut self, body: &BodyId) -> bool {
        let before = self.bodies.len();
        self.bodies.retain(|(b, _, _)| b != body);
        self.regions.retain(|r| r.body != *body);
        self.bodies.len() != before
    }

    /// Every verb the entity can currently perform, across all bodies.
    pub fn repertoire(&self) -> Vec<Verb> {
        let mut out: Vec<Verb> = self
            .regions
            .iter()
            .filter(|r| r.alive)
            .flat_map(|r| r.capabilities.iter().copied())
            .collect();
        out.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        out.dedup();
        out
    }
}
