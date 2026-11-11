//! BFS  LEVEL    Domain 2 Autonomic - the BLUR quadruped body (wire, acquisition, supervisor)  [crate brainos-blur]
//! BFS  PURPOSE  Maps a granted skill request to the motion limits it allows, and the body-side check that verifies the grant's signature before obeying it
//! BFS  DEFINES  SKILL_STAND, SKILL_SIT, SKILL_LIE, SKILL_STANDUP, SKILL_WALK, SKILL_HOP, SKILL_POSE, SKILL_GRIP, skill_code, session_envelope, session_ms, Refused, +1 more
//! BFS  USES     this crate: descriptor, supervisor | brainos: abi, body, crypto, kira
//! BFS  USED BY  blur/policy, blur/tests/blur_suite, host/blur_gate2
//!
//! Skill sessions: what one KIRA grant lets BLUR do, and the check the
//! body runs before it believes the grant.
//!
//! KIRA gates *sessions*, not motor ticks. "walk, vx ≤ 0.5 m/s, 60 s" is
//! one decision through the eight stages; the Autonomic layer then holds
//! the body inside the envelope that decision implies, a thousand times a
//! second, without asking again.
//!
//! The envelope is a pure function of the verb and its arguments
//! (`session_envelope`), computed the same way at the seat (to tell the
//! audit what was granted) and on the Teensy (to enforce it). The Teensy
//! does not take the seat's word for it: `admit` verifies the grant's
//! seal against the entity's public key, checks it authorizes *this*
//! request now, on *this* body, and only then derives the envelope from
//! the request's own arguments. A model that writes "authorized" into its
//! output has produced text; it has not produced a signature.

use crate::descriptor::{actuator_index, ACTUATORS, N_ACT};
use crate::supervisor::{Envelope, Grant};
use brainos_abi::ids::BodyId;
use brainos_abi::time::Tick;
use brainos_abi::verb::Verb;
use brainos_body::policy::Arg;
use brainos_crypto::key::PublicKey;
use brainos_kira::action::Request;
use brainos_kira::capability::Granted;

/// Skill codes on the wire (a byte, not a string, inside the Teensy).
pub const SKILL_STAND: u8 = 1;
pub const SKILL_SIT: u8 = 2;
pub const SKILL_LIE: u8 = 3;
pub const SKILL_STANDUP: u8 = 4;
pub const SKILL_WALK: u8 = 5;
pub const SKILL_HOP: u8 = 6;
pub const SKILL_POSE: u8 = 7;
pub const SKILL_GRIP: u8 = 8;

pub fn skill_code(v: &Verb) -> Option<u8> {
    Some(match v.as_str() {
        "legs.stand" => SKILL_STAND,
        "legs.sit" => SKILL_SIT,
        "legs.lie" => SKILL_LIE,
        "legs.standup" => SKILL_STANDUP,
        "legs.walk" => SKILL_WALK,
        "legs.hop" => SKILL_HOP,
        "body.pose" => SKILL_POSE,
        "tail.grip" => SKILL_GRIP,
        _ => return None,
    })
}

fn scalar(args: &[(alloc::string::String, Arg)], name: &str) -> Option<f64> {
    args.iter().find(|(n, _)| n == name).and_then(|(_, a)| a.as_scalar())
}

fn text<'a>(args: &'a [(alloc::string::String, Arg)], name: &str) -> Option<&'a str> {
    args.iter().find(|(n, _)| n == name).and_then(|(_, a)| a.as_text())
}

/// The envelope a session runs inside. Day-one sessions run at 80 % of
/// the drivers' current; tilt beyond 35° ends any session (the reflexes
/// take over); `body.pose` frees exactly one joint, inside its range, and
/// pins every other joint's range to a band around where the stand pose
/// holds it.
pub fn session_envelope(verb: &Verb, args: &[(alloc::string::String, Arg)]) -> Option<Envelope> {
    let mut e = Envelope::physical().with_current_fraction(0.8);
    e.tilt_max_deg = 35.0;
    match skill_code(verb)? {
        SKILL_HOP => {
            // a hop needs the drivers' peak for the push-off
            e = Envelope::physical();
            e.tilt_max_deg = 35.0;
        }
        SKILL_POSE => {
            let j = actuator_index(text(args, "joint")?)?;
            let angle = scalar(args, "angle")? as f32;
            let a = &ACTUATORS[j];
            if !(angle >= a.q_min && angle <= a.q_max) {
                return None;
            }
            for (k, b) in ACTUATORS.iter().enumerate() {
                if k == j {
                    continue;
                }
                let c = if b.tendon { b.q_rest } else { b.q_stand };
                let band = if b.tendon { 0.01 } else { 0.25 };
                if c.is_finite() {
                    e.q_min[k] = (c - band).max(b.q_min);
                    e.q_max[k] = (c + band).min(b.q_max);
                }
            }
            let lo = angle.min(a.q_stand) - 0.05;
            let hi = angle.max(a.q_stand) + 0.05;
            e.q_min[j] = lo.max(a.q_min);
            e.q_max[j] = hi.min(a.q_max);
        }
        _ => {}
    }
    Some(e)
}

/// How long a session lasts, in Autonomic ticks (ms): the verb's own
/// worst case from the schema would do; these are the day-one numbers.
pub fn session_ms(verb: &Verb) -> u32 {
    match skill_code(verb) {
        Some(SKILL_WALK) => 60_000,
        Some(SKILL_HOP) => 2_000,
        Some(SKILL_STANDUP) => 4_000,
        _ => 3_000,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// the seal does not verify against the entity's key
    BadSeal,
    /// the grant is for a different request, or has expired
    NotThisRequest,
    /// the grant is for another body
    WrongBody,
    /// the verb is not a skill this body runs
    UnknownSkill,
    /// the arguments do not describe an envelope inside this body's
    UnenvelopedArgs,
}

/// The Teensy's check of a grant. `now` is the entity tick the grant was
/// evaluated at (carried with it); `now_ms` the Autonomic clock.
pub fn admit(
    granted: &Granted,
    request: &Request,
    entity: &PublicKey,
    body: BodyId,
    now: Tick,
    now_ms: u32,
) -> Result<Grant, Refused> {
    if !granted.verify(entity) {
        return Err(Refused::BadSeal);
    }
    if !granted.authorizes(&request.digest(), now) || granted.verb != request.verb {
        return Err(Refused::NotThisRequest);
    }
    if granted.body != Some(body) || request.body != Some(body) {
        return Err(Refused::WrongBody);
    }
    let skill = skill_code(&request.verb).ok_or(Refused::UnknownSkill)?;
    let env = session_envelope(&request.verb, &request.args).ok_or(Refused::UnenvelopedArgs)?;
    if !env.is_within(&Envelope::physical()) {
        return Err(Refused::UnenvelopedArgs);
    }
    let _ = N_ACT;
    Ok(Grant {
        id: *granted.id.as_bytes(),
        skill,
        env,
        until_tick: now_ms.saturating_add(session_ms(&request.verb)),
    })
}
