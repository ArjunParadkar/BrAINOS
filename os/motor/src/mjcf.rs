//! BFS  LEVEL    Domain 2 Autonomic - motor memory (self-model, mandates, primitives, reflexes)  [crate brainos-motor]
//! BFS  PURPOSE  Writes a BodyModel out as MuJoCo MJCF text, with every assumed number listed in a comment at the top, and hashes the result
//! BFS  DEFINES  Drive, Emit, emit, digest
//! BFS  USES     this crate: model | brainos: crypto
//! BFS  USED BY  host/motor, motor/lib, motor/tests/selfmodel, nostd/lib
//!
//! The emitter: a `BodyModel` in, MJCF text out.
//!
//! It runs with no operating system, because it is the entity's own
//! description of its body and belongs with the entity. It writes only
//! what the model carries. Collision is body-against-world only: links
//! of one body do not collide with each other, which is stated in the
//! output and is a limit of every result obtained in it.

use crate::model::{Base, BodyModel, JointKind, ModelError, Shape};
use alloc::string::String;
use brainos_crypto::sha256::{Hash, Sha256};
use core::fmt::Write;

/// How joints are driven in the emitted model.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Drive {
    /// torque (or force) commanded directly, clipped to the joint's
    /// effort; the body runtime closes its own loop
    Effort,
    /// a position servo inside the simulator, for bodies whose real
    /// actuators are position servos (a hobby servo is one).
    /// `kp` in N·m/rad, `kv` in N·m·s/rad
    Servo { kp: f32, kv: f32 },
}

/// Knobs that are about the simulation rather than the body.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Emit {
    pub drive: Drive,
    pub timestep_s: f32,
    /// joint damping, N·m·s/rad; an assumption, and listed as one
    pub damping: f32,
    /// armature (reflected rotor inertia), kg·m²; an assumption
    pub armature: f32,
    pub friction: f32,
    /// add a ground plane and a light, so the file loads on its own
    pub with_ground: bool,
}

impl Default for Emit {
    fn default() -> Emit {
        Emit {
            drive: Drive::Effort,
            timestep_s: 0.002,
            damping: 0.01,
            armature: 0.0001,
            friction: 1.0,
            with_ground: true,
        }
    }
}

fn v3(out: &mut String, v: &[f32; 3]) {
    let _ = write!(out, "{:.6} {:.6} {:.6}", v[0], v[1], v[2]);
}

fn indent(out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push_str("  ");
    }
}

fn body(m: &BodyModel, e: &Emit, i: usize, depth: usize, out: &mut String) {
    let l = &m.links[i];
    indent(out, depth);
    let _ = write!(out, "<body name=\"{}\" pos=\"", l.name);
    if i == 0 {
        let z = match m.base {
            Base::Fixed => 0.0,
            Base::Floating { height_m } => height_m,
        };
        v3(out, &[l.origin[0], l.origin[1], l.origin[2] + z]);
    } else {
        v3(out, &l.origin);
    }
    let _ = writeln!(
        out,
        "\" quat=\"{:.8} {:.8} {:.8} {:.8}\">",
        l.quat[0], l.quat[1], l.quat[2], l.quat[3]
    );
    if i == 0 {
        if let Base::Floating { .. } = m.base {
            indent(out, depth + 1);
            out.push_str("<freejoint name=\"root\"/>\n");
        }
    }
    if let Some(j) = &l.joint {
        indent(out, depth + 1);
        let kind = match j.kind {
            JointKind::Hinge => "hinge",
            JointKind::Slide => "slide",
        };
        let _ = write!(out, "<joint name=\"{}\" type=\"{}\" axis=\"", j.name, kind);
        v3(out, &j.axis);
        out.push_str("\" pos=\"");
        v3(out, &j.anchor);
        let _ = write!(
            out,
            "\" range=\"{:.6} {:.6}\" damping=\"{:.6}\" armature=\"{:.6}\"",
            j.min, j.max, e.damping, e.armature
        );
        if j.stiffness > 0.0 {
            let _ = write!(
                out,
                " stiffness=\"{:.6}\" springref=\"{:.6}\"",
                j.stiffness, j.spring_ref
            );
        }
        out.push_str("/>\n");
    }
    if let Some(n) = &l.inertial {
        indent(out, depth + 1);
        out.push_str("<inertial pos=\"");
        v3(out, &n.com);
        let _ = write!(
            out,
            "\" quat=\"{:.8} {:.8} {:.8} {:.8}\" mass=\"{:.8}\" diaginertia=\"{:.10} {:.10} {:.10}\"/>\n",
            n.quat[0], n.quat[1], n.quat[2], n.quat[3],
            l.mass.value(), n.diag[0], n.diag[1], n.diag[2]
        );
    }
    // with explicit mass properties the solid is for contact and looks
    // only, and must not be weighed a second time
    let mass = if l.inertial.is_some() { 0.0 } else { l.mass.value() };
    match l.shape {
        Shape::Rod { radius } => {
            indent(out, depth + 1);
            let _ = write!(out, "<geom name=\"{}_g\" type=\"capsule\" fromto=\"0 0 0 ", l.name);
            v3(out, &l.tip);
            let _ = writeln!(out, "\" size=\"{:.6}\" mass=\"{:.8}\"/>", radius, mass);
        }
        Shape::Block { half } => {
            indent(out, depth + 1);
            let _ = write!(out, "<geom name=\"{}_g\" type=\"box\" size=\"", l.name);
            v3(out, &half);
            let _ = writeln!(out, "\" mass=\"{:.8}\"/>", mass);
        }
        Shape::None => {}
    }
    if l.touches {
        indent(out, depth + 1);
        let r = match l.shape {
            Shape::Rod { radius } => radius,
            _ => 0.003,
        };
        let _ = write!(out, "<geom name=\"{}_pad\" type=\"sphere\" pos=\"", l.name);
        v3(out, &l.tip);
        let _ = writeln!(
            out,
            "\" size=\"{:.6}\" mass=\"0\" friction=\"{:.4} 0.005 0.0001\"/>",
            r * 1.05,
            e.friction
        );
    }
    for s in m.sites.iter().filter(|s| s.link == i) {
        indent(out, depth + 1);
        let _ = write!(out, "<site name=\"{}\" pos=\"", s.name);
        v3(out, &s.pos);
        out.push_str("\" size=\"0.002\"/>\n");
    }
    for c in m.children(i) {
        body(m, e, c, depth + 1, out);
    }
    indent(out, depth);
    out.push_str("</body>\n");
}

