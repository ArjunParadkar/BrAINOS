//! BFS  LEVEL    Domain 2 Autonomic - motor memory (self-model, mandates, primitives, reflexes)  [crate brainos-motor]
//! BFS  PURPOSE  Tests one-shot learning on a synthetic demonstration: it is cut into grasp, wipe and place, lengths are bound to the object rather than copied, and a demonstration that does not chain is refused
//! BFS  DEFINES  -
//! BFS  USES     brainos: motor
//! BFS  USED BY  (entry point or test)
//!

use brainos_motor::primitive::{desk_arm_library, Kind};
use brainos_motor::recipe::{learn, Bind, LearnError, Place, Recipe, Sample};

fn scene(mug_r: f32) -> Vec<Place> {
    let p = |name: &str, pos: [f32; 3], attrs: &[(&str, f32)]| Place {
        name: name.into(),
        pos,
        yaw_deg: 0.0,
        attrs: attrs.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
    };
    vec![
        p("brush.grip", [0.15, -0.08, 0.075], &[("width", 0.012), ("reach", 0.075)]),
        p("holder.rest", [0.15, -0.08, 0.0], &[]),
        p("mug.mouth", [0.20, 0.05, 0.08], &[("radius", mug_r), ("depth", 0.074)]),
    ]
}

/// A hand-made demonstration at 100 Hz: close on the brush, carry it,
/// circle inside the mug touching the wall, carry it back, let go.
fn demo(mug_r: f32, wipe: bool) -> Vec<Sample> {
    let places = scene(mug_r);
    let mut out = Vec::new();
    let mut t = 0.0f32;
    let mut push = |tool: [f32; 3], open: f32, grip: f32, touch: f32, out: &mut Vec<Sample>| {
        out.push(Sample { t_ms: t, tool, open_m: open, roll_deg: 0.0, grip_n: grip, tool_n: touch, places: places.clone() });
        t += 10.0;
    };
    let grip_at = [0.15, -0.08, 0.075];
    for _ in 0..50 {
        push([0.15, -0.08, 0.13], 0.02, 0.0, 0.0, &mut out);
    }
    for _ in 0..30 {
        push(grip_at, 0.02, 0.0, 0.0, &mut out);
    }
    for _ in 0..40 {
        push(grip_at, 0.006, 5.0, 0.0, &mut out);
    }
    if wipe {
        // above the mug, then down inside; the brush tip is 0.075 below
        for _ in 0..40 {
            push([0.20, 0.05, 0.20], 0.006, 5.0, 0.0, &mut out);
        }
        let reach = mug_r - 0.009;
        for k in 0..200 {
            let a = k as f32 * 2.0 * core::f32::consts::TAU / 200.0;
            push([0.20 + reach * a.cos(), 0.05 + reach * a.sin(), 0.08 + 0.075 - 0.05], 0.006, 5.0, 2.0, &mut out);
        }
        for _ in 0..40 {
            push([0.20, 0.05, 0.20], 0.006, 5.0, 0.0, &mut out);
        }
    }
    for _ in 0..40 {
        push([0.15, -0.08, 0.076], 0.006, 5.0, 0.0, &mut out);
    }
    for _ in 0..40 {
        push([0.15, -0.08, 0.076], 0.02, 0.0, 0.0, &mut out);
    }
    out
}

#[test]
fn one_demonstration_becomes_grasp_wipe_place() {
    let r = learn(&demo(0.035, true), &desk_arm_library()).unwrap();
    let kinds: Vec<Kind> = r.steps.iter().map(|s| s.kind).collect();
    assert_eq!(kinds, vec![Kind::Grasp, Kind::Wipe, Kind::Place]);
    assert_eq!(r.steps[0].frame, "brush.grip");
    assert_eq!(r.steps[1].frame, "mug.mouth");
    assert_eq!(r.steps[2].frame, "holder.rest");
    assert_eq!(r.steps[2].it, "brush");
    assert_eq!(r.steps[2].to, "holder");
    // the goal: the mug is wiped, the brush is back, the hand is empty
    let goal: Vec<String> = r.goal.iter().map(|f| format!("{} {}", f.pred.name(), f.about.as_str())).collect();
    assert!(goal.contains(&"wiped mug".to_string()), "{goal:?}");
    assert!(goal.contains(&"rests brush".to_string()), "{goal:?}");
    assert!(goal.contains(&"hand-free ".to_string()), "{goal:?}");
}

#[test]
fn the_wipe_is_bound_to_the_mug_not_copied_from_the_demonstration() {
    let r = learn(&demo(0.035, true), &desk_arm_library()).unwrap();
    let wipe = &r.steps[1];
    let (_, radius) = wipe.params.iter().find(|(n, _)| n == "radius_m").unwrap();
    let Bind::Of { attr, scale, offset } = radius else { panic!("the radius was copied: {radius:?}") };
    assert_eq!(attr, "radius");
    assert_eq!(*scale, 1.0);
    assert!((offset + 0.009).abs() < 1e-4, "the brush head is 9 mm: {offset}");
    // on a mug half again as wide, the wipe reaches half again as far
    let wide = &scene(0.0525)[2];
    assert!((radius.value(wide).unwrap() - 0.0435).abs() < 1e-4);
    // the tool's length was read off the brush
    let (_, tool) = wipe.params.iter().find(|(n, _)| n == "tool_m").unwrap();
    assert_eq!(*tool, Bind::Fixed(0.075));
}

#[test]
fn a_recipe_survives_being_written_down_and_read_back() {
    let r = learn(&demo(0.035, true), &desk_arm_library()).unwrap();
    let text = r.to_text();
    let back = Recipe::from_text(&text).unwrap();
    assert_eq!(back.steps.len(), r.steps.len());
    for (a, b) in r.steps.iter().zip(back.steps.iter()) {
        assert_eq!(a.kind, b.kind);
        assert_eq!(a.frame, b.frame);
        assert_eq!(a.params.len(), b.params.len());
    }
    assert!(Recipe::from_text("").is_none());
    assert!(Recipe::from_text("juggle frame=x it=y to=").is_none());
}

#[test]
fn demonstrations_that_cannot_be_learned_from_are_refused_with_a_reason() {
    let lib = desk_arm_library();
    assert_eq!(learn(&[], &lib), Err(LearnError::TooShort));

    // stops while still holding the brush
    let mut d = demo(0.035, true);
    d.truncate(d.len() - 45);
    assert_eq!(learn(&d, &lib), Err(LearnError::EndsHolding));

    // closes the fingers where there is nothing to hold
    let mut d = demo(0.035, false);
    for s in d.iter_mut() {
        s.tool[0] += 0.3;
    }
    assert!(matches!(learn(&d, &lib), Err(LearnError::NothingNear(_))));

    // a library with no wipe cannot name the wipe
    let no_wipe: Vec<_> = desk_arm_library().into_iter().filter(|p| p.kind != Kind::Wipe).collect();
    assert_eq!(learn(&demo(0.035, true), &no_wipe), Err(LearnError::NotInLibrary(Kind::Wipe)));
}

#[test]
fn chatter_shorter_than_the_debounce_does_not_cut_the_demonstration() {
    let mut d = demo(0.035, true);
    // two samples (20 ms) of lost grip in the middle of the carry
    let mid = 50 + 30 + 40 + 20;
    d[mid].grip_n = 0.0;
    d[mid + 1].grip_n = 0.0;
    let r = learn(&d, &desk_arm_library()).unwrap();
    assert_eq!(r.steps.len(), 3);
}
