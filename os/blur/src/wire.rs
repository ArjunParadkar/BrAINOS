//! The wire: what BLUR's nerves carry, byte for byte.
//!
//! Two layers, and only the bottom one differs between the twin and the
//! robot:
//!
//! * **CAN frames** between the Teensy (Autonomic) and the motor nodes —
//!   twelve B-G431B-ESC1s and three ESP32-S3 nodes on three classic CAN
//!   buses at 1 Mbit/s. This module is the frame layout; the ESC and
//!   node firmware implement the other side of it, and so does the
//!   MuJoCo bridge (`blur/sim/wire.py`). The two implementations are
//!   tied together by the golden vectors in the tests here and in
//!   `blur/sim/test_wire.py`.
//! * **The BMI270 burst** — the IMU's own data registers, 0x0C..0x1A,
//!   exactly as an SPI burst read returns them.
//!
//! `NerveDown` / `NerveUp` are the Teensy's pin boundary in
//! software-in-the-loop: every CAN frame the controllers would put on the
//! buses this millisecond, and everything the pins would read back. On
//! the robot the same struct is filled by the FlexCAN and LPSPI drivers;
//! in the twin, by a datagram from the bridge. Nothing above it can tell.
//!
//! ## CAN identifier layout (11-bit)
//!
//! ```text
//!   id = function << 7 | node          lower id wins arbitration
//!   0x000          ESTOP      broadcast, highest priority on every bus
//!   0x080          CENSUS     broadcast request: every node sends IDENTITY
//!   0x100 | node   SETPOINT   q*, τff, kp, kd for one slot
//!   0x180 | node   FEEDBACK   q, dq, current, status for one slot
//!   0x200 | node   CONFIG     one calibration / envelope value for one slot
//!   0x280 | node   CONFIG_ACK the value the node now holds
//!   0x300 | node   IDENTITY   silicon UID, kind, slot count (three pages)
//!   0x380 | node   RAW        raw encoder counts + turns + temperature
//! ```
//!
//! The joint impedance loop runs **on the node** (q*, kp, kd, τff), not
//! across the bus: see `bus.rs` for why a 1 kHz loop over CAN does not
//! fit, by a factor of seven.

use crate::descriptor::{Bus, MotorKind};

pub const FC_ESTOP: u16 = 0;
pub const FC_CENSUS: u16 = 1;
pub const FC_SETPOINT: u16 = 2;
pub const FC_FEEDBACK: u16 = 3;
pub const FC_CONFIG: u16 = 4;
pub const FC_CONFIG_ACK: u16 = 5;
pub const FC_IDENTITY: u16 = 6;
pub const FC_RAW: u16 = 7;

/// The e-stop frame's magic, so a corrupted frame with id 0 is not an
/// e-stop *release* — there is no release frame at all; see `ESTOP`.
pub const ESTOP_MAGIC: [u8; 2] = [0xE5, 0x70];

/// One classic CAN frame on one of the three buses.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CanFrame {
    pub bus: Bus,
    pub id: u16,
    pub len: u8,
    pub data: [u8; 8],
}

impl CanFrame {
    pub fn new(bus: Bus, function: u16, node: u8, data: &[u8]) -> CanFrame {
        let mut d = [0u8; 8];
        let n = data.len().min(8);
        d[..n].copy_from_slice(&data[..n]);
        CanFrame {
            bus,
            id: (function << 7) | (node as u16 & 0x7F),
            len: n as u8,
            data: d,
        }
    }
    pub fn function(&self) -> u16 {
        self.id >> 7
    }
    pub fn node(&self) -> u8 {
        (self.id & 0x7F) as u8
    }
    /// Is this a well-formed 11-bit classic frame?
    pub fn is_valid(&self) -> bool {
        self.id < 0x800 && self.len <= 8
    }
    /// Bits on the wire, worst-case stuffing included (classic, 11-bit
    /// id): 44 frame bits + 8 per data byte + 3 bits of interframe space,
    /// plus one stuff bit per four over the 34 + 8n stuffable bits
    /// (SOF through CRC). 135 bits for an 8-byte frame.
    pub fn wire_bits_worst(len: u8) -> u32 {
        let n = len as u32;
        47 + 8 * n + (33 + 8 * n) / 4
    }
    pub const WIRE_BYTES: usize = 12;
    /// Serialized as it rides inside a nerve datagram: bus, id LE, len, data.
    pub fn write(&self, out: &mut [u8]) {
        out[0] = self.bus as u8;
        out[1..3].copy_from_slice(&self.id.to_le_bytes());
        out[3] = self.len;
        out[4..12].copy_from_slice(&self.data);
    }
    pub fn read(b: &[u8]) -> Option<CanFrame> {
        if b.len() < Self::WIRE_BYTES {
            return None;
        }
        let f = CanFrame {
            bus: Bus::from_u8(b[0])?,
            id: u16::from_le_bytes([b[1], b[2]]),
            len: b[3],
            data: [b[4], b[5], b[6], b[7], b[8], b[9], b[10], b[11]],
        };
        if f.is_valid() {
            Some(f)
        } else {
            None
        }
    }
}

