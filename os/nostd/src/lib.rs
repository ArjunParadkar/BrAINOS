//! BFS  LEVEL    Discipline - the no_std canary and its checks  [crate brainos-nostd-check]
//! BFS  PURPOSE  A library that touches every crate below host, so building it for a bare target proves none of them needs an operating system
//! BFS  DEFINES  touch_everything
//! BFS  USES     brainos: abi, adapters, blur, body, bpu, cognition, crypto, entity, export, fleet, +7 more
//! BFS  USED BY  -
//!
//! The `no_std` canary.
//!
//! Every crate in this workspace except `host` claims to compile with no
//! operating system beneath it. That claim is easy to make and easy to
//! break: one `use std::collections::HashMap` in a crate that is only
//! ever built for Linux, and the claim is false while every test still
//! passes.
//!
//! This crate is the check. It is `#![no_std]` *unconditionally* — not
//! `cfg_attr(not(test))` like the others, because a canary that becomes
//! `std` under `cfg(test)` is not a canary. It depends on everything and
//! touches enough of each to force the linker to pull it in.
//!
//! **How to actually run the check.** Building this for the host target
//! proves less than it looks like, because `std` is available there and
//! a leaked dependency would still resolve. The real check needs a
//! target with no `std` at all:
//!
//! ```sh
//! rustup target add thumbv7em-none-eabihf
//! cargo build -p brainos-nostd-check --target thumbv7em-none-eabihf
//! ```
//!
//! On a machine with no bare target installed — which is the state of
//! this one, as of writing — that command cannot run, and this file
//! says so rather than implying a check that did not happen. What
//! *does* hold on the host is weaker and still worth having: this crate
//! is `#![no_std]`, so any dependency that re-exported a `std` type
//! through its public API in a way this file touches would fail here.
//!
//! The strongest evidence today is different and is worth stating
//! plainly: `../../firmware/miniblur-a` builds for
//! `xtensa-esp32s3-none-elf` and links `miniblur-core`, and the same
//! `no_std` discipline was applied here by the same hand. That is a
//! reason to believe it, not a proof of it. The proof is one
//! `rustup target add` away and belongs in CI.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::vec::Vec;

