//! Calibration — step 5, and where the Memory-Integrity Law bites.
//!
//! Every physical body is slightly wrong. A servo's mechanical zero is
//! not its commanded zero; a leg is 2 mm longer than the CAD; an IMU has
//! a bias. Calibration is the table of corrections that makes a
//! *specific* body usable, and it is learned by trying.
//!
//! The obvious place to keep it is on the body. That is exactly wrong,
//! and the reason is not philosophical: a body that remembers its own
//! calibration is a body whose memory the entity does not control, so
//! replacing a compute board loses it, and a second entity inhabiting
//! that body inherits corrections it never learned. Calibration lives in
//! the state graph and is pushed down at every power-on.
//!
//! The cost is honest: the body is useless for the few hundred
//! milliseconds between link-up and the calibration push. That is why
//! `Acquisition` will not leave step 5 until the push is acknowledged.

use alloc::string::String;
use alloc::vec::Vec;
use brainos_abi::codec::{Decode, Decoder, Encode, Encoder};
use brainos_abi::error::{Error, Result};
use brainos_abi::ids::BodyId;
use brainos_abi::text::Text;
use brainos_abi::time::Tick;

/// The correction for one joint.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct JointTrim {
    /// which servo channel or joint index
    pub channel: u8,
    /// degrees to add to a commanded angle so the joint reaches it
    pub offset_deg: f32,
    /// multiplier on commanded travel, for a linkage that is not 1:1
    pub scale: f32,
    /// the mechanical limits found by probing, which may be tighter than
    /// the datasheet — this is the number that actually stops a fall
    pub min_deg: f32,
    pub max_deg: f32,
    /// true when the limits were measured rather than assumed
    pub probed: bool,
}

impl JointTrim {
    pub fn identity(channel: u8, min_deg: f32, max_deg: f32) -> JointTrim {
        JointTrim {
            channel,
            offset_deg: 0.0,
            scale: 1.0,
            min_deg,
            max_deg,
            probed: false,
        }
    }

    /// Apply the correction to a commanded angle, and clamp to the
    /// measured limits. Clamping here as well as in the envelope is
    /// deliberate: the envelope is what the entity was *granted*, the
    /// trim is what this particular body can *survive*, and they are
    /// different facts that happen to usually agree.
    pub fn apply(&self, commanded_deg: f32) -> f32 {
        if commanded_deg.is_nan() {
            return self.min_deg;
        }
        let v = commanded_deg * self.scale + self.offset_deg;
        v.clamp(self.min_deg, self.max_deg)
    }

    pub fn is_sane(&self) -> bool {
        self.scale.is_finite()
            && self.scale.abs() > 0.01
            && self.offset_deg.is_finite()
            && self.min_deg.is_finite()
            && self.max_deg.is_finite()
            && self.min_deg < self.max_deg
    }
}

impl Encode for JointTrim {
    fn encode(&self, e: &mut Encoder) {
        e.u8(self.channel);
        e.f32(self.offset_deg);
        e.f32(self.scale);
        e.f32(self.min_deg);
        e.f32(self.max_deg);
        e.bool(self.probed);
    }
}

impl Decode for JointTrim {
    fn decode(d: &mut Decoder) -> Result<JointTrim> {
        Ok(JointTrim {
            channel: d.u8()?,
            offset_deg: d.f32()?,
            scale: d.f32()?,
            min_deg: d.f32()?,
            max_deg: d.f32()?,
            probed: d.bool()?,
        })
    }
}

/// Everything the entity has learned about one specific body.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Calibration {
    pub body: BodyId,
    pub joints: Vec<JointTrim>,
    /// sensor biases, by channel name
    pub biases: Vec<(String, f32)>,
    /// when this calibration was established
    pub at: Tick,
    /// how many times it has been confirmed by use without drift
    pub confirmations: u32,
}

impl Calibration {
    pub fn new(body: BodyId, at: Tick) -> Calibration {
        Calibration {
            body,
            joints: Vec::new(),
            biases: Vec::new(),
            at,
            confirmations: 0,
        }
    }

