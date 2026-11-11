//! BFS  LEVEL    Phase 0 - Domain 2 firmware for the ESP32-S3  [crate miniblur-a]
//! BFS  PURPOSE  ESP32-S3 hexapod firmware: verifies the seat's signature, takes commands over USB serial, re-checks limits, drives ten servos, stops on lost link
//! BFS  DEFINES  -
//! BFS  USES     brainos: miniblur-core | external: ed25519-dalek, esp-backtrace, esp-hal, esp-println
//! BFS  USED BY  (entry point or test)
//!
//! Mini-Blur board A — Domain 2 on the ESP32-S3 (Execution Plan §4, WP0.4/0.8).
//!
//! This is the body's reflex layer: ten servos on MCPWM, the tripod gait
//! played from `miniblur-core`, the same envelope / forward-model / invariant
//! checks KIRA ran at the seat run again here before anything moves, and a
//! stop-on-lost-link watchdog that stands the body still on its own
//! authority when the mind goes quiet (Architecture §14).
//!
//! The board speaks the Reach wire protocol (`miniblur_core::wire`) as
//! newline-framed JSON over the USB-Serial-JTAG port. On the bench,
//! `miniblur/tools/serial-reach-bridge.py` relays that line to the seat's
//! Reach port, so to the operator this board and the simulator are the same
//! body. Wi-Fi (esp-radio) is the next transport; it is NOT in this file.
//!
//! The body never remembers (§13.2): calibration arrives from the seat at
//! incorporation and is gone at power-off. The only thing baked in is the
//! entity's PUBLIC key (`pairing.hex`) — the pairing, so a stranger's seat is
//! refused and logged.
//!
//! Status: BUILT (compiles for xtensa-esp32s3-none-elf). Pin map, servo
//! direction and every number in the schema are hypotheses until the wiring
//! doc; metal is MANUAL.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::main;
use esp_hal::mcpwm::operator::{PwmPin, PwmPinConfig};
use esp_hal::mcpwm::timer::PwmWorkingMode;
use esp_hal::mcpwm::{McPwm, PeripheralClockConfig, PwmPeripheral};
use esp_hal::rng::Rng;
use esp_hal::time::{Instant, Rate};
use esp_hal::usb::usb_serial_jtag::UsbSerialJtag;
use esp_println::println;
use miniblur_core::runtime::{Body, ServoBus};
use miniblur_core::schema::{self, Model};
use miniblur_core::servo::ServoCal;
use miniblur_core::wire::{self, FromSeat};
use miniblur_core::MAX_SERVOS;

esp_bootloader_esp_idf::esp_app_desc!();

/// The entity this body is paired with (its PUBLIC key, hex). Written by
/// `./build.sh --esp32` from `key/brain_key.json`; a different seat is refused.
const PAIRING_HEX: &str = include_str!("../pairing.hex");
const NAME: &str = "miniblur";
/// Servo pulses: 1 µs per timer tick, 20 ms period → 50 Hz.
const SERVO_PERIOD_TICKS: u16 = 19_999;
const HELLO_RETRY_MS: u64 = 2_000;
const LINE_MAX: usize = 4_096;

// ---- the servo bus -----------------------------------------------------

/// One MCPWM output that takes a pulse width in microseconds.
trait Servo {
    fn set_us(&mut self, us: u16);
}

impl<'d, PWM: PwmPeripheral, const OP: u8, const IS_A: bool> Servo for PwmPin<'d, PWM, OP, IS_A> {
    fn set_us(&mut self, us: u16) {
        self.set_timestamp(us);
    }
}

/// Channels 0..=9 on two MCPWM units, three operators each, A and B pins.
/// Hypothesis pin map (WP0.7 decides the real one):
///   ch0 GPIO4  ch1 GPIO5  ch2 GPIO6  ch3 GPIO7  ch4 GPIO15 ch5 GPIO16   (MCPWM0)
///   ch6 GPIO17 ch7 GPIO18 ch8 GPIO8  ch9 GPIO3                          (MCPWM1)
struct McpwmBus<'d> {
    ch: Vec<Option<Box<dyn Servo + 'd>>>,
    last_us: [u16; MAX_SERVOS],
}

