//! BFS  LEVEL    Section 8 Acquisition - a device becomes a region of the body  [crate brainos-body]
//! BFS  PURPOSE  The minimal hardware interface: Actuator, Sensor and BodyLink traits plus Command, Reading and Telemetry values, so upper layers never see a specific bus
//! BFS  DEFINES  Command, Reading, ActuatorFault, Actuator, Sensor, Telemetry, BodyLink
//! BFS  USES     brainos: abi, rt
//! BFS  USED BY  blur/policy, blur/tests/blur_suite, body/lib, body/policy, body/tests/body_suite, entity/entity, entity/tests/entity_suite, host/bench, +15 more
//!
//! The hardware abstraction, such as it is.
//!
//! Deliberately thin. Everything above this line reasons about verbs,
//! envelopes and schemas; everything below it is a specific bus on a
//! specific board. The interface between them is three traits and two
//! value types, and it is kept small because a fat HAL is a place for
//! platform assumptions to hide.
//!
//! Note what is *not* here: no async, no allocation, no error strings.
//! An actuator write either happened or it did not, and the reason it
//! did not is one of a handful of physical situations that the reflex
//! layer already knows how to respond to.

use brainos_abi::error::Result;
use brainos_abi::ids::RegionId;
use brainos_abi::time::Instant;
use brainos_rt::envelope::Channel;

/// One thing to do to one channel of one region.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Command {
    pub region: RegionId,
    pub channel: Channel,
    /// already clamped into the envelope by `ReflexBank`; a driver does
    /// not clamp, because a driver that clamps is a driver that has an
    /// opinion about safety, and safety opinions belong in one place
    pub value: f32,
    pub at: Instant,
}

/// One reading.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Reading {
    pub region: RegionId,
    pub channel: Channel,
    pub value: f32,
    pub at: Instant,
    /// false when the driver knows this value is stale or estimated —
    /// a sensor that reports its own uncertainty is worth two that do not
    pub fresh: bool,
}

/// Why a write did not happen. Small and physical.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ActuatorFault {
    /// the bus did not acknowledge
    NoResponse,
    /// the actuator reported it is over current
    OverCurrent,
    /// the actuator reported it is over temperature
    OverTemperature,
    /// the actuator is at a mechanical stop
    Stalled,
    /// the rail is below the floor
    Brownout,
    /// the channel is not wired on this body
    NotPresent,
}

/// Something that can be commanded.
pub trait Actuator {
    fn region(&self) -> RegionId;

    /// Which channels this actuator accepts.
    fn channels(&self) -> &[Channel];

    /// Apply a command. Total: no panics, no blocking beyond the bus
    /// transaction, and a fault is a value rather than an error string.
    fn apply(&mut self, cmd: &Command) -> core::result::Result<(), ActuatorFault>;

    /// Go to the safe pose immediately. Called by the reflex layer on a
    /// stop, and it must not fail — if the bus is gone, torque is
    /// released by the hardware's own watchdog, which is why every body
    /// that can fall has one.
    fn safe(&mut self);
}

/// Something that can be read.
pub trait Sensor {
    fn region(&self) -> RegionId;

    fn channels(&self) -> &[Channel];

    /// Read one channel. `None` means this sensor has nothing to say
    /// right now, which is different from a reading of zero.
    fn read(&mut self, channel: Channel, at: Instant) -> Option<Reading>;
}

/// A body's report about itself, assembled from its sensors.
#[derive(Clone, Copy, Debug, Default)]
pub struct Telemetry {
    pub at: Instant,
    pub link_alive: bool,
    /// volts on the actuator rail
    pub rail_v: Option<f32>,
    /// amperes drawn
    pub current_a: Option<f32>,
    /// degrees from vertical
    pub tilt_deg: Option<f32>,
    /// hottest actuator, degrees Celsius
    pub hottest_c: Option<f32>,
    /// how many reflex stops have occurred since power-on
    pub stops: u32,
    /// how many loop overruns the body has seen
    pub overruns: u32,
}

impl Telemetry {
    /// Is the body in a state where it can safely be asked to do
    /// something new? A conservative gate, used by the body policy's
    /// stage 6 — it answers "right now", not "in general".
    pub fn is_fit(&self) -> bool {
        self.link_alive
            && self.stops == 0
            && self.rail_v.map(|v| v > 6.0).unwrap_or(true)
            && self.hottest_c.map(|c| c < 70.0).unwrap_or(true)
            && self.tilt_deg.map(|t| t.abs() < 20.0).unwrap_or(true)
    }

    /// The single reason it is not fit, for the refusal. First match in
    /// the order a human would check.
    pub fn unfitness(&self) -> Option<&'static str> {
        if !self.link_alive {
            return Some("link to the body is down");
        }
        if self.stops > 0 {
            return Some("the body is stopped and needs a decision, not a retry");
        }
        if self.rail_v.map(|v| v <= 6.0).unwrap_or(false) {
            return Some("actuator rail is below the floor");
        }
        if self.hottest_c.map(|c| c >= 70.0).unwrap_or(false) {
            return Some("an actuator is over temperature");
        }
        if self.tilt_deg.map(|t| t.abs() >= 20.0).unwrap_or(false) {
            return Some("the body is not level");
        }
        None
    }
}

/// A transport to a body. Implemented by serial, by UDP, by a loopback
/// for the simulator — and by nothing that knows what a verb is.
pub trait BodyLink {
    fn send(&mut self, frame: &[u8]) -> Result<()>;
    /// Non-blocking. `Ok(0)` means nothing arrived, which is not an error.
    fn recv(&mut self, into: &mut [u8]) -> Result<usize>;
    fn last_heard(&self) -> Instant;
    fn is_alive(&self, now: Instant) -> bool;
}
