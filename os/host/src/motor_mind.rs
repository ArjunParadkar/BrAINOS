//! BFS  LEVEL    Host - the only crate with an operating system (CLI, scenarios, OS nerves)  [crate brainos-host]
//! BFS  PURPOSE  The cognition process for the simulated arm: reads a recipe and what is perceived, and asks the seat for one primitive at a time, either ahead of time from predicted positions or stop-and-wait
//! BFS  DEFINES  request, run
//! BFS  USES     brainos: abi, bodyd, motor | external: std
//! BFS  USED BY  host/motor
//!
//! Cognition, as a separate process, for the simulated desk arm.
//!
//! It holds no key and no authority. It reads a recipe, watches what is
//! perceived, and *asks*: one `ask` line per primitive, which the seat
//! puts to the gate. Killing this process must not stop the arm's
//! reflexes or its standing posture, and the tests kill it for real.
//!
//! Two ways of asking:
//!
//! * **fluid**: ask for the next primitive while the current one is
//!   still running, working out where things will be from the recipe
//!   and the last thing perceived (a forward model), so the seat can
//!   join the two movements without a stop.
//! * **stop-go** (the baseline): wait for the current primitive to end,
//!   wait for a perception newer than that, then ask.
//!
//! Lines in:
//!   p T_US NAME X Y Z YAW k=v ...     something perceived
//!   pose T_US X Y Z                   where the tool is
//!   up ...                            the seat's reports, relayed
//! Lines out:
//!   ask ID VERB k=v ...
//!   done ok | done fail WHY

use brainos_bodyd::armloop::{DeskArm, HOLD};
use brainos_bodyd::deskarm::DeskArmConfig;
use brainos_motor::mandate::{P_BOX, P_DURATION_MS, P_FORCE_N, P_RATE_DPS};
use brainos_motor::primitive::{desk_arm_library, place, program, Frame, Given, Kind, MotorPrimitive};
use brainos_motor::recipe::{Place, Recipe, Step};
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, Write};
use std::process::ExitCode;

/// Build the request for one step: its arguments, and where the tool
/// will be when it is over.
pub fn request(
    prim: &MotorPrimitive,
    step: &Step,
    places: &HashMap<String, (u64, Place)>,
    from: [f32; 3],
    arm: &DeskArm,
) -> Result<(Vec<(String, f32)>, [f32; 3]), String> {
    let (_, at) = places
        .get(&step.frame)
        .ok_or_else(|| format!("nothing perceived called {}", step.frame))?;
    let mut args: Vec<(String, f32)> = Vec::new();
    for (name, bind) in &step.params {
        let v = bind
            .value(at)
            .ok_or_else(|| format!("{} has no length for {name}", step.frame))?;
        // inside the range the primitive was built for, or not at all
        let p = prim.param(name).ok_or_else(|| format!("no parameter {name}"))?;
        if v < p.min - 1e-6 || v > p.max + 1e-6 {
            return Err(format!("{name}={v} is outside what {} was built for", prim.kind.name()));
        }
        args.push((name.clone(), v.clamp(p.min, p.max)));
    }
    let mut yaw = at.yaw_deg;
    match prim.kind {
        Kind::Push => {
            // the frame's x axis points at where the thing should go
            let (_, target) = places
                .get(&format!("{}.rest", step.to))
                .ok_or_else(|| format!("nothing perceived called {}.rest", step.to))?;
            let (dx, dy) = (target.pos[0] - at.pos[0], target.pos[1] - at.pos[1]);
            yaw = dy.atan2(dx).to_degrees();
            let d = (dx * dx + dy * dy).sqrt();
            args.retain(|(n, _)| n != "distance_m");
            args.push(("distance_m".into(), d));
        }
        Kind::Pour => {
            yaw = at.pos[1].atan2(at.pos[0]).to_degrees();
        }
        _ => {}
    }
    let frame = Frame { pos: at.pos, yaw_deg: yaw };
    let values: Vec<(&str, f32)> = args.iter().map(|(n, v)| (n.as_str(), *v)).collect();
    let strokes = program(&Given { primitive: prim, values: &values }).map_err(|e| e.to_string())?;
    let goals = place(&strokes, &frame);
    let end = goals.last().map(|g| [g.at[0], g.at[1], g.at[2]]).unwrap_or(from);

    // the box: everywhere the strokes go, and where the tool starts
    let mut lo = from;
    let mut hi = from;
    for g in &goals {
        for k in 0..3 {
            lo[k] = lo[k].min(g.at[k]);
            hi[k] = hi[k].max(g.at[k]);
        }
    }
    let reach = arm.ceiling().reach.expect("an arm has a reach");
    let m = prim.envelope.margin_m;
    for k in 0..3 {
        // a hair inside the reach: the schema's edge is an f32, and a
        // decimal string for the same number can land outside it
        lo[k] = (lo[k] - m).max(reach.min[k] + 1e-4);
        hi[k] = (hi[k] + m).min(reach.max[k] - 1e-4);
    }
    args.push((P_DURATION_MS.into(), prim.envelope.duration_ms as f32));
    args.push((P_FORCE_N.into(), prim.envelope.force_n));
    args.push((P_RATE_DPS.into(), prim.envelope.rate_dps));
    for (i, name) in P_BOX.iter().enumerate() {
        args.push(((*name).into(), if i % 2 == 0 { lo[i / 2] } else { hi[i / 2] }));
    }
    args.push(("fx".into(), frame.pos[0]));
    args.push(("fy".into(), frame.pos[1]));
    args.push(("fz".into(), frame.pos[2]));
    args.push(("fyaw".into(), frame.yaw_deg));
    Ok((args, end))
}