/// Touch every crate, so none of them can be quietly dropped by the
/// linker and leave the check meaningless.
///
/// Returns a number nobody uses; the point is entirely in what had to
/// compile to produce it.
pub fn touch_everything() -> u64 {
    let mut n: u64 = 0;

    // abi
    let verb = brainos_abi::verb::Verb::literal("legs.walk");
    n += verb.as_str().len() as u64;
    let mut t: brainos_abi::text::Text<32> = brainos_abi::text::Text::new();
    t.push_u64(7);
    n += t.len() as u64;
    n += brainos_abi::error::Stage::ALL.len() as u64;

    // crypto
    let key = brainos_crypto::key::BrainKey::from_seed(&[1; 32]);
    let sig = key.sign(brainos_crypto::domain::Tag::Request, b"x");
    n += sig.as_bytes()[0] as u64;
    let mut log = brainos_crypto::merkle::MerkleLog::new();
    log.append(b"a");
    n += log.root().as_bytes()[0] as u64;

    // rt
    let env = brainos_rt::envelope::Envelope::new(
        1,
        brainos_abi::time::Duration::from_millis(10),
        brainos_abi::time::Duration::from_millis(200),
    )
    .with(brainos_rt::envelope::Channel::Angle, -30.0, 30.0);
    n += env.check(brainos_rt::envelope::Channel::Angle, 0.0).is_ok() as u64;
    let mut sched: brainos_rt::sched::Scheduler<4> = brainos_rt::sched::Scheduler::new();
    n += sched
        .admit(
            brainos_rt::sched::TaskSpec::periodic(
                1,
                "gait",
                brainos_abi::time::Duration::from_millis(10),
                brainos_abi::time::Duration::from_millis(1),
            ),
            brainos_abi::time::Instant::ZERO,
        )
        .is_ok() as u64;

    // memory
    let handle = brainos_memory::law::MemoryHandle::issue(&key);
    let mut graph = brainos_memory::graph::StateGraph::with_bounds(
        brainos_memory::graph::Bounds::EMBEDDED,
    );
    graph.add(
        &handle,
        brainos_memory::graph::NodeKind::Episode,
        "something happened",
        100,
        brainos_memory::provenance::Provenance::own(brainos_abi::time::Tick(1)),
    );
    let index = brainos_memory::index::Index::build(&graph);
    n += index.docs() as u64;
    let mut skills = brainos_memory::skill::SkillGraph::new();
    skills.learn(
        &handle,
        &mut graph,
        verb,
        "walk",
        brainos_memory::provenance::Provenance::own(brainos_abi::time::Tick(1)),
    );
    n += skills.len() as u64;

    // body
    let schema = brainos_body::schema::CapabilitySchema::new("b", "native").with(
        brainos_body::schema::VerbSpec::new(
            verb,
            brainos_body::region::RegionClass::Legs,
            "walk",
        )
        .with(brainos_body::schema::ParamSpec::scalar(
            "speed",
            brainos_rt::envelope::Channel::Speed,
            0.0,
            0.6,
        )),
    );
    n += schema.digest().as_bytes()[0] as u64;
    let map = brainos_body::region::BodyMap::new();
    n += map.repertoire().len() as u64;

    // kira
    let mut kira = brainos_kira::gate::Kira::new(key.entity_id());
    n += kira.rules.drives().len() as u64;
    n += kira.would_refuse(&brainos_abi::verb::Verb::literal("memory.erase")).is_some() as u64;
    let req = brainos_kira::action::Request::intrinsic(verb, brainos_abi::time::Tick(1));
    let proposal = req.proposed_by(&key);
    let out = kira.evaluate(
        &proposal,
        &key.public(),
        &map,
        brainos_abi::time::Tick(1),
        Some(&key),
    );
    n += out.trace.is_well_formed() as u64;

    // cognition
    let mut forward = brainos_cognition::predict::Forward::new();
    n += forward.observe_named("rail", 7.4).novel as u64;
    let plan = brainos_cognition::placement::Plan::distributed();
    n += plan.is_coherent() as u64;

    // worlds
    let mut ingress = brainos_worlds::ingress::Ingress::default();
    n += ingress
        .inspect_text(
            "hello",
            brainos_worlds::world::World::Internet,
            brainos_abi::time::Tick(1),
        )
        .is_ok() as u64;
    let mut advocate = brainos_worlds::advocate::Advocate::default();
    n += advocate.examine("act now").score as u64;

    // entity
    let mut entity = brainos_entity::entity::Entity::new(
        brainos_crypto::key::BrainKey::from_seed(&[2; 32]),
        brainos_abi::time::Tick(0),
    );
    n += entity.wake(None, None, brainos_abi::time::Tick(1)).is_ok() as u64;
    n += entity.describe().len() as u64;

    // transport
    let frame = brainos_transport::frame::Frame::ping(1);
    let sealed = frame.seal(&[0; 32]);
    n += sealed.len() as u64;

    // adapters
    let graph_desc = brainos_adapters::ros2::NodeGraph::new("r").with(
        brainos_adapters::ros2::Topic::command("/cmd_vel", "geometry_msgs/msg/Twist")
            .bounded("linear_x", -0.2, 0.2),
    );
    n += brainos_adapters::ros2::from_node_graph(&graph_desc).is_ok() as u64;
    n += brainos_adapters::native::mini_blur().len() as u64;

    // proof
    n += brainos_proof::domain::Domain::SIZE as u64;
    n += brainos_proof::properties::PROPERTIES.len() as u64;

    // fleet
    let registry = brainos_fleet::registry::Registry::new(8);
    n += registry.len() as u64;

    // export
    let archive = brainos_export::archive::export(
        &key,
        &graph,
        &brainos_entity::life::Life::begin(brainos_abi::time::Tick(0)),
        1,
        0,
        brainos_abi::time::Tick(2),
    );
    n += archive.is_ok() as u64;

    let _unused: Vec<u8> = Vec::new();

    // blur — the Teensy's procedures and the wire, as the board would run them
    let mut auto = brainos_blur::autonomic::Autonomic::new();
    let mut down = brainos_blur::wire::NerveDown::new(0);
    n += auto.request(brainos_blur::autonomic::Request::Census) as u64;
    n += brainos_blur::bus::buses_needed_for(1000) as u64;
    n += brainos_blur::schema::blur_schema().len() as u64;
    let f = brainos_blur::wire::estop(brainos_blur::descriptor::Bus::A, 1, 1);
    n += brainos_blur::wire::is_estop(&f) as u64;
    n += down.frames.push(f) as u64;
    let _ = &mut auto;

    // part — every piece of hardware as a body part
    n += brainos_part::class::PartClass::ALL.len() as u64;
    let probe = brainos_part::schema::PartSchema::new(
        brainos_part::class::PartClass::Sensor,
        "probe",
        "canary",
    )
    .sense(brainos_part::schema::SenseSpec::new(
        "temp.c",
        brainos_part::schema::Quantity::Temperature,
        -40.0,
        150.0,
    ));
    n += probe.facets().as_bytes()[0] as u64;

    // bpu — the integer contract and the planner, as silicon would run them
    n += brainos_bpu::contract::rne_shift(3 << 4, 5) as u64;
    n += brainos_bpu::contract::sigma_code_from_milli(20) as u64;
    n += brainos_bpu::prng::EtaPrng::new(1, 328, 4).read_block(1).len() as u64;
    let d = brainos_cognition::bpu::BpuDescriptor::paper_geometry(4, 1_000_000_000);
    n += brainos_bpu::planner::plan(
        &d,
        &[brainos_bpu::planner::LayerShape { inputs: 256, outputs: 256 }],
        1_000_000_000,
    )
    .map(|p| p.tiles as u64)
    .unwrap_or(0);
    n
}
