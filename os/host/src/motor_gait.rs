//! BFS  LEVEL    Host - the only crate with an operating system (CLI, scenarios, OS nerves)  [crate brainos-host]
//! BFS  PURPOSE  The `brainos motor gait-reference` command: prints the hand-tuned tripod gait as timed joint angles so a simulator can replay it in the generated model
//! BFS  DEFINES  samples, line, run
//! BFS  USES     brainos: bodyd, miniblur-core | external: std
//! BFS  USED BY  host/motor
//!
//! `brainos motor gait-reference [SPEED_MPS] [SECONDS]`
//!
//! The hand-tuned tripod gait from `miniblur_core::gait`, sampled the way
//! `bodyd::hexapod` plans a forward walk: the requested ground speed is
//! divided by `TOP_SPEED_MPS` to get the gait's cycle scalar, and below
//! `MIN_GAIT_SPEED` the request is refused, as the body refuses it.
//! Defaults: `TOP_SPEED_MPS` (the fastest cycle) and 10 seconds.
//!
//! One JSON object per line, one line per gait sample:
//!
//! ```text
//! {"t_ms":0,"L1_swing":-18,"L1_lift":0,"R1_swing":-18,"R1_lift":0,...}
//! ```
//!
//! `t_ms` is when the sample starts; it holds until the next line, as the
//! firmware holds each sample for `Plan::sample_ms`. Angles are degrees
//! from each joint's centre, named as the generated model names its
//! joints, all twelve (the linked rear swings included; the gait gives
//! them the same angle as the front swing they follow).
//!
//! What differs from the body: the body plays at most `MAX_MOTION`
//! (4 s, at most 4 cycles) per walk verb. This prints one continuous run
//! of whole cycles covering SECONDS, so a simulator can measure the gait
//! over a longer episode. The speed in m/s is the body's scaling, not a
//! measured ground speed (see `bodyd::hexapod`).

use brainos_bodyd::hexapod::{HexapodConfig, MIN_GAIT_SPEED, TOP_SPEED_MPS};
use miniblur_core::gait::{Dir, Plan, Walk};
use miniblur_core::{LegId, Pose};
use std::io::Write;
use std::process::ExitCode;

/// Longest run printed, seconds.
const MAX_SECONDS: f32 = 600.0;

/// The gait sampled for `seconds` at `speed_mps`: (ms per sample, poses).
pub fn samples(speed_mps: f32, seconds: f32) -> Result<(u32, Vec<Pose>), String> {
    if !speed_mps.is_finite() || speed_mps <= 0.0 || speed_mps > TOP_SPEED_MPS {
        return Err(format!("speed must be in (0, {TOP_SPEED_MPS}] m/s"));
    }
    if !seconds.is_finite() || seconds <= 0.0 || seconds > MAX_SECONDS {
        return Err(format!("seconds must be in (0, {MAX_SECONDS}]"));
    }
    let gait_speed = speed_mps / TOP_SPEED_MPS;
    if gait_speed < MIN_GAIT_SPEED {
        return Err(format!(
            "below {:.3} m/s the body refuses to walk (MIN_GAIT_SPEED)",
            MIN_GAIT_SPEED * TOP_SPEED_MPS
        ));
    }
    let params = HexapodConfig::default().gait;
    let one = Plan::new(params, Walk { dir: Dir::Fwd, steps: 1, speed: gait_speed.min(1.0) });
    let sample_ms = one.sample_ms().max(1);
    let wanted = ((seconds * 1000.0) / sample_ms as f32).ceil() as usize;
    let spc = params.samples_per_cycle as usize;
    let cycles = wanted.div_ceil(spc).clamp(1, u16::MAX as usize) as u16;
    let plan = Plan::new(params, Walk { steps: cycles, ..one.walk });
    Ok((sample_ms, plan.iter().take(wanted).collect()))
}

/// One sample as a JSON line (no newline).
pub fn line(t_ms: u64, pose: &Pose) -> String {
    let mut s = format!("{{\"t_ms\":{t_ms}");
    for (i, leg) in LegId::ALL.iter().enumerate() {
        s.push_str(&format!(
            ",\"{0}_swing\":{1},\"{0}_lift\":{2}",
            leg.name(),
            pose.swing[i],
            pose.lift[i]
        ));
    }
    s.push('}');
    s
}

pub fn run(rest: &[String]) -> ExitCode {
    let num = |i: usize, default: f32| -> Option<f32> {
        match rest.get(i) {
            None => Some(default),
            Some(s) => s.parse::<f32>().ok(),
        }
    };
    let (Some(speed), Some(seconds)) = (num(0, TOP_SPEED_MPS), num(1, 10.0)) else {
        eprintln!("usage: brainos motor gait-reference [SPEED_MPS] [SECONDS]");
        return ExitCode::FAILURE;
    };
    let (sample_ms, poses) = match samples(speed, seconds) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("gait-reference: {e}");
            return ExitCode::FAILURE;
        }
    };
    eprintln!(
        "gait-reference: tripod fwd, cycle scalar {:.3} ({speed} m/s by the body's scaling), {sample_ms} ms per sample, {} samples",
        (speed / TOP_SPEED_MPS).min(1.0),
        poses.len()
    );
    let mut out = std::io::BufWriter::new(std::io::stdout());
    for (n, p) in poses.iter().enumerate() {
        if writeln!(out, "{}", line(n as u64 * sample_ms as u64, p)).is_err() {
            return ExitCode::FAILURE;
        }
    }
    if out.flush().is_err() {
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_speed_is_the_default_cycle() {
        let (ms, p) = samples(TOP_SPEED_MPS, 1.2).unwrap();
        assert_eq!(ms, 25); // 1200 ms / 48 samples
        assert_eq!(p.len(), 48);
        assert_eq!(p[0], miniblur_core::gait::pose_at(&HexapodConfig::default().gait, Dir::Fwd, 0.0));
    }

    #[test]
    fn runs_longer_than_one_motion_repeat_the_cycle() {
        let (ms, p) = samples(TOP_SPEED_MPS, 10.0).unwrap();
        assert_eq!(p.len() as u32 * ms, 10_000);
        assert_eq!(p[48], p[0]);
        assert_eq!(p[399], p[399 % 48]);
    }

    #[test]
    fn the_body_s_refusals_hold() {
        assert!(samples(0.01, 1.0).is_err()); // below MIN_GAIT_SPEED
        assert!(samples(0.2, 1.0).is_err()); // above TOP_SPEED_MPS
        assert!(samples(f32::NAN, 1.0).is_err());
        assert!(samples(0.06, 0.0).is_err());
        assert!(samples(0.06, 1.0).is_ok());
    }

    #[test]
    fn lines_name_all_twelve_joints() {
        let (_, p) = samples(TOP_SPEED_MPS, 0.1).unwrap();
        let l = line(0, &p[0]);
        for leg in LegId::ALL {
            assert!(l.contains(&format!("\"{}_swing\":", leg.name())));
            assert!(l.contains(&format!("\"{}_lift\":", leg.name())));
        }
        assert!(l.starts_with("{\"t_ms\":0,") && l.ends_with('}'));
    }
}
