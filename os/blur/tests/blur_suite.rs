//! brainos-blur: the descriptor, the wire, the budget, and the checks
//! §8 runs — everything that can be tested without the twin. The twin
//! end-to-end is `brainos blur gate1`.

use brainos_abi::ids::{EntityId, SessionId};
use brainos_abi::version::ABI;
use brainos_blur::bus;
use brainos_blur::calibrate::{self, CalFault, HomeResult, ImuResult, ProbeResult};
use brainos_blur::census::{check_census, BuildSheet, CensusBuilder, CensusFault, Conflict};
use brainos_blur::descriptor::{self, actuator_index, Bus, EncoderSide, MotorKind, ACTUATORS, N_ACT, NODES};
use brainos_blur::firmware::{absorb, Firmware};
use brainos_blur::schema::{blur_schema, check_schema, schema_with, SchemaFault};
use brainos_blur::wire::*;
use brainos_body::acquire::{Acquisition, Step};
use brainos_crypto::key::BrainKey;
use brainos_crypto::rng::Drbg;

// ---------------------------------------------------------------- descriptor

#[test]
fn descriptor_was_generated_from_the_model_on_disk() {
    let dir = std::env::var("BLUR_MJCF_DIR").unwrap_or_else(|_| {
        format!("{}/Desktop/Arenda_Projects/BLUR/blur_mujoco_v36", std::env::var("HOME").unwrap())
    });
    let path = std::path::Path::new(&dir).join("blur.xml");
    let Ok(bytes) = std::fs::read(&path) else {
        eprintln!("SKIP: {} not present on this machine", path.display());
        return;
    };
    let mut h = sha256_hex(&bytes);
    h.truncate(64);
    assert_eq!(h, descriptor::MJCF_SHA256, "blur.xml changed: rerun blur/gen_descriptor.py");
}

fn sha256_hex(b: &[u8]) -> String {
    // the generator used Python's hashlib (plain SHA-256); the workspace's
    // own hasher only offers domain-tagged digests, so use the system tool
    let out = std::process::Command::new("sha256sum")
        .arg("-")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut c| {
            use std::io::Write;
            c.stdin.take().unwrap().write_all(b)?;
            c.wait_with_output()
        })
        .expect("sha256sum");
    String::from_utf8_lossy(&out.stdout).split_whitespace().next().unwrap().to_string()
}

#[test]
fn twenty_six_actuators_in_the_prompts_families() {
    assert_eq!(ACTUATORS.len(), N_ACT);
    let count = |k: MotorKind| ACTUATORS.iter().filter(|a| a.motor == k).count();
    assert_eq!(count(MotorKind::Esc2806), 12, "hip/knee/wrist on four legs");
    assert_eq!(count(MotorKind::Gimbal4015), 7, "4 abduction + 3 spine");
    assert_eq!(count(MotorKind::Gimbal2204) + count(MotorKind::Winch2204), 6, "neck x3, jaw, 2 tail winches");
    assert_eq!(count(MotorKind::N20), 1, "tail-tip gripper");
    assert!((descriptor::MASS_KG - 3.67155).abs() < 1e-3);
}

#[test]
fn every_actuator_has_exactly_one_node_slot() {
    for (j, a) in ACTUATORS.iter().enumerate() {
        assert_eq!(descriptor::actuator_at(a.node, a.slot), Some(j), "{}", a.name);
        let n = descriptor::node(a.node).expect("node exists");
        assert_eq!(n.bus, a.bus, "{} is on its node's bus", a.name);
        assert!(a.slot < n.slots);
    }
    let slots: usize = NODES.iter().map(|n| n.slots as usize).sum();
    assert_eq!(slots, N_ACT);
}

#[test]
fn the_4015_encoders_are_ambiguous_at_power_on_and_the_descriptor_says_so() {
    let amb: Vec<&str> = descriptor::ambiguous_at_power_on().map(|(_, a)| a.name).collect();
    assert_eq!(amb.len(), 7);
    for a in ACTUATORS.iter().filter(|a| a.motor == MotorKind::Gimbal4015) {
        assert_eq!(a.encoder, EncoderSide::Motor);
        assert!(amb.contains(&a.name));
    }
}

