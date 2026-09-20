//! The native adapter — our own firmware.
//!
//! The reference implementation, and the one the other two are measured
//! against. A body we built declares its schema directly, with real
//! bounds, because we are the ones who measured them.
//!
//! Mini-Blur's numbers are here as the worked example: a hexapod with
//! six legs, MG90S metal-gear servos on a 2S pack through a UBEC, with a
//! servo-angle envelope, a current limit, a tip-over invariant, and
//! stop-on-lost-link. The values are the Phase 0 body's and they are the
//! ones the Domain 2 firmware checks on the other side of the link — the
//! same envelope, checked twice.

use brainos_abi::verb::Verb;
use brainos_body::region::RegionClass;
use brainos_body::schema::{CapabilitySchema, ParamSpec, VerbSpec};
use brainos_rt::envelope::Channel;

/// Build a native schema from a list of (verb, region, summary, params).
pub fn native_schema(label: &str, verbs: &[VerbSpec]) -> CapabilitySchema {
    let mut s = CapabilitySchema::new(label, "native");
    for v in verbs {
        s.declare(v.clone());
    }
    s
}

/// Mini-Blur, the Phase 0 body.
///
/// A chihuahua-scale hexapod: ten MG90S servos on six legs, a 2S LiPo
/// through a matched UBEC to a 5 V servo rail isolated from logic, an
/// INMP441 microphone and a MAX98357A amplifier. The angle envelope is
/// the femur's usable travel measured at the joint, not the servo's
/// datasheet range; the current limit is the rail's, not the sum of the
/// servos' stall currents, because they do not all stall at once and
/// budgeting as though they do makes the robot unable to stand.
pub fn mini_blur() -> CapabilitySchema {
    native_schema(
        "mini-blur",
        &[
            VerbSpec::new(
                Verb::literal("legs.walk"),
                RegionClass::Legs,
                "tripod gait, forward",
            )
            .with(ParamSpec::scalar("speed", Channel::Speed, 0.0, 0.12))
            .with(ParamSpec::scalar("heading", Channel::Angle, -30.0, 30.0))
            .taking_up_to(10_000)
            .irreversible(),
            VerbSpec::new(
                Verb::literal("legs.stand"),
                RegionClass::Legs,
                "stand and hold",
            )
            .with(ParamSpec::scalar("height", Channel::Angle, -20.0, 20.0))
            .taking_up_to(2_000),
            VerbSpec::new(
                Verb::literal("legs.sit"),
                RegionClass::Legs,
                "lower to the ground",
            )
            .with(ParamSpec::scalar("height", Channel::Angle, -35.0, 0.0))
            .taking_up_to(2_000),
            VerbSpec::new(
                Verb::literal("legs.turn"),
                RegionClass::Legs,
                "turn in place",
            )
            .with(ParamSpec::scalar("rate", Channel::Rate, -40.0, 40.0))
            .taking_up_to(5_000)
            .irreversible(),
            VerbSpec::new(
                Verb::literal("voice.say"),
                RegionClass::Speaker,
                "speak through the amplifier",
            )
            .with(ParamSpec::text("text", 240))
            .taking_up_to(20_000),
        ],
    )
}

/// The ThinkPad — the Stage 2 body, which is a body like any other.
pub fn workstation() -> CapabilitySchema {
    native_schema(
        "thinkpad",
        &[
            VerbSpec::new(
                Verb::literal("screen.say"),
                RegionClass::Screen,
                "render text",
            )
            .with(ParamSpec::text("text", 2_000)),
            VerbSpec::new(
                Verb::literal("fs.read"),
                RegionClass::Storage,
                "read a file from the world volume",
            )
            .with(ParamSpec::text("path", 240)),
            VerbSpec::new(
                Verb::literal("fs.write"),
                RegionClass::Storage,
                "write a file to the world volume",
            )
            .with(ParamSpec::text("path", 240))
            .with(ParamSpec::text("content", 4_000))
            .irreversible(),
            VerbSpec::new(
                Verb::literal("voice.say"),
                RegionClass::Speaker,
                "speak",
            )
            .with(ParamSpec::text("text", 2_000)),
        ],
    )
}
