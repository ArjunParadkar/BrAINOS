//! brainos-blur — BLUR v36, the body the entity is learning to inhabit.
//!
//! A small-cat-sized printed cheetah: 26 actuators (12 × 2806.5 on
//! B-G431B-ESC1, 7 × 4015 gimbals for abduction and the direct-drive
//! spine, 6 × 2204 for neck, jaw and the two tail winches, an N20 tail
//! gripper), a BMI270 in the chest, MT6701 encoders, a Teensy 4.1 as
//! Domain 2 and a Raspberry Pi 5 as the seat.
//!
//! ```text
//!   descriptor   the body as numbers, generated from the v36 MJCF
//!   wire         CAN frames, BMI270 bursts, the Teensy's pin boundary
//!   bus          the CAN budget, and why the joint loop lives on the nodes
//!   census       which silicon is on the buses; the build sheet
//!   schema       what BLUR declares; the phantom-limb check
//!   firmware     the body's attestation key and owner list
//!   autonomic    Domain 2's acquisition-time procedures (1 kHz)
//!   fit          the probe's least-squares: which way did the *motor* move it
//!   calibrate    probe results → calibration, or a refusal naming the part
//!   acquire      §8 end to end over brainos-body's state machine
//!   attitude     the inner ear: Mahony filter on the BMI270
//!   mlp          policy inference (the learned skills and reflexes)
//!   session      what one KIRA grant lets the body do; the Teensy's check of it
//!   policy       KIRA stages 4–6 for BLUR (rollout-backed simulate)
//!   supervisor   Domain 2 at run time: e-stop, reflexes, the granted envelope
//! ```
//!
//! The MuJoCo twin speaks exactly the `wire` formats (`blur/sim/`), so
//! everything in this crate runs unchanged against the twin and the
//! robot; only the thing that fills `NerveUp` differs.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod acquire;
pub mod attitude;
pub mod autonomic;
pub mod bus;
pub mod calibrate;
pub mod census;
pub mod descriptor;
pub mod firmware;
pub mod fit;
mod gen;
pub mod mlp;
pub mod policy;
#[rustfmt::skip]
pub mod refs_gen;
pub mod schema;
pub mod session;
pub mod supervisor;
pub mod wire;
