//! The tripod gait: three legs down, three up, alternating. Any stable gait
//! is enough for Phase 0; elegance comes later (Team Briefing §7).
//!
//! The gait is a pure function of phase `t ∈ [0,1)`: tripod A swings (lifted,
//! moving forward) during the first half and stances (down, pushing back)
//! during the second; tripod B the opposite. A `Plan` samples one or more
//! cycles into poses the firmware plays at its loop rate and the seat
//! forward-models for the stability check. Both sides see the same poses.

use crate::{LegId, Pose, Tripod, LEGS};
use alloc::string::String;
use libm::{cosf, sinf};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dir {
    Fwd,
    Back,
    Left,
    Right,
}

impl Dir {
    pub fn parse(s: &str) -> Option<Dir> {
        match s {
            "fwd" | "forward" | "forwards" | "ahead" => Some(Dir::Fwd),
            "back" | "backward" | "backwards" | "reverse" => Some(Dir::Back),
            "left" => Some(Dir::Left),
            "right" => Some(Dir::Right),
            _ => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Dir::Fwd => "fwd",
            Dir::Back => "back",
            Dir::Left => "left",
            Dir::Right => "right",
        }
    }
}

/// Gait shape. Degrees are joint-relative.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct GaitParams {
    /// half the swing travel: a leg moves from -amp to +amp over a cycle
    pub swing_amp_deg: f32,
    /// how high a swinging leg lifts
    pub lift_deg: f32,
    /// one full cycle at speed 1.0, ms
    pub cycle_ms: u32,
    /// samples per cycle (the firmware plays one sample per loop tick)
    pub samples_per_cycle: u16,
}

impl Default for GaitParams {
    fn default() -> GaitParams {
        GaitParams { swing_amp_deg: 18.0, lift_deg: 28.0, cycle_ms: 1200, samples_per_cycle: 48 }
    }
}

/// One requested walk.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Walk {
    pub dir: Dir,
    pub steps: u16,
    /// 0.1..=1.0 — scales cycle time, not amplitude
    pub speed: f32,
}

impl Walk {
    /// Parse the KIRA target form `"<dir> <steps> <speed>"` (speed optional).
    pub fn parse_target(t: &str) -> Result<Walk, String> {
        let mut it = t.split_whitespace();
        let dir = it
            .next()
            .and_then(Dir::parse)
            .ok_or_else(|| String::from("gait.walk wants a direction: fwd, back, left or right"))?;
        let steps: u16 = match it.next() {
            None => 1,
            Some(s) => s.parse().map_err(|_| String::from("gait.walk wants a whole number of steps"))?,
        };
        let speed: f32 = match it.next() {
            None => 0.5,
            Some(s) => s.parse().map_err(|_| String::from("gait.walk wants a speed between 0.1 and 1.0"))?,
        };
        Ok(Walk { dir, steps, speed })
    }
    pub fn target(&self) -> String {
        alloc::format!("{} {} {:.2}", self.dir.name(), self.steps, self.speed)
    }
}

/// Which way each side's swing goes for a direction: +1 = the leg's
/// "forward" sense, -1 = its "backward" sense.
fn side_sense(dir: Dir, leg: LegId) -> f32 {
    match dir {
        Dir::Fwd => 1.0,
        Dir::Back => -1.0,
        // turning: sides move opposite ways
        Dir::Left => if leg.is_left() { -1.0 } else { 1.0 },
        Dir::Right => if leg.is_left() { 1.0 } else { -1.0 },
    }
}

/// The joint-relative swing that moves a leg toward the front. Left legs
/// yaw negative to go forward, right legs positive (see geometry::mount).
fn forward_sign(leg: LegId) -> f32 {
    if leg.is_left() { -1.0 } else { 1.0 }
}

/// Pose at phase `t ∈ [0,1)` of a cycle.
pub fn pose_at(p: &GaitParams, dir: Dir, t: f32) -> Pose {
    let t = t - libm::floorf(t);
    let mut pose = Pose::STAND;
    for (i, leg) in LegId::ALL.iter().enumerate() {
        // local phase: 0..1 within this leg's half-cycle, and which half
        let (u, swinging) = match leg.tripod() {
            Tripod::A => if t < 0.5 { (t * 2.0, true) } else { ((t - 0.5) * 2.0, false) },
            Tripod::B => if t < 0.5 { (t * 2.0, false) } else { ((t - 0.5) * 2.0, true) },
        };
        let sense = side_sense(dir, *leg) * forward_sign(*leg);
        if swinging {
            // in the air: travel from -amp to +amp on a cosine, lift on a half-sine
            let travel = -cosf(u * core::f32::consts::PI); // -1 → +1
            pose.swing[i] = sense * p.swing_amp_deg * travel;
            pose.lift[i] = p.lift_deg * sinf(u * core::f32::consts::PI);
        } else {
            // on the ground: push back from +amp to -amp, linear, foot down
            pose.swing[i] = sense * p.swing_amp_deg * (1.0 - 2.0 * u);
            pose.lift[i] = 0.0;
        }
    }
    pose
}

