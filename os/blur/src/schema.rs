//! What BLUR says it can do, and the check that it is telling the truth
//! about its own parts.
//!
//! The verbs are *skill sessions*, not motor commands: KIRA grants
//! "walk at ≤ 0.5 m/s for 60 s", once, and the Autonomic layer holds
//! the body inside that grant at 1 kHz. Joint-level motion exists only
//! as `body.pose`, whose `joint` parameter is an enum of every actuated
//! joint the body claims — which is also how the schema states the
//! body's parts, and therefore how a phantom limb becomes visible: the
//! schema is hashed into the `BodyId`, so a part claimed is a part the
//! body is identified by.

use crate::census::Census;
use crate::descriptor::{actuator_index, ACTUATORS};
use alloc::string::String;
use alloc::vec::Vec;
use brainos_abi::verb::Verb;
use brainos_body::region::RegionClass;
use brainos_body::schema::{CapabilitySchema, ParamKind, ParamSpec, VerbSpec};
use brainos_rt::envelope::Channel;

pub const LABEL: &str = "blur-v36";

/// Declared skill bounds. Day-one limits; Phase 4 widens them only
/// through a new schema (a new BodyId), never by a parameter change.
pub const WALK_VX: (f64, f64) = (-0.25, 0.5);
pub const WALK_VY: (f64, f64) = (-0.15, 0.15);
pub const WALK_YAW: (f64, f64) = (-1.0, 1.0);
pub const STAND_HEIGHT: (f64, f64) = (0.16, 0.30);
pub const HOP_HEIGHT: (f64, f64) = (0.0, 0.06);

/// The schema BLUR's firmware declares, given the joints it believes it
/// has. A correctly built BLUR passes every actuator name.
pub fn schema_with(joints: &[&str]) -> CapabilitySchema {
    let mut s = CapabilitySchema::new(LABEL, "native");
    fn legs(verb: &'static str, summary: &str) -> VerbSpec {
        VerbSpec::new(Verb::literal(verb), RegionClass::Legs, summary)
    }
    s.declare(
        legs("legs.stand", "stand and hold a body height (m)")
            .with(ParamSpec::scalar("height", Channel::Clearance, STAND_HEIGHT.0, STAND_HEIGHT.1))
            .taking_up_to(3_000),
    );
    s.declare(legs("legs.sit", "sit on the haunches").taking_up_to(3_000));
    s.declare(legs("legs.lie", "lie down").taking_up_to(3_000));
    s.declare(legs("legs.standup", "stand up from lying").taking_up_to(4_000));
    s.declare(
        legs("legs.walk", "walk: forward/lateral speed (m/s), yaw rate (rad/s)")
            .with(ParamSpec::scalar("vx", Channel::Speed, WALK_VX.0, WALK_VX.1))
            .with(ParamSpec::scalar("vy", Channel::Speed, WALK_VY.0, WALK_VY.1))
            .with(ParamSpec::scalar("yaw", Channel::Rate, WALK_YAW.0, WALK_YAW.1))
            .taking_up_to(60_000)
            .irreversible(),
    );
    s.declare(
        legs("legs.hop", "small hop, apex clearance (m)")
            .with(ParamSpec::scalar("height", Channel::Clearance, HOP_HEIGHT.0, HOP_HEIGHT.1))
            .taking_up_to(2_000)
            .irreversible(),
    );
    s.declare(
        VerbSpec::new(Verb::literal("body.pose"), RegionClass::Effector, "hold one joint at an angle (rad; winch m)")
            .with(ParamSpec::choice("joint", joints))
            .with(ParamSpec::scalar("angle", Channel::Angle, -3.2, 3.2))
            .taking_up_to(2_000),
    );
    s.declare(
        VerbSpec::new(Verb::literal("tail.grip"), RegionClass::Gripper, "close or open the tail-tip gripper")
            .with(ParamSpec::flag("close"))
            .taking_up_to(2_000),
    );
    s.declare(
        VerbSpec::new(Verb::literal("voice.say"), RegionClass::Speaker, "speak through the speaker")
            .with(ParamSpec::text("text", 240))
            .taking_up_to(20_000),
    );
    s
}

/// The schema of a correctly built BLUR.
pub fn blur_schema() -> CapabilitySchema {
    let names: Vec<&str> = ACTUATORS.iter().map(|a| a.name).collect();
    schema_with(&names)
}

/// The joints a schema claims, from `body.pose`'s enum.
pub fn claimed_joints(s: &CapabilitySchema) -> Vec<String> {
    s.get(&Verb::literal("body.pose"))
        .and_then(|v| v.param("joint"))
        .filter(|p| p.kind == ParamKind::Enum)
        .map(|p| p.choices.clone())
        .unwrap_or_default()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SchemaFault {
    /// the schema claims a joint BLUR v36 does not have
    UnknownJoint(String),
    /// the schema claims a joint whose node did not answer the census
    PhantomLimb { joint: String, node: u8 },
    /// a joint of BLUR v36 is neither claimed nor present — this is not
    /// a whole BLUR
    MissingJoint(String),
    /// no IMU answered: the body would have no inner ear
    NoInnerEar,
    /// the schema has no body.pose at all
    NoParts,
}

/// The schema-stage check: every part claimed is a part that answered,
/// every part BLUR v36 has is claimed, and the inner ear is there.
pub fn check_schema(s: &CapabilitySchema, c: &Census) -> Result<(), SchemaFault> {
    let claimed = claimed_joints(s);
    if claimed.is_empty() {
        return Err(SchemaFault::NoParts);
    }
    for j in &claimed {
        let Some(i) = actuator_index(j) else {
            return Err(SchemaFault::UnknownJoint(j.clone()));
        };
        let a = &ACTUATORS[i];
        if c.get(a.node).is_none() {
            return Err(SchemaFault::PhantomLimb {
                joint: j.clone(),
                node: a.node,
            });
        }
    }
    for a in ACTUATORS.iter() {
        if !claimed.iter().any(|j| j == a.name) {
            return Err(SchemaFault::MissingJoint(alloc::string::ToString::to_string(a.name)));
        }
    }
    if !c.imu_present {
        return Err(SchemaFault::NoInnerEar);
    }
    Ok(())
}