#[test]
fn the_4015_driver_cannot_reach_the_mjcf_torque_under_the_stated_assumptions() {
    // 3.8 N·m needs 2.93 A at KV 100 / 16:1 / 85 %; the DRV8313 peaks at 2.5 A
    for a in ACTUATORS.iter().filter(|a| a.motor == MotorKind::Gimbal4015) {
        assert!(!a.driver_reaches_tau_max(), "{}", a.name);
        assert!((a.tau_deliverable() - 3.247).abs() < 0.01, "{} {}", a.name, a.tau_deliverable());
    }
    for a in ACTUATORS.iter().filter(|a| a.motor == MotorKind::Esc2806) {
        assert!(a.driver_reaches_tau_max(), "{}", a.name);
    }
}

#[test]
fn torque_derate_is_flat_then_linear() {
    let a = &ACTUATORS[actuator_index("RR_knee").unwrap()];
    assert_eq!(a.tau_available(0.0), a.tau_max);
    assert_eq!(a.tau_available(0.5 * a.w0), a.tau_max);
    assert!((a.tau_available(0.75 * a.w0) - 0.5 * a.tau_max).abs() < 1e-4);
    assert_eq!(a.tau_available(a.w0), 0.0);
    assert_eq!(a.tau_available(-0.75 * a.w0), a.tau_available(0.75 * a.w0));
}

// ---------------------------------------------------------------- the budget

#[test]
fn a_1khz_joint_loop_over_can_needs_eight_buses_and_we_have_three() {
    assert_eq!(CanFrame::wire_bits_worst(8), 135);
    assert_eq!(bus::buses_needed_for(1000), 8);
    assert!(bus::load_at(Bus::A, 1000) > 3.0);
}

#[test]
fn the_planned_setpoint_rate_fits_every_bus_under_the_planning_load() {
    for b in Bus::ALL {
        assert!(bus::load_at(b, bus::SETPOINT_HZ) <= bus::PLAN_LOAD, "{b:?} {}", bus::load_at(b, bus::SETPOINT_HZ));
        assert!(bus::max_rate_hz(b) >= bus::SETPOINT_HZ as f32);
    }
    assert_eq!(bus::motors_on(Bus::A) + bus::motors_on(Bus::B) + bus::motors_on(Bus::C), N_ACT);
}

// ---------------------------------------------------------------- the wire

#[test]
fn setpoint_round_trips_and_has_a_fixed_byte_layout() {
    let sp = Setpoint { slot: 2, flags: SP_ENABLE, q: -1.0622, tau_ff: 0.25, kp: 20.0, kd: 0.5 };
    let f = sp.encode(Bus::A, 0x11, MotorKind::Gimbal2204);
    assert_eq!(f.id, 0x100 | 0x11);
    // golden vector, shared with blur/sim/test_wire.py
    assert_eq!(f.data, [0x02, 0x01, 0x67, 0xEF, 0xFA, 0x00, 0x50, 0x32]);
    let back = Setpoint::decode(&f, MotorKind::Gimbal2204).unwrap();
    assert!((back.q - sp.q).abs() < 2.5e-4 && (back.tau_ff - 0.25).abs() < 1e-3);
    assert_eq!(back.kp, 20.0);
}

#[test]
fn feedback_golden_vector() {
    let fb = Feedback { slot: 0, status: ST_ENC_OK | ST_CALIBRATED, q: 1.5351, dq: -3.21, current: 12.34 };
    let f = fb.encode(Bus::B, 0x08, MotorKind::Esc2806);
    assert_eq!(f.id, 0x188);
    assert_eq!(f.data, [0x00, 0x42, 0xFC, 0x17, 0xBF, 0xFE, 0xD2, 0x04]);
    let b = Feedback::decode(&f, MotorKind::Esc2806).unwrap();
    assert!((b.q - 1.5351).abs() < 2.5e-4 && (b.dq + 3.21).abs() < 1e-2 && (b.current - 12.34).abs() < 1e-2);
}

#[test]
fn winch_units_cover_the_cable_travel() {
    for a in ACTUATORS.iter().filter(|a| a.tendon) {
        let s = scales(a.motor);
        assert!(a.q_max.abs() / s.q < 32767.0 && a.q_min.abs() / s.q < 32767.0, "{}", a.name);
    }
    // uncalibrated, a joint reads anywhere in [-π, π) plus its whole travel
    for a in ACTUATORS.iter().filter(|a| !a.tendon) {
        let s = scales(a.motor);
        let worst = core::f32::consts::PI + (a.q_max - a.q_min);
        assert!(worst / s.q < 32767.0, "{} {}", a.name, worst / s.q);
    }
}

