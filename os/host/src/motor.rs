//! BFS  LEVEL    Host - the only crate with an operating system (CLI, scenarios, OS nerves)  [crate brainos-host]
//! BFS  PURPOSE  The `brainos motor` commands: print a body's generated MJCF self-model, and print reference kinematics for checking it against a simulator
//! BFS  DEFINES  Lcg, read_skeleton, body_model, emit_options, cli
//! BFS  USES     brainos: bodyd, miniblur-core, motor | external: std
//! BFS  USED BY  -
//!
//! `brainos motor ...`: the motor-memory commands.
//!
//!   brainos motor self-model hexapod|planar-arm|desk-arm [--servo]
//!   brainos motor reference  hexapod|planar-arm|desk-arm N SEED

use brainos_bodyd::arm::{Arm, ArmConfig, MAX_JOINTS};
use brainos_bodyd::deskarm::{DeskArmConfig, Tool};
use brainos_bodyd::hexapod::HexapodConfig;
use brainos_bodyd::selfmodel;
use brainos_motor::mjcf::{digest, emit, Drive, Emit};
use brainos_motor::model::BodyModel;
use miniblur_core::servo::ServoTable;
use miniblur_core::LegId;
use std::process::ExitCode;

/// A small deterministic generator, so a reference run can be repeated
/// from its seed. Not for anything that needs to be unpredictable.
pub struct Lcg(pub u64);

impl Lcg {
    pub fn unit(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 40) as f32) / ((1u64 << 24) as f32)
    }

    pub fn between(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }
}

/// Read a skeleton file (written by `motor/blur_skeleton.py` from a
/// reference twin) into the common model. One record per line:
///
///   link NAME PARENT px py pz qw qx qy qz MASS cx cy cz iw ix iy iz d0 d1 d2
///   joint NAME ax ay az px py pz MIN MAX EFFORT STIFF SPRINGREF DRIVEN
///   follow JOINT LEADER RATIO
///   omit TEXT...
///
/// A `joint` line belongs to the `link` line before it.
pub fn read_skeleton(text: &str) -> Result<BodyModel, String> {
    use brainos_motor::model::{Base, Inertial, JointSpec, Link, Num};
    let mut m = BodyModel::new("skeleton", Base::Floating { height_m: 0.0 });
    let mut follows: Vec<(String, String, f32)> = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let f: Vec<&str> = line.split_whitespace().collect();
        let num = |i: usize| -> Result<f32, String> {
            f.get(i)
                .and_then(|s| s.parse::<f32>().ok())
                .ok_or_else(|| format!("line {}: field {} is not a number", n + 1, i))
        };
        match f.first().copied() {
            Some("name") => m.name = f.get(1).unwrap_or(&"skeleton").to_string(),
            Some("link") if f.len() == 21 => {
                let parent: i64 = f[2].parse().map_err(|_| format!("line {}: parent", n + 1))?;
                let l = Link::new(f[1], if parent < 0 { None } else { Some(parent as usize) })
                    .at([num(3)?, num(4)?, num(5)?])
                    .turned([num(6)?, num(7)?, num(8)?, num(9)?])
                    .weighing(Num::Stated(num(10)?))
                    .with_inertial(Inertial {
                        com: [num(11)?, num(12)?, num(13)?],
                        quat: [num(14)?, num(15)?, num(16)?, num(17)?],
                        diag: [num(18)?, num(19)?, num(20)?],
                    });
                m.push(l);
            }
            Some("joint") if f.len() == 14 => {
                let mut j = JointSpec::hinge(f[1], [num(2)?, num(3)?, num(4)?], num(8)?, num(9)?)
                    .anchored([num(5)?, num(6)?, num(7)?])
                    .effort(Num::Stated(num(10)?));
                j.stiffness = num(11)?;
                j.spring_ref = num(12)?;
                j.actuated = num(13)? > 0.5;
                let last = m.links.len().checked_sub(1).ok_or("joint before any link")?;
                m.links[last].joint = Some(j);
            }
            Some("follow") if f.len() == 4 => {
                follows.push((f[1].to_string(), f[2].to_string(), num(3)?));
            }
            Some("omit") => m.omit(&f[1..].join(" ")),
            None => {}
            Some(other) => return Err(format!("line {}: unknown record `{other}`", n + 1)),
        }
    }
    for (who, leader, ratio) in follows {
        let find = |name: &str| {
            m.links
                .iter()
                .position(|l| l.joint.as_ref().map(|j| j.name == name).unwrap_or(false))
        };
        let (Some(a), Some(b)) = (find(&who), find(&leader)) else {
            return Err(format!("follow: no joint `{who}` or `{leader}`"));
        };
        if let Some(j) = m.links[a].joint.as_mut() {
            j.follows = Some((b, ratio));
        }
    }
    Ok(m)
}

