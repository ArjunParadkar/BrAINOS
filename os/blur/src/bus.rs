//! BFS  LEVEL    Domain 2 Autonomic - the BLUR quadruped body (wire, acquisition, supervisor)  [crate brainos-blur]
//! BFS  PURPOSE  Computes CAN bus load from motor count and rate, showing a 1 kHz joint loop cannot cross the bus, so motor nodes close it and setpoints go at 200 Hz
//! BFS  DEFINES  BITRATE, PLAN_LOAD, SETPOINT_HZ, motors_on, capacity_fps, load_at, max_rate_hz, buses_needed_for
//! BFS  USES     this crate: descriptor, wire
//! BFS  USED BY  blur/tests/blur_suite, nostd/lib
//!
//! The CAN budget, and why the joint loop lives on the nodes.
//!
//! Every motor costs two frames per control cycle: a SETPOINT down and a
//! FEEDBACK up. A classic 8-byte frame is at most 135 bits with
//! worst-case stuffing, so a 1 Mbit/s bus carries at most ~7,400 of them
//! a second. SN65HVD230 transceivers are rated to 1 Mbit/s, so CAN-FD's
//! fast data phase is not available to us.
//!
//! ```text
//!   26 motors × 2 frames × 1 kHz = 52,000 frames/s  vs  7,407 per bus
//! ```
//!
//! A 1 kHz joint loop closed *across* the bus needs eight buses at 100 %
//! load (7.02, rounded up). The Teensy has three. So the impedance loop (q*, kp, kd, τff) runs on
//! each node at its own rate — the ESC1 at its FOC rate, the ESP32-S3
//! nodes at theirs — and each node clamps current to the envelope
//! locally, which is what makes "an envelope violation is clamped within
//! 1 ms" true at the place where torque is made. The Teensy's 1 kHz loop
//! does IMU fusion, reflexes and envelope supervision, and refreshes
//! setpoints at [`SETPOINT_HZ`].

use crate::descriptor::{Bus, ACTUATORS};
use crate::wire::CanFrame;

pub const BITRATE: u32 = 1_000_000;

/// The load we are willing to plan to. Above ~70 % a lower-priority
/// frame (RAW, CONFIG) can starve behind a burst of setpoints.
pub const PLAN_LOAD: f32 = 0.70;

/// The setpoint / feedback refresh rate the budget allows on every bus.
pub const SETPOINT_HZ: u32 = 200;

/// Motors on a bus.
pub fn motors_on(bus: Bus) -> usize {
    ACTUATORS.iter().filter(|a| a.bus == bus).count()
}

/// Frames per second a bus can carry at a given load fraction.
pub fn capacity_fps(load: f32) -> f32 {
    BITRATE as f32 * load / CanFrame::wire_bits_worst(8) as f32
}

/// Bus load at a refresh rate, as a fraction of the raw bit rate.
pub fn load_at(bus: Bus, hz: u32) -> f32 {
    let frames = (2 * motors_on(bus)) as f32 * hz as f32;
    frames * CanFrame::wire_bits_worst(8) as f32 / BITRATE as f32
}

/// The fastest refresh each bus supports at the planning load.
pub fn max_rate_hz(bus: Bus) -> f32 {
    let n = motors_on(bus);
    if n == 0 {
        return f32::INFINITY;
    }
    capacity_fps(PLAN_LOAD) / (2 * n) as f32
}

/// How many 1 Mbit/s buses a joint loop closed across CAN would need.
pub fn buses_needed_for(hz: u32) -> u32 {
    let frames = 2.0 * ACTUATORS.len() as f32 * hz as f32;
    let per_bus = capacity_fps(1.0);
    let n = frames / per_bus;
    let whole = n as u32;
    if (whole as f32) < n {
        whole + 1
    } else {
        whole
    }
}
