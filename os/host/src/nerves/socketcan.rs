//! BFS  LEVEL    Host - the only crate with an operating system (CLI, scenarios, OS nerves)  [crate brainos-host]
//! BFS  PURPOSE  The CAN nerve: BLUR motor nodes found by census and felt through feedback frames, over SocketCAN or an in-memory bus
//! BFS  DEFINES  CanBus, SocketCanBus, SimNode, MemBus, BlurCanNerve
//! BFS  USES     this crate: can_ffi, stable_key | brainos: abi, blur, body, crypto, part
//! BFS  USED BY  host/tests/nerves_can
//!
//! BLUR motor nodes over CAN.
//!
//! This nerve speaks the one CAN protocol the repo already has
//! (`brainos_blur::wire`, specified in `firmware/protocol.md`, with
//! golden vectors shared by the Rust, Python and C implementations). It
//! adds no frame and changes none.
//!
//! ```text
//!   enumerate   send CENSUS; every node that answers with IDENTITY page 0 is a part
//!   read        collect FEEDBACK frames; per motor slot: position, rate, current, fault
//!   safe        send ESTOP (the nodes latch; there is no release frame)
//! ```
//!
//! | sense | from | vital? |
//! |---|---|---|
//! | `sN.q` | feedback position, in the joint's own unit | no |
//! | `sN.dq` | feedback rate | no |
//! | `sN.a` | feedback current, amperes | no |
//! | `sN.fault` | status bits OVERCURRENT, OVERTEMP, FAULT | **yes**: any of them is critical |
//!
//! **It has no verbs, deliberately.** Moving a BLUR joint already has a
//! path: a KIRA-granted session envelope installed in the Teensy's
//! supervisor (`brainos_blur::session`), which streams setpoints at
//! 200 Hz inside it. Offering `motor.set` here would be a second way
//! to move the same joint, outside that envelope. So this nerve feels
//! the nodes and can stop them, and the existing path moves them.
//!
//! **Identity.** A node's 4-byte hardware uid, with its bus and node
//! number, is the stable identifier. The protocol has no per-node
//! challenge, so this is identity and not attestation — the same
//! standing as the sysfs parts. (BLUR's attestation key belongs to the
//! Teensy, at the level of the whole body: `brainos_blur::firmware`.)
//!
//! `CanBus` is the transport seam. `SocketCanBus` is the real one;
//! `MemBus` is an in-memory bus with simulated nodes, which is what
//! the tests that need no CAN interface run on.

use super::can_ffi::{CanSocket, OpenError};
use super::stable_key;
use brainos_abi::text::Text;
use brainos_abi::time::Instant;
use brainos_blur::descriptor::{actuator_at, Bus, ACTUATORS};
use brainos_blur::wire::{
    census, estop, CanFrame, Feedback, IdentityPage, FC_CENSUS, FC_FEEDBACK, FC_IDENTITY,
    ST_FAULT, ST_OVERCURRENT, ST_OVERTEMP,
};
use brainos_body::vital::Band;
use brainos_crypto::key::{BrainKey, Challenge, Signature};
use brainos_part::class::PartClass;
use brainos_part::nerve::{Found, Mandate, Nerve, NerveFault, Sensed};
use brainos_part::schema::{PartSchema, Quantity, SenseSpec, VitalKind, VitalSpec};

/// Something frames can be sent on and received from.
pub trait CanBus {
    fn send(&mut self, f: &CanFrame) -> bool;
    /// One frame, or `None` when nothing arrives in time.
    fn recv(&mut self) -> Option<CanFrame>;
}

/// A real interface.
pub struct SocketCanBus {
    sock: CanSocket,
    bus: Bus,
}

impl SocketCanBus {
    /// `ifname` is the kernel's name (`can0`, `vcan0`); `bus` is which
    /// of BLUR's three buses it is wired as.
    pub fn open(ifname: &str, bus: Bus) -> Result<SocketCanBus, OpenError> {
        Ok(SocketCanBus {
            sock: CanSocket::open(ifname, 50)?,
            bus,
        })
    }
}

impl CanBus for SocketCanBus {
    fn send(&mut self, f: &CanFrame) -> bool {
        self.sock.send(f.id, &f.data[..f.len as usize])
    }

