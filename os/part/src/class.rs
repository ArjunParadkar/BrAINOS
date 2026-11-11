//! BFS  LEVEL    Parts and nerves - every piece of hardware as a body part  [crate brainos-part]
//! BFS  PURPOSE  The eight classes a part can be, and which of the five architecture domains looks after each class
//! BFS  DEFINES  Domain, PartClass
//! BFS  USES     brainos: body | external: PartClass
//! BFS  USED BY  bpu/nerve, bpu/tests/seam, host/nerves/hwmon, host/nerves/machine, host/nerves/nvml, host/nerves/socketcan, host/tests/nerves_live, nostd/lib, +5 more
//!
//! Part classes, and where each sits in the five domains.
//!
//! Architecture v1.3 has five domains. Every part passes through three
//! of them the same way: Domain 1 attests it (it has a key, and its id
//! is derived from that key), Domain 3 gates every verb on it, and
//! Domain 5 remembers what it reported. What differs by class is which
//! domain *looks after it day to day*, and that is what `served_by`
//! encodes:
//!
//! ```text
//!   Actuator, Sensor, Thermal, Power  -> Domain 2 Autonomic
//!       regulated by reflexes and homeostats, with no model call,
//!       and they keep running if cognition dies
//!   Compute, Comms                    -> Domain 4 Cognition
//!       Compute parts are where thinking can be placed; Comms parts
//!       are what makes a remote site reachable at all
//!   Storage                           -> Domain 5 State
//!       hosts the state graph's sealed bytes; never owns it
//!   Composite                         -> whatever its members are
//! ```
//!
//! These are functions rather than a table in a document so that the
//! rest of the code asks the class instead of re-deciding: the entity
//! registers a placement target *because* `is_placement_target()` says
//! so, and refuses to give a storage part anything but sealed bytes
//! *because* `hosts_state()` says that is all it may hold.

use brainos_body::region::RegionClass;

/// One of the five domains of Architecture v1.3 §4.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[repr(u8)]
pub enum Domain {
    Silicon = 1,
    Autonomic = 2,
    Kira = 3,
    Cognition = 4,
    State = 5,
}

impl Domain {
    pub const fn name(self) -> &'static str {
        match self {
            Domain::Silicon => "silicon",
            Domain::Autonomic => "autonomic",
            Domain::Kira => "kira",
            Domain::Cognition => "cognition",
            Domain::State => "state",
        }
    }
}

/// What kind of part this is.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[repr(u8)]
pub enum PartClass {
    /// moves or changes something: a motor, a servo, a valve
    Actuator = 0,
    /// only reports: a camera, an IMU, an encoder
    Sensor = 1,
    /// executes: a CPU, a GPU, an NPU, a BPU, an MCU
    Compute = 2,
    /// holds bytes: RAM, flash, a disk
    Storage = 3,
    /// supplies or stores energy: a battery, a rail, a charger
    Power = 4,
    /// moves heat: a fan, a pump, a heater. Also thermal sensors that
    /// exist to protect something else.
    Thermal = 5,
    /// carries messages: a NIC, a radio, a bus adapter
    Comms = 6,
    /// several of the above behind one attestation key
    Composite = 7,
}

impl PartClass {
    pub const ALL: [PartClass; 8] = [
        PartClass::Actuator,
        PartClass::Sensor,
        PartClass::Compute,
        PartClass::Storage,
        PartClass::Power,
        PartClass::Thermal,
        PartClass::Comms,
        PartClass::Composite,
    ];

    pub fn from_u8(v: u8) -> Option<PartClass> {
        PartClass::ALL.get(v as usize).copied()
    }