#[test]
fn there_is_no_frame_that_releases_an_estop() {
    let f = estop(Bus::C, 3, 9);
    assert!(is_estop(&f));
    assert_eq!(f.id, 0, "highest priority on every bus");
    // id 0 without the magic is not an e-stop and not anything else either
    let g = CanFrame::new(Bus::C, 0, 0, &[0, 0]);
    assert!(!is_estop(&g));
}

#[test]
fn nerve_up_parses_what_the_bridge_writes() {
    // 'U' tick flags vbat imu[15] n frames — built by hand, as wire.py does
    let mut b = vec![b'U'];
    b.extend_from_slice(&1234u32.to_le_bytes());
    b.push(0b101);
    b.extend_from_slice(&24000u16.to_le_bytes());
    let imu = Bmi270Burst { acc: [0, 0, 4096], gyr: [16, -16, 0], sensortime: 0x123456 };
    let mut ib = [0u8; 15];
    imu.write(&mut ib);
    b.extend_from_slice(&ib);
    b.push(1);
    let mut fb = [0u8; 12];
    census(Bus::B).write(&mut fb);
    b.extend_from_slice(&fb);
    let up = NerveUp::read(&b).unwrap();
    assert_eq!(up.tick, 1234);
    assert!(up.rail_on && !up.estop_latched);
    assert_eq!(up.imu.unwrap(), imu);
    assert!((up.imu.unwrap().accel()[2] - G).abs() < 1e-4);
    assert_eq!(up.frames.len(), 1);
    assert_eq!(up.frames.as_slice()[0].id, 0x080);
}

#[test]
fn a_truncated_or_lying_nerve_datagram_is_refused() {
    assert!(NerveUp::read(b"U").is_none());
    let mut b = vec![b'U', 0, 0, 0, 0, 0, 0, 0];
    b.extend_from_slice(&[0u8; 15]);
    b.push(5); // claims five frames, carries none
    assert!(NerveUp::read(&b).is_none());
}

// ---------------------------------------------------------------- census

fn pages(b: &mut CensusBuilder, bus: Bus, id: u8, kind: u8, slots: u8, uid: [u8; 12]) {
    for p in 0..3u8 {
        let mut u = [0u8; 4];
        u.copy_from_slice(&uid[4 * p as usize..4 * p as usize + 4]);
        b.page(&IdentityPage { page: p, kind, slots, fw: 1, uid: u }.encode(bus, id));
    }
}

fn full_census() -> (brainos_blur::census::Census, BuildSheet) {
    let mut b = CensusBuilder::new();
    for n in NODES.iter() {
        let kind = match n.kind {
            descriptor::NodeKind::Esc1 => KIND_ESC1,
            descriptor::NodeKind::Esp32S3 => KIND_ESP32S3,
        };
        pages(&mut b, n.bus, n.id, kind, n.slots, [n.id; 12]);
    }
    let c = b.finish(true);
    let s = BuildSheet::from_census(&c);
    (c, s)
}

#[test]
fn a_whole_blur_passes_the_census_and_the_schema() {
    let (c, s) = full_census();
    assert_eq!(c.nodes.len(), 15);
    assert_eq!(check_census(&c, Some(&s)), Ok(()));
    assert_eq!(check_schema(&blur_schema(), &c), Ok(()));
}

#[test]
fn a_swapped_board_is_caught_against_the_build_sheet() {
    let (_, sheet) = full_census();
    let mut b = CensusBuilder::new();
    for n in NODES.iter() {
        let kind = if n.kind == descriptor::NodeKind::Esc1 { KIND_ESC1 } else { KIND_ESP32S3 };
        let uid = if n.id == 5 { [0xEE; 12] } else { [n.id; 12] };
        pages(&mut b, n.bus, n.id, kind, n.slots, uid);
    }
    let c = b.finish(true);
    assert_eq!(check_census(&c, Some(&sheet)), Err(CensusFault::Swapped { id: 5 }));
    // with no sheet (first inhabitation) the same census is acceptable — and
    // becomes the sheet the human is asked to confirm
    assert_eq!(check_census(&c, None), Ok(()));
}

