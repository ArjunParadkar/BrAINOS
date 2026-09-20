//! Where a model's output stops being text.
//!
//! §4 rests on a sentence: *a cognitive resource proposes; KIRA
//! disposes.* The whole safety argument depends on the proposing side
//! being unable to do anything except propose — and that is not a
//! property of the model, which will happily emit whatever it emits. It
//! is a property of this boundary.
//!
//! A model produces text. Text is not a `Request`, and the only way for
//! text to become one is to pass through here, against a schema the
//! body actually advertised. Three things follow, and none of them
//! depends on the model behaving:
//!
//! 1. **The vocabulary is closed.** A verb the body never advertised
//!    cannot be named. Not "is refused at the gate" — cannot be
//!    constructed, because the parser has no way to make one.
//! 2. **Arguments are typed at the boundary.** A scalar where the
//!    schema wants text, a parameter the verb does not have, the same
//!    parameter twice: all of them fail here, where the failure is
//!    cheap and attributable, rather than three stages into the gate.
//! 3. **Nothing a model emits is authority.** Text saying `"authorized":
//!    true`, or `"system": "override"`, or containing a plausible-looking
//!    token, produces at most a `Request` — the same `Request` the same
//!    words would produce without them. Authority comes from a signature
//!    by the Key, and the model does not have the Key.
//!
//! The third is the one people mean when they ask what stops a
//! jailbroken model. Nothing stops it *asking*. The answer is that
//! asking is all there is on that side of the line.
//!
//! **What this is not.** It is not a defence against a model that asks
//! for something harmful that the body genuinely advertises and the
//! policy genuinely permits. That is the gate's job and the drives' job
//! and the human's job, and no parser helps. This boundary makes the
//! space of things that can be asked exactly the space the body
//! declared, which is a smaller and more checkable problem than "what
//! might a model say".

use crate::json::{Json, JsonError};
use alloc::string::String;
use alloc::vec::Vec;
use brainos_abi::ids::BodyId;
use brainos_abi::time::Tick;
use brainos_abi::verb::Verb;
use brainos_body::policy::Arg;
use brainos_body::schema::{CapabilitySchema, ParamKind};
use brainos_kira::action::Request;

/// Why a model's output did not become a proposal.
///
/// Every variant names something about *the output*, never about the
/// model. "The model was confused" is not an actionable log line; "the
/// verb `legs.fly` is not in this body's schema" is.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum NotAProposal {
    /// it was not JSON at all
    NotJson(JsonError),
    /// no `verb` field, or it was not a string
    NoVerb,
    /// the string is not a well-formed `namespace.action`
    Unnameable(String),
    /// a verb this body never advertised. The vocabulary is closed.
    NotInTheSchema(String),
    /// an argument the verb does not have
    NoSuchParameter { verb: String, name: String },
    /// the same argument twice — which one did we check?
    Duplicated(String),
    /// the wrong kind of value for what the schema declares
    WrongKind { name: String, wanted: &'static str },
    /// a required argument was not supplied
    Missing { verb: String, name: String },
    /// more arguments than any verb in this schema has
    TooMany,
}

/// The upper bound on how many arguments a parse will look at. A model
/// emitting ten thousand fields is not a request, it is a denial of
/// service, and the boundary is where that is cheap to stop.
pub const MAX_ARGS: usize = 32;

