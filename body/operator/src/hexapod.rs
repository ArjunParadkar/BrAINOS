//! Mini-Blur at the seat (Execution Plan §4.5, §4.6): the hexapod's KIRA
//! policy, and its calibration in the state graph.
//!
//! `HexapodPolicy` implements `brainos_mind::kira::BodyPolicy` by delegating
//! to `miniblur_core::policy` — the same functions the board runs in its
//! reflex loop. The seat mirrors the body's live state (battery, mode, how
//! long since it was heard) from the digests that come back over Reach, so
//! the POLICY stage can refuse a walk when the link is stale or the battery
//! is flat before anything is sent.
//!
//! Calibration (§8 step 5) lives in the state graph as semantic nodes:
//!
//!   miniblur:cal:ch6 = L1+L3 swing center 92.5 min 30.0 max 150.0 dir 1
//!   miniblur:layout  = 0:L1lift;1:R1lift;...
//!
//! The body never remembers (§13.2). When a body says hello with the same
//! joint layout, its calibration comes from memory and is pushed back down
//! to it; when the layout differs, memory is not applied and the body is
//! reported as uncalibrated.

use brainos_mind::kira::BodyPolicy;
use brainos_mind::state::{NodeKind, StateGraph};
use miniblur_core::policy::{self, BodyState, Mode};
use miniblur_core::schema::{self, Model};
use std::time::Instant;

pub struct HexapodPolicy {
    pub model: Model,
    state: BodyState,
    last_heard: Instant,
    /// true when the calibration came from the state graph at incorporation
    pub calibration_from_memory: bool,
}

impl HexapodPolicy {
    pub fn new(model: Model, calibration_from_memory: bool) -> HexapodPolicy {
        HexapodPolicy {
            model,
            state: BodyState { sitting: true, ..BodyState::default() },
            last_heard: Instant::now(),
            calibration_from_memory,
        }
    }

    fn live(&self) -> BodyState {
        BodyState { link_age_ms: self.last_heard.elapsed().as_millis() as u64, ..self.state }
    }

    pub fn state(&self) -> BodyState {
        self.live()
    }
}

