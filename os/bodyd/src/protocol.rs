//! BFS  LEVEL    Domain 2 Autonomic - the runtime on the body's own computer  [crate brainos-bodyd]
//! BFS  PURPOSE  Defines the only messages between seat and body (a verb Command down; telemetry, refusal and stop Reports up) and their byte encoding
//! BFS  DEFINES  Command, Report, Wire
//! BFS  USES     this crate: mechanism | brainos: abi, transport
//! BFS  USED BY  bodyd/lib, bodyd/runtime, bodyd/tests/bodyd_suite, host/fullstack, host/tests/fullstack_suite, proof/envelopes, proof/hostile
//!
//! What the seat and the body say to each other.
//!
//! Deliberately small. The seat sends a verb and its arguments; the body
//! sends back telemetry, refusals and stops. There is no "set
//! configuration", no "run script", no general remote-procedure call —
//! every one of those would be a way for the seat to make the body do
//! something the body did not itself check.
//!
//! Frames are carried by `brainos-transport`, which supplies framing,
//! authentication and freshness. This module only says what goes inside
//! one, and its encoding is the ABI's canonical codec so that the wire
//! and the audit log agree byte for byte about what was asked.

use crate::mechanism::Refusal;
use alloc::string::String;
use alloc::vec::Vec;
use brainos_abi::codec::{Decode, Decoder, Encode, Encoder};
use brainos_abi::error::{Error, Result};
use brainos_abi::verb::Verb;

/// The seat asking for something.
#[derive(Clone, Debug, PartialEq)]
pub struct Command {
    pub verb: Verb,
    /// named scalars, sorted, so the encoding is canonical
    pub args: Vec<(String, f32)>,
    /// the capability id the seat minted for this, so the body's
    /// telemetry can name which decision it is answering. The body does
    /// **not** treat it as authority — it re-checks everything itself —
    /// but quoting it back is what lets the seat match a refusal to the
    /// grant that provoked it.
    pub grant: [u8; 32],
}

impl Command {
    pub fn new(verb: Verb) -> Command {
        Command {
            verb,
            args: Vec::new(),
            grant: [0; 32],
        }
    }

    pub fn arg(mut self, name: &str, value: f32) -> Command {
        self.args
            .push((alloc::string::ToString::to_string(name), value));
        self.args.sort_by(|a, b| a.0.cmp(&b.0));
        self
    }

    pub fn under(mut self, grant: [u8; 32]) -> Command {
        self.grant = grant;
        self
    }

    /// Borrowed view, for handing to a `Mechanism`.
    pub fn as_args(&self) -> Vec<(&str, f32)> {
        self.args.iter().map(|(n, v)| (n.as_str(), *v)).collect()
    }
}

impl Encode for Command {
    fn encode(&self, e: &mut Encoder) {
        self.verb.encode(e);
        e.seq(self.args.len());
        for (n, v) in &self.args {
            e.str(n);
            e.f32(*v);
        }
        e.raw(&self.grant);
    }
}

impl Decode for Command {
    fn decode(d: &mut Decoder) -> Result<Command> {
        let verb = Verb::decode(d)?;
        let n = d.seq()?;
        let mut args = Vec::new();
        for _ in 0..n {
            let name = alloc::string::ToString::to_string(d.str()?);
            args.push((name, d.f32()?));
        }
        // a command with the same argument twice is one where "which did
        // the body use" is unanswerable
        for i in 1..args.len() {
            if args[i].0 == args[i - 1].0 {
                return Err(Error::Malformed("an argument is supplied twice"));
            }
        }
        let mut grant = [0u8; 32];
        d.read_exact(&mut grant)?;
        Ok(Command { verb, args, grant })
    }
}