/// Fixed-point scales, per motor kind. A winch's "angle" is cable length.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scales {
    /// units of q per count (rad or m)
    pub q: f32,
    /// N·m (or N) per count
    pub tau: f32,
    /// rad/s (or m/s) per count
    pub dq: f32,
    /// N·m/rad (or N/m) per kp count
    pub kp: f32,
    /// N·m·s/rad (or N·s/m) per kd count
    pub kd: f32,
}

pub const AMPS_PER_COUNT: f32 = 0.01;

pub fn scales(kind: MotorKind) -> Scales {
    match kind {
        MotorKind::Winch2204 => Scales {
            q: 2e-6,
            tau: 0.01,
            dq: 1e-4,
            kp: 20.0,
            kd: 0.2,
        },
        // 2.5e-4 rad: ±8.2 rad of headroom, because an uncalibrated joint
        // reads wherever its magnet happens to sit (±π) plus its travel,
        // and still finer than the MT6701's 3.8e-4 rad count
        _ => Scales {
            q: 2.5e-4,
            tau: 1e-3,
            dq: 1e-2,
            kp: 0.25,
            kd: 0.01,
        },
    }
}

fn q16(v: f32, scale: f32) -> i16 {
    let c = v / scale;
    if c.is_nan() {
        0
    } else if c >= 32767.0 {
        32767
    } else if c <= -32767.0 {
        -32767
    } else {
        // round half away from zero, the same in both implementations
        (if c >= 0.0 { c + 0.5 } else { c - 0.5 }) as i16
    }
}

fn q8(v: f32, scale: f32) -> u8 {
    let c = v / scale;
    if c.is_nan() || c <= 0.0 {
        0
    } else if c >= 255.0 {
        255
    } else {
        (c + 0.5) as u8
    }
}

/// SETPOINT flags
pub const SP_ENABLE: u8 = 1 << 0;

/// The seat of the joint's impedance loop: τ = kp(q* − q) + kd(0 − dq) + τff,
/// evaluated on the node at its own rate and clamped there to the
/// envelope's current limit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Setpoint {
    pub slot: u8,
    pub flags: u8,
    pub q: f32,
    pub tau_ff: f32,
    pub kp: f32,
    pub kd: f32,
}

impl Setpoint {
    pub fn encode(&self, bus: Bus, node: u8, kind: MotorKind) -> CanFrame {
        let s = scales(kind);
        let q = q16(self.q, s.q).to_le_bytes();
        let t = q16(self.tau_ff, s.tau).to_le_bytes();
        CanFrame::new(
            bus,
            FC_SETPOINT,
            node,
            &[self.slot, self.flags, q[0], q[1], t[0], t[1], q8(self.kp, s.kp), q8(self.kd, s.kd)],
        )
    }
    pub fn decode(f: &CanFrame, kind: MotorKind) -> Option<Setpoint> {
        if f.function() != FC_SETPOINT || f.len != 8 {
            return None;
        }
        let s = scales(kind);
        let d = &f.data;
        Some(Setpoint {
            slot: d[0],
            flags: d[1],
            q: i16::from_le_bytes([d[2], d[3]]) as f32 * s.q,
            tau_ff: i16::from_le_bytes([d[4], d[5]]) as f32 * s.tau,
            kp: d[6] as f32 * s.kp,
            kd: d[7] as f32 * s.kd,
        })
    }
}

/// FEEDBACK / RAW status bits
pub const ST_ENABLED: u8 = 1 << 0;
pub const ST_ENC_OK: u8 = 1 << 1;
pub const ST_OVERCURRENT: u8 = 1 << 2;
pub const ST_OVERTEMP: u8 = 1 << 3;
pub const ST_FAULT: u8 = 1 << 4;
pub const ST_ESTOPPED: u8 = 1 << 5;
pub const ST_CALIBRATED: u8 = 1 << 6;
pub const ST_CLAMPED: u8 = 1 << 7;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Feedback {
    pub slot: u8,
    pub status: u8,
    /// joint angle as the node sees it: raw encoder through whatever
    /// calibration the node currently holds (none = identity)
    pub q: f32,
    pub dq: f32,
    /// phase current magnitude with the sign of the torque, A
    pub current: f32,
}