pub fn body_model(name: &str) -> Option<BodyModel> {
    if let Some(path) = name.strip_prefix("skeleton:") {
        let text = std::fs::read_to_string(path).ok()?;
        return match read_skeleton(&text) {
            Ok(m) => Some(m),
            Err(e) => {
                eprintln!("{path}: {e}");
                None
            }
        };
    }
    match name {
        "hexapod" => Some(selfmodel::hexapod(
            &HexapodConfig::default(),
            &ServoTable::bom_10_servo(),
        )),
        "hexapod-as-stated" => Some(selfmodel::hexapod_with(
            &HexapodConfig::default(),
            &ServoTable::bom_10_servo(),
            selfmodel::Shin::KeptVertical,
        )),
        "planar-arm" => Some(selfmodel::planar_arm(&ArmConfig::collaborative_six_axis())),
        "desk-arm" => Some(selfmodel::desk_arm(&DeskArmConfig::default())),
        _ => None,
    }
}

/// How each body is driven in its generated model.
pub fn emit_options(name: &str, servo: bool) -> Emit {
    let mut e = Emit::default();
    if name.starts_with("skeleton:") {
        // a skeleton carries the twin's own numbers; add nothing
        e.damping = 0.0;
        e.armature = 0.0;
        e.timestep_s = 0.0005;
    }
    if name.starts_with("hexapod") {
        // millimetre-scale contact needs a shorter step
        e.timestep_s = 0.001;
        e.damping = 0.002;
        e.armature = 0.00002;
    }
    if servo {
        e.drive = match name {
            n if n.starts_with("hexapod") => Drive::Servo { kp: 2.0, kv: 0.02 },
            _ => Drive::Servo { kp: 40.0, kv: 2.0 },
        };
    }
    e
}