impl BodyPolicy for HexapodPolicy {
    fn validate(&self, verb: &str, target: &str) -> Result<(), String> {
        policy::validate(&self.model, verb, target)
    }
    fn simulate(&self, verb: &str, target: &str) -> Result<String, String> {
        policy::simulate(&self.model, verb, target)
    }
    fn policy(&self, verb: &str, target: &str) -> Result<(), String> {
        policy::policy(&self.model, &self.live(), verb, target)
    }
    /// Every frame from the body is a heartbeat; a telemetry digest also
    /// carries the battery, the mode and whether it is sitting.
    fn observe(&mut self, report: &str) {
        self.last_heard = Instant::now();
        for part in report.split(';').map(str::trim) {
            if let Some(v) = part.strip_suffix(" V").and_then(|s| s.trim().parse::<f32>().ok()) {
                self.state.vbat = v;
            } else if let Some(m) = part.strip_prefix("mode ") {
                self.state.mode = match m.trim() {
                    "walking" => Mode::Walking,
                    "calibrating" => Mode::Calibrating,
                    _ => Mode::Idle,
                };
            } else if part == "sitting" {
                self.state.sitting = true;
            } else if part == "standing" {
                self.state.sitting = false;
            }
        }
        // a calibration record coming back updates the seat's copy of the table
        if let Some((ch, rest)) = report.strip_prefix("ch").and_then(|r| r.split_once(' ')) {
            if let Ok(ch) = ch.parse::<u8>() {
                if let Some(c) = self.model.table.find_mut(ch) {
                    apply_cal_words(c, rest);
                }
            }
        }
    }
    fn describe(&self) -> String {
        let s = self.live();
        format!(
            "{}; {}; {:.2} V; mode {}; heard {} ms ago",
            self.model.describe(),
            if self.calibration_from_memory { "calibration from memory" } else { "uncalibrated (defaults)" },
            s.vbat,
            s.mode.name(),
            s.link_age_ms
        )
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Parse `center 92.5 min 30.0 max 150.0 dir 1` (any subset, any order).
fn apply_cal_words(c: &mut miniblur_core::servo::ServoCal, words: &str) {
    let w: Vec<&str> = words.split_whitespace().collect();
    let mut i = 0;
    while i + 1 < w.len() {
        let v = w[i + 1].parse::<f32>();
        match (w[i], v) {
            ("center", Ok(v)) => c.center_deg = v,
            ("min", Ok(v)) => c.min_deg = v,
            ("max", Ok(v)) => c.max_deg = v,
            ("dir", Ok(v)) => c.dir = v as i8,
            _ => {}
        }
        i += 2;
    }
}

/// The remembered calibration for a body name: (layout fingerprint, records
/// as `chN <words>`), newest node per channel wins.
pub fn remembered(state: &StateGraph, body: &str) -> Option<(String, Vec<(u8, String)>)> {
    let layout_key = format!("{body}:layout = ");
    let cal_key = format!("{body}:cal:ch");
    let mut layout: Option<String> = None;
    let mut cals: Vec<(u8, String)> = Vec::new();
    for nd in state.nodes().iter().filter(|n| n.kind == NodeKind::Semantic) {
        if let Some(l) = nd.content.strip_prefix(&layout_key) {
            layout = Some(l.trim().to_string());
        } else if let Some(rest) = nd.content.strip_prefix(&cal_key) {
            // "6 = L1+L3 swing center 92.5 min 30.0 max 150.0 dir 1"
            if let Some((ch, words)) = rest.split_once(" = ") {
                if let Ok(ch) = ch.trim().parse::<u8>() {
                    // drop the "L1+L3 swing" prefix (two words) before the numbers
                    let nums = words.splitn(3, ' ').nth(2).unwrap_or("").to_string();
                    cals.retain(|(c, _)| *c != ch);
                    cals.push((ch, nums));
                }
            }
        }
    }
    layout.map(|l| (l, cals))
}

/// Apply memory to a model that just said hello. Returns true if the layout
/// matched and the calibration was applied.
pub fn apply_remembered(model: &mut Model, state: &StateGraph, body: &str) -> bool {
    let Some((layout, cals)) = remembered(state, body) else { return false };
    if layout != model.layout_fingerprint() {
        tracing::info!("[hexapod] '{body}' has a different joint layout than remembered; not applying memory");
        return false;
    }
    let mut n = 0;
    for (ch, words) in &cals {
        if let Some(c) = model.table.find_mut(*ch) {
            apply_cal_words(c, words);
            n += 1;
        }
    }
    tracing::info!("[hexapod] '{body}': calibration for {n} joint(s) from memory");
    n > 0
}

/// Write the whole calibration of a model into the state graph (after a
/// `body.calibrate` record comes back, or at first incorporation so the
/// layout is known next time).
pub fn remember(state: &mut StateGraph, body: &str, model: &Model, tick: u64) {
    for line in schema::calibration_lines(body, model) {
        state.add(NodeKind::Semantic, line, 95, tick);
    }
}

/// The per-channel records to push down to a body at incorporation.
pub fn push_records(model: &Model) -> Vec<serde_json::Value> {
    model
        .table
        .channels()
        .iter()
        .map(|c| serde_json::json!({"ch": c.ch, "center": c.center_deg, "min": c.min_deg, "max": c.max_deg, "dir": c.dir}))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observe_parses_telemetry_and_calibration_records() {
        let mut p = HexapodPolicy::new(Model::default(), false);
        p.observe("mode walking; standing; 7.12 V; 1.90 A; link 3 ms; steps 4; lift 0/0/0/0/0/0");
        let s = p.state();
        assert_eq!(s.vbat, 7.12);
        assert_eq!(s.mode, Mode::Walking);
        assert!(!s.sitting);
        p.observe("ch6 center 92.5 min 30.0 max 150.0 dir 1");
        assert_eq!(p.model.table.find(6).unwrap().center_deg, 92.5);
        assert_eq!(p.model.table.find(6).unwrap().min_deg, 30.0);
    }

    #[test]
    fn a_flat_battery_reported_by_the_body_refuses_the_next_walk_at_policy() {
        let mut p = HexapodPolicy::new(Model::default(), false);
        p.observe("mode idle; sitting; 6.30 V; 0.80 A; link 0 ms; steps 0; lift 0/0/0/0/0/0");
        assert!(p.policy("gait.walk", "fwd 1 0.5").unwrap_err().contains("battery"));
        assert!(p.policy("gait.stop", "").is_ok());
    }

    #[test]
    fn calibration_round_trips_through_the_state_graph() {
        let mut g = StateGraph::new();
        let mut m = Model::default();
        m.table.find_mut(6).unwrap().center_deg = 92.5;
        m.table.find_mut(6).unwrap().min_deg = 30.0;
        remember(&mut g, "miniblur", &m, 1);
        // a fresh hello with default calibration
        let mut fresh = Model::default();
        assert!(apply_remembered(&mut fresh, &g, "miniblur"));
        assert_eq!(fresh.table.find(6).unwrap().center_deg, 92.5);
        assert_eq!(fresh.table.find(6).unwrap().min_deg, 30.0);
        // the newest record wins
        m.table.find_mut(6).unwrap().center_deg = 93.0;
        remember(&mut g, "miniblur", &m, 2);
        let mut fresh2 = Model::default();
        assert!(apply_remembered(&mut fresh2, &g, "miniblur"));
        assert_eq!(fresh2.table.find(6).unwrap().center_deg, 93.0);
        // an unknown body has no memory; a different layout is not applied
        assert!(!apply_remembered(&mut Model::default(), &g, "other"));
        let s = g.serialize();
        let mut g2 = StateGraph::new();
        g2.load(s.as_bytes());
        let mut fresh3 = Model::default();
        assert!(apply_remembered(&mut fresh3, &g2, "miniblur"), "survives the journal round trip");
        assert_eq!(fresh3.table.find(6).unwrap().center_deg, 93.0);
    }
}