#[test]
fn two_answers_on_one_id_and_one_id_on_two_buses_are_both_conflicts() {
    let mut b = CensusBuilder::new();
    pages(&mut b, Bus::A, 5, KIND_ESC1, 1, [1; 12]);
    pages(&mut b, Bus::A, 5, KIND_ESC1, 1, [2; 12]);
    let c = b.finish(true);
    assert!(c.conflicts.contains(&Conflict::TwoAnswers { bus: Bus::A, id: 5 }));
    let mut b = CensusBuilder::new();
    pages(&mut b, Bus::A, 5, KIND_ESC1, 1, [1; 12]);
    pages(&mut b, Bus::B, 5, KIND_ESC1, 1, [1; 12]);
    let c = b.finish(true);
    assert!(c.conflicts.contains(&Conflict::TwoBuses { id: 5 }));
    assert!(matches!(check_census(&c, None), Err(CensusFault::Conflict(_))));
}

#[test]
fn a_node_on_the_wrong_bus_or_of_the_wrong_kind_is_the_wrong_part() {
    let mut b = CensusBuilder::new();
    pages(&mut b, Bus::B, 1, KIND_ESC1, 1, [1; 12]); // FR_pitch's ESC belongs on bus A
    assert_eq!(check_census(&b.finish(true), None), Err(CensusFault::WrongPart { id: 1 }));
    let mut b = CensusBuilder::new();
    pages(&mut b, Bus::A, 1, KIND_ESP32S3, 1, [1; 12]);
    assert_eq!(check_census(&b.finish(true), None), Err(CensusFault::WrongPart { id: 1 }));
}

#[test]
fn census_digest_is_canonical() {
    let (a, _) = full_census();
    let (b, _) = full_census();
    assert_eq!(a.digest(), b.digest());
    let mut c = a.clone();
    c.imu_present = false;
    assert_ne!(a.digest(), c.digest());
}

// ---------------------------------------------------------------- schema

#[test]
fn a_joint_blur_does_not_have_is_a_phantom() {
    let (c, _) = full_census();
    let mut names: Vec<&str> = ACTUATORS.iter().map(|a| a.name).collect();
    names.push("FR_wing");
    assert_eq!(check_schema(&schema_with(&names), &c), Err(SchemaFault::UnknownJoint("FR_wing".into())));
}

#[test]
fn a_claimed_joint_whose_node_is_silent_is_a_phantom() {
    let (mut c, _) = full_census();
    c.nodes.retain(|n| n.id != 9);
    assert_eq!(
        check_schema(&blur_schema(), &c),
        Err(SchemaFault::PhantomLimb { joint: "RR_dist".into(), node: 9 })
    );
}

#[test]
fn a_body_that_omits_a_joint_is_not_a_whole_blur() {
    let (c, _) = full_census();
    let names: Vec<&str> = ACTUATORS.iter().map(|a| a.name).filter(|n| *n != "jaw").collect();
    assert_eq!(check_schema(&schema_with(&names), &c), Err(SchemaFault::MissingJoint("jaw".into())));
}

#[test]
fn claiming_different_parts_is_being_a_different_body() {
    let all: Vec<&str> = ACTUATORS.iter().map(|a| a.name).collect();
    let fewer: Vec<&str> = all.iter().copied().filter(|n| *n != "jaw").collect();
    assert_ne!(schema_with(&all).digest(), schema_with(&fewer).digest());
}

// ---------------------------------------------------------------- firmware

#[test]
fn the_body_verifies_the_entity_over_the_same_transcript_the_protocol_signs() {
    let entity = BrainKey::from_seed(&[3u8; 32]);
    let fw = Firmware::new(&[4u8; 32], blur_schema(), vec![entity.public()]);
    let mut a = Acquisition::discover(entity.entity_id(), SessionId::from_bytes([1; 32]));
    let expect = absorb(&a.transcript(), "device", &fw.public().to_bytes());
    let mut rng = Drbg::deterministic(b"t");
    let r = a.handshake(&entity, &mut rng, fw.public(), ABI, |c| fw.answer(c), |pk, sig| fw.accepts(pk, sig, &expect));
    assert!(r.is_ok());
    assert_eq!(a.step, Step::Attested);
    // and a stranger is refused by the body
    let other = BrainKey::from_seed(&[5u8; 32]);
    let mut b = Acquisition::discover(other.entity_id(), SessionId::from_bytes([2; 32]));
    let expect = absorb(&b.transcript(), "device", &fw.public().to_bytes());
    assert!(b.handshake(&other, &mut rng, fw.public(), ABI, |c| fw.answer(c), |pk, sig| fw.accepts(pk, sig, &expect)).is_err());
    let _ = EntityId::ZERO;
}

