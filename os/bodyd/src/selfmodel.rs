//! BFS  LEVEL    Domain 2 Autonomic - the runtime on the body's own computer  [crate brainos-bodyd]
//! BFS  PURPOSE  Turns each mechanism's own numbers (hexapod geometry and servo table, planar arm, desk arm) into the common self-model, marking what the source does not state as assumed
//! BFS  DEFINES  HEX_ASSUMED_SERVO_NM, HEX_ASSUMED_SERVO_DPS, HEX_ASSUMED_BODY_KG, HEX_ASSUMED_COXA_KG, HEX_ASSUMED_FEMUR_KG, HEX_ASSUMED_TIBIA_KG, HEX_ASSUMED_BODY_THICK_M, Shin, hexapod, hexapod_with, planar_arm, desk_arm
//! BFS  USES     this crate: arm, deskarm, hexapod | brainos: miniblur-core, motor
//! BFS  USED BY  host/motor
//!
//! Each mechanism describes itself in the common shape.
//!
//! These functions add nothing. Where a mechanism's own numbers are
//! silent the value goes in as `Num::Assumed` and is carried, visibly,
//! into every model generated from it.

use crate::arm::ArmConfig;
use crate::deskarm::DeskArmConfig;
use crate::hexapod::HexapodConfig;
use alloc::format;
use brainos_motor::model::{Base, BodyModel, JointSpec, Link, Num};
use miniblur_core::servo::ServoTable;
use miniblur_core::{Axis, LegId};

/// What the Mini-Blur description does not say, and what is used
/// instead. A 9 g hobby servo: about 0.18 N·m stalled, 60 degrees in
/// about 0.1 s.
pub const HEX_ASSUMED_SERVO_NM: f32 = 0.18;
pub const HEX_ASSUMED_SERVO_DPS: f32 = 600.0;
pub const HEX_ASSUMED_BODY_KG: f32 = 0.20;
pub const HEX_ASSUMED_COXA_KG: f32 = 0.012;
pub const HEX_ASSUMED_FEMUR_KG: f32 = 0.012;
pub const HEX_ASSUMED_TIBIA_KG: f32 = 0.005;
pub const HEX_ASSUMED_BODY_THICK_M: f32 = 0.020;

fn rel_range(table: &ServoTable, leg: LegId, axis: Axis) -> Option<(f32, f32, bool)> {
    for c in table.channels() {
        for (slot, j) in c.joints.iter().enumerate() {
            if *j == Some((leg, axis)) {
                let a = (c.min_deg - c.center_deg) * c.dir as f32;
                let b = (c.max_deg - c.center_deg) * c.dir as f32;
                return Some((a.min(b), a.max(b), slot == 0));
            }
        }
    }
    None
}

fn leader_of(table: &ServoTable, leg: LegId, axis: Axis) -> Option<LegId> {
    for c in table.channels() {
        if c.joints[1] == Some((leg, axis)) {
            return c.joints[0].map(|(l, _)| l);
        }
    }
    None
}

/// What the shin does when the thigh lifts.
///
/// `LegGeom::foot` computes the foot as if the shin stayed vertical at
/// every lift angle (`z = -tibia + femur * sin(lift)`). A leg with two
/// servos has the shin fixed to the thigh, so it swings with it. The two
/// agree only at lift = 0. Generating both is how the difference was
/// found and how large it is can be measured.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shin {
    /// fixed to the thigh, as a two-servo leg is built
    Rigid,
    /// kept vertical by a joint that undoes the lift: the mechanism the
    /// reference kinematics describe, which the bill of materials does
    /// not contain
    KeptVertical,
}

/// The hexapod, from its leg geometry and the servo table the entity
/// pushed down. A joint with no servo is welded at its centre.
pub fn hexapod(config: &HexapodConfig, table: &ServoTable) -> BodyModel {
    hexapod_with(config, table, Shin::Rigid)
}