/// Write the model as MJCF. Refuses a model that does not validate.
pub fn emit(m: &BodyModel, e: &Emit) -> Result<String, ModelError> {
    m.validate()?;
    let mut out = String::new();
    out.push_str("<!-- GENERATED by brainos-motor from the body's own description. Do not edit.\n");
    let _ = writeln!(out, "     body: {}", m.name);
    out.push_str("     collision: body against world only; links of this body do not collide with each other\n");
    let _ = writeln!(
        out,
        "     simulation assumptions: damping {:.6}, armature {:.6}, friction {:.4}",
        e.damping, e.armature, e.friction
    );
    for a in m.assumptions() {
        let _ = writeln!(out, "     ASSUMED: {}", a);
    }
    for o in &m.omits {
        let _ = writeln!(out, "     OMITTED: {}", o);
    }
    out.push_str("-->\n");
    let _ = writeln!(out, "<mujoco model=\"{}\">", m.name);
    out.push_str("  <compiler angle=\"degree\" autolimits=\"true\"/>\n");
    let _ = writeln!(out, "  <option timestep=\"{:.6}\"/>", e.timestep_s);
    out.push_str("  <default>\n    <geom contype=\"1\" conaffinity=\"0\" condim=\"4\"/>\n  </default>\n");
    out.push_str("  <worldbody>\n");
    if e.with_ground {
        out.push_str("    <light pos=\"0 0 2\" dir=\"0 0 -1\"/>\n");
        let _ = writeln!(
            out,
            "    <geom name=\"ground\" type=\"plane\" size=\"0 0 0.05\" contype=\"0\" conaffinity=\"1\" friction=\"{:.4} 0.005 0.0001\"/>",
            e.friction
        );
    }
    body(m, e, 0, 2, &mut out);
    out.push_str("  </worldbody>\n");

    let followers: alloc::vec::Vec<_> = m
        .joints()
        .filter_map(|(_, j)| j.follows.map(|(leader, ratio)| (j, leader, ratio)))
        .collect();
    if !followers.is_empty() {
        out.push_str("  <equality>\n");
        for (j, leader, ratio) in followers {
            let lead = m.links[leader].joint.as_ref().map(|x| x.name.as_str()).unwrap_or("");
            let _ = writeln!(
                out,
                "    <joint joint1=\"{}\" joint2=\"{}\" polycoef=\"0 {:.6} 0 0 0\"/>",
                j.name, lead, ratio
            );
        }
        out.push_str("  </equality>\n");
    }

    out.push_str("  <actuator>\n");
    for (_, j) in m.actuated() {
        let f = j.effort.value();
        match e.drive {
            Drive::Effort => {
                let _ = writeln!(
                    out,
                    "    <motor name=\"{}\" joint=\"{}\" ctrlrange=\"{:.6} {:.6}\"/>",
                    j.name, j.name, -f, f
                );
            }
            Drive::Servo { kp, kv } => {
                // a position actuator's control is the joint coordinate
                // in the simulator's own unit, radians for a hinge, and
                // `angle="degree"` does not convert its range
                let k = match j.kind {
                    JointKind::Hinge => core::f32::consts::PI / 180.0,
                    JointKind::Slide => 1.0,
                };
                let _ = writeln!(
                    out,
                    "    <position name=\"{}\" joint=\"{}\" kp=\"{:.6}\" kv=\"{:.6}\" ctrlrange=\"{:.6} {:.6}\" forcerange=\"{:.6} {:.6}\"/>",
                    j.name, j.name, kp, kv, j.min * k, j.max * k, -f, f
                );
            }
        }
    }
    out.push_str("  </actuator>\n</mujoco>\n");
    Ok(out)
}

/// The identity of a generated model: the hash of its text.
pub fn digest(mjcf: &str) -> Hash {
    let mut h = Sha256::new();
    h.update(b"brainos.motor.selfmodel.v1");
    h.update(mjcf.as_bytes());
    h.finish()
}
