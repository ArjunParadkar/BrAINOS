//! BFS  LEVEL    Parts and nerves - every piece of hardware as a body part  [crate brainos-part]
//! BFS  PURPOSE  SimNerve: a simulated fan and GPU with a small thermal model behind the Nerve trait, for tests and the proof domain (not hardware)
//! BFS  DEFINES  FAN, GPU, AMBIENT_C, SimNerve
//! BFS  USES     this crate: class, homeostat, nerve, schema | brainos: abi, body, cognition, crypto
//! BFS  USED BY  entity/tests/parts_suite, host/tests/autonomic_survives
//!
//! A simulated nerve: one fan, one GPU, and the heat between them.
//!
//! **This is not hardware and nothing measured with it is a hardware
//! result.** It exists because the properties worth testing about parts
//! are about what happens when a real fan must not be stopped — and a
//! test suite cannot be allowed to stop a real fan. The live nerves
//! (sysfs, NVML, SocketCAN) are in `host`; this is the bench they are
//! checked against.
//!
//! ```text
//!   handle 0  "sim fan"  Thermal   verbs fan.set(duty_pct), fan.regulate(min, max)
//!                                  senses fan.duty, fan.rpm, fan.stall (vital)
//!   handle 1  "sim gpu"  Compute   no verbs
//!                                  senses temp.c (vital: 85 / 95), util
//! ```
//!
//! The plant is one line of physics per step: the GPU heats in
//! proportion to its load and cools in proportion to fan duty and to
//! how far above ambient it is. It is crude on purpose. What matters is
//! the sign: with the fan stopped under load the temperature rises past
//! the limit, and with the fan running it does not.
//!
//! The knobs (`declare_no_limits`, `gpu_limit_c`, `gpu_silent`,
//! `fan_jammed`) are the adversarial cases: a part that will not state
//! its limits, a part that changes what it claims, a part that stops
//! reporting, a part that faults.

use crate::class::PartClass;
use crate::homeostat::ReflexAct;
use crate::nerve::{Found, Mandate, Nerve, NerveFault, Sensed};
use crate::schema::{PartSchema, Quantity, SenseSpec, VitalKind, VitalSpec};
use alloc::vec::Vec;
use brainos_abi::text::Text;
use brainos_abi::time::Instant;
use brainos_abi::verb::Verb;
use brainos_body::region::RegionClass;
use brainos_body::schema::{ParamSpec, VerbSpec};
use brainos_body::vital::Band;
use brainos_cognition::placement::Substrate;
use brainos_crypto::key::{BrainKey, Challenge, Signature};

pub const FAN: u32 = 0;
pub const GPU: u32 = 1;
pub const AMBIENT_C: f64 = 25.0;

/// The simulated bench.
pub struct SimNerve {
    fan_key: BrainKey,
    gpu_key: BrainKey,
    /// fan duty, 0..100
    pub duty: f64,
    /// GPU temperature, Celsius
    pub temp_c: f64,
    /// GPU load, 0..1
    pub util: f64,
    /// the fan declares no vital, so it gets no verbs
    pub declare_no_limits: bool,
    /// the GPU's declared critical temperature (warn is 10 below)
    pub gpu_limit_c: f64,
    /// the GPU stops reporting
    pub gpu_silent: bool,
    /// the fan is commanded but does not turn
    pub fan_jammed: bool,
    /// how many granted commands and reflex steps reached the fan
    pub applied: u32,
    pub reflexes: u32,
}

impl SimNerve {
    pub fn new(seed: u8) -> SimNerve {
        SimNerve {
            fan_key: BrainKey::from_seed(&[seed; 32]),
            gpu_key: BrainKey::from_seed(&[seed.wrapping_add(1); 32]),
            duty: 40.0,
            temp_c: 45.0,
            util: 0.2,
            declare_no_limits: false,
            gpu_limit_c: 95.0,
            gpu_silent: false,
            fan_jammed: false,
            applied: 0,
            reflexes: 0,
        }
    }

    pub fn fan(&self) -> Found {
        Found {
            handle: FAN,
            key: self.fan_key.public(),
            label: Text::from_str_truncating("sim fan"),
        }
    }

    pub fn gpu(&self) -> Found {
        Found {
            handle: GPU,
            key: self.gpu_key.public(),
            label: Text::from_str_truncating("sim gpu"),
        }
    }

    fn turning(&self) -> f64 {
        if self.fan_jammed {
            0.0
        } else {
            self.duty
        }
    }

    /// Advance the thermal model by `ms` milliseconds.
    pub fn advance(&mut self, ms: u32) {
        let dt = ms as f64 / 1000.0;
        let heating = 30.0 * self.util;
        let cooling = (0.02 + 0.006 * self.turning()) * (self.temp_c - AMBIENT_C);
        self.temp_c += dt * (heating - cooling);
    }
}

fn verb(s: &'static str) -> Verb {
    Verb::literal(s)
}