    pub const fn name(self) -> &'static str {
        match self {
            PartClass::Actuator => "actuator",
            PartClass::Sensor => "sensor",
            PartClass::Compute => "compute",
            PartClass::Storage => "storage",
            PartClass::Power => "power",
            PartClass::Thermal => "thermal",
            PartClass::Comms => "comms",
            PartClass::Composite => "composite",
        }
    }

    /// The domain that looks after this class day to day. `None` for a
    /// composite, which is looked after by whatever its members are.
    pub const fn served_by(self) -> Option<Domain> {
        match self {
            PartClass::Actuator | PartClass::Sensor | PartClass::Thermal | PartClass::Power => {
                Some(Domain::Autonomic)
            }
            PartClass::Compute | PartClass::Comms => Some(Domain::Cognition),
            PartClass::Storage => Some(Domain::State),
            PartClass::Composite => None,
        }
    }

    /// Is this a part cognition can be placed on?
    pub const fn is_placement_target(self) -> bool {
        matches!(self, PartClass::Compute)
    }

    /// May this part hold the state graph's sealed bytes? It never owns
    /// the graph: the graph is bound to the Key, journaled, and leaves
    /// with the entity.
    pub const fn hosts_state(self) -> bool {
        matches!(self, PartClass::Storage)
    }

    /// Does this class exist to keep the body alive, so that its
    /// regulation must not depend on cognition?
    pub const fn is_homeostatic(self) -> bool {
        matches!(self, PartClass::Thermal | PartClass::Power)
    }

    /// Can a part of this class have verbs at all? A sensor cannot: it
    /// only reports, and a verb declared on one is a schema error (the
    /// same rule §8 applies to afferent-only regions).
    pub const fn may_act(self) -> bool {
        !matches!(self, PartClass::Sensor)
    }

    /// The body-map region classes a part of this class may declare its
    /// verbs on. This is the join between the part model and the
    /// existing body map: a part's verbs are advertised on ordinary
    /// regions, so the gate's authorize stage needs no change.
    pub const fn regions(self) -> &'static [RegionClass] {
        match self {
            PartClass::Actuator => &[
                RegionClass::Legs,
                RegionClass::Arm,
                RegionClass::Gripper,
                RegionClass::Drive,
                RegionClass::Flight,
                RegionClass::Effector,
            ],
            PartClass::Sensor => &[],
            PartClass::Compute => &[RegionClass::Compute],
            PartClass::Storage => &[RegionClass::Storage, RegionClass::Ram],
            PartClass::Power | PartClass::Thermal => &[RegionClass::Effector],
            PartClass::Comms => &[RegionClass::Network, RegionClass::Link],
            PartClass::Composite => &RegionClass::ALL,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_class_round_trips_through_its_number() {
        for c in PartClass::ALL {
            assert_eq!(PartClass::from_u8(c as u8), Some(c));
        }
        assert_eq!(PartClass::from_u8(8), None);
    }

    #[test]
    fn the_domain_mapping_is_the_one_the_architecture_states() {
        use PartClass::*;
        for c in [Actuator, Sensor, Thermal, Power] {
            assert_eq!(c.served_by(), Some(Domain::Autonomic));
        }
        assert_eq!(Compute.served_by(), Some(Domain::Cognition));
        assert_eq!(Comms.served_by(), Some(Domain::Cognition));
        assert_eq!(Storage.served_by(), Some(Domain::State));
        assert_eq!(Composite.served_by(), None);
    }

    #[test]
    fn only_compute_is_a_placement_target_and_only_storage_hosts_state() {
        for c in PartClass::ALL {
            assert_eq!(c.is_placement_target(), c == PartClass::Compute);
            assert_eq!(c.hosts_state(), c == PartClass::Storage);
        }
    }

    #[test]
    fn a_sensor_cannot_act_and_has_no_region_to_act_through() {
        assert!(!PartClass::Sensor.may_act());
        assert!(PartClass::Sensor.regions().is_empty());
        for c in PartClass::ALL.iter().filter(|c| **c != PartClass::Sensor) {
            assert!(c.may_act());
            assert!(!c.regions().is_empty());
        }
    }

    #[test]
    fn thermal_and_power_parts_act_through_a_consequential_region() {
        // so that §8 insists on calibration and a policy before they act
        for c in [PartClass::Thermal, PartClass::Power] {
            assert!(c.is_homeostatic());
            assert!(c.regions().iter().all(|r| r.is_consequential()));
        }
    }
}
