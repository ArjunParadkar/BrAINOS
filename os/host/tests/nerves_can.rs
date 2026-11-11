//! BFS  LEVEL    Host - the only crate with an operating system (CLI, scenarios, OS nerves)  [crate brainos-host]
//! BFS  PURPOSE  Tests of the CAN nerve on an in-memory bus with the real BLUR codec and golden vectors, plus a live test that runs if vcan0 exists
//! BFS  DEFINES  -
//! BFS  USES     brainos: abi, blur, body, crypto, entity, host, part
//! BFS  USED BY  (entry point or test)
//!
//! The CAN nerve.
//!
//! `mem_` tests run on `MemBus`: simulated nodes, real codec. They pin
//! that the nerve uses the existing BLUR frames unchanged — the golden
//! vectors here are the ones in `os/blur/tests/blur_suite.rs`, which
//! are shared with the Python twin and the C firmware.
//!
//! `live_` runs only where a `vcan0` interface exists. Creating one
//! needs root (`docs/NERVES.md` has the runbook), so on a machine
//! without it the test prints SKIPPED and the nerve's SocketCAN half
//! is Built, not Verified.

#![cfg(feature = "socketcan")]

use brainos_abi::time::{Instant, Tick};
use brainos_blur::descriptor::{actuator_at, Bus, MotorKind, ACTUATORS};
use brainos_blur::wire::{
    is_estop, CanFrame, Feedback, FC_CENSUS, ST_CALIBRATED, ST_ENC_OK, ST_OVERTEMP,
};
use brainos_body::vital::Health;
use brainos_crypto::key::BrainKey;
use brainos_entity::entity::Entity;
use brainos_host::nerves::acquire_all;
use brainos_host::nerves::socketcan::{BlurCanNerve, CanBus, MemBus, SimNode, SocketCanBus};
use brainos_part::nerve::Nerve;

/// Find two (node, slot) pairs the body descriptor knows about, so the
/// test follows the real body rather than assuming its wiring.
fn known_nodes() -> Vec<(u8, u8)> {
    let mut v = Vec::new();
    for node in 0..=127u8 {
        if actuator_at(node, 0).is_some() {
            v.push((node, 0));
        }
        if v.len() == 2 {
            break;
        }
    }
    assert_eq!(v.len(), 2, "the descriptor has fewer than two nodes");
    v
}

fn bus() -> MemBus {
    let n = known_nodes();
    MemBus::new(
        Bus::B,
        vec![
            SimNode {
                node: n[0].0,
                uid: [0xDE, 0xAD, 0xBE, 0xEF],
                kind: 1,
                slots: vec![(0, ST_ENC_OK | ST_CALIBRATED, 0.25, -0.5, 1.5)],
            },
            SimNode {
                node: n[1].0,
                uid: [1, 2, 3, 4],
                kind: 1,
                slots: vec![(0, ST_ENC_OK | ST_CALIBRATED, 0.0, 0.0, 0.2)],
            },
        ],
    )
}

#[test]
fn mem_the_feedback_golden_vector_is_the_one_the_firmware_and_the_twin_share() {
    let fb = Feedback {
        slot: 0,
        status: ST_ENC_OK | ST_CALIBRATED,
        q: 1.5351,
        dq: -3.21,
        current: 12.34,
    };
    let f = fb.encode(Bus::B, 0x08, MotorKind::Esc2806);
    assert_eq!(f.id, 0x188);
    assert_eq!(f.data, [0x00, 0x42, 0xFC, 0x17, 0xBF, 0xFE, 0xD2, 0x04]);
}

#[test]
fn mem_a_census_finds_each_node_once_with_a_stable_identity() {
    let mut n = BlurCanNerve::new(bus(), Bus::B);
    let a = n.enumerate();
    assert_eq!(a.len(), 2);
    assert_eq!(n.bus.sent[0].function(), FC_CENSUS);
    let again = n.enumerate();
    assert_eq!(
        a.iter().map(|f| f.key.to_bytes()).collect::<Vec<_>>(),
        again.iter().map(|f| f.key.to_bytes()).collect::<Vec<_>>(),
        "the same node got a different identity on a second census"
    );
    assert_ne!(a[0].key.to_bytes(), a[1].key.to_bytes());
    assert_eq!(n.uids().len(), 2);
}

#[test]
fn mem_a_swapped_node_is_a_different_part() {
    let mut one = BlurCanNerve::new(bus(), Bus::B);
    let before = one.enumerate()[0].key.to_bytes();
    let mut b = bus();
    b.nodes[0].uid = [9, 9, 9, 9];
    let mut two = BlurCanNerve::new(b, Bus::B);
    assert_ne!(before, two.enumerate()[0].key.to_bytes());
}

