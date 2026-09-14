//! §8 step 3 — the capability schema the body publishes in its hello, and
//! step 5 — the calibration record the seat keeps in the state graph. Both
//! are one JSON object (Execution Plan §4.6). The body emits it; the seat
//! reads it back with the same scanner; nothing else parses it.

use crate::gait::GaitParams;
use crate::geometry::LegGeom;
use crate::json;
use crate::power::PowerLimits;
use crate::servo::{ServoCal, ServoTable};
use crate::{Axis, LegId};
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;

pub const SCHEMA_VERSION: u32 = 1;

/// The verbs a Mini-Blur offers. Mirrored in the seat's OFFERABLE list.
pub const HANDS: &[&str] = &[
    "gait.stand",
    "gait.sit",
    "gait.walk",
    "gait.stop",
    "servo.set",
    "body.telemetry",
    "body.calibrate",
    "body.schema",
];

/// Everything that describes this body's envelope.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub power: PowerLimits,
    pub lost_link_ms: u32,
    pub tipover_margin_mm: f32,
    pub max_steps: u16,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits { power: PowerLimits::default(), lost_link_ms: 500, tipover_margin_mm: 8.0, max_steps: 20 }
    }
}

/// The body model: the schema is this, written down.
#[derive(Clone, Copy, Debug)]
pub struct Model {
    pub table: ServoTable,
    pub geom: LegGeom,
    pub gait: GaitParams,
    pub limits: Limits,
}

impl Default for Model {
    fn default() -> Model {
        Model {
            table: ServoTable::bom_10_servo(),
            geom: LegGeom::default(),
            gait: GaitParams::default(),
            limits: Limits::default(),
        }
    }
}

impl Model {
    /// The hello's `schema` object.
    pub fn schema_json(&self) -> String {
        let mut o = String::new();
        let _ = write!(o, "{{\"v\":{SCHEMA_VERSION},\"body\":\"hexapod\",\"joints\":[");
        for (i, c) in self.table.channels().iter().enumerate() {
            if i > 0 {
                o.push(',');
            }
            let j = c.joints[0].unwrap();
            let _ = write!(
                o,
                "{{\"ch\":{},\"leg\":\"{}\",\"axis\":\"{}\",\"min\":{:.1},\"max\":{:.1},\"center\":{:.1},\"dir\":{}",
                c.ch, j.0.name(), j.1.name(), c.min_deg, c.max_deg, c.center_deg, c.dir
            );
            if let Some(l) = c.joints[1] {
                let _ = write!(o, ",\"linked\":\"{}\"", l.0.name());
            }
            o.push('}');
        }
        let g = &self.geom;
        let _ = write!(
            o,
            "],\"legs\":{{\"count\":6,\"coxa_mm\":{:.1},\"femur_mm\":{:.1},\"tibia_mm\":{:.1},\"body_w_mm\":{:.1},\"body_l_mm\":{:.1},\"com_x_mm\":{:.1},\"com_y_mm\":{:.1}}}",
            g.coxa_mm, g.femur_mm, g.tibia_mm, g.body_w_mm, g.body_l_mm, g.com.x, g.com.y
        );
        let _ = write!(
            o,
            ",\"gait\":{{\"swing_amp_deg\":{:.1},\"lift_deg\":{:.1},\"cycle_ms\":{},\"samples_per_cycle\":{}}}",
            self.gait.swing_amp_deg, self.gait.lift_deg, self.gait.cycle_ms, self.gait.samples_per_cycle
        );
        let l = &self.limits;
        let _ = write!(
            o,
            ",\"limits\":{{\"rail_amps\":{:.2},\"servo_moving_amps\":{:.2},\"vbat_floor\":{:.2},\"lost_link_ms\":{},\"tipover_margin_mm\":{:.1},\"max_steps\":{}}}",
            l.power.rail_amps, l.power.servo_moving_amps, l.power.vbat_floor, l.lost_link_ms, l.tipover_margin_mm, l.max_steps
        );
        o.push_str(",\"verbs\":{\"gait.walk\":{\"dir\":[\"fwd\",\"back\",\"left\",\"right\"],\"steps\":[1,");
        let _ = write!(o, "{}],\"speed\":[0.1,1.0]}},\"gait.stand\":{{}},\"gait.sit\":{{}},\"gait.stop\":{{}},", l.max_steps);
        o.push_str("\"servo.set\":{\"ch\":[0,11],\"deg\":[0,180]},\"body.telemetry\":{},\"body.calibrate\":{},\"body.schema\":{}}}");
        o
    }

