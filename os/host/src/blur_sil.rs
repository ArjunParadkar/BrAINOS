//! BLUR in the loop: the Autonomic layer and the entity against the
//! MuJoCo twin, over the real nerve formats.
//!
//! Three processes' worth of code, two processes:
//!
//! ```text
//!   blur/sim/nerves.py     MuJoCo + the Silicon layer (nodes, encoders, CAN timing, BMI270)
//!        ▲  NerveDown / NerveUp datagrams, one per millisecond (the Teensy's pin boundary)
//!        ▼
//!   SilPort                Autonomic (the Teensy loop, brainos-blur, no_std) + Firmware (its key)
//!        ▲  acquire::Port  (on the robot: the USB link to the Teensy)
//!        ▼
//!   Acquirer / Entity      §8, KIRA, the state graph
//! ```
//!
//! The `Port` trait is the swap point. `SilPort` runs the Teensy's code
//! in this process against simulated pins; on the robot the same
//! `Autonomic` is compiled for the Teensy and the seat talks to it over
//! USB through a port that does not exist yet, because the firmware
//! image it talks to does not exist yet. Nothing above `Port` changes.

use brainos_abi::version::ABI;
use brainos_blur::acquire::{Hello, Port};
use brainos_blur::autonomic::{Autonomic, Outcome, Request};
use brainos_blur::census::Census;
use brainos_blur::firmware::Firmware;
use brainos_blur::wire::{NerveDown, NerveUp};
use brainos_body::schema::CapabilitySchema;
use brainos_crypto::key::{Challenge, PublicKey, Signature};
use std::os::unix::net::UnixDatagram;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

pub fn repo_root() -> PathBuf {
    // os/host → os → repo
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().unwrap().parent().unwrap().to_path_buf()
}

pub fn python() -> PathBuf {
    std::env::var_os("BLUR_PY").map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(std::env::var("HOME").unwrap_or_default()).join("Desktop/Arenda_Projects/BLUR/blur_mujoco/.venv/bin/python")
    })
}

/// The twin, as a child process.
pub struct Twin {
    child: Child,
    pub sock: PathBuf,
}

#[derive(Clone, Debug, Default)]
pub struct TwinOptions {
    pub jig: bool,
    pub faults: Vec<String>,
    pub seed: u64,
    pub unit: u64,
    pub scene: String,
    pub record: Option<PathBuf>,
    pub latency_ms: Option<(f32, f32)>,
}

impl Twin {
    pub fn spawn(tag: &str, o: &TwinOptions) -> std::io::Result<Twin> {
        let dir = std::env::temp_dir().join("brainos-blur");
        std::fs::create_dir_all(&dir)?;
        let sock = dir.join(format!("nerves-{}-{}.sock", std::process::id(), tag));
        let _ = std::fs::remove_file(&sock);
        let mut c = Command::new(python());
        c.arg(repo_root().join("blur/sim/nerves.py"))
            .arg("--sock")
            .arg(&sock)
            .arg("--seed")
            .arg(o.seed.to_string())
            .arg("--unit")
            .arg(if o.unit == 0 { 36 } else { o.unit }.to_string())
            .arg("--scene")
            .arg(if o.scene.is_empty() { "flat" } else { &o.scene });
        if o.jig {
            c.arg("--jig");
        }
        for f in &o.faults {
            c.arg("--fault").arg(f);
        }
        if let Some(r) = &o.record {
            c.arg("--record").arg(r);
        }
        if let Some((lo, hi)) = o.latency_ms {
            c.arg("--latency").arg(lo.to_string()).arg(hi.to_string());
        }
        c.env("OMP_NUM_THREADS", "1").env("MUJOCO_GL", "egl").stdout(Stdio::null()).stderr(Stdio::inherit());
        let child = c.spawn()?;
        let t0 = Instant::now();
        // start-up is slow on a loaded machine: BLUR_TWIN_START_S overrides the 60 s
        let wait = std::env::var("BLUR_TWIN_START_S").ok().and_then(|v| v.parse().ok()).unwrap_or(60);
        while !sock.exists() {
            if t0.elapsed() > Duration::from_secs(wait) {
                return Err(std::io::Error::other("twin did not open its socket"));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        Ok(Twin { child, sock })
    }
}

impl Drop for Twin {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.sock);
    }
}

/// The Teensy's pins, in software.
pub struct Nerves {
    sock: UnixDatagram,
    peer: PathBuf,
    own: PathBuf,
    buf: Vec<u8>,
}