// ---------------------------------------------------------------- calibrate

fn fitted(b: f32, se: f32, motion: f32) -> Option<brainos_blur::fit::FitResult> {
    Some(brainos_blur::fit::FitResult { b, se, motion, n: 24 })
}

#[test]
fn probe_verdicts() {
    let j = actuator_index("RR_knee").unwrap();
    let good = ProbeResult { fit: fitted(50.0, 1.0, 0.05), enc_ok: true, current: 3.0, fresh: true };
    assert_eq!(calibrate::check_probe(j, &good), Ok(()));
    let flipped = ProbeResult { fit: fitted(-50.0, 1.0, -0.05), ..good.clone() };
    assert!(matches!(calibrate::check_probe(j, &flipped), Err(CalFault::Flipped { .. })));
    // significant but tiny: quantisation, not motion
    let still = ProbeResult { fit: fitted(50.0, 1.0, 0.0001), ..good.clone() };
    assert_eq!(calibrate::check_probe(j, &still), Err(CalFault::DeadEncoder { joint: j }));
    // large but not significant: noise, not a verdict either way
    let noisy = ProbeResult { fit: fitted(-50.0, 20.0, -0.05), ..good.clone() };
    assert_eq!(calibrate::check_probe(j, &noisy), Err(CalFault::DeadEncoder { joint: j }));
    let nofield = ProbeResult { enc_ok: false, ..good.clone() };
    assert_eq!(calibrate::check_probe(j, &nofield), Err(CalFault::DeadEncoder { joint: j }));
    let silent = ProbeResult { fresh: false, ..good };
    assert_eq!(calibrate::check_probe(j, &silent), Err(CalFault::Silent { joint: j }));
}

#[test]
fn the_doublet_fit_finds_the_motor_through_gravity_swing_and_noise() {
    use brainos_blur::fit::{phi, Fit, DOUBLET_MS};
    // a joint swinging at 0.8 rad/s, sagging under 6 rad/s² of gravity,
    // inertia such that b = 400 rad/(N·m·s²), 0.2 N·m probe, 2e-4 rad noise
    for (b_true, want_pos) in [(400.0f32, true), (-400.0, false)] {
        let mut f = Fit::new();
        let mut seed = 12345u32;
        let mut t_ms = 0;
        while t_ms <= DOUBLET_MS + 40 {
            let t = t_ms as f32 * 0.001;
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let noise = ((seed >> 8) as f32 / 16777216.0 - 0.5) * 4e-4;
            let q = 0.3 + 0.8 * t - 0.5 * 6.0 * t * t + b_true * 0.2 * phi(t) + noise;
            f.push(t, q);
            t_ms += 5;
        }
        let r = f.solve(0.2).unwrap();
        assert_eq!(r.b > 0.0, want_pos, "{r:?}");
        assert!(r.t_stat().abs() > 20.0, "{r:?}");
        assert!((r.b - b_true).abs() / b_true.abs() < 0.1, "{r:?}");
    }
}

#[test]
fn pooling_keeps_the_free_direction_and_the_sign_of_a_flipped_joint() {
    use brainos_blur::fit::{combine, FitResult};
    let free = FitResult { b: 300.0, se: 50.0, motion: 0.08, n: 25 };
    let blocked = FitResult { b: 10.0, se: 40.0, motion: 0.002, n: 25 };
    let c = combine(&[free, blocked]).unwrap();
    assert!(c.b > 0.0 && c.t_stat() > 4.0 && (c.motion - 0.08).abs() < 1e-6);
    let f1 = FitResult { b: -300.0, se: 50.0, motion: -0.08, n: 25 };
    let f2 = FitResult { b: -250.0, se: 60.0, motion: -0.06, n: 25 };
    let c = combine(&[f1, f2]).unwrap();
    assert!(c.b < 0.0 && c.t_stat() < -4.0);
}

#[test]
fn the_doublet_has_zero_net_impulse() {
    use brainos_blur::fit::{profile, DOUBLET_MS};
    let s: f32 = (0..DOUBLET_MS).map(profile).sum();
    assert_eq!(s, 0.0);
}

