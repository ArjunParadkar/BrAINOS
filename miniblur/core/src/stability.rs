//! The tip-over invariant: the centre of mass, projected onto the ground,
//! must stay inside the support polygon of the feet that are down — with a
//! margin. This is what KIRA's SIMULATE stage checks for a hexapod, and what
//! the board's reflex loop checks before it plays a sample.

use crate::geometry::{LegGeom, Vec2};
use crate::{Pose, LEGS};

/// A foot counts as grounded if it is within this much of standing height.
pub const CONTACT_MM: f32 = 2.0;

/// Support polygon of a pose: the convex hull of grounded feet. Returns the
/// hull vertices (≤ 6) and how many.
pub fn support_polygon(g: &LegGeom, pose: &Pose) -> ([Vec2; LEGS], usize) {
    let feet = g.feet(pose);
    let mut pts = [Vec2::default(); LEGS];
    let mut n = 0;
    for (xy, z) in feet.iter() {
        if *z <= g.ground_z() + CONTACT_MM {
            pts[n] = *xy;
            n += 1;
        }
    }
    convex_hull(&mut pts, n)
}

/// Andrew's monotone chain on ≤ 6 points, no allocation.
fn convex_hull(pts: &mut [Vec2; LEGS], n: usize) -> ([Vec2; LEGS], usize) {
    if n < 3 {
        return (*pts, n);
    }
    // sort by x then y (insertion sort; n ≤ 6)
    for i in 1..n {
        let mut j = i;
        while j > 0 && (pts[j].x, pts[j].y) < (pts[j - 1].x, pts[j - 1].y) {
            pts.swap(j, j - 1);
            j -= 1;
        }
    }
    let mut hull = [Vec2::default(); LEGS * 2];
    let mut k = 0;
    for i in 0..n {
        while k >= 2 && hull[k - 1].sub(hull[k - 2]).cross(pts[i].sub(hull[k - 2])) <= 0.0 {
            k -= 1;
        }
        hull[k] = pts[i];
        k += 1;
    }
    let lower = k + 1;
    for i in (0..n - 1).rev() {
        while k >= lower && hull[k - 1].sub(hull[k - 2]).cross(pts[i].sub(hull[k - 2])) <= 0.0 {
            k -= 1;
        }
        hull[k] = pts[i];
        k += 1;
    }
    let m = k - 1;
    let mut out = [Vec2::default(); LEGS];
    out[..m].copy_from_slice(&hull[..m]);
    (out, m)
}

/// Signed distance from `p` to the polygon boundary: positive inside,
/// negative outside. With fewer than 3 grounded feet there is no polygon
/// and the answer is a large negative number.
pub fn margin_mm(hull: &[Vec2; LEGS], n: usize, p: Vec2) -> f32 {
    if n < 3 {
        return -1000.0;
    }
    let mut best = f32::MAX;
    for i in 0..n {
        let a = hull[i];
        let b = hull[(i + 1) % n];
        let e = b.sub(a);
        let len = e.len();
        if len < 1e-6 {
            continue;
        }
        // hull is CCW: inside is to the left of every edge → cross > 0
        let d = e.cross(p.sub(a)) / len;
        if d < best {
            best = d;
        }
    }
    best
}

/// Stability of one pose: the CoM's margin inside the support polygon.
pub fn pose_margin_mm(g: &LegGeom, pose: &Pose) -> f32 {
    let (hull, n) = support_polygon(g, pose);
    margin_mm(&hull, n, g.com)
}

/// Worst-case margin over a whole plan. This is the forward model KIRA runs.
pub fn plan_min_margin_mm(g: &LegGeom, plan: &crate::gait::Plan) -> f32 {
    let mut worst = f32::MAX;
    for pose in plan.iter() {
        let m = pose_margin_mm(g, &pose);
        if m < worst {
            worst = m;
        }
    }
    worst
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gait::{Dir, GaitParams, Plan, Walk};
    use crate::LegId;

    #[test]
    fn standing_on_six_feet_is_very_stable() {
        let g = LegGeom::default();
        let m = pose_margin_mm(&g, &Pose::STAND);
        assert!(m > 30.0, "margin {m}");
    }

    #[test]
    fn the_tripod_gait_never_tips_the_default_body() {
        let g = LegGeom::default();
        let plan = Plan::new(GaitParams::default(), Walk { dir: Dir::Fwd, steps: 1, speed: 1.0 });
        let m = plan_min_margin_mm(&g, &plan);
        assert!(m > 8.0, "worst margin over a cycle {m} mm");
    }

    #[test]
    fn lifting_a_whole_side_tips_it() {
        let g = LegGeom::default();
        let mut p = Pose::STAND;
        for l in [LegId::L1, LegId::L2, LegId::L3] {
            p.lift[l as usize] = 30.0;
        }
        let m = pose_margin_mm(&g, &p);
        assert!(m < 0.0, "three feet in a line is not a polygon that contains the CoM: {m}");
    }

    #[test]
    fn two_feet_is_not_standing() {
        let g = LegGeom::default();
        let mut p = Pose::STAND;
        for l in [LegId::L1, LegId::L2, LegId::L3, LegId::R2] {
            p.lift[l as usize] = 30.0;
        }
        assert!(pose_margin_mm(&g, &p) < -100.0);
    }

    #[test]
    fn a_far_aft_battery_reduces_the_margin() {
        let mut g = LegGeom::default();
        let m0 = pose_margin_mm(&g, &Pose::STAND);
        g.com = Vec2::new(-40.0, 0.0);
        let m1 = pose_margin_mm(&g, &Pose::STAND);
        assert!(m1 < m0);
    }
}
