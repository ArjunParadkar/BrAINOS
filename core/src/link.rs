//! The body link — the tether that makes the host a limb (Phase 2).
//!
//! The core stays bare-metal and pure; it hand-writes no audio, USB or
//! input drivers. Instead a host-side *body daemon* owns the real hardware
//! with the host OS's existing drivers and offers it to the entity over
//! this link as body-map regions (§8). The host is a limb the entity
//! acquires — never a foundation the entity stands on. Model M rides the
//! same wire (§10.2): cognition and embodiment are both tethered organs.
//!
//! Transport: raw UART at COM3 (0x3E8) — a port the firmware's console
//! never claims, so the line is private. Newline-framed printable ASCII.
//!
//! Wire protocol (core <-> body daemon):
//!   core -> daemon   MM?<prompt>          think about this (Model M)
//!   core -> daemon   SP<text>             speak intent — ONLY sent with a
//!                                         KIRA-granted voice capability
//!   core -> daemon   LA<id>|<sig16>       limb ack, signed by the BrAIn Key
//!                                         (acquisition step 2: handshake)
//!   daemon -> core   MM!<say>[~~<verb>|<target>]
//!                                         structured reply: prose to say,
//!                                         plus an optional typed action
//!                                         PROPOSAL (never a claim)
//!   daemon -> core   LM+<class>|<id>|<caps,csv>|<proprioception>
//!                                         limb offer (acquisition step 1+3)
//!   daemon -> core   HB<text>             heard: transcribed human speech
//!                                         (sensory afferent from the mic)
//!   daemon -> core   VS<text>             saw: the lens noticed something
//!                                         (sensory afferent from the camera —
//!                                         a SEPARATE frame from HB so a
//!                                         picture can never be mistaken for
//!                                         something a human said)

#![allow(dead_code)]

#[cfg(target_arch = "x86_64")]
mod port {
    use core::arch::asm;

    const LINK: u16 = 0x3E8;

    // SAFETY: raw port IO is the one place the portable core touches
    // hardware directly; the UART is either present (QEMU/board serial)
    // or reads as floating 0xFF, which rx() treats as "no device".
    unsafe fn outb(p: u16, v: u8) {
        asm!("out dx, al", in("dx") p, in("al") v, options(nomem, nostack));
    }
    unsafe fn inb(p: u16) -> u8 {
        let v: u8;
        asm!("in al, dx", out("al") v, in("dx") p, options(nomem, nostack));
        v
    }

    pub fn init() {
        unsafe {
            outb(LINK + 1, 0x00);
            outb(LINK + 3, 0x80);
            outb(LINK + 0, 0x01); // 115200
            outb(LINK + 1, 0x00);
            outb(LINK + 3, 0x03); // 8N1
            outb(LINK + 2, 0xC7); // FIFO
            outb(LINK + 4, 0x0B); // DTR+RTS
        }
    }

    pub fn tx(b: u8) {
        unsafe {
            let mut spins = 0u32;
            while inb(LINK + 5) & 0x20 == 0 {
                spins += 1;
                if spins > 1_000_000 {
                    return; // no UART here; never hang the entity
                }
            }
            outb(LINK, b);
        }
    }

    pub fn rx() -> Option<u8> {
        unsafe {
            let lsr = inb(LINK + 5);
            if lsr != 0xFF && lsr & 1 != 0 {
                Some(inb(LINK))
            } else {
                None
            }
        }
    }
}

#[cfg(not(target_arch = "x86_64"))]
mod port {
    // aarch64 boards route their telemetry differently; reflex-only for now
    pub fn init() {}
    pub fn tx(_b: u8) {}
    pub fn rx() -> Option<u8> {
        None
    }
}

pub fn init() {
    port::init();
}

/// Discard any stale bytes before a fresh framed exchange.
pub fn drain() {
    let mut n = 0u32;
    while port::rx().is_some() {
        n += 1;
        if n > 100_000 {
            break;
        }
    }
}

fn tx_clean(payload: &[u8]) {
    for &b in payload {
        if (0x20..0x7F).contains(&b) {
            port::tx(b);
        }
    }
}