impl Feedback {
    pub fn encode(&self, bus: Bus, node: u8, kind: MotorKind) -> CanFrame {
        let s = scales(kind);
        let q = q16(self.q, s.q).to_le_bytes();
        let v = q16(self.dq, s.dq).to_le_bytes();
        let i = q16(self.current, AMPS_PER_COUNT).to_le_bytes();
        CanFrame::new(
            bus,
            FC_FEEDBACK,
            node,
            &[self.slot, self.status, q[0], q[1], v[0], v[1], i[0], i[1]],
        )
    }
    pub fn decode(f: &CanFrame, kind: MotorKind) -> Option<Feedback> {
        if f.function() != FC_FEEDBACK || f.len != 8 {
            return None;
        }
        let s = scales(kind);
        let d = &f.data;
        Some(Feedback {
            slot: d[0],
            status: d[1],
            q: i16::from_le_bytes([d[2], d[3]]) as f32 * s.q,
            dq: i16::from_le_bytes([d[4], d[5]]) as f32 * s.dq,
            current: i16::from_le_bytes([d[6], d[7]]) as f32 * AMPS_PER_COUNT,
        })
    }
}

/// Raw encoder state, for calibration: the MT6701's 14-bit count, the
/// node's turn counter since power-on (meaningful for motor-side
/// encoders), and the driver temperature.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Raw {
    pub slot: u8,
    pub status: u8,
    pub counts: u16,
    pub turns: i16,
    pub temp_c: i8,
}

impl Raw {
    pub fn encode(&self, bus: Bus, node: u8) -> CanFrame {
        let c = self.counts.to_le_bytes();
        let t = self.turns.to_le_bytes();
        CanFrame::new(
            bus,
            FC_RAW,
            node,
            &[self.slot, self.status, c[0], c[1], t[0], t[1], self.temp_c as u8, 0],
        )
    }
    pub fn decode(f: &CanFrame) -> Option<Raw> {
        if f.function() != FC_RAW || f.len != 8 {
            return None;
        }
        let d = &f.data;
        Some(Raw {
            slot: d[0],
            status: d[1],
            counts: u16::from_le_bytes([d[2], d[3]]),
            turns: i16::from_le_bytes([d[4], d[5]]),
            temp_c: d[6] as i8,
        })
    }
}

/// CONFIG keys. A node holds these only while powered (Memory-Integrity
/// Law); the entity pushes them at every acquisition.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum ConfigKey {
    /// added to the direction-corrected encoder angle, rad (m)
    Offset = 1,
    /// +1 or −1
    Direction = 2,
    /// phase current clamp, A — the envelope, enforced at the node
    CurrentLimit = 3,
    QMin = 4,
    QMax = 5,
    /// whole motor turns to add, for a motor-side encoder
    TurnOffset = 6,
    /// 1 = calibration complete; the node will accept ENABLE
    Commit = 7,
    /// a raw-mode torque request for probing, N·m, applied with kp=kd=0
    ProbeTorque = 8,
}