impl Nerve for SimNerve {
    fn name(&self) -> &'static str {
        "sim"
    }

    fn enumerate(&mut self) -> Vec<Found> {
        alloc::vec![self.fan(), self.gpu()]
    }

    fn answer(&mut self, part: &Found, c: &Challenge) -> Option<Signature> {
        match part.handle {
            FAN => Some(self.fan_key.answer(c)),
            GPU => Some(self.gpu_key.answer(c)),
            _ => None,
        }
    }

    fn describe(&mut self, part: &Found) -> Option<PartSchema> {
        match part.handle {
            FAN => {
                let mut s = PartSchema::new(PartClass::Thermal, "sim fan", "sim")
                    .verb(
                        VerbSpec::new(verb("fan.set"), RegionClass::Effector, "set the fan duty")
                            .with(ParamSpec::count("duty_pct", 0, 100)),
                    )
                    .verb(
                        VerbSpec::new(
                            verb("fan.regulate"),
                            RegionClass::Effector,
                            "let the fan follow a temperature inside a range",
                        )
                        .with(ParamSpec::count("min", 0, 100))
                        .with(ParamSpec::count("max", 0, 100)),
                    )
                    .sense(SenseSpec::new("fan.duty", Quantity::Fraction, 0.0, 100.0))
                    .sense(SenseSpec::new("fan.rpm", Quantity::Rpm, 0.0, 9000.0))
                    .sense(SenseSpec::new("fan.stall", Quantity::Flag, 0.0, 1.0));
                if !self.declare_no_limits {
                    s = s.vital(VitalSpec::new(
                        "fan.stall",
                        VitalKind::Faults,
                        Band::rising(0.5, 1.0),
                    ));
                }
                Some(s)
            }
            GPU => Some(
                PartSchema::new(PartClass::Compute, "sim gpu", "sim")
                    .made_of(Substrate::Gpu)
                    .sense(SenseSpec::new("temp.c", Quantity::Temperature, -20.0, 150.0))
                    .sense(SenseSpec::new("util", Quantity::Fraction, 0.0, 1.0))
                    .vital(VitalSpec::new(
                        "temp.c",
                        VitalKind::Thermal,
                        Band::rising(self.gpu_limit_c - 10.0, self.gpu_limit_c),
                    )),
            ),
            _ => None,
        }
    }

    fn read(&mut self, part: &Found, _at: Instant, out: &mut Vec<Sensed>) {
        match part.handle {
            FAN => {
                out.push(Sensed::new("fan.duty", self.duty));
                out.push(Sensed::new("fan.rpm", self.turning() * 60.0));
                let stalled = self.fan_jammed && self.duty > 0.0;
                out.push(Sensed::new("fan.stall", if stalled { 1.0 } else { 0.0 }));
            }
            GPU if !self.gpu_silent => {
                out.push(Sensed::new("temp.c", self.temp_c));
                out.push(Sensed::new("util", self.util));
            }
            _ => {}
        }
    }

    fn apply(&mut self, part: &Found, m: &Mandate) -> Result<(), NerveFault> {
        if part.handle != FAN {
            return Err(NerveFault::NoSuchPart);
        }
        match m.verb().as_str() {
            "fan.set" => {
                self.duty = m.scalar("duty_pct").ok_or(NerveFault::NotSupported)?;
                self.applied += 1;
                Ok(())
            }
            // installing a regulator changes nothing on the bench by
            // itself: the homeostat's steps arrive through `reflex`
            "fan.regulate" => {
                self.applied += 1;
                Ok(())
            }
            _ => Err(NerveFault::NotSupported),
        }
    }

    fn reflex(&mut self, part: &Found, act: &ReflexAct) -> Result<(), NerveFault> {
        if part.handle != FAN || act.param() != "duty_pct" {
            return Err(NerveFault::NotSupported);
        }
        self.duty = act.value().clamp(0.0, 100.0);
        self.reflexes += 1;
        Ok(())
    }

    fn safe(&mut self) {
        self.duty = 100.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn under_load_a_stopped_fan_overheats_and_a_running_one_does_not() {
        let mut hot = SimNerve::new(1);
        hot.util = 1.0;
        hot.duty = 0.0;
        let mut cooled = SimNerve::new(1);
        cooled.util = 1.0;
        cooled.duty = 100.0;
        for _ in 0..600 {
            hot.advance(1000);
            cooled.advance(1000);
        }
        assert!(hot.temp_c > 95.0, "stopped fan settled at {}", hot.temp_c);
        assert!(cooled.temp_c < 85.0, "full fan settled at {}", cooled.temp_c);
    }

    #[test]
    fn the_bench_describes_two_well_formed_parts() {
        let mut n = SimNerve::new(1);
        for f in n.enumerate() {
            assert_eq!(n.describe(&f).unwrap().check(), Ok(()));
        }
        assert_eq!(n.describe(&n.fan()).unwrap().actionable().len(), 2);
        n.declare_no_limits = true;
        assert!(n.describe(&n.fan()).unwrap().actionable().is_empty());
    }

    #[test]
    fn safe_is_full_cooling() {
        let mut n = SimNerve::new(1);
        n.duty = 0.0;
        n.safe();
        assert_eq!(n.duty, 100.0);
    }
}