#[test]
fn homing_fixes_the_zero_from_the_home_stop_and_checks_the_reach() {
    let j = actuator_index("FR_knee").unwrap();
    let a = &ACTUATORS[j];
    assert!(a.home_high, "the front knee's passive load pushes it to its high stop");
    let stop = 5.0;
    let travel = a.q_max - a.q_min;
    let h = HomeResult { stop: Some(stop), reached: Some(stop - 0.9 * travel), enc_ok: true };
    let c = calibrate::solve_home(j, &h).unwrap();
    assert!((c.offset - (a.q_max - stop)).abs() < 1e-6);
    // jammed: barely moved off the stop
    let jam = HomeResult { reached: Some(stop - 0.1 * travel), ..h.clone() };
    assert!(matches!(calibrate::solve_home(j, &jam), Err(CalFault::Travel { .. })));
    // went right through where the other stop should be: wrong stop
    let through = HomeResult { reached: Some(stop - 1.3 * travel), ..h };
    assert!(matches!(calibrate::solve_home(j, &through), Err(CalFault::Travel { .. })));
}

#[test]
fn a_joint_side_zero_that_moved_is_drift_and_a_whole_turn_is_not() {
    let j = actuator_index("FR_knee").unwrap();
    let c = calibrate::JointCal { offset: 1.0, q_min: 0.0, q_max: 1.0 };
    assert_eq!(calibrate::check_drift(j, &c, 1.0 + 2.0 * core::f32::consts::PI), Ok(()));
    assert!(matches!(calibrate::check_drift(j, &c, 1.2), Err(CalFault::Drift { .. })));
    // a motor-side zero changes every power-on and is not compared
    let s = actuator_index("spine_s1").unwrap();
    assert_eq!(calibrate::check_drift(s, &c, 1.2), Ok(()));
}

#[test]
fn imu_level_solves_tilt_and_refuses_a_body_that_is_not_still() {
    let r = ImuResult { n: 500, missing: 0, acc: [0.0, 0.1712, 9.805], gyro: [0.01, 0.0, -0.02], acc_norm_sd: 0.03 };
    let c = calibrate::solve_imu(&r, 500).unwrap();
    assert!((c.roll.to_degrees() - 1.0).abs() < 0.02);
    let falling = ImuResult { acc: [0.0, 0.0, 1.0], ..r.clone() };
    assert!(matches!(calibrate::solve_imu(&falling, 500), Err(CalFault::ImuImplausible { .. })));
    let gone = ImuResult { n: 40, missing: 460, ..r };
    assert!(matches!(calibrate::solve_imu(&gone, 500), Err(CalFault::ImuLost { .. })));
}

// ---------------------------------------------------------------- policies, attitude, sessions

#[test]
fn rust_inference_matches_the_trainer_on_the_exported_policy() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../blur/learn/policies");
    let Ok(blob) = std::fs::read(dir.join("getup.mlp")) else {
        eprintln!("SKIP: no exported policy");
        return;
    };
    let parity: String = std::fs::read_to_string(dir.join("getup.mlp.parity.json")).unwrap();
    let nums = |key: &str| -> Vec<f32> {
        let k = parity.find(key).unwrap() + key.len();
        let t = &parity[k..];
        let a = t.find('[').unwrap();
        let b = t.find(']').unwrap();
        t[a + 1..b].split(',').map(|x| x.trim().parse().unwrap()).collect()
    };
    let obs = nums("\"obs\"");
    let want = nums("\"act\"");
    let m = brainos_blur::mlp::Mlp::from_bytes(&blob).unwrap();
    let mut out = vec![0.0f32; m.act_dim];
    assert!(m.act(&obs, &mut out));
    for (a, b) in out.iter().zip(want.iter()) {
        assert!((a - b).abs() < 1e-4, "{a} vs {b}");
    }
    // a wrong-length or non-finite observation is no action, not zeros
    assert!(!m.act(&obs[1..], &mut out));
    let mut bad = obs.clone();
    bad[0] = f32::NAN;
    assert!(!m.act(&bad, &mut out));
}

#[test]
fn a_corrupt_policy_blob_is_refused() {
    use brainos_blur::mlp::{Mlp, MlpError};
    assert_eq!(Mlp::from_bytes(b"NOTAPOLICY").err(), Some(MlpError::BadMagic));
    let mut b = b"BLURMLP1".to_vec();
    b.extend_from_slice(&1u32.to_le_bytes());
    assert_eq!(Mlp::from_bytes(&b).err(), Some(MlpError::Truncated));
}