    fn recv(&mut self) -> Option<CanFrame> {
        let (id, len, data) = self.sock.recv()?;
        let f = CanFrame { bus: self.bus, id, len, data };
        f.is_valid().then_some(f)
    }
}

/// One simulated node on a `MemBus`.
#[derive(Clone, Debug)]
pub struct SimNode {
    pub node: u8,
    pub uid: [u8; 4],
    pub kind: u8,
    /// (slot, status bits, position, rate, current)
    pub slots: Vec<(u8, u8, f32, f32, f32)>,
}

/// An in-memory bus with nodes that answer a census and report
/// feedback, encoded and decoded by the real codec.
pub struct MemBus {
    pub bus: Bus,
    pub nodes: Vec<SimNode>,
    inbox: std::collections::VecDeque<CanFrame>,
    /// every frame the nerve sent, for assertions
    pub sent: Vec<CanFrame>,
    pub estopped: bool,
}

impl MemBus {
    pub fn new(bus: Bus, nodes: Vec<SimNode>) -> MemBus {
        MemBus {
            bus,
            nodes,
            inbox: Default::default(),
            sent: Vec::new(),
            estopped: false,
        }
    }

    /// Have every node emit one round of feedback, as they do
    /// unprompted on the real bus.
    pub fn tick(&mut self) {
        for n in &self.nodes {
            for (slot, status, q, dq, current) in &n.slots {
                let Some(ai) = actuator_at(n.node, *slot) else {
                    continue;
                };
                let fb = Feedback { slot: *slot, status: *status, q: *q, dq: *dq, current: *current };
                self.inbox.push_back(fb.encode(self.bus, n.node, ACTUATORS[ai].motor));
            }
        }
    }
}

impl CanBus for MemBus {
    fn send(&mut self, f: &CanFrame) -> bool {
        self.sent.push(*f);
        if brainos_blur::wire::is_estop(f) {
            self.estopped = true;
        } else if f.function() == FC_CENSUS {
            for n in &self.nodes {
                let page = IdentityPage {
                    page: 0,
                    kind: n.kind,
                    slots: n.slots.len() as u8,
                    fw: 1,
                    uid: n.uid,
                };
                self.inbox.push_back(page.encode(self.bus, n.node));
            }
        }
        true
    }

    fn recv(&mut self) -> Option<CanFrame> {
        self.inbox.pop_front()
    }
}

struct NodePart {
    node: u8,
    uid: [u8; 4],
    slots: u8,
    key: BrainKey,
    label: String,
}

/// The nerve, over any bus.
pub struct BlurCanNerve<B: CanBus> {
    pub bus: B,
    which: Bus,
    parts: Vec<NodePart>,
    /// frames read while looking for something else, kept until the
    /// next read. Bounded; the oldest are dropped.
    pending: Vec<CanFrame>,
}

impl<B: CanBus> BlurCanNerve<B> {
    pub fn new(bus: B, which: Bus) -> BlurCanNerve<B> {
        BlurCanNerve { bus, which, parts: Vec::new(), pending: Vec::new() }
    }

    fn drain(&mut self, limit: usize) {
        for _ in 0..limit {
            match self.bus.recv() {
                Some(f) => {
                    if self.pending.len() >= 512 {
                        self.pending.remove(0);
                    }
                    self.pending.push(f);
                }
                None => break,
            }
        }
    }
}