fn self_model(rest: &[String]) -> ExitCode {
    let Some(name) = rest.first() else {
        eprintln!("usage: brainos motor self-model hexapod|planar-arm|desk-arm [--servo]");
        return ExitCode::FAILURE;
    };
    let servo = rest.iter().any(|a| a == "--servo");
    let Some(m) = body_model(name) else {
        eprintln!("unknown body `{name}`");
        return ExitCode::FAILURE;
    };
    match emit(&m, &emit_options(name, servo)) {
        Ok(text) => {
            let mut hex = [0u8; 64];
            digest(&text).write_hex(&mut hex);
            eprintln!(
                "{}: {} links, {} driven joints, {:.3} kg, {} assumed numbers, digest {}",
                m.name,
                m.links.len(),
                m.actuated().count(),
                m.total_mass(),
                m.assumptions().len(),
                core::str::from_utf8(&hex[..16]).unwrap_or("")
            );
            print!("{text}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("the model does not validate: {e:?}");
            ExitCode::FAILURE
        }
    }
}

/// Reference kinematics from the mechanism's own code, one JSON object
/// per line: the pose asked for and where the mechanism says things are.
fn reference(rest: &[String]) -> ExitCode {
    let (Some(name), Some(n), Some(seed)) = (
        rest.first(),
        rest.get(1).and_then(|s| s.parse::<usize>().ok()),
        rest.get(2).and_then(|s| s.parse::<u64>().ok()),
    ) else {
        eprintln!("usage: brainos motor reference BODY N SEED");
        return ExitCode::FAILURE;
    };
    let mut r = Lcg(seed);
    match name.as_str() {
        "hexapod" => {
            let cfg = HexapodConfig::default();
            let m = body_model("hexapod").unwrap();
            // limits per joint, by name, from the generated model: the
            // check that they equal the servo table is separate
            for _ in 0..n {
                let mut q = Vec::new();
                let mut feet = Vec::new();
                let mut rigid = Vec::new();
                let mut swing = [0.0f32; 6];
                let mut lift = [0.0f32; 6];
                for (i, leg) in LegId::ALL.iter().enumerate() {
                    for (_, j) in m.joints() {
                        if j.name == format!("{}_swing", leg.name()) {
                            swing[i] = r.between(j.min, j.max);
                        }
                        if j.name == format!("{}_lift", leg.name()) {
                            lift[i] = r.between(j.min, j.max);
                        }
                    }
                }
                // a linked pair shares one angle
                for (li, l) in m.links.iter().enumerate() {
                    let _ = li;
                    if let Some(j) = &l.joint {
                        if let Some((leader, _)) = j.follows {
                            let lead = m.links[leader].joint.as_ref().unwrap().name.clone();
                            let a = LegId::ALL.iter().position(|x| j.name.starts_with(x.name())).unwrap();
                            let b = LegId::ALL.iter().position(|x| lead.starts_with(x.name())).unwrap();
                            swing[a] = swing[b];
                        }
                    }
                }
                for (i, leg) in LegId::ALL.iter().enumerate() {
                    let (xy, z) = cfg.geom.foot(*leg, swing[i], lift[i]);
                    q.push(format!("\"{}_swing\":{},\"{}_lift\":{}", leg.name(), swing[i], leg.name(), lift[i]));
                    feet.push(format!("\"{}_foot\":[{},{},{}]", leg.name(), xy.x * 0.001, xy.y * 0.001, z * 0.001));
                    // the same leg with the shin fixed to the thigh, worked
                    // out here rather than taken from the generated model
                    let g = &cfg.geom;
                    let (mount, yaw0) = g.mount(*leg);
                    let (yaw, l) = ((yaw0 + swing[i]).to_radians(), lift[i].to_radians());
                    let reach = g.coxa_mm + g.femur_mm * l.cos() + g.tibia_mm * l.sin();
                    let zr = g.femur_mm * l.sin() - g.tibia_mm * l.cos();
                    rigid.push(format!(
                        "\"{}_foot\":[{},{},{}]",
                        leg.name(),
                        (mount.x + reach * yaw.cos()) * 0.001,
                        (mount.y + reach * yaw.sin()) * 0.001,
                        zr * 0.001
                    ));
                }
                println!(
                    "{{\"q\":{{{}}},\"sites\":{{{}}},\"rigid_sites\":{{{}}}}}",
                    q.join(","),
                    feet.join(","),
                    rigid.join(",")
                );
            }
        }
        "planar-arm" => {
            let cfg = ArmConfig::collaborative_six_axis();
            let arm = Arm::new(cfg);
            for _ in 0..n {
                let mut pose = [0.0f32; MAX_JOINTS];
                let mut q = Vec::new();
                for i in 0..cfg.used {
                    pose[i] = r.between(cfg.joints[i].min_deg.max(-180.0), cfg.joints[i].max_deg.min(180.0));
                    q.push(format!("\"j{i}\":{}", pose[i]));
                }
                let (x, y) = arm.end_effector(&pose);
                println!(
                    "{{\"q\":{{{}}},\"sites\":{{\"tool\":[{},0,{}]}},\"shoulder_nm\":{}}}",
                    q.join(","),
                    x,
                    y,
                    arm.shoulder_torque_nm(&pose)
                );
            }
        }
        "desk-arm" => {
            let c = DeskArmConfig::default();
            let names = ["pan", "shoulder", "elbow", "wrist", "roll"];
            for _ in 0..n {
                let mut pose = [0.0f32; 6];
                let mut q = Vec::new();
                for i in 0..5 {
                    pose[i] = r.between(c.limits[i].0, c.limits[i].1);
                    q.push(format!("\"{}\":{}", names[i], pose[i]));
                }
                pose[5] = r.between(0.0, c.grip_max_m);
                q.push(format!("\"grip\":{}", pose[5]));
                let Tool { pos, pitch_deg, finger_yaw_deg } = c.tool(&pose);
                println!(
                    "{{\"q\":{{{}}},\"sites\":{{\"grasp\":[{},{},{}]}},\"pitch_deg\":{},\"finger_yaw_deg\":{}}}",
                    q.join(","), pos[0], pos[1], pos[2], pitch_deg, finger_yaw_deg
                );
            }
        }
        other => {
            eprintln!("unknown body `{other}`");
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}

pub fn cli(rest: &[String]) -> ExitCode {
    match rest.first().map(|s| s.as_str()) {
        Some("self-model") => self_model(&rest[1..]),
        Some("reference") => reference(&rest[1..]),
        _ => {
            eprintln!("brainos motor self-model BODY [--servo]\nbrainos motor reference BODY N SEED");
            ExitCode::FAILURE
        }
    }
}