#[test]
fn mem_feedback_is_decoded_in_the_joints_own_units_and_only_the_newest_is_kept() {
    let mut n = BlurCanNerve::new(bus(), Bus::B);
    let found = n.enumerate();
    n.bus.tick();
    n.bus.nodes[0].slots[0].2 = 0.30; // it moved
    n.bus.tick();
    let mut out = Vec::new();
    n.read(&found[0], Instant(1), &mut out);
    let get = |k: &str| out.iter().find(|s| s.sense.as_str() == k).map(|s| s.value).unwrap();
    assert_eq!(out.len(), 4, "one reading per sense, not one per frame");
    assert!((get("s0.q") - 0.30).abs() < 1e-3, "{}", get("s0.q"));
    assert!((get("s0.dq") + 0.5).abs() < 2e-2);
    assert!((get("s0.a") - 1.5).abs() < 2e-2);
    assert_eq!(get("s0.fault"), 0.0);

    // the other node's frames were not eaten by reading this one
    let mut other = Vec::new();
    n.read(&found[1], Instant(1), &mut other);
    assert_eq!(other.len(), 4);
}

#[test]
fn mem_a_node_reporting_overtemperature_is_a_critical_part_in_the_entity() {
    let mut e = Entity::new(BrainKey::from_seed(&[0x81; 32]), Tick(0));
    e.wake(None, None, Tick(1)).unwrap();
    let mut b = bus();
    b.nodes[0].slots[0].1 |= ST_OVERTEMP;
    let mut n = BlurCanNerve::new(b, Bus::B);
    // feedback must be on the bus before the survey reads it
    n.enumerate();
    n.bus.tick();
    let found = acquire_all(&mut e, &mut n, Instant(1_000));
    assert_eq!(found.len(), 2);
    assert!(found.iter().all(|p| p.error.is_none()), "{found:?}");
    assert_eq!(found[0].class, "actuator");
    assert_eq!(found[0].verbs, 0, "this nerve must not offer a second way to move a joint");
    assert_eq!(found[0].health, Health::Critical);
    assert_eq!(found[1].health, Health::Nominal);
    assert!(e.recall("critical", 1).first().is_some(), "the fault was not remembered");
}

#[test]
fn mem_safe_sends_the_estop_frame_and_nothing_else() {
    let mut n = BlurCanNerve::new(bus(), Bus::B);
    n.enumerate();
    let before = n.bus.sent.len();
    n.safe();
    assert!(n.bus.estopped);
    let sent = &n.bus.sent[before..];
    assert!(!sent.is_empty() && sent.iter().all(is_estop), "{sent:?}");
    assert_eq!(sent[0].id, 0, "e-stop must be the highest-priority identifier");
}

#[test]
fn mem_a_slot_the_descriptor_does_not_know_is_not_decoded_with_a_guessed_unit() {
    let n0 = known_nodes()[0].0;
    // a slot number beyond anything the body has
    let unknown = (0..=255u8).find(|s| actuator_at(n0, *s).is_none()).unwrap();
    let b = MemBus::new(
        Bus::B,
        vec![SimNode {
            node: n0,
            uid: [7; 4],
            kind: 2,
            slots: vec![(0, ST_ENC_OK, 0.1, 0.0, 0.0), (unknown, ST_ENC_OK, 0.1, 0.0, 0.0)],
        }],
    );
    let mut n = BlurCanNerve::new(b, Bus::B);
    let found = n.enumerate();
    // identity says two slots; only the known one has senses
    let schema = n.describe(&found[0]).unwrap();
    let per_slot = 4;
    assert!(schema.senses().len() % per_slot == 0);
    assert!(schema.senses().iter().all(|s| !s.name.as_str().starts_with(&format!("s{unknown}."))));
    let _ = ACTUATORS.len();
}

#[test]
fn live_vcan0_carries_a_census_and_feedback_through_the_real_socket() {
    // two sockets on the same interface: one is the nerve, the other
    // plays a node. The kernel delivers each frame to the other socket.
    let (Ok(nerve_side), Ok(mut node_side)) =
        (SocketCanBus::open("vcan0", Bus::B), SocketCanBus::open("vcan0", Bus::B))
    else {
        println!(
            "SKIPPED (no vcan0: `sudo modprobe vcan && sudo ip link add dev vcan0 type vcan && \
             sudo ip link set up vcan0`)"
        );
        return;
    };
    let node = known_nodes()[0].0;
    let mut n = BlurCanNerve::new(nerve_side, Bus::B);

    // the node answers the census a moment after it is sent
    let answer = std::thread::spawn(move || {
        let mut saw_census = false;
        for _ in 0..40 {
            if let Some(f) = node_side.recv() {
                if f.function() == FC_CENSUS {
                    saw_census = true;
                    let page = brainos_blur::wire::IdentityPage {
                        page: 0,
                        kind: 1,
                        slots: 1,
                        fw: 1,
                        uid: [0xCA, 0xFE, 0xF0, 0x0D],
                    };
                    node_side.send(&page.encode(Bus::B, node));
                    let ai = actuator_at(node, 0).unwrap();
                    let fb = Feedback { slot: 0, status: ST_ENC_OK, q: 0.2, dq: 0.0, current: 0.5 };
                    node_side.send(&fb.encode(Bus::B, node, ACTUATORS[ai].motor));
                    break;
                }
            }
        }
        saw_census
    });
    let found = n.enumerate();
    assert!(answer.join().unwrap(), "the census never arrived on the wire");
    assert_eq!(found.len(), 1);
    let mut out = Vec::new();
    n.read(&found[0], Instant(1), &mut out);
    assert_eq!(out.len(), 4);
    println!("LIVE socketcan: node {node} found on vcan0, feedback {out:?}");
    let _: Option<CanFrame> = None;
}
