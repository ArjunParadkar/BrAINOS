//! Shared scaffolding for the Stage 1 suites: real keys, real instances,
//! real body maps — no mocks of anything KIRA actually consults.
#![allow(dead_code)] // each test binary uses the subset it needs

use brainos_mind::body::{Region, RegionClass};
use brainos_mind::instance::Instance;
use brainos_mind::key::BrainKey;

pub fn key(n: u8) -> BrainKey {
    BrainKey::from_seed([n; 32])
}

fn region(id: &str, class: RegionClass, caps: &[&str]) -> Region {
    Region {
        id: String::from(id),
        class,
        capabilities: caps.iter().map(|c| String::from(*c)).collect(),
        proprioception: String::from("test region"),
    }
}

/// A bare instance: identity and KIRA, but an empty body map — the shape
/// of the entity before any limb is acquired.
pub fn bare_instance() -> Instance {
    Instance::new(key(7))
}

/// An instance embodied the way the Blur VM is: screen, voice, host
/// tools over the tether, and the internal limbs (notes, fs, code, ui).
pub fn embodied_instance() -> Instance {
    let mut me = Instance::new(key(7));
    me.body.incorporate(region("this-machine/screen", RegionClass::Screen, &[]));
    me.body
        .incorporate(region("host/speaker", RegionClass::Speaker, &["voice.speak"]));
    me.body
        .incorporate(region("host/mic", RegionClass::Microphone, &[]));
    me.body.incorporate(region(
        "host/tools",
        RegionClass::HostTool,
        &["sh", "web.fetch", "app.open"],
    ));
    me.body.incorporate(region(
        "key/notebook",
        RegionClass::KeyMedium,
        &["notes.read", "notes.write"],
    ));
    me.body.incorporate(region(
        "world/disk",
        RegionClass::HostTool,
        &[
            "fs.list", "fs.read", "fs.write", "fs.mkdir", "fs.delete", "fs.move",
            "fs.stat", "fs.search",
        ],
    ));
    me.body
        .incorporate(region("this-machine/compute", RegionClass::Compute, &["code.run"]));
    me.body
        .incorporate(region("this-machine/ui", RegionClass::Screen, &["ui.set"]));
    me.body
        .incorporate(region("this-machine/net", RegionClass::Network, &[]));
    me
}