#[test]
fn the_attitude_filter_finds_a_static_tilt_and_matches_the_python_mirror() {
    use brainos_blur::attitude::{ImuMount, Mahony};
    let mut f = Mahony::new(ImuMount::default());
    let r = 20.0f32.to_radians();
    let acc = [0.0, 9.80665 * r.sin(), 9.80665 * r.cos()];
    f.align([0.0, 0.0, 9.80665]);
    for _ in 0..3000 {
        f.update(acc, [0.0; 3], 0.001);
    }
    // blur/learn/attitude.py on the same trace prints 20.19
    assert!((f.tilt_deg() - 20.19).abs() < 0.02, "{}", f.tilt_deg());
    // an impact (3 g) is not trusted as gravity
    f.update([0.0, 0.0, 30.0], [0.0; 3], 0.001);
    assert!(!f.accel_trusted);
}

#[test]
fn a_session_envelope_is_always_inside_the_physical_one() {
    use brainos_blur::session::session_envelope;
    use brainos_blur::supervisor::Envelope;
    use brainos_body::policy::Arg;
    let phys = Envelope::physical();
    for (v, args) in [
        ("legs.walk", vec![("vx".to_string(), Arg::Scalar(0.3))]),
        ("legs.hop", vec![]),
        ("body.pose", vec![("joint".to_string(), Arg::Choice("neck_pan".into())), ("angle".to_string(), Arg::Scalar(0.3))]),
    ] {
        let e = session_envelope(&brainos_abi::verb::Verb::parse(v).unwrap(), &args).unwrap();
        assert!(e.is_within(&phys), "{v}");
    }
    // a pose outside the joint's range has no envelope at all
    let bad = vec![("joint".to_string(), Arg::Choice("RR_knee".into())), ("angle".to_string(), Arg::Scalar(3.0))];
    assert!(session_envelope(&brainos_abi::verb::Verb::parse("body.pose").unwrap(), &bad).is_none());
}

#[test]
fn the_supervisor_refuses_a_grant_wider_than_the_body_and_never_leaves_estop() {
    use brainos_blur::attitude::ImuMount;
    use brainos_blur::supervisor::{Envelope, Grant, Mode, Supervisor};
    let mut s = Supervisor::new(ImuMount::default());
    let mut wide = Envelope::physical();
    wide.i_max[0] *= 2.0;
    assert!(!s.install(Grant { id: [0; 32], skill: 1, env: wide, until_tick: 1000 }));
    // e-stop: once latched, a grant is refused and the mode does not change back
    let mut up = brainos_blur::wire::NerveUp::read(&{
        let mut b = vec![b'U'];
        b.extend_from_slice(&1u32.to_le_bytes());
        b.push(0b011); // imu present, e-stop latched
        b.extend_from_slice(&24000u16.to_le_bytes());
        b.extend_from_slice(&[0u8; 15]);
        b.push(0);
        b
    }).unwrap();
    let seen = [brainos_blur::autonomic::JointSeen::default(); N_ACT];
    let mut down = brainos_blur::wire::NerveDown::new(1);
    s.step(&up, &seen, &mut down);
    assert_eq!(s.mode, Mode::EStopped);
    assert!(down.relay_open);
    assert!(down.frames.as_slice().iter().all(|f| is_estop(f)));
    assert_eq!(down.frames.len(), 3, "one ESTOP per bus");
    assert!(!s.install(Grant { id: [0; 32], skill: 1, env: Envelope::physical(), until_tick: 1000 }));
    up.estop_latched = false;
    for t in 2..100 {
        up.tick = t;
        let mut d = brainos_blur::wire::NerveDown::new(t);
        s.step(&up, &seen, &mut d);
        assert_eq!(s.mode, Mode::EStopped, "released switch does not resume the body");
        assert!(d.frames.as_slice().iter().all(|f| is_estop(f)), "no setpoint ever goes out");
    }
}

// ---------------------------------------------------------------- KIRA stage for a dead body
struct AlwaysFalls;
impl brainos_blur::policy::Predictor for AlwaysFalls {
    fn predict(&self, _v: &brainos_abi::verb::Verb, _a: &brainos_body::policy::Args) -> Option<brainos_blur::policy::Prediction> {
        Some(brainos_blur::policy::Prediction { falls: true, joint_limit: None, person_contact: false, peak_torque_frac: 0.0, horizon_s: 1.0 })
    }
}