pub fn hexapod_with(config: &HexapodConfig, table: &ServoTable, shin: Shin) -> BodyModel {
    let g = &config.geom;
    let mm = 0.001f32;
    let mut m = BodyModel::new(
        "miniblur",
        Base::Floating {
            // standing: feet on the ground with every lift at zero
            height_m: g.tibia_mm * mm + 0.003,
        },
    );
    m.tilt_limit_deg = Some(20.0);
    m.omit("link masses, body mass and servo torque: the geometry states none, so all are assumed");
    m.omit("centre-of-mass offset is applied to the body block only");
    if shin == Shin::KeptVertical {
        m.omit("the shin is held vertical by a joint no servo drives: this is the reference kinematics' mechanism, not the built one");
    }
    let base = m.push(
        Link::new("body", None)
            .at([g.com.x * mm, g.com.y * mm, 0.0])
            .block([
                g.body_l_mm * mm * 0.5,
                g.body_w_mm * mm * 0.5,
                HEX_ASSUMED_BODY_THICK_M * 0.5,
            ])
            .weighing(Num::Assumed(HEX_ASSUMED_BODY_KG)),
    );
    // the body block is centred on the centre of mass; mounts are given
    // from the body origin, so shift them back
    let shift = [-g.com.x * mm, -g.com.y * mm, 0.0];
    let mut swing_link = [0usize; 6];
    for (i, leg) in LegId::ALL.iter().enumerate() {
        let (mount, yaw0) = g.mount(*leg);
        let n = leg.name();
        let mut coxa = Link::new(&format!("{n}_coxa"), Some(base))
            .at([mount.x * mm + shift[0], mount.y * mm + shift[1], 0.0])
            .yawed(yaw0)
            .rod([g.coxa_mm * mm, 0.0, 0.0], 0.004)
            .weighing(Num::Assumed(HEX_ASSUMED_COXA_KG));
        if let Some((lo, hi, _)) = rel_range(table, *leg, Axis::Swing) {
            coxa = coxa.jointed(
                JointSpec::hinge(&format!("{n}_swing"), [0.0, 0.0, 1.0], lo, hi)
                    .effort(Num::Assumed(HEX_ASSUMED_SERVO_NM))
                    .rate(Num::Assumed(HEX_ASSUMED_SERVO_DPS)),
            );
        }
        let c = m.push(coxa);
        swing_link[i] = c;
        let mut femur = Link::new(&format!("{n}_femur"), Some(c))
            .at([g.coxa_mm * mm, 0.0, 0.0])
            .rod([g.femur_mm * mm, 0.0, 0.0], 0.004)
            .weighing(Num::Assumed(HEX_ASSUMED_FEMUR_KG));
        if let Some((lo, hi, _)) = rel_range(table, *leg, Axis::Lift) {
            // positive lift raises the foot: a turn about -y
            femur = femur.jointed(
                JointSpec::hinge(&format!("{n}_lift"), [0.0, -1.0, 0.0], lo, hi)
                    .effort(Num::Assumed(HEX_ASSUMED_SERVO_NM))
                    .rate(Num::Assumed(HEX_ASSUMED_SERVO_DPS)),
            );
        }
        let f = m.push(femur);
        let mut tibia = Link::new(&format!("{n}_tibia"), Some(f))
            .at([g.femur_mm * mm, 0.0, 0.0])
            .rod([0.0, 0.0, -g.tibia_mm * mm], 0.003)
            .weighing(Num::Assumed(HEX_ASSUMED_TIBIA_KG))
            .touching();
        if shin == Shin::KeptVertical {
            if let Some(lift) = m.links[f].joint.as_ref() {
                tibia = tibia.jointed(
                    JointSpec::hinge(&format!("{n}_level"), [0.0, -1.0, 0.0], -lift.max, -lift.min)
                        .following(f, -1.0),
                );
            }
        }
        let t = m.push(tibia);
        m.site(&format!("{n}_foot"), t, [0.0, 0.0, -g.tibia_mm * mm]);
    }
    // linked pairs: the second joint on a shared servo follows the first
    for (i, leg) in LegId::ALL.iter().enumerate() {
        if let Some(lead) = leader_of(table, *leg, Axis::Swing) {
            let lead_link = swing_link[lead as usize];
            if let Some(j) = m.links[swing_link[i]].joint.take() {
                m.links[swing_link[i]].joint = Some(j.following(lead_link, 1.0));
            }
        }
    }
    m
}