impl ConfigKey {
    pub fn from_u8(v: u8) -> Option<ConfigKey> {
        Some(match v {
            1 => ConfigKey::Offset,
            2 => ConfigKey::Direction,
            3 => ConfigKey::CurrentLimit,
            4 => ConfigKey::QMin,
            5 => ConfigKey::QMax,
            6 => ConfigKey::TurnOffset,
            7 => ConfigKey::Commit,
            8 => ConfigKey::ProbeTorque,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Config {
    pub slot: u8,
    pub key: ConfigKey,
    pub value: f32,
    pub seq: u8,
}

impl Config {
    pub fn encode(&self, bus: Bus, node: u8, ack: bool) -> CanFrame {
        let v = self.value.to_le_bytes();
        CanFrame::new(
            bus,
            if ack { FC_CONFIG_ACK } else { FC_CONFIG },
            node,
            &[self.slot, self.key as u8, v[0], v[1], v[2], v[3], self.seq, 0],
        )
    }
    pub fn decode(f: &CanFrame) -> Option<(Config, bool)> {
        let ack = match f.function() {
            FC_CONFIG => false,
            FC_CONFIG_ACK => true,
            _ => return None,
        };
        if f.len != 8 {
            return None;
        }
        let d = &f.data;
        Some((
            Config {
                slot: d[0],
                key: ConfigKey::from_u8(d[1])?,
                value: f32::from_le_bytes([d[2], d[3], d[4], d[5]]),
                seq: d[6],
            },
            ack,
        ))
    }
}

pub const KIND_ESC1: u8 = 1;
pub const KIND_ESP32S3: u8 = 2;

/// One page of a node's identity. Three pages carry a 12-byte UID (the
/// STM32G431's 96-bit unique id; an ESP32-S3 pads its 6-byte MAC).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IdentityPage {
    pub page: u8,
    pub kind: u8,
    pub slots: u8,
    pub fw: u8,
    pub uid: [u8; 4],
}

impl IdentityPage {
    pub fn encode(&self, bus: Bus, node: u8) -> CanFrame {
        let u = self.uid;
        CanFrame::new(
            bus,
            FC_IDENTITY,
            node,
            &[self.page, self.kind, self.slots, self.fw, u[0], u[1], u[2], u[3]],
        )
    }
    pub fn decode(f: &CanFrame) -> Option<IdentityPage> {
        if f.function() != FC_IDENTITY || f.len != 8 || f.data[0] > 2 {
            return None;
        }
        let d = &f.data;
        Some(IdentityPage {
            page: d[0],
            kind: d[1],
            slots: d[2],
            fw: d[3],
            uid: [d[4], d[5], d[6], d[7]],
        })
    }
}

/// The e-stop frame. There is deliberately no frame that *releases* an
/// e-stop: a node that saw one stays disabled until it is power-cycled
/// or re-acquired (CONFIG Commit), so a replayed or forged frame can
/// only ever stop the robot, never start it.
pub fn estop(bus: Bus, reason: u8, seq: u8) -> CanFrame {
    CanFrame::new(bus, FC_ESTOP, 0, &[ESTOP_MAGIC[0], ESTOP_MAGIC[1], reason, seq])
}

pub fn is_estop(f: &CanFrame) -> bool {
    f.id == 0 && f.len >= 2 && f.data[0] == ESTOP_MAGIC[0] && f.data[1] == ESTOP_MAGIC[1]
}

pub fn census(bus: Bus) -> CanFrame {
    CanFrame::new(bus, FC_CENSUS, 0, &[0])
}

// ------------------------------------------------------------- BMI270

pub const BMI270_ACC_LSB_PER_G: f32 = 4096.0; // ±8 g
pub const BMI270_GYR_LSB_PER_DPS: f32 = 16.384; // ±2000 °/s
pub const G: f32 = 9.80665;

/// Registers 0x0C..0x1A of a BMI270 burst read: ACC xyz, GYR xyz (i16 LE),
/// SENSORTIME (u24 LE, 39.0625 µs per tick).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Bmi270Burst {
    pub acc: [i16; 3],
    pub gyr: [i16; 3],
    pub sensortime: u32,
}

impl Bmi270Burst {
    pub const LEN: usize = 15;
    pub fn read(b: &[u8]) -> Option<Bmi270Burst> {
        if b.len() < Self::LEN {
            return None;
        }
        let w = |i: usize| i16::from_le_bytes([b[i], b[i + 1]]);
        Some(Bmi270Burst {
            acc: [w(0), w(2), w(4)],
            gyr: [w(6), w(8), w(10)],
            sensortime: b[12] as u32 | (b[13] as u32) << 8 | (b[14] as u32) << 16,
        })
    }
    pub fn write(&self, out: &mut [u8]) {
        for k in 0..3 {
            out[2 * k..2 * k + 2].copy_from_slice(&self.acc[k].to_le_bytes());
            out[6 + 2 * k..8 + 2 * k].copy_from_slice(&self.gyr[k].to_le_bytes());
        }
        out[12] = self.sensortime as u8;
        out[13] = (self.sensortime >> 8) as u8;
        out[14] = (self.sensortime >> 16) as u8;
    }
    /// m/s² in the sensor frame
    pub fn accel(&self) -> [f32; 3] {
        let s = G / BMI270_ACC_LSB_PER_G;
        [self.acc[0] as f32 * s, self.acc[1] as f32 * s, self.acc[2] as f32 * s]
    }
    /// rad/s in the sensor frame
    pub fn gyro(&self) -> [f32; 3] {
        let s = core::f32::consts::PI / 180.0 / BMI270_GYR_LSB_PER_DPS;
        [self.gyr[0] as f32 * s, self.gyr[1] as f32 * s, self.gyr[2] as f32 * s]
    }
}

// ------------------------------------------------------------- the pin boundary

pub const MAX_FRAMES: usize = 96;

/// Everything the Teensy puts on its pins in one millisecond.
#[derive(Clone, Debug)]
pub struct NerveDown {
    pub tick: u32,
    /// the Teensy's own e-stop relay output (it can only *open* the
    /// motor rail; the latching switch is in series and is not software)
    pub relay_open: bool,
    pub frames: heapless_vec::Frames,
}

/// Everything the Teensy's pins read back in one millisecond.
#[derive(Clone, Debug)]
pub struct NerveUp {
    pub tick: u32,
    /// None = the IMU did not answer the SPI burst (disconnected)
    pub imu: Option<Bmi270Burst>,
    /// the latching e-stop switch, read on a GPIO: true = pressed
    pub estop_latched: bool,
    /// the motor rail is live (after relay and switch)
    pub rail_on: bool,
    pub vbat_mv: u16,
    pub frames: heapless_vec::Frames,
}

pub mod heapless_vec {
    //! A fixed-capacity frame list, so the 1 kHz loop never allocates.
    use super::{CanFrame, MAX_FRAMES};
    use crate::descriptor::Bus;

    #[derive(Clone, Debug)]
    pub struct Frames {
        len: usize,
        buf: [CanFrame; MAX_FRAMES],
    }

    impl Default for Frames {
        fn default() -> Self {
            Frames::new()
        }
    }

    impl Frames {
        pub const fn new() -> Frames {
            Frames {
                len: 0,
                buf: [CanFrame {
                    bus: Bus::A,
                    id: 0,
                    len: 0,
                    data: [0; 8],
                }; MAX_FRAMES],
            }
        }
        /// false when full: the frame is dropped, exactly like a CAN
        /// controller's full TX mailbox, and the caller can count it
        pub fn push(&mut self, f: CanFrame) -> bool {
            if self.len == MAX_FRAMES {
                return false;
            }
            self.buf[self.len] = f;
            self.len += 1;
            true
        }
        pub fn as_slice(&self) -> &[CanFrame] {
            &self.buf[..self.len]
        }
        pub fn len(&self) -> usize {
            self.len
        }
        pub fn is_empty(&self) -> bool {
            self.len == 0
        }
        pub fn clear(&mut self) {
            self.len = 0;
        }
    }
}

impl NerveDown {
    pub fn new(tick: u32) -> NerveDown {
        NerveDown {
            tick,
            relay_open: false,
            frames: heapless_vec::Frames::new(),
        }
    }
    /// 'D' tick:u32 relay:u8 n:u8 frames[n]
    pub fn write(&self, out: &mut [u8]) -> usize {
        out[0] = b'D';
        out[1..5].copy_from_slice(&self.tick.to_le_bytes());
        out[5] = self.relay_open as u8;
        out[6] = self.frames.len() as u8;
        let mut p = 7;
        for f in self.frames.as_slice() {
            f.write(&mut out[p..p + CanFrame::WIRE_BYTES]);
            p += CanFrame::WIRE_BYTES;
        }
        p
    }
    pub const MAX_BYTES: usize = 7 + MAX_FRAMES * CanFrame::WIRE_BYTES;
}

impl NerveUp {
    /// 'U' tick:u32 flags:u8 vbat:u16 imu[15] n:u8 frames[n]
    /// flags: bit0 imu present, bit1 e-stop latched, bit2 rail on
    pub fn read(b: &[u8]) -> Option<NerveUp> {
        if b.len() < 24 || b[0] != b'U' {
            return None;
        }
        let tick = u32::from_le_bytes([b[1], b[2], b[3], b[4]]);
        let flags = b[5];
        let vbat_mv = u16::from_le_bytes([b[6], b[7]]);
        let imu = if flags & 1 != 0 {
            Some(Bmi270Burst::read(&b[8..23])?)
        } else {
            None
        };
        let n = b[23] as usize;
        if b.len() < 24 + n * CanFrame::WIRE_BYTES || n > MAX_FRAMES {
            return None;
        }
        let mut frames = heapless_vec::Frames::new();
        for k in 0..n {
            let p = 24 + k * CanFrame::WIRE_BYTES;
            frames.push(CanFrame::read(&b[p..p + CanFrame::WIRE_BYTES])?);
        }
        Some(NerveUp {
            tick,
            imu,
            estop_latched: flags & 2 != 0,
            rail_on: flags & 4 != 0,
            vbat_mv,
            frames,
        })
    }
}
