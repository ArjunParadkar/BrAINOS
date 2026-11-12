//! BFS  LEVEL    Host - the only crate with an operating system (CLI, scenarios, OS nerves)  [crate brainos-host]
//! BFS  PURPOSE  The seat for a simulated desk arm: an entity with the arm acquired as a part, the gate asked once per primitive, the grant turned into a run mandate, and the arm's loop ticked from sensor lines on stdin
//! BFS  DEFINES  Seat, ending, run
//! BFS  USES     brainos: abi, body, bodyd, crypto, entity, kira, motor, part | external: std
//! BFS  USED BY  host/motor, host/tests/motor_mandate
//!
//! The seat and the body for a simulated desk arm, in one process.
//!
//! The simulator is only physics. It writes what the arm feels, one
//! line per millisecond, and reads back six efforts. Everything else is
//! here: the entity, its gate, the arm acquired as a part through the
//! ordinary §8 path, and the arm's loop.
//!
//! A request to run a primitive arrives as an `ask` line (from the
//! separate cognition process, relayed by the harness). It goes through
//! `Entity::propose` like any other request. A grant becomes a
//! `Mandate`, then a `RunMandate`, then a run. The gate is asked once
//! per primitive and never per tick; `gate_calls` counts.
//!
//! Lines in:
//!   s T_US q*6 dq*6 TOOL_N GRIP_N SLIP    one tick of sensing
//!   ask ID VERB k=v ...                   a request from cognition
//!   style fluid|stop-go
//!   quit
//! Lines out:
//!   c e*6 RUNNING GOAL STIFF SQUEEZE SLIP YIELD WAIT STANDING   (one per `s`)
//!   up ID granted|queued|denied CODE|refused WHY
//!   up ID ended HOW applied=N peak=F ms=T
//!   up pose X Y Z

use brainos_abi::ids::BodyId;
use brainos_abi::time::{Instant, Tick};
use brainos_abi::verb::Verb;
use brainos_body::policy::Arg;
use brainos_bodyd::armloop::{part_schema, ArmBody, DeskArm, Drive, Event, Sense, HOLD};
use brainos_bodyd::deskarm::{DeskArmConfig, Pose};
use brainos_crypto::key::BrainKey;
use brainos_entity::entity::{Entity, Wiring};
use brainos_kira::action::Request;
use brainos_kira::gate::Verdict;
use brainos_motor::control::Style;
use brainos_motor::mandate::{RunMandate, P_DURATION_MS};
use brainos_motor::primitive::{desk_arm_library, place, program, Frame, Given, MotorPrimitive};
use brainos_motor::run::{Ending, Summary};
use brainos_part::nerve::{Mandate, Sensed};
use std::io::{BufRead, Write};
use std::process::ExitCode;

pub struct Seat {
    pub e: Entity,
    pub arm: BodyId,
    pub body: ArmBody,
    pub lib: Vec<MotorPrimitive>,
    /// how many times the gate has been asked
    pub gate_calls: u64,
    last_feel_us: u64,
    peak_load: f32,
}

impl Seat {
    pub fn new(style: Style) -> Result<Seat, String> {
        let mut e = Entity::new(BrainKey::from_seed(&[0x4D; 32]), Tick(0));
        e.wake(None, None, Tick(1)).map_err(|_| "the entity did not wake".to_string())?;
        let part_key = BrainKey::from_seed(&[0xA7; 32]);
        let lib = desk_arm_library();
        // the arm's id is not known until it is acquired; the schema
        // does not depend on it
        let probe = DeskArm {
            config: DeskArmConfig::default(),
            id: BodyId::from_bytes([0; 32]),
        };
        let schema = part_schema(&probe, &lib);
        let arm = e
            .acquire_part(part_key.public(), |c| part_key.answer(c), schema, Wiring::default())
            .map_err(|err| format!("the arm was not acquired: {err:?}"))?;
        let mut body = ArmBody::new(DeskArmConfig::default(), arm, Instant(0));
        body.style = style;
        let mut seat = Seat {
            e,
            arm,
            body,
            lib,
            gate_calls: 0,
            last_feel_us: 0,
            peak_load: 0.0,
        };
        // the first feeling, so the part's vitals are known before
        // anything is asked of it
        seat.feel(Instant(1), 0.0, 0.0);
        Ok(seat)
    }

    fn feel(&mut self, now: Instant, load: f32, tool_n: f32) {
        self.e.feel(
            &self.arm,
            &[
                Sensed::new("motor.load", load as f64),
                Sensed::new("tool.force", tool_n as f64),
            ],
            now,
        );
    }