/// The planar six-axis arm, laid out in the x-z plane with every joint
/// turning about -y, so that a positive angle raises the link exactly as
/// `Arm::joint_positions` has it.
pub fn planar_arm(config: &ArmConfig) -> BodyModel {
    let mut m = BodyModel::new("planar_arm", Base::Fixed);
    m.omit("the arm is modelled in one plane, as its mechanism models it");
    let mut parent = m.push(Link::new("column", None));
    let mut at = [0.0f32; 3];
    for (i, j) in config.joints[..config.used].iter().enumerate() {
        let link = Link::new(&format!("link{i}"), Some(parent))
            .at(at)
            .jointed(
                JointSpec::hinge(&format!("j{i}"), [0.0, -1.0, 0.0], j.min_deg, j.max_deg)
                    .effort(Num::Stated(j.torque_nm))
                    .rate(Num::Stated(config.max_rate_dps)),
            )
            .rod([j.link_m, 0.0, 0.0], 0.02)
            .weighing(Num::Stated(j.mass_kg));
        parent = m.push(link);
        at = [j.link_m, 0.0, 0.0];
    }
    m.site("tool", parent, at);
    m
}

/// The desk arm and its gripper. Every number is assumed, because every
/// number in `DeskArmConfig` is.
pub fn desk_arm(c: &DeskArmConfig) -> BodyModel {
    let mut m = BodyModel::new("desk_arm", Base::Fixed);
    m.omit("every dimension, mass and torque is plausible, not measured");
    let nm = Num::Assumed(c.torque_nm);
    let dps = Num::Assumed(c.rate_dps);
    let pitch = [0.0, -1.0, 0.0];
    let hinge = |name: &str, axis: [f32; 3], i: usize| {
        JointSpec::hinge(name, axis, c.limits[i].0, c.limits[i].1)
            .effort(nm)
            .rate(dps)
    };
    let base = m.push(
        Link::new("pedestal", None)
            .at([0.0, 0.0, c.shoulder_h_m * 0.5])
            .block([0.03, 0.03, c.shoulder_h_m * 0.5 - 0.012])
            .weighing(Num::Assumed(0.3)),
    );
    let turret = m.push(
        Link::new("turret", Some(base))
            .at([0.0, 0.0, c.shoulder_h_m * 0.5])
            .jointed(hinge("pan", [0.0, 0.0, 1.0], 0))
            .rod([0.0, 0.0, -0.01], 0.012)
            .weighing(Num::Assumed(c.link_mass_kg)),
    );
    let upper = m.push(
        Link::new("upper", Some(turret))
            .jointed(hinge("shoulder", pitch, 1))
            .rod([c.upper_m, 0.0, 0.0], 0.012)
            .weighing(Num::Assumed(c.link_mass_kg)),
    );
    let fore = m.push(
        Link::new("fore", Some(upper))
            .at([c.upper_m, 0.0, 0.0])
            .jointed(hinge("elbow", pitch, 2))
            .rod([c.fore_m, 0.0, 0.0], 0.011)
            .weighing(Num::Assumed(c.link_mass_kg)),
    );
    let wrist = m.push(
        Link::new("wristlink", Some(fore))
            .at([c.fore_m, 0.0, 0.0])
            .jointed(hinge("wrist", pitch, 3))
            .rod([c.wrist_m, 0.0, 0.0], 0.010)
            .weighing(Num::Assumed(c.link_mass_kg * 0.5)),
    );
    let hand = m.push(
        Link::new("hand", Some(wrist))
            .at([c.wrist_m, 0.0, 0.0])
            .jointed(hinge("roll", [1.0, 0.0, 0.0], 4))
            .rod([c.palm_m, 0.0, 0.0], 0.010)
            .weighing(Num::Assumed(c.hand_mass_kg)),
    );
    let grip = JointSpec::slide("grip", [0.0, 1.0, 0.0], 0.0, c.grip_max_m)
        .effort(Num::Assumed(c.grip_force_n))
        .rate(Num::Assumed(c.grip_rate_mps));
    let left = m.push(
        Link::new("finger_l", Some(hand))
            .at([c.palm_m, 0.004, 0.0])
            .jointed(grip)
            .rod([c.finger_m, 0.0, 0.0], 0.004)
            .weighing(Num::Assumed(0.015))
            .touching(),
    );
    m.push(
        Link::new("finger_r", Some(hand))
            .at([c.palm_m, -0.004, 0.0])
            .jointed(
                JointSpec::slide("grip_r", [0.0, -1.0, 0.0], 0.0, c.grip_max_m)
                    .following(left, 1.0),
            )
            .rod([c.finger_m, 0.0, 0.0], 0.004)
            .weighing(Num::Assumed(0.015))
            .touching(),
    );
    m.site("grasp", hand, [c.palm_m + 0.6 * c.finger_m, 0.0, 0.0]);
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use brainos_motor::model::ModelError;

    #[test]
    fn every_generated_model_validates() {
        let table = ServoTable::bom_10_servo();
        let hex = hexapod(&HexapodConfig::default(), &table);
        assert_eq!(hex.validate(), Ok::<(), ModelError>(()));
        assert_eq!(planar_arm(&ArmConfig::collaborative_six_axis()).validate(), Ok(()));
        assert_eq!(desk_arm(&DeskArmConfig::default()).validate(), Ok(()));
        assert_eq!(
            hexapod_with(&HexapodConfig::default(), &table, Shin::KeptVertical).validate(),
            Ok(())
        );
    }

    #[test]
    fn hexapod_joint_limits_sit_exactly_on_the_servo_envelope() {
        let table = ServoTable::bom_10_servo();
        let m = hexapod(&HexapodConfig::default(), &table);
        let mut checked = 0;
        for c in table.channels() {
            for j in c.joints.iter().flatten() {
                let name = format!("{}_{}", j.0.name(), j.1.name());
                let (_, spec) = m.joints().find(|(_, s)| s.name == name).expect("joint present");
                for edge in [spec.min, spec.max] {
                    assert!(c.in_envelope(c.servo_deg(edge)), "{name}: {edge} refused by its servo");
                    let beyond = edge + 0.01 * edge.signum();
                    assert!(!c.in_envelope(c.servo_deg(beyond)), "{name}: {beyond} admitted");
                }
                checked += 1;
            }
        }
        assert_eq!(checked, 12, "ten servos drive twelve joints");
    }

    #[test]
    fn ten_servos_mean_ten_driven_joints_and_two_followers() {
        let m = hexapod(&HexapodConfig::default(), &ServoTable::bom_10_servo());
        assert_eq!(m.actuated().count(), 10);
        assert_eq!(m.joints().filter(|(_, j)| j.follows.is_some()).count(), 2);
    }

    #[test]
    fn the_hexapod_says_that_its_masses_and_torques_are_assumed() {
        let m = hexapod(&HexapodConfig::default(), &ServoTable::bom_10_servo());
        // 19 masses, and effort and rate for each of 10 driven joints
        assert_eq!(m.assumptions().len(), 19 + 20);
        let arm = planar_arm(&ArmConfig::collaborative_six_axis());
        // the arm states its own; only the massless column is assumed
        assert_eq!(arm.assumptions().len(), 1);
    }
}
