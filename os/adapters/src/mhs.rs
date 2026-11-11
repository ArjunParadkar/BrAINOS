//! BFS  LEVEL    Section 8 Acquisition - adapters (ROS 2, MHS, the model boundary)  [crate brainos-adapters]
//! BFS  PURPOSE  Translates a Model Hardware Standard (MHS) device's JSON tool listing, including its min/max bounds, into a CapabilitySchema the entity can use
//! BFS  DEFINES  MhsError, from_mhs
//! BFS  USES     this crate: json | brainos: abi, body, rt
//! BFS  USED BY  adapters/lib, adapters/tests/adapters_suite, host/fullstack, host/life, host/migrate, host/tests/fullstack_suite, proof/hostile
//!
//! The MHS adapter.
//!
//! MHS is Anthropic's Model Hardware Standard: a shared specification
//! that lets models discover and operate physical equipment through MCP.
//! It sits in the middleware row — a driver standard, a competitor to
//! ROS 2, not to us. In the metaphor the team briefing uses, it is
//! peripheral nerves: it carries signal to the actuator and readings
//! back, knows nothing about who it belongs to, and remembers nothing.
//!
//! **Why this is the easier of the two adapters.** MHS devices declare
//! their tools with JSON Schema, and JSON Schema has `minimum` and
//! `maximum`. A ROS topic says "this is a Twist"; an MHS tool says "this
//! takes a speed between 0 and 0.6 m/s". The second is a capability
//! schema already; this module is mostly a translation.
//!
//! **What MHS does not have, and we supply.** No persistent identity, no
//! memory across sessions, no gate below the model. As shipped, its
//! framing is a model as a junior operator under supervision: pre-flight
//! bounds checks declared by the device, human approval workflows in the
//! loop. Every MHS device is therefore a body BrAInOS can inhabit,
//! pre-wrapped by its manufacturer, for free — and what we add is the
//! self that inhabits it.
//!
//! **One vocabulary rule.** MHS says *agent*. We say *entity*. This
//! module reads their word off the wire and never emits it; a device
//! description is data, and its vocabulary does not become ours.

use crate::json::{Json, JsonError};
use alloc::string::String;
use alloc::vec::Vec;
use brainos_abi::verb::Verb;
use brainos_body::region::RegionClass;
use brainos_body::schema::{CapabilitySchema, ParamKind, ParamSpec, VerbSpec};
use brainos_rt::envelope::Channel;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MhsError {
    Json(JsonError),
    /// the description is not an object with a `tools` array
    NotADeviceDescription,
    /// no tool in the description could be named as a verb
    NothingUsable,
    /// the description declares the same parameter twice with different
    /// bounds — a description nobody should act on
    Contradictory,
}

/// Read an MHS device description into a capability schema.
pub fn from_mhs(description: &str) -> Result<CapabilitySchema, MhsError> {
    let doc = Json::parse(description).map_err(MhsError::Json)?;
    if doc.is_duplicated() {
        return Err(MhsError::Contradictory);
    }

    let label = doc
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("mhs device");
    let tools = doc
        .get("tools")
        .and_then(|v| v.as_array())
        .ok_or(MhsError::NotADeviceDescription)?;

    let default_class = doc
        .get("device_type")
        .and_then(|v| v.as_str())
        .map(RegionClass::from_offer)
        .unwrap_or(RegionClass::Tool);

    let mut schema = CapabilitySchema::new(label, "mhs");

    for tool in tools {
        let Some(name) = tool.get("name").and_then(|v| v.as_str()) else {
            continue;
        };
        let class = tool
            .get("region")
            .and_then(|v| v.as_str())
            .map(RegionClass::from_offer)
            .unwrap_or(default_class);
        if class.is_afferent_only() {
            continue;
        }
        let Some(verb) = verb_for(name, class) else {
            continue;
        };

        let summary = tool
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or(name);
        let mut spec = VerbSpec::new(verb, class, summary);

        // MHS mirrors JSON Schema: input_schema.properties, with
        // `required` naming the ones that must be supplied
        if let Some(props) = tool
            .get("input_schema")
            .and_then(|s| s.get("properties"))
            .and_then(|p| p.as_object())
        {
            for (pname, pschema) in props {
                spec = spec.with(param_from_schema(pname, pschema));
            }
        }

        // the device may say an operation cannot be undone; MHS devices
        // that do are exactly the ones worth believing
        if tool
            .get("irreversible")
            .and_then(|v| v.as_bool())
            .unwrap_or(class.is_consequential())
        {
            spec = spec.irreversible();
        }
        if let Some(ms) = tool.get("timeout_ms").and_then(|v| v.as_f64()) {
            if ms > 0.0 && ms < 600_000.0 {
                spec = spec.taking_up_to(ms as u32);
            }
        }

        schema.declare(spec);
    }

    if schema.is_empty() {
        return Err(MhsError::NothingUsable);
    }
    Ok(schema)
}