fn parse_place(f: &[&str]) -> Option<(u64, Place)> {
    let t: u64 = f.first()?.parse().ok()?;
    let name = f.get(1)?.to_string();
    let n = |i: usize| f.get(i).and_then(|s| s.parse::<f32>().ok());
    let mut attrs = Vec::new();
    for kv in f.iter().skip(6) {
        let (k, v) = kv.split_once('=')?;
        attrs.push((k.to_string(), v.parse().ok()?));
    }
    Some((
        t,
        Place {
            name,
            pos: [n(2)?, n(3)?, n(4)?],
            yaw_deg: n(5)?,
            attrs,
        },
    ))
}

/// `brainos motor mind RECIPE [--stop-go] [--no-hold]`
pub fn run(rest: &[String]) -> ExitCode {
    let Some(path) = rest.first() else {
        eprintln!("usage: brainos motor mind RECIPE [--stop-go]");
        return ExitCode::FAILURE;
    };
    let stop_go = rest.iter().any(|a| a == "--stop-go");
    let Some(recipe) = std::fs::read_to_string(path).ok().and_then(|t| Recipe::from_text(&t)) else {
        eprintln!("{path}: not a recipe");
        return ExitCode::FAILURE;
    };
    let lib = desk_arm_library();
    let arm = DeskArm {
        config: DeskArmConfig::default(),
        id: brainos_abi::ids::BodyId::from_bytes([0; 32]),
    };
    let mut places: HashMap<String, (u64, Place)> = HashMap::new();
    let mut pose: Option<[f32; 3]> = None;
    let mut next = 0usize;
    // steps asked for and not yet ended
    let mut out_standing: VecDeque<usize> = VecDeque::new();
    let mut acked = 0usize;
    let mut asked = 0usize;
    let mut hold_asked = false;
    let mut hold_ok = false;
    let mut predicted_end: Option<[f32; 3]> = None;
    let mut last_end_us = 0u64;
    let mut now_us = 0u64;
    let mut finished = 0usize;
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    let say = |out: &mut std::io::Stdout, s: String| {
        let _ = writeln!(out, "{s}");
        let _ = out.flush();
    };

    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let f: Vec<&str> = line.split_whitespace().collect();
        match f.first().copied() {
            Some("p") => {
                if let Some((t, p)) = parse_place(&f[1..]) {
                    now_us = now_us.max(t);
                    places.insert(p.name.clone(), (t, p));
                }
            }
            Some("pose") if f.len() == 5 => {
                let n = |i: usize| f[i].parse::<f32>().unwrap_or(0.0);
                now_us = now_us.max(f[1].parse().unwrap_or(0));
                pose = Some([n(2), n(3), n(4)]);
            }
            Some("up") if f.len() >= 3 => {
                let (id, what) = (f[1], f[2]);
                if id == "h" {
                    match what {
                        "granted" => hold_ok = true,
                        "ended" => {}
                        _ => {
                            say(&mut out, format!("done fail hold {}", f[2..].join(" ")));
                            return ExitCode::SUCCESS;
                        }
                    }
                } else if id.parse::<usize>().is_ok() {
                    match what {
                        "granted" | "queued" => acked += 1,
                        "ended" => {
                            out_standing.pop_front();
                            last_end_us = now_us;
                            if f.get(3) == Some(&"finished") {
                                finished += 1;
                                if finished == recipe.steps.len() {
                                    say(&mut out, "done ok".into());
                                    return ExitCode::SUCCESS;
                                }
                            } else {
                                say(&mut out, format!("done fail step={id} {}", f[3..].join(" ")));
                                return ExitCode::SUCCESS;
                            }
                        }
                        _ => {
                            say(&mut out, format!("done fail step={id} {}", f[2..].join(" ")));
                            return ExitCode::SUCCESS;
                        }
                    }
                }
            }
            _ => {}
        }

        let Some(here) = pose else { continue };
        if !hold_asked {
            let r = arm.ceiling().reach.expect("an arm has a reach");
            say(
                &mut out,
                format!(
                    "ask h {HOLD} duration_ms=600000 force_n=20 rate_dps=270 x_min={} x_max={} y_min={} y_max={} z_min={} z_max={}",
                    r.min[0] + 1e-4, r.max[0] - 1e-4, r.min[1] + 1e-4, r.max[1] - 1e-4, r.min[2] + 1e-4, r.max[2] - 1e-4
                ),
            );
            hold_asked = true;
            continue;
        }
        if !hold_ok || next >= recipe.steps.len() || acked < asked {
            continue;
        }
        let step = &recipe.steps[next];
        let ready = if stop_go {
            // only after everything has stopped, and only on a look at
            // the world taken since
            out_standing.is_empty()
                && places
                    .get(&step.frame)
                    .map(|(t, _)| next == 0 || *t > last_end_us)
                    .unwrap_or(false)
        } else {
            out_standing.len() < 2 && places.contains_key(&step.frame)
        };
        if !ready {
            continue;
        }
        let Some(prim) = lib.iter().find(|p| p.kind == step.kind) else {
            say(&mut out, format!("done fail step={next} not-in-library"));
            return ExitCode::SUCCESS;
        };
        // where the tool will be when this step starts: where the step
        // before it ends, if one is still running
        let from = if out_standing.is_empty() { here } else { predicted_end.unwrap_or(here) };
        match request(prim, step, &places, from, &arm) {
            Ok((args, end)) => {
                let mut line = format!("ask {next} {}", prim.verb.as_str());
                for (k, v) in &args {
                    line.push_str(&format!(" {k}={v}"));
                }
                say(&mut out, line);
                out_standing.push_back(next);
                predicted_end = Some(end);
                asked += 1;
                next += 1;
            }
            Err(why) => {
                say(&mut out, format!("done fail step={next} {}", why.replace(' ', "-")));
                return ExitCode::SUCCESS;
            }
        }
    }
    ExitCode::SUCCESS
}