#[test]
fn a_motion_asked_of_an_estopped_body_is_refused_at_policy_not_simulate() {
    use brainos_body::policy::BodyPolicy;
    let verb = brainos_abi::verb::Verb::parse("legs.stand").unwrap();
    let args: brainos_body::policy::Args = Default::default();
    let mut p = brainos_blur::policy::BlurPolicy::new(Default::default(), blur_schema(), Default::default(), Some(Box::new(AlwaysFalls)));
    // powered and still: the (pessimistic) rollout is what refuses
    p.observe(&brainos_body::hal::Telemetry { link_alive: true, rail_v: Some(12.0), tilt_deg: Some(1.0), stops: 0, ..Default::default() });
    assert_eq!(p.simulate(&verb, &args).unwrap_err().code, brainos_abi::error::DenyCode::PredictedInvariantBreak);
    // the switch pressed: the rollout is not run, stage 6 refuses for the true reason
    p.observe(&brainos_body::hal::Telemetry { link_alive: true, rail_v: Some(0.0), tilt_deg: Some(60.0), stops: 1, ..Default::default() });
    assert!(p.simulate(&verb, &args).is_ok());
    assert_eq!(p.policy(&verb, &args).unwrap_err().code, brainos_abi::error::DenyCode::BodyInvariant);
    // a reflex owning a powered body: same
    p.observe(&brainos_body::hal::Telemetry { link_alive: true, rail_v: Some(12.0), tilt_deg: Some(60.0), stops: 1, ..Default::default() });
    assert!(p.simulate(&verb, &args).is_ok());
    assert_eq!(p.policy(&verb, &args).unwrap_err().code, brainos_abi::error::DenyCode::BodyInvariant);
}

#[test]
fn the_supervisors_sternal_and_ready_poses_are_the_trainers() {
    use brainos_blur::supervisor::{policy_joints, ready_pose, sternal_pose};
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../blur/learn/ref/poses.txt");
    let text = std::fs::read_to_string(path).unwrap();
    let rows: Vec<Vec<&str>> = text.lines().filter(|l| !l.starts_with('#')).map(|l| l.split_whitespace().collect()).collect();
    let col = |c: usize| -> Vec<f32> { rows.iter().map(|r| r[c].parse::<f32>().unwrap()).collect() };
    let (st, rd) = (col(1), col(2));
    for (k, a) in ACTUATORS.iter().enumerate() {
        assert_eq!(rows[k][0], a.name);
    }
    let (s, r) = (sternal_pose(), ready_pose());
    for jj in policy_joints() {
        assert!((s[jj] - st[jj]).abs() < 1e-3, "sternal {}: rust {} python {}", ACTUATORS[jj].name, s[jj], st[jj]);
        assert!((r[jj] - rd[jj]).abs() < 1e-3, "ready {}: rust {} python {}", ACTUATORS[jj].name, r[jj], rd[jj]);
    }
}

#[test]
fn the_supervisors_keyframe_references_are_the_trainers() {
    use brainos_blur::refs_gen::{RIGHTING_BACK, RIGHTING_LEFT, RIGHTING_RIGHT, STANDUP};
    use brainos_blur::supervisor::reference_at;
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../blur/learn/ref/ref_samples.txt");
    let text = std::fs::read_to_string(path).unwrap();
    let mut n = 0;
    for line in text.lines().filter(|l| !l.starts_with('#')) {
        let f: Vec<&str> = line.split_whitespace().collect();
        let keys = match f[0] {
            "STANDUP" => STANDUP,
            "RIGHTING_LEFT" => RIGHTING_LEFT,
            "RIGHTING_RIGHT" => RIGHTING_RIGHT,
            "RIGHTING_BACK" => RIGHTING_BACK,
            other => panic!("unknown reference {other}"),
        };
        let t: f32 = f[1].parse().unwrap();
        let q = reference_at(keys, t);
        for j in 0..N_ACT {
            let want: f32 = f[2 + j].parse().unwrap();
            assert!((q[j] - want).abs() < 1e-4, "{} t={t} {}: rust {} python {want}", f[0], ACTUATORS[j].name, q[j]);
        }
        n += 1;
    }
    assert!(n >= 4 * 20);
}