    /// Put a request to the gate and, if it is granted, turn the grant
    /// into a run mandate. This is the only place in this file that
    /// makes one, and it cannot make one without the gate saying yes.
    pub fn grant(&mut self, verb: &str, args: &[(String, f64)]) -> Result<RunMandate, String> {
        let v = Verb::parse(verb).map_err(|_| "refused bad-verb".to_string())?;
        let mut r = Request::on_body(v, self.arm, self.e.tick);
        for (k, val) in args {
            let arg = if k == P_DURATION_MS {
                Arg::Count(*val as i64)
            } else {
                Arg::Scalar(*val)
            };
            r = r.arg(k, arg);
        }
        self.gate_calls += 1;
        let granted = match self.e.propose(r.clone()).verdict {
            Verdict::Granted(g) => g,
            Verdict::Denied(d) => return Err(format!("denied {:?}/{:?}", d.stage, d.code)),
        };
        let mandate = Mandate::check(granted, &r, &self.e.key().public(), self.e.tick)
            .map_err(|e| format!("refused token-{e:?}"))?;
        RunMandate::bounded(mandate).map_err(|e| format!("refused mandate-{e:?}"))
    }

    /// One request from cognition. `Ok(true)` means it was queued behind
    /// the run in progress.
    pub fn ask(&mut self, verb: &str, args: &[(String, f64)], q: &Pose, now: Instant) -> Result<bool, String> {
        let run = self.grant(verb, args)?;
        if verb == HOLD {
            return self
                .body
                .stand(run, q, now)
                .map(|_| false)
                .map_err(|b| format!("refused {}", b.name()));
        }
        let Some(prim) = self.lib.iter().find(|p| p.verb.as_str() == verb) else {
            return Err("refused no-such-primitive".to_string());
        };
        let names: Vec<String> = prim.params.iter().map(|p| p.name.clone()).collect();
        let values: Vec<(&str, f32)> = names
            .iter()
            .filter_map(|n| run.param(n).map(|v| (n.as_str(), v)))
            .collect();
        let strokes = program(&Given { primitive: prim, values: &values })
            .map_err(|why| format!("refused {}", why.replace(' ', "-")))?;
        let need = |n: &str| run.param(n).ok_or_else(|| format!("refused missing-{n}"));
        let frame = Frame {
            pos: [need("fx")?, need("fy")?, need("fz")?],
            yaw_deg: need("fyaw")?,
        };
        let goals = place(&strokes, &frame);
        let queued = self.body.is_running();
        self.body
            .start(run, goals, q, now)
            .map(|_| queued)
            .map_err(|b| format!("refused {}", b.name()))
    }

    pub fn tick(&mut self, now: Instant, s: &Sense) -> (Drive, Option<Event>) {
        let (drive, event) = self.body.tick(now, s);
        let load = (0..5)
            .map(|i| drive.effort[i].abs() / self.body.arm.config.torque_nm)
            .fold(0.0f32, f32::max);
        self.peak_load = self.peak_load.max(load);
        if now.0.saturating_sub(self.last_feel_us) >= 100_000 {
            self.last_feel_us = now.0;
            let l = self.peak_load;
            self.peak_load = 0.0;
            self.feel(now, l, s.tool_n);
        }
        (drive, event)
    }
}

pub fn ending(s: &Summary) -> String {
    let how = match s.ending {
        Ending::Finished => "finished".to_string(),
        Ending::Breached(b) => format!("breach:{}", b.name()),
    };
    format!(
        "{how} applied={} peak={:.3} ms={}",
        s.applied,
        s.peak_felt_n,
        s.lasted.as_millis()
    )
}