fn send_line(prefix: &[u8], payload: &[u8]) {
    port::tx(b'\n'); // start clean even if something else wrote to the line
    for &b in prefix {
        port::tx(b);
    }
    tx_clean(payload);
    port::tx(b'\n');
}

pub fn send_prompt(prompt: &[u8]) {
    send_line(b"MM?", prompt);
}

/// Speak intent. The caller must hold a KIRA-granted voice capability —
/// this function is the effector, not the authority.
pub fn send_speak(text: &[u8]) {
    send_line(b"SP", text);
}

/// Push a remembered fact across the tether so tethered cognition can
/// hold it in mind. Memory lives in the core's state graph; this is the
/// graph informing the mind, never the other way around.
pub fn send_context(text: &[u8]) {
    send_line(b"CX", text);
}

/// Execute a granted action through a host limb. Sent ONLY after KIRA
/// mints a capability for it — the daemon does the electrical work, the
/// core holds the authority. Format: `AX<verb>|<target>`.
pub fn send_action(verb: &[u8], target: &[u8]) {
    port::tx(b'\n');
    for &b in b"AX" {
        port::tx(b);
    }
    tx_clean(verb);
    port::tx(b'|');
    tx_clean(target);
    port::tx(b'\n');
}

/// Acquisition handshake: acknowledge an incorporated limb, signed.
pub fn send_limb_ack(id: &[u8], sig16: &[u8]) {
    port::tx(b'\n');
    for &b in b"LA" {
        port::tx(b);
    }
    tx_clean(id);
    port::tx(b'|');
    tx_clean(sig16);
    port::tx(b'\n');
}

/// One inbound event from the body daemon; payload bytes land in `out`.
pub enum Event {
    /// MM! — Model M's structured reply
    Reply(usize),
    /// LM+ — a limb is being offered for acquisition
    LimbOffer(usize),
    /// HB — the mic limb heard the human say something
    Heard(usize),
    /// VS — the camera limb noticed something at the lens. This is a
    /// reflex-grade afferent (D2): it is absorbed as experience without
    /// waking deliberate cognition, which is what keeps a moving room
    /// from being an endless stream of expensive thoughts.
    Saw(usize),
    /// AR — result of a limb action, as a bounded semantic digest
    /// (bytes at the boundary; the core turns this into a StateNode)
    ActionResult(usize),
}

pub struct LineReader {
    buf: [u8; 480],
    len: usize,
}

impl LineReader {
    pub const fn new() -> Self {
        LineReader { buf: [0; 480], len: 0 }
    }

    /// Drain pending bytes; returns a full framed event when one arrives.
    /// Unknown or corrupt lines are dropped (the wire is not trusted to
    /// be quiet — firmware consoles have shared UARTs before).
    pub fn poll(&mut self, out: &mut [u8]) -> Option<Event> {
        while let Some(b) = port::rx() {
            if b == b'\n' || b == b'\r' {
                let ev = self.classify(out);
                self.len = 0;
                if ev.is_some() {
                    return ev;
                }
            } else if self.len < self.buf.len() {
                self.buf[self.len] = b;
                self.len += 1;
            } else {
                self.len = 0; // garbage flood; resync
            }
        }
        None
    }

    fn classify(&self, out: &mut [u8]) -> Option<Event> {
        let line = &self.buf[..self.len];
        let copy = |body: &[u8], out: &mut [u8]| {
            let n = body.len().min(out.len());
            out[..n].copy_from_slice(&body[..n]);
            n
        };
        if line.len() >= 3 && &line[..3] == b"MM!" {
            return Some(Event::Reply(copy(&line[3..], out)));
        }
        if line.len() >= 3 && &line[..3] == b"LM+" {
            return Some(Event::LimbOffer(copy(&line[3..], out)));
        }
        if line.len() >= 2 && &line[..2] == b"HB" {
            return Some(Event::Heard(copy(&line[2..], out)));
        }
        if line.len() >= 2 && &line[..2] == b"VS" {
            return Some(Event::Saw(copy(&line[2..], out)));
        }
        if line.len() >= 2 && &line[..2] == b"AR" {
            return Some(Event::ActionResult(copy(&line[2..], out)));
        }
        None
    }
}