impl Nerves {
    pub fn connect(peer: &Path) -> std::io::Result<Nerves> {
        // one path per connection: two sockets bound to one path means the
        // twin's replies go to whichever bound last
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let k = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let own = peer.with_extension(format!("teensy.{}.{k}", std::process::id()));
        let _ = std::fs::remove_file(&own);
        let sock = UnixDatagram::bind(&own)?;
        sock.set_read_timeout(Some(Duration::from_secs(10)))?;
        Ok(Nerves {
            sock,
            peer: peer.to_path_buf(),
            own,
            buf: vec![0u8; 65536],
        })
    }

    pub fn exchange(&mut self, down: &NerveDown) -> std::io::Result<NerveUp> {
        let mut out = vec![0u8; NerveDown::MAX_BYTES];
        let n = down.write(&mut out);
        self.sock.send_to(&out[..n], &self.peer)?;
        let n = self.sock.recv(&mut self.buf)?;
        NerveUp::read(&self.buf[..n]).ok_or_else(|| std::io::Error::other("malformed NerveUp"))
    }

    /// The test harness's hand: not a nerve, and not available to the
    /// Autonomic layer.
    pub fn control(&mut self, json: &str) -> std::io::Result<String> {
        let mut m = b"C".to_vec();
        m.extend_from_slice(json.as_bytes());
        self.sock.send_to(&m, &self.peer)?;
        let n = self.sock.recv(&mut self.buf)?;
        Ok(String::from_utf8_lossy(&self.buf[1..n]).into_owned())
    }
}

impl Drop for Nerves {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.own);
    }
}

/// The Teensy, running in this process against simulated pins.
pub struct SilPort {
    pub nerves: Nerves,
    pub autonomic: Autonomic,
    pub firmware: Firmware,
    down: NerveDown,
    pub ticks: u64,
    pub timeout_ticks: u32,
    pub last_up: Option<NerveUp>,
    pub io_error: Option<String>,
}

impl SilPort {
    pub fn new(nerves: Nerves, firmware: Firmware) -> SilPort {
        SilPort {
            nerves,
            autonomic: Autonomic::new(),
            firmware,
            down: NerveDown::new(0),
            ticks: 0,
            timeout_ticks: 30_000,
            last_up: None,
            io_error: None,
        }
    }

    /// One millisecond of the Teensy's life.
    pub fn tick(&mut self) -> bool {
        let down = std::mem::replace(&mut self.down, NerveDown::new(0));
        match self.nerves.exchange(&down) {
            Ok(up) => {
                let mut next = NerveDown::new(up.tick);
                self.autonomic.step(&up, &mut next);
                self.down = next;
                self.last_up = Some(up);
                self.ticks += 1;
                true
            }
            Err(e) => {
                self.io_error = Some(e.to_string());
                false
            }
        }
    }

    pub fn run_ticks(&mut self, n: u32) -> bool {
        (0..n).all(|_| self.tick())
    }

    /// One millisecond with the Teensy dead: nothing goes down the wire
    /// and the Autonomic does not run (a test hook; the gate's "Teensy
    /// dies" case).
    pub fn tick_silent(&mut self) -> bool {
        self.tick_with(NerveDown::new(0))
    }

    /// One millisecond in which the Teensy sends exactly `down` and the
    /// Autonomic does not run (a test hook; the gate's rogue-Teensy case).
    pub fn tick_with(&mut self, down: NerveDown) -> bool {
        self.down = NerveDown::new(0);
        match self.nerves.exchange(&down) {
            Ok(up) => {
                self.last_up = Some(up);
                self.ticks += 1;
                true
            }
            Err(e) => {
                self.io_error = Some(e.to_string());
                false
            }
        }
    }
}

impl Port for SilPort {
    fn hello(&mut self) -> Option<Hello> {
        if !self.run_ticks(20) {
            return None;
        }
        let up = self.last_up.as_ref()?;
        Some(Hello {
            device: self.firmware.public(),
            abi: ABI,
            rail_on: up.rail_on,
            estop_latched: up.estop_latched,
        })
    }
    fn answer(&mut self, c: &Challenge) -> Signature {
        self.firmware.answer(c)
    }
    fn offer(&mut self, entity: &PublicKey, sig: &Signature, transcript: &[u8; 32]) -> bool {
        self.firmware.accepts(entity, sig, transcript)
    }
    fn census(&mut self, transcript: &[u8; 32]) -> Option<(Census, Signature)> {
        match self.run(Request::Census)? {
            Outcome::Census(c) => {
                let s = self.firmware.sign_census(transcript, &c);
                Some((c, s))
            }
            _ => None,
        }
    }
    fn schema(&mut self) -> CapabilitySchema {
        self.firmware.schema.clone()
    }
    fn run(&mut self, r: Request) -> Option<Outcome> {
        if !self.autonomic.request(r) {
            return None;
        }
        for _ in 0..self.timeout_ticks {
            if !self.tick() {
                return None;
            }
            if let Some(o) = self.autonomic.take() {
                return Some(o);
            }
        }
        None
    }
}