    /// Rebuild a model from a schema object (the seat, from memory or from
    /// the hello). Unknown fields keep their defaults; a schema with no
    /// joints is refused.
    pub fn from_schema_json(s: &str) -> Result<Model, String> {
        let mut m = Model::default();
        let joints = json::get_obj(s, "joints").ok_or_else(|| String::from("schema has no joints"))?;
        let mut cals: Vec<ServoCal> = Vec::new();
        for item in split_array(joints) {
            let ch = json::get_f32(item, "ch").ok_or_else(|| String::from("joint without ch"))? as u8;
            let leg = json::get_str(item, "leg").and_then(|l| LegId::from_name(&l)).ok_or_else(|| String::from("joint without leg"))?;
            let axis = json::get_str(item, "axis").and_then(|a| Axis::from_name(&a)).ok_or_else(|| String::from("joint without axis"))?;
            let mut c = ServoCal::one(ch, leg, axis, json::get_f32(item, "center").unwrap_or(90.0), json::get_f32(item, "dir").unwrap_or(1.0) as i8);
            c.min_deg = json::get_f32(item, "min").unwrap_or(20.0);
            c.max_deg = json::get_f32(item, "max").unwrap_or(160.0);
            if let Some(l) = json::get_str(item, "linked").and_then(|l| LegId::from_name(&l)) {
                c.joints[1] = Some((l, axis));
            }
            cals.push(c);
        }
        if cals.is_empty() {
            return Err(String::from("schema has an empty joint table"));
        }
        m.table = ServoTable::from_slice(&cals);
        if let Some(g) = json::get_obj(s, "legs") {
            if let Some(v) = json::get_f32(g, "coxa_mm") { m.geom.coxa_mm = v }
            if let Some(v) = json::get_f32(g, "femur_mm") { m.geom.femur_mm = v }
            if let Some(v) = json::get_f32(g, "tibia_mm") { m.geom.tibia_mm = v }
            if let Some(v) = json::get_f32(g, "body_w_mm") { m.geom.body_w_mm = v }
            if let Some(v) = json::get_f32(g, "body_l_mm") { m.geom.body_l_mm = v }
            if let Some(v) = json::get_f32(g, "com_x_mm") { m.geom.com.x = v }
            if let Some(v) = json::get_f32(g, "com_y_mm") { m.geom.com.y = v }
        }
        if let Some(g) = json::get_obj(s, "gait") {
            if let Some(v) = json::get_f32(g, "swing_amp_deg") { m.gait.swing_amp_deg = v }
            if let Some(v) = json::get_f32(g, "lift_deg") { m.gait.lift_deg = v }
            if let Some(v) = json::get_f32(g, "cycle_ms") { m.gait.cycle_ms = v as u32 }
            if let Some(v) = json::get_f32(g, "samples_per_cycle") { m.gait.samples_per_cycle = v as u16 }
        }
        if let Some(l) = json::get_obj(s, "limits") {
            if let Some(v) = json::get_f32(l, "rail_amps") { m.limits.power.rail_amps = v }
            if let Some(v) = json::get_f32(l, "servo_moving_amps") { m.limits.power.servo_moving_amps = v }
            if let Some(v) = json::get_f32(l, "vbat_floor") { m.limits.power.vbat_floor = v }
            if let Some(v) = json::get_f32(l, "lost_link_ms") { m.limits.lost_link_ms = v as u32 }
            if let Some(v) = json::get_f32(l, "tipover_margin_mm") { m.limits.tipover_margin_mm = v }
            if let Some(v) = json::get_f32(l, "max_steps") { m.limits.max_steps = v as u16 }
        }
        Ok(m)
    }

    /// A stable fingerprint of the joint layout: the seat uses it to decide
    /// whether a remembered calibration still applies to the body that just
    /// said hello (same channels, legs, axes and linkages).
    pub fn layout_fingerprint(&self) -> String {
        let mut o = String::new();
        for c in self.table.channels() {
            let j = c.joints[0].unwrap();
            let _ = write!(o, "{}:{}{}", c.ch, j.0.name(), j.1.name());
            if let Some(l) = c.joints[1] {
                let _ = write!(o, "+{}", l.0.name());
            }
            o.push(';');
        }
        o
    }