/// One JSON Schema property becomes one `ParamSpec`.
///
/// The bound is taken only from an explicit `minimum`/`maximum`. A
/// property with a type and no bounds is `unbounded`, which makes the
/// whole verb unactionable — the correct outcome for a device that
/// declared a motor and would not say how far it goes.
fn param_from_schema(name: &str, schema: &Json) -> ParamSpec {
    let ty = schema.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let unit = schema.get("unit").and_then(|v| v.as_str()).unwrap_or("");

    match ty {
        "number" | "integer" => {
            let min = schema.get("minimum").and_then(|v| v.as_f64());
            let max = schema.get("maximum").and_then(|v| v.as_f64());
            match (min, max) {
                (Some(lo), Some(hi)) if lo <= hi => {
                    if ty == "integer" {
                        ParamSpec::count(name, lo as i64, hi as i64)
                    } else {
                        ParamSpec::scalar(name, channel_for(name, unit), lo, hi)
                    }
                }
                // half a bound is not a bound: a motor with a maximum and
                // no minimum can still be driven to negative infinity
                _ => ParamSpec::unbounded(
                    name,
                    if ty == "integer" {
                        ParamKind::Count
                    } else {
                        ParamKind::Scalar
                    },
                ),
            }
        }
        "boolean" => ParamSpec::flag(name),
        "string" => {
            if let Some(choices) = schema.get("enum").and_then(|v| v.as_array()) {
                let names: Vec<&str> = choices.iter().filter_map(|c| c.as_str()).collect();
                if !names.is_empty() {
                    return ParamSpec::choice(name, &names);
                }
            }
            match schema.get("maxLength").and_then(|v| v.as_f64()) {
                Some(n) if n > 0.0 && n < 100_000.0 => ParamSpec::text(name, n as u32),
                // an unbounded string on a device that can move is a
                // place for an injected command to live
                _ => ParamSpec::unbounded(name, ParamKind::Text),
            }
        }
        _ => ParamSpec::unbounded(name, ParamKind::Text),
    }
}

/// Map a parameter's unit onto a physical channel, so the envelope and
/// the schema agree about what is being bounded.
fn channel_for(name: &str, unit: &str) -> Channel {
    let u = unit.to_ascii_lowercase();
    let n = name.to_ascii_lowercase();
    match u.as_str() {
        "deg" | "degree" | "degrees" | "rad" => Channel::Angle,
        "deg/s" | "rad/s" | "dps" => Channel::Rate,
        "a" | "amp" | "amps" | "ampere" | "ma" => Channel::Current,
        "v" | "volt" | "volts" => Channel::Voltage,
        "c" | "celsius" | "degc" => Channel::Temperature,
        "m/s" | "mm/s" | "mps" => Channel::Speed,
        "m" | "mm" | "cm" => Channel::Clearance,
        _ => {
            if n.contains("angle") || n.contains("position") || n.contains("rotation") {
                Channel::Angle
            } else if n.contains("speed") || n.contains("velocity") {
                Channel::Speed
            } else if n.contains("current") || n.contains("force") || n.contains("torque") {
                Channel::Current
            } else if n.contains("temp") {
                Channel::Temperature
            } else {
                Channel::Angle
            }
        }
    }
}

/// MHS tool names are `snake_case`; the verb grammar wants
/// `namespace.action`.
fn verb_for(tool: &str, class: RegionClass) -> Option<Verb> {
    let mut action = String::new();
    for c in tool.chars() {
        if c.is_ascii_alphanumeric() {
            action.push(c.to_ascii_lowercase());
        } else if !action.is_empty() && !action.ends_with('_') {
            action.push('_');
        }
        if action.len() >= 23 {
            break;
        }
    }
    let action = action.trim_end_matches('_');
    if action.is_empty() {
        return None;
    }
    let mut v = String::new();
    v.push_str(class.name());
    v.push('.');
    v.push_str(action);
    Verb::parse(&v).ok()
}