    /// Add or replace a joint's trim. An insane trim is refused — a scale
    /// of zero or inverted limits would silently disable a joint or
    /// invert its direction, and both are discovered by a robot falling.
    pub fn trim(&mut self, t: JointTrim) -> Result<()> {
        if !t.is_sane() {
            return Err(Error::Range("joint trim is not physically sensible"));
        }
        self.joints.retain(|j| j.channel != t.channel);
        self.joints.push(t);
        self.joints.sort_by_key(|j| j.channel);
        Ok(())
    }

    pub fn bias(&mut self, channel: &str, value: f32) -> Result<()> {
        if !value.is_finite() {
            return Err(Error::Range("bias is not finite"));
        }
        let name = alloc::string::ToString::to_string(channel);
        self.biases.retain(|(n, _)| n != &name);
        self.biases.push((name, value));
        self.biases.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(())
    }

    pub fn joint(&self, channel: u8) -> Option<&JointTrim> {
        self.joints
            .binary_search_by_key(&channel, |j| j.channel)
            .ok()
            .map(|i| &self.joints[i])
    }

    pub fn bias_of(&self, channel: &str) -> Option<f32> {
        self.biases
            .iter()
            .find(|(n, _)| n == channel)
            .map(|(_, v)| *v)
    }

    /// How much of this calibration was measured rather than assumed.
    /// An entity should know the difference, because a body running on
    /// assumed limits is a body whose envelope is a guess.
    pub fn probed_fraction_pct(&self) -> u8 {
        if self.joints.is_empty() {
            return 0;
        }
        let probed = self.joints.iter().filter(|j| j.probed).count();
        ((probed * 100) / self.joints.len()) as u8
    }

    /// Is there enough here to move this body?
    ///
    /// A body with no joints — a speaker, a camera, a lab instrument
    /// with a fixed program — has nothing to calibrate and is complete
    /// immediately. A body *with* joints is complete only when every one
    /// of them was probed: an assumed limit on a leg is a limit nobody
    /// measured, and the entity should not be moving on it.
    pub fn is_complete(&self, expected_joints: usize) -> bool {
        if expected_joints == 0 && self.joints.is_empty() {
            return true;
        }
        self.joints.len() >= expected_joints && self.probed_fraction_pct() == 100
    }

    pub fn confirm(&mut self) {
        self.confirmations = self.confirmations.saturating_add(1);
    }

    /// The line written into the state graph. Prefixed with the body's
    /// fingerprint so that reloading for a specific body is a prefix scan
    /// rather than a decode of everything.
    pub fn render(&self) -> String {
        let mut t: Text<240> = Text::new();
        t.push_str("calib ");
        t.push_str(self.body.fingerprint().as_str());
        t.push_str(" joints=");
        t.push_u64(self.joints.len() as u64);
        t.push_str(" probed=");
        t.push_u64(self.probed_fraction_pct() as u64);
        t.push_str("% confirms=");
        t.push_u64(self.confirmations as u64);
        alloc::string::ToString::to_string(t.as_str())
    }
}

impl Encode for Calibration {
    fn encode(&self, e: &mut Encoder) {
        self.body.encode(e);
        e.seq(self.joints.len());
        for j in &self.joints {
            j.encode(e);
        }
        e.seq(self.biases.len());
        for (n, v) in &self.biases {
            e.str(n);
            e.f32(*v);
        }
        self.at.encode(e);
        e.u32(self.confirmations);
    }
}

impl Decode for Calibration {
    fn decode(d: &mut Decoder) -> Result<Calibration> {
        let body = BodyId::decode(d)?;
        let n = d.seq()?;
        let mut joints = Vec::new();
        for _ in 0..n {
            joints.push(JointTrim::decode(d)?);
        }
        let n = d.seq()?;
        let mut biases = Vec::new();
        for _ in 0..n {
            let name = alloc::string::ToString::to_string(d.str()?);
            biases.push((name, d.f32()?));
        }
        Ok(Calibration {
            body,
            joints,
            biases,
            at: Tick::decode(d)?,
            confirmations: d.u32()?,
        })
    }
}