    /// One line for the body map's proprioception.
    pub fn describe(&self) -> String {
        alloc::format!(
            "hexapod, {} servos, {}-step cycle {} ms, envelope ±{:.0}° swing / {:.0}° lift, rail {:.1} A, tip-over margin {:.0} mm, link {} ms",
            self.table.len, self.gait.samples_per_cycle, self.gait.cycle_ms, self.gait.swing_amp_deg, self.gait.lift_deg,
            self.limits.power.rail_amps, self.limits.tipover_margin_mm, self.limits.lost_link_ms
        )
    }
}

/// Split a JSON array's raw slice into its element slices (objects only).
pub fn split_array(arr: &str) -> Vec<&str> {
    let b = arr.as_bytes();
    let mut out = Vec::new();
    let mut i = 1;
    while i < b.len() {
        if b[i] == b'{' {
            let mut depth = 0;
            let mut in_str = false;
            let mut esc = false;
            let start = i;
            while i < b.len() {
                let c = b[i];
                if in_str {
                    if esc { esc = false } else if c == b'\\' { esc = true } else if c == b'"' { in_str = false }
                } else {
                    match c {
                        b'"' => in_str = true,
                        b'{' => depth += 1,
                        b'}' => {
                            depth -= 1;
                            if depth == 0 {
                                out.push(&arr[start..=i]);
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                i += 1;
            }
        }
        i += 1;
    }
    out
}

/// A joint calibration line for the state graph: `miniblur:cal:ch6 = L1+L3 swing center 92.0 min 30 max 150 dir 1`.
pub fn calibration_lines(body: &str, m: &Model) -> Vec<String> {
    let mut v = Vec::new();
    for c in m.table.channels() {
        let j = c.joints[0].unwrap();
        let mut line = alloc::format!("{body}:cal:ch{} = {}", c.ch, j.0.name());
        if let Some(l) = c.joints[1] {
            let _ = write!(line, "+{}", l.0.name());
        }
        let _ = write!(line, " {} center {:.1} min {:.1} max {:.1} dir {}", j.1.name(), c.center_deg, c.min_deg, c.max_deg, c.dir);
        v.push(line);
    }
    let g = &m.geom;
    v.push(alloc::format!(
        "{body}:geom = coxa {:.1} femur {:.1} tibia {:.1} body {:.0}x{:.0} com {:.1},{:.1}",
        g.coxa_mm, g.femur_mm, g.tibia_mm, g.body_w_mm, g.body_l_mm, g.com.x, g.com.y
    ));
    v.push(alloc::format!("{body}:layout = {}", m.layout_fingerprint()));
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_round_trips_through_the_scanner() {
        let m = Model::default();
        let s = m.schema_json();
        assert!(s.starts_with("{\"v\":1"));
        let back = Model::from_schema_json(&s).unwrap();
        assert_eq!(back.table, m.table);
        assert_eq!(back.geom, m.geom);
        assert_eq!(back.gait, m.gait);
        assert_eq!(back.limits.lost_link_ms, m.limits.lost_link_ms);
        assert_eq!(back.layout_fingerprint(), m.layout_fingerprint());
    }

    #[test]
    fn a_recalibrated_joint_survives_the_round_trip() {
        let mut m = Model::default();
        m.table.find_mut(6).unwrap().center_deg = 92.5;
        m.table.find_mut(6).unwrap().min_deg = 30.0;
        let back = Model::from_schema_json(&m.schema_json()).unwrap();
        let c = back.table.find(6).unwrap();
        assert_eq!(c.center_deg, 92.5);
        assert_eq!(c.min_deg, 30.0);
        assert_eq!(c.joints[1].unwrap().0, LegId::L3);
    }

    #[test]
    fn a_schema_without_joints_is_refused() {
        assert!(Model::from_schema_json("{\"v\":1}").is_err());
        assert!(Model::from_schema_json("{\"v\":1,\"joints\":[]}").is_err());
    }

    #[test]
    fn calibration_lines_name_every_channel() {
        let lines = calibration_lines("miniblur", &Model::default());
        assert_eq!(lines.len(), 12);
        assert!(lines[6].starts_with("miniblur:cal:ch6 = L1+L3 swing"));
        assert!(lines[11].starts_with("miniblur:layout = 0:L1lift;"));
    }
}