impl<'d> ServoBus for McpwmBus<'d> {
    fn set_deg(&mut self, ch: u8, deg: f32) {
        let us = ServoCal::pulse_us(deg);
        if let Some(Some(s)) = self.ch.get_mut(ch as usize) {
            s.set_us(us);
            self.last_us[ch as usize] = us;
        }
    }
    fn relax(&mut self) {
        // MG90S holds position while pulsed; "relax" is a pulse of 0 (no
        // signal) so the rail stops fighting gravity while sitting.
        for (i, s) in self.ch.iter_mut().enumerate() {
            if let Some(s) = s {
                s.set_us(0);
                self.last_us[i] = 0;
            }
        }
    }
}

// ---- the link -----------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Link {
    /// hello sent, waiting for the seat to prove it holds the key
    Hailing,
    /// attested; commands flow
    Bound,
}

struct Pairing {
    key: VerifyingKey,
    hex: String,
}

fn pairing() -> Option<Pairing> {
    let hex = PAIRING_HEX.lines().next().unwrap_or("").trim();
    let bytes = wire::hex_decode(hex)?;
    let arr: [u8; 32] = bytes.as_slice().try_into().ok()?;
    let key = VerifyingKey::from_bytes(&arr).ok()?;
    Some(Pairing { key, hex: String::from(hex) })
}

fn now_ms() -> u64 {
    Instant::now().duration_since_epoch().as_millis()
}

fn nonce_hex(rng: &Rng) -> String {
    let mut b = [0u8; 32];
    rng.read(&mut b);
    wire::hex_encode(&b)
}