impl<B: CanBus> Nerve for BlurCanNerve<B> {
    fn name(&self) -> &'static str {
        "socketcan"
    }

    fn enumerate(&mut self) -> Vec<Found> {
        if !self.bus.send(&census(self.which)) {
            return Vec::new();
        }
        self.drain(256);
        let mut parts: Vec<NodePart> = Vec::new();
        for f in &self.pending {
            if f.function() != FC_IDENTITY {
                continue;
            }
            let Some(page) = IdentityPage::decode(f) else {
                continue;
            };
            if page.page != 0 || parts.iter().any(|p| p.node == f.node()) {
                continue;
            }
            let uid = page.uid;
            let id = format!(
                "can:{}:{}:{:02x}{:02x}{:02x}{:02x}",
                self.which as u8,
                f.node(),
                uid[0],
                uid[1],
                uid[2],
                uid[3]
            );
            parts.push(NodePart {
                node: f.node(),
                uid,
                slots: page.slots,
                key: stable_key("socketcan", &id),
                label: format!("blur node {} bus {}", f.node(), ["A", "B", "C"][self.which as usize]),
            });
        }
        self.pending.retain(|f| f.function() != FC_IDENTITY);
        parts.sort_by_key(|p| p.node);
        self.parts = parts;
        self.parts
            .iter()
            .enumerate()
            .map(|(i, p)| Found {
                handle: i as u32,
                key: p.key.public(),
                label: Text::from_str_truncating(&p.label),
            })
            .collect()
    }

    fn answer(&mut self, part: &Found, c: &Challenge) -> Option<Signature> {
        self.parts.get(part.handle as usize).map(|p| p.key.answer(c))
    }

    fn describe(&mut self, part: &Found) -> Option<PartSchema> {
        let p = self.parts.get(part.handle as usize)?;
        let mut s = PartSchema::new(PartClass::Actuator, &p.label, "socketcan");
        let mut any = false;
        for slot in 0..p.slots {
            // only slots the body descriptor knows: an unknown slot has
            // no unit to decode its position in
            let Some(ai) = actuator_at(p.node, slot) else {
                continue;
            };
            let a = &ACTUATORS[ai];
            any = true;
            let span = (a.q_max - a.q_min).abs().max(1.0) as f64;
            s = s
                .sense(SenseSpec::new(
                    &format!("s{slot}.q"),
                    Quantity::Position,
                    a.q_min as f64 - span,
                    a.q_max as f64 + span,
                ))
                .sense(SenseSpec::new(&format!("s{slot}.dq"), Quantity::Position, -1e4, 1e4))
                .sense(SenseSpec::new(&format!("s{slot}.a"), Quantity::Current, -400.0, 400.0))
                .sense(SenseSpec::new(&format!("s{slot}.fault"), Quantity::Flag, 0.0, 1.0))
                .vital(VitalSpec::new(
                    &format!("s{slot}.fault"),
                    VitalKind::Faults,
                    Band::rising(0.5, 1.0),
                ));
        }
        any.then_some(s)
    }

    fn read(&mut self, part: &Found, _at: Instant, out: &mut Vec<Sensed>) {
        self.drain(256);
        let Some(p) = self.parts.get(part.handle as usize) else {
            return;
        };
        let node = p.node;
        // the newest feedback per slot; older frames for this node are
        // dropped, frames for other nodes are kept for their own read
        let mut latest: Vec<Feedback> = Vec::new();
        for f in self.pending.iter().filter(|f| f.function() == FC_FEEDBACK && f.node() == node) {
            let slot = f.data[0];
            let Some(ai) = actuator_at(node, slot) else {
                continue;
            };
            if let Some(fb) = Feedback::decode(f, ACTUATORS[ai].motor) {
                latest.retain(|x| x.slot != fb.slot);
                latest.push(fb);
            }
        }
        self.pending
            .retain(|f| !(f.function() == FC_FEEDBACK && f.node() == node));
        latest.sort_by_key(|f| f.slot);
        for fb in latest {
            let s = fb.slot;
            out.push(Sensed::new(&format!("s{s}.q"), fb.q as f64));
            out.push(Sensed::new(&format!("s{s}.dq"), fb.dq as f64));
            out.push(Sensed::new(&format!("s{s}.a"), fb.current as f64));
            let fault = fb.status & (ST_OVERCURRENT | ST_OVERTEMP | ST_FAULT) != 0;
            out.push(Sensed::new(&format!("s{s}.fault"), if fault { 1.0 } else { 0.0 }));
        }
    }

    fn apply(&mut self, _part: &Found, _m: &Mandate) -> Result<(), NerveFault> {
        // see the module note: joints move through the session envelope
        Err(NerveFault::NotSupported)
    }

    fn safe(&mut self) {
        // sent three times with increasing sequence numbers. That is
        // this nerve's choice, not something the protocol document
        // requires: one frame can be lost, and a node that has already
        // latched ignores the repeats.
        for seq in 0..3 {
            self.bus.send(&estop(self.which, 0, seq));
        }
    }
}

impl<B: CanBus> BlurCanNerve<B> {
    /// The hardware uid of each node found, for the survey.
    pub fn uids(&self) -> Vec<(u8, [u8; 4])> {
        self.parts.iter().map(|p| (p.node, p.uid)).collect()
    }
}