/// `brainos motor seat [--stop-go]`
pub fn run(rest: &[String]) -> ExitCode {
    let style = if rest.iter().any(|a| a == "--stop-go") {
        Style::STOP_GO
    } else {
        Style::FLUID
    };
    let mut seat = match Seat::new(style) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let stdin = std::io::stdin();
    let mut out = std::io::BufWriter::new(std::io::stdout());
    let mut q: Pose = [0.0; 6];
    let mut sensed = false;
    let mut now = Instant(0);
    // ids of the runs in order: the one running, then the one queued
    let mut ids: std::collections::VecDeque<String> = std::collections::VecDeque::new();
    let mut hold_id: Option<String> = None;
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let mut f = line.split_whitespace();
        match f.next() {
            Some("s") => {
                let v: Vec<f32> = f.filter_map(|x| x.parse().ok()).collect();
                if v.len() != 16 {
                    let _ = writeln!(out, "c 0 0 0 0 0 0 0 0 0 0 0 0 0 0");
                    let _ = out.flush();
                    continue;
                }
                now = Instant(v[0] as u64);
                let mut s = Sense::default();
                s.q.copy_from_slice(&v[1..7]);
                s.dq.copy_from_slice(&v[7..13]);
                s.tool_n = v[13];
                s.grip_n = v[14];
                s.slip_mps = v[15];
                q = s.q;
                sensed = true;
                let (d, event) = seat.tick(now, &s);
                match event {
                    Some(Event::Ended(sum)) => {
                        let id = ids.pop_front().unwrap_or_default();
                        let _ = writeln!(out, "up {id} ended {}", ending(&sum));
                        if let Some(p) = seat.body.arm.config.tool(&q).pos.get(0..3) {
                            let _ = writeln!(out, "up pose {} {} {}", p[0], p[1], p[2]);
                        }
                        if matches!(sum.ending, Ending::Breached(_)) {
                            // a breach drops whatever was queued too
                            for id in ids.drain(..) {
                                let _ = writeln!(out, "up {id} ended dropped applied=0 peak=0 ms=0");
                            }
                        }
                    }
                    Some(Event::Unreachable(sum)) | Some(Event::NothingGripped(sum)) => {
                        let why = if matches!(event, Some(Event::Unreachable(_))) {
                            "unreachable"
                        } else {
                            "nothing-gripped"
                        };
                        let id = ids.pop_front().unwrap_or_default();
                        let _ = writeln!(out, "up {id} ended {why} applied={} peak={:.3} ms={}", sum.applied, sum.peak_felt_n, sum.lasted.as_millis());
                        for id in ids.drain(..) {
                            let _ = writeln!(out, "up {id} ended dropped applied=0 peak=0 ms=0");
                        }
                    }
                    Some(Event::StandingEnded(sum)) => {
                        let id = hold_id.take().unwrap_or_default();
                        let _ = writeln!(out, "up {id} ended {}", ending(&sum));
                    }
                    Some(Event::Joined) | None => {}
                }
                let e = d.effort;
                let _ = writeln!(
                    out,
                    "c {} {} {} {} {} {} {} {} {:.3} {:.3} {} {} {} {}",
                    e[0], e[1], e[2], e[3], e[4], e[5],
                    d.running as u8, d.goal, d.stiff, d.squeeze_n,
                    d.slip_fired as u8, d.yield_fired as u8, d.wait_fired as u8, d.standing as u8
                );
                let _ = out.flush();
            }
            Some("ask") => {
                let (Some(id), Some(verb)) = (f.next(), f.next()) else { continue };
                if !sensed {
                    // nothing is started from a posture nobody has felt
                    let _ = writeln!(out, "up {id} refused not-sensed-yet");
                    let _ = out.flush();
                    continue;
                }
                let args: Vec<(String, f64)> = f
                    .filter_map(|kv| kv.split_once('='))
                    .filter_map(|(k, v)| v.parse().ok().map(|v| (k.to_string(), v)))
                    .collect();
                match seat.ask(verb, &args, &q, now) {
                    Ok(queued) => {
                        if verb == HOLD {
                            hold_id = Some(id.to_string());
                        } else {
                            ids.push_back(id.to_string());
                        }
                        let _ = writeln!(out, "up {id} {}", if queued { "queued" } else { "granted" });
                    }
                    Err(why) => {
                        let _ = writeln!(out, "up {id} {why}");
                    }
                }
                let _ = out.flush();
            }
            Some("style") => {
                seat.body.style = match f.next() {
                    Some("stop-go") => Style::STOP_GO,
                    _ => Style::FLUID,
                };
            }
            Some("stat") => {
                let _ = writeln!(
                    out,
                    "up stat gate_calls={} ticks={} refused={} slips={} yields={} waits={} nodes={}",
                    seat.gate_calls,
                    seat.body.ticks,
                    seat.body.ticks_refused,
                    seat.body.arcs().slips,
                    seat.body.arcs().yields,
                    seat.body.arcs().waits,
                    seat.e.graph.len()
                );
                let _ = out.flush();
            }
            Some("quit") => break,
            _ => {}
        }
    }
    ExitCode::SUCCESS
}