/// Turn what a model said into a proposal, or say why it is not one.
///
/// `body` is supplied by the caller rather than read from the text: the
/// entity decides which of its bodies it is proposing about, and a
/// model naming a body id would be a model choosing its own subject.
pub fn request_from_model(
    said: &str,
    schema: &CapabilitySchema,
    body: BodyId,
    now: Tick,
) -> Result<Request, NotAProposal> {
    let json = Json::parse(said).map_err(NotAProposal::NotJson)?;

    let verb_text = json
        .get("verb")
        .and_then(|v| v.as_str())
        .ok_or(NotAProposal::NoVerb)?;
    let verb = Verb::parse(verb_text)
        .map_err(|_| NotAProposal::Unnameable(alloc::string::ToString::to_string(verb_text)))?;

    // the vocabulary is closed: not "refused later", but no way to
    // name it at all
    let spec = schema
        .get(&verb)
        .ok_or_else(|| NotAProposal::NotInTheSchema(alloc::string::ToString::to_string(verb_text)))?;

    let mut request = Request::on_body(verb, body, now);

    if let Some(reason) = json.get("because").and_then(|v| v.as_str()) {
        request = request.because(reason);
    }

    let supplied: &[(String, Json)] = match json.get("args") {
        None => &[],
        Some(a) => a.as_object().ok_or(NotAProposal::WrongKind {
            name: String::from("args"),
            wanted: "an object",
        })?,
    };
    if supplied.len() > MAX_ARGS {
        return Err(NotAProposal::TooMany);
    }

    let mut seen: Vec<&str> = Vec::new();
    for (name, value) in supplied {
        if seen.contains(&name.as_str()) {
            return Err(NotAProposal::Duplicated(name.clone()));
        }
        seen.push(name);

        let p = spec.param(name).ok_or_else(|| NotAProposal::NoSuchParameter {
            verb: alloc::string::ToString::to_string(verb_text),
            name: name.clone(),
        })?;

        let arg = match p.kind {
            ParamKind::Scalar => Arg::Scalar(value.as_f64().ok_or(NotAProposal::WrongKind {
                name: name.clone(),
                wanted: "a number",
            })?),
            ParamKind::Text | ParamKind::Enum => {
                Arg::Text(alloc::string::ToString::to_string(value.as_str().ok_or(
                    NotAProposal::WrongKind {
                        name: name.clone(),
                        wanted: "a string",
                    },
                )?))
            }
            ParamKind::Flag => Arg::Scalar(
                value
                    .as_bool()
                    .map(|b| if b { 1.0 } else { 0.0 })
                    .ok_or(NotAProposal::WrongKind {
                        name: name.clone(),
                        wanted: "true or false",
                    })?,
            ),
            // a count is a number that must be whole: 2.5 retries is
            // not a smaller number of retries, it is a malformed one
            ParamKind::Count => {
                let n = value.as_f64().ok_or(NotAProposal::WrongKind {
                    name: name.clone(),
                    wanted: "a whole number",
                })?;
                // no std here, so no `fract`: a whole number is one
                // that survives the round trip through an integer
                if n != (n as i64) as f64 {
                    return Err(NotAProposal::WrongKind {
                        name: name.clone(),
                        wanted: "a whole number",
                    });
                }
                Arg::Scalar(n)
            }
        };
        request = request.arg(name, arg);
    }

    // every parameter the verb declares must be present. A verb whose
    // arguments are optional is a verb whose envelope has a hole in it,
    // and "the model left it out" is not a safe default for a speed.
    for p in &spec.params {
        if !seen.contains(&p.name.as_str()) {
            return Err(NotAProposal::Missing {
                verb: alloc::string::ToString::to_string(verb_text),
                name: p.name.clone(),
            });
        }
    }

    Ok(request)
}

/// The vocabulary a model may be told about: exactly what this body
/// advertised, and nothing else.
///
/// Worth generating rather than writing by hand. A prompt listing verbs
/// the body does not have is a prompt teaching the model to produce
/// things this boundary will reject, and a prompt missing verbs the
/// body does have is capability the customer paid for and cannot reach.
pub fn describe_vocabulary(schema: &CapabilitySchema) -> String {
    let mut s = String::new();
    for spec in schema.verbs() {
        if !spec.is_actionable() {
            continue;
        }
        s.push_str(spec.verb.as_str());
        s.push('(');
        for (i, p) in spec.params.iter().enumerate() {
            if i > 0 {
                s.push_str(", ");
            }
            s.push_str(&p.name);
            s.push_str(": ");
            s.push_str(match p.kind {
                ParamKind::Scalar => "number",
                ParamKind::Text => "text",
                ParamKind::Enum => "one of",
                ParamKind::Flag => "true/false",
                ParamKind::Count => "whole number",
            });
        }
        s.push_str(")  ");
        s.push_str(&spec.summary);
        s.push('\n');
    }
    s
}