#[main]
fn main() -> ! {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);
    esp_alloc::heap_allocator!(size: 96 * 1024);

    // ---- servos: two MCPWM units at 1 MHz, 50 Hz period, timestamp = µs
    let clock = PeripheralClockConfig::with_frequency(Rate::from_mhz(1)).expect("mcpwm clock");
    let mut pwm0 = McPwm::new(peripherals.MCPWM0, clock);
    let mut pwm1 = McPwm::new(peripherals.MCPWM1, clock);
    let timer_cfg = clock
        .timer_clock_with_frequency(SERVO_PERIOD_TICKS, PwmWorkingMode::Increase, Rate::from_hz(50))
        .expect("servo timer");
    pwm0.operator0.set_timer(&pwm0.timer0);
    pwm0.operator1.set_timer(&pwm0.timer0);
    pwm0.operator2.set_timer(&pwm0.timer0);
    pwm1.operator0.set_timer(&pwm1.timer0);
    pwm1.operator1.set_timer(&pwm1.timer0);
    let (c0, c1) = pwm0.operator0.with_pins(peripherals.GPIO4, PwmPinConfig::UP_ACTIVE_HIGH, peripherals.GPIO5, PwmPinConfig::UP_ACTIVE_HIGH);
    let (c2, c3) = pwm0.operator1.with_pins(peripherals.GPIO6, PwmPinConfig::UP_ACTIVE_HIGH, peripherals.GPIO7, PwmPinConfig::UP_ACTIVE_HIGH);
    let (c4, c5) = pwm0.operator2.with_pins(peripherals.GPIO15, PwmPinConfig::UP_ACTIVE_HIGH, peripherals.GPIO16, PwmPinConfig::UP_ACTIVE_HIGH);
    let (c6, c7) = pwm1.operator0.with_pins(peripherals.GPIO17, PwmPinConfig::UP_ACTIVE_HIGH, peripherals.GPIO18, PwmPinConfig::UP_ACTIVE_HIGH);
    let (c8, c9) = pwm1.operator1.with_pins(peripherals.GPIO8, PwmPinConfig::UP_ACTIVE_HIGH, peripherals.GPIO3, PwmPinConfig::UP_ACTIVE_HIGH);
    pwm0.timer0.start(timer_cfg);
    pwm1.timer0.start(timer_cfg);
    let mut ch: Vec<Option<Box<dyn Servo>>> = Vec::with_capacity(MAX_SERVOS);
    ch.push(Some(Box::new(c0)));
    ch.push(Some(Box::new(c1)));
    ch.push(Some(Box::new(c2)));
    ch.push(Some(Box::new(c3)));
    ch.push(Some(Box::new(c4)));
    ch.push(Some(Box::new(c5)));
    ch.push(Some(Box::new(c6)));
    ch.push(Some(Box::new(c7)));
    ch.push(Some(Box::new(c8)));
    ch.push(Some(Box::new(c9)));
    while ch.len() < MAX_SERVOS {
        ch.push(None);
    }
    let bus = McpwmBus { ch, last_us: [0; MAX_SERVOS] };

    // ---- the body: model + Domain 2 loop. Sits, servos relaxed, until the mind arrives.
    let model = Model::default();
    let mut body = Body::new(model, bus, now_ms());
    body.state.vbat = f32::NAN; // not measured on this board yet (no ADC divider wired)
    body.bus.relax();

    // ---- the link: USB-Serial-JTAG, newline-framed JSON, the Reach protocol
    let mut serial = UsbSerialJtag::new(peripherals.USB_DEVICE);
    let rng = Rng::new();
    let pair = pairing();
    match &pair {
        Some(p) => println!("[miniblur-a] paired with {}..", &p.hex[..8]),
        None => println!("[miniblur-a] NOT PAIRED: pairing.hex is not a 32-byte hex key; every seat will be refused"),
    }

    let mut line: Vec<u8> = Vec::with_capacity(LINE_MAX);
    let mut link = Link::Hailing;
    let mut nonce = nonce_hex(&rng);
    let mut hello_at: u64 = 0;
    let send = |serial: &mut UsbSerialJtag<'_, esp_hal::Blocking>, s: &str| {
        let _ = serial.write(s.as_bytes());
        let _ = serial.flush_tx();
    };

    loop {
        let t = now_ms();

        // hail the seat until it attests
        if link == Link::Hailing && t.saturating_sub(hello_at) >= HELLO_RETRY_MS {
            hello_at = t;
            nonce = nonce_hex(&rng);
            let h = wire::hello(NAME, &nonce, "esp32s3", schema::HANDS, &body.model.schema_json());
            send(&mut serial, &h);
        }

        // drain the serial line into frames
        loop {
            match serial.read_byte() {
                Ok(b'\n') => {
                    let text = String::from(core::str::from_utf8(&line).unwrap_or("").trim());
                    line.clear();
                    if text.is_empty() {
                        continue;
                    }
                    match wire::parse(&text) {
                        FromSeat::Attest { pub_hex, sig_hex } => {
                            let Some(p) = &pair else {
                                send(&mut serial, &wire::refused("this body is not paired"));
                                continue;
                            };
                            if pub_hex != p.hex {
                                println!("[miniblur-a] TAMPER: a seat with a different key ({}..) tried to bind", &pub_hex[..pub_hex.len().min(8)]);
                                send(&mut serial, &wire::refused("that is not my entity"));
                                continue;
                            }
                            let sig = wire::hex_decode(&sig_hex).and_then(|v| <[u8; 64]>::try_from(v.as_slice()).ok());
                            let ok = match sig {
                                Some(s) => p.key.verify(&wire::challenge_bytes(&nonce, NAME), &Signature::from_bytes(&s)).is_ok(),
                                None => false,
                            };
                            if ok {
                                link = Link::Bound;
                                body.heard(t);
                                send(&mut serial, wire::ready());
                                println!("[miniblur-a] bound to {}..", &p.hex[..8]);
                            } else {
                                println!("[miniblur-a] TAMPER: bad signature over the challenge");
                                send(&mut serial, &wire::refused("signature did not verify"));
                            }
                        }
                        FromSeat::Cmd { id, verb, input } => {
                            if link != Link::Bound {
                                send(&mut serial, &wire::result(id, false, "not bound: attest first"));
                                continue;
                            }
                            // Domain 2 plays the plan over ticks (instant = false)
                            let (digest, ok) = body.handle(&verb, &input, t, false);
                            send(&mut serial, &wire::result(id, ok, &digest));
                        }
                        FromSeat::Ping => {
                            body.heard(t);
                            send(&mut serial, wire::pong());
                        }
                        FromSeat::Pong => body.heard(t),
                        FromSeat::Bye { .. } => {
                            link = Link::Hailing;
                            hello_at = 0;
                        }
                        FromSeat::Unknown(_) => {}
                    }
                }
                Ok(b) => {
                    if line.len() < LINE_MAX {
                        line.push(b);
                    } else {
                        line.clear(); // an oversize line is dropped whole
                    }
                }
                Err(nb::Error::WouldBlock) => break,
                Err(nb::Error::Other(_)) => break,
            }
        }

        // the reflex loop: plays the next gait sample when due, and stops
        // and stands by itself if the mind has gone quiet
        if let Some(msg) = body.tick(t) {
            println!("[miniblur-a] reflex: {msg}");
            if msg.starts_with("link lost") {
                link = Link::Hailing;
                hello_at = 0;
            }
        }
    }
}