/// A sampled walk: `steps` cycles, `samples_per_cycle` poses each.
#[derive(Clone, Copy, Debug)]
pub struct Plan {
    pub params: GaitParams,
    pub walk: Walk,
}

impl Plan {
    pub fn new(params: GaitParams, walk: Walk) -> Plan {
        Plan { params, walk }
    }
    pub fn samples(&self) -> usize {
        self.walk.steps as usize * self.params.samples_per_cycle as usize
    }
    /// Pose of sample `n` (0..samples()).
    pub fn sample(&self, n: usize) -> Pose {
        let spc = self.params.samples_per_cycle as usize;
        pose_at(&self.params, self.walk.dir, (n % spc) as f32 / spc as f32)
    }
    /// Milliseconds between samples at this walk's speed.
    pub fn sample_ms(&self) -> u32 {
        let speed = if self.walk.speed < 0.1 { 0.1 } else if self.walk.speed > 1.0 { 1.0 } else { self.walk.speed };
        let cycle = self.params.cycle_ms as f32 / speed;
        (cycle / self.params.samples_per_cycle as f32) as u32
    }
    pub fn total_ms(&self) -> u32 {
        self.sample_ms() * self.samples() as u32
    }
    pub fn iter(&self) -> impl Iterator<Item = Pose> + '_ {
        (0..self.samples()).map(move |n| self.sample(n))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn at_every_phase_exactly_one_tripod_is_on_the_ground() {
        let p = GaitParams::default();
        for k in 0..48 {
            let t = k as f32 / 48.0;
            let pose = pose_at(&p, Dir::Fwd, t);
            let down: usize = LEGS - pose.lift.iter().filter(|l| **l > 0.5).count();
            assert!(down >= 3, "phase {t}: only {down} feet down");
            // the grounded three are one whole tripod
            let a_down = [LegId::L1, LegId::R2, LegId::L3].iter().all(|l| pose.lift[*l as usize] <= 0.5);
            let b_down = [LegId::R1, LegId::L2, LegId::R3].iter().all(|l| pose.lift[*l as usize] <= 0.5);
            assert!(a_down || b_down, "phase {t}: no complete tripod down");
        }
    }

    #[test]
    fn a_walk_sums_to_zero_net_swing_and_lift_stays_in_range() {
        let p = GaitParams::default();
        let plan = Plan::new(p, Walk { dir: Dir::Fwd, steps: 2, speed: 1.0 });
        let mut net = [0.0f32; LEGS];
        for pose in plan.iter() {
            for i in 0..LEGS {
                net[i] += pose.swing[i];
                assert!(pose.lift[i] >= -1e-3 && pose.lift[i] <= p.lift_deg + 1e-3);
                assert!(pose.swing[i].abs() <= p.swing_amp_deg + 1e-3);
            }
        }
        for n in net {
            assert!(n.abs() < 1.0, "net swing {n}");
        }
    }

    #[test]
    fn turning_moves_the_sides_opposite_ways() {
        let p = GaitParams::default();
        let l = pose_at(&p, Dir::Left, 0.25); // tripod A mid-swing
        // L1 (left, tripod A) and R2 (right, tripod A) both swinging
        assert!(l.swing[LegId::L1 as usize] * l.swing[LegId::R2 as usize] > 0.0 || l.swing[LegId::L1 as usize].abs() < 1e-3,
            "turn: L1 {} R2 {}", l.swing[LegId::L1 as usize], l.swing[LegId::R2 as usize]);
        let f = pose_at(&p, Dir::Fwd, 0.25);
        assert!(f.swing[LegId::L1 as usize] * f.swing[LegId::R2 as usize] < 0.0 || f.swing[LegId::L1 as usize].abs() < 1e-3,
            "fwd: L1 {} R2 {}", f.swing[LegId::L1 as usize], f.swing[LegId::R2 as usize]);
    }

    #[test]
    fn timing_scales_with_speed() {
        let p = GaitParams::default();
        let slow = Plan::new(p, Walk { dir: Dir::Fwd, steps: 1, speed: 0.5 });
        let fast = Plan::new(p, Walk { dir: Dir::Fwd, steps: 1, speed: 1.0 });
        assert_eq!(slow.samples(), fast.samples());
        assert!(slow.total_ms() > fast.total_ms());
        assert_eq!(fast.total_ms(), 1200);
    }

    #[test]
    fn targets_parse_and_round_trip() {
        let w = Walk::parse_target("fwd 3 0.5").unwrap();
        assert_eq!(w, Walk { dir: Dir::Fwd, steps: 3, speed: 0.5 });
        assert_eq!(Walk::parse_target("back").unwrap().steps, 1);
        assert!(Walk::parse_target("sideways 3").is_err());
        assert!(Walk::parse_target("fwd many").is_err());
        assert_eq!(Walk::parse_target(&w.target()).unwrap(), w);
    }
}