/// What the body says back.
#[derive(Clone, Debug, PartialEq)]
pub enum Report {
    /// the motion was accepted and is being played
    Accepted {
        verb: Verb,
        samples: u32,
        duration_ms: u32,
        worst_margin: f32,
    },
    /// the body refused something the seat granted
    Refused {
        verb: Verb,
        refusal: Refusal,
        /// true when this is the body disagreeing with the seat about
        /// physics rather than a configuration mismatch
        disagreement: bool,
    },
    /// the motion finished
    Finished { verb: Verb, samples_played: u32 },
    /// a reflex stopped the body
    Stopped {
        reason: u8,
        /// how far into the motion it got
        samples_played: u32,
    },
    /// ordinary telemetry
    Telemetry {
        rail_v: f32,
        current_a: f32,
        tilt_deg: f32,
        hottest_c: f32,
        margin_mm: f32,
        stops: u32,
        overruns: u32,
    },
}

impl Encode for Report {
    fn encode(&self, e: &mut Encoder) {
        match self {
            Report::Accepted {
                verb,
                samples,
                duration_ms,
                worst_margin,
            } => {
                e.u8(0);
                verb.encode(e);
                e.u32(*samples);
                e.u32(*duration_ms);
                e.f32(*worst_margin);
            }
            Report::Refused {
                verb,
                refusal,
                disagreement,
            } => {
                e.u8(1);
                verb.encode(e);
                e.u8(*refusal as u8);
                e.bool(*disagreement);
            }
            Report::Finished {
                verb,
                samples_played,
            } => {
                e.u8(2);
                verb.encode(e);
                e.u32(*samples_played);
            }
            Report::Stopped {
                reason,
                samples_played,
            } => {
                e.u8(3);
                e.u8(*reason);
                e.u32(*samples_played);
            }
            Report::Telemetry {
                rail_v,
                current_a,
                tilt_deg,
                hottest_c,
                margin_mm,
                stops,
                overruns,
            } => {
                e.u8(4);
                e.f32(*rail_v);
                e.f32(*current_a);
                e.f32(*tilt_deg);
                e.f32(*hottest_c);
                e.f32(*margin_mm);
                e.u32(*stops);
                e.u32(*overruns);
            }
        }
    }
}

impl Decode for Report {
    fn decode(d: &mut Decoder) -> Result<Report> {
        Ok(match d.u8()? {
            0 => Report::Accepted {
                verb: Verb::decode(d)?,
                samples: d.u32()?,
                duration_ms: d.u32()?,
                worst_margin: d.f32()?,
            },
            1 => {
                let verb = Verb::decode(d)?;
                let refusal = Refusal::from_u8(d.u8()?)
                    .ok_or(Error::Malformed("unknown refusal"))?;
                Report::Refused {
                    verb,
                    refusal,
                    disagreement: d.bool()?,
                }
            }
            2 => Report::Finished {
                verb: Verb::decode(d)?,
                samples_played: d.u32()?,
            },
            3 => Report::Stopped {
                reason: d.u8()?,
                samples_played: d.u32()?,
            },
            4 => Report::Telemetry {
                rail_v: d.f32()?,
                current_a: d.f32()?,
                tilt_deg: d.f32()?,
                hottest_c: d.f32()?,
                margin_mm: d.f32()?,
                stops: d.u32()?,
                overruns: d.u32()?,
            },
            _ => return Err(Error::Malformed("unknown report kind")),
        })
    }
}

/// The frame kinds this protocol uses, named so the mapping is in one
/// place rather than spread across call sites.
pub struct Wire;

impl Wire {
    pub const COMMAND: brainos_transport::frame::Kind = brainos_transport::frame::Kind::Command;
    pub const TELEMETRY: brainos_transport::frame::Kind =
        brainos_transport::frame::Kind::Telemetry;
    pub const STOPPED: brainos_transport::frame::Kind = brainos_transport::frame::Kind::Stopped;

    pub fn encode_command(c: &Command) -> Result<Vec<u8>> {
        c.to_bytes()
    }

    pub fn decode_command(raw: &[u8]) -> Result<Command> {
        Command::from_bytes(raw)
    }

    pub fn encode_report(r: &Report) -> Result<Vec<u8>> {
        r.to_bytes()
    }

    pub fn decode_report(raw: &[u8]) -> Result<Report> {
        Report::from_bytes(raw)
    }
}
