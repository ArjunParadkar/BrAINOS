//! BFS  LEVEL    Domain 4 substrate - the BPU seam (descriptor, mock, nerve, planner)  [crate brainos-bpu]
//! BFS  PURPOSE  BpuDevice: the trait a real BPU driver will implement; every call that writes weights or changes the dial requires a Mandate
//! BFS  DEFINES  TileImage, ModelImage, BpuVitals, BpuFault, WriteReport, BpuDevice
//! BFS  USES     brainos: cognition, crypto, part
//! BFS  USED BY  bpu/mock, bpu/nerve, bpu/tests/seam
//!
//! The device seam.
//!
//! `BpuDevice` is what stands between BrAInOS and a BPU. Today the only
//! implementation is `mock::MockBpu`. When silicon exists, its PCIe/CXL
//! driver implements this trait and nothing above it changes — that is
//! the meaning of "seam".
//!
//! The trait is split by what a call can do:
//!
//! * **reading** (`descriptor`, `vitals`, `output`) needs nothing;
//! * **staging** (`stage_image`, `stage_input`) puts data where the
//!   device can see it and changes no device state. Model weights do
//!   not travel in a KIRA request — a request carries bounded scalars —
//!   so the data path and the authority path are separate, and the
//!   request names the staged image by its digest;
//! * **acting** (`load`, `write_verify`, `reverify`, `set_noise_dial`,
//!   `infer`) takes a `&Mandate`. A `Mandate` exists only as the result
//!   of verifying a token the gate minted, so there is no way to write
//!   a weight, even by calling the device directly, without one.
//!
//! The chip's own on-chip controller (which loads models, schedules
//! layers and runs write-verify) is the BPU's Domain 2. It is on the
//! far side of this trait, and it stays subordinate to KIRA for
//! exactly the calls that take a `Mandate`.

use alloc::vec::Vec;
use brainos_cognition::bpu::BpuDescriptor;
use brainos_part::nerve::Mandate;

/// The targets for one tile: a state code per device, and which
/// crossings exist.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TileImage {
    /// `rows * cols` target codes for the G+ devices, 0..=255
    pub w_plus: Vec<u8>,
    /// `rows * cols` target codes for the G- devices
    pub w_minus: Vec<u8>,
    /// `rows * cols` connectivity
    pub mask: Vec<bool>,
}

/// A model, as tiles to be written starting at `first_tile`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelImage {
    pub first_tile: u32,
    pub tiles: Vec<TileImage>,
}

impl ModelImage {
    /// 48 bits of the image's SHA-256, as a number a KIRA request can
    /// carry exactly. The load request names the image by this, and
    /// since the grant is over the request's digest, a granted load is
    /// a grant for *this* image and no other.
    pub fn digest48(&self) -> u64 {
        use brainos_crypto::domain::Tag;
        use brainos_crypto::sha256::Sha256;
        let mut h = Sha256::tagged(Tag::Package);
        h.field(b"bpu-image/v1");
        h.field(&self.first_tile.to_le_bytes());
        h.field(&(self.tiles.len() as u64).to_le_bytes());
        for t in &self.tiles {
            h.field(&t.w_plus);
            h.field(&t.w_minus);
            let m: Vec<u8> = t.mask.iter().map(|b| *b as u8).collect();
            h.field(&m);
        }
        let d = h.finish();
        let b = d.as_bytes();
        u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], 0, 0])
    }
}

/// What the device reports about its own health.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct BpuVitals {
    /// write pulses left before the endurance budget is gone
    pub pulses_left: u64,
    /// the worst tile's dead-device fraction, parts per million
    pub worst_dead_ppm: u32,
    /// the largest distance of any loaded live device from its target,
    /// as a fraction of full scale, at the last verify or drift. The
    /// contract's band is 0.02.
    pub drift: f64,
    pub temperature_c: f64,
    /// the noise dial, as sigma
    pub noise_sigma: f64,
    pub tiles_loaded: u32,
}

/// Why the device did not do what it was asked.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BpuFault {
    /// no image is staged, or it is not the one the mandate names
    WrongImage,
    /// a tile index is outside the device
    NoSuchTile,
    /// the image does not match the tile geometry
    BadGeometry,
    /// the write would take the budget below the reserve. The device
    /// checks this itself, whatever the gate concluded.
    BudgetExhausted,
    /// a tile in the region has more dead devices than tolerated
    TooManyDead,
    /// the dial setting is outside the device's range
    DialOutOfRange,
    /// the tile holds no model
    NotLoaded,
    /// a required argument is missing from the mandate
    BadArguments,
}

/// What a write did.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct WriteReport {
    pub tiles: u32,
    /// write pulses actually applied — what was debited
    pub pulses: u64,
    /// did write-verify reach a round with nothing left to correct?
    /// False does not mean the write failed: see `contract::Programmed`.
    pub settled: bool,
}

/// A BPU, as BrAInOS sees one.
pub trait BpuDevice {
    fn descriptor(&self) -> BpuDescriptor;
    fn vitals(&self) -> BpuVitals;
    /// The dead-device fraction of the worst tile in a range, ppm.
    fn dead_ppm(&self, first_tile: u32, tiles: u32) -> Option<u32>;

    /// Put a model where the device can load it. Changes no weights.
    fn stage_image(&mut self, image: ModelImage);
    /// Put an activation vector where the device can read it.
    fn stage_input(&mut self, x: Vec<i32>);
    /// The ADC codes of the last inference.
    fn output(&self) -> &[i8];

    /// Write the staged image. Mandate arguments: `image` (its
    /// `digest48`), `first_tile`, `tiles`.
    fn load(&mut self, auth: &Mandate) -> Result<WriteReport, BpuFault>;
    /// Rewrite one tile from the staged image. Arguments: `image`, `tile`.
    fn write_verify(&mut self, auth: &Mandate) -> Result<WriteReport, BpuFault>;
    /// Re-verify one loaded tile against the targets it already holds,
    /// correcting drift. Argument: `tile`.
    fn reverify(&mut self, auth: &Mandate) -> Result<WriteReport, BpuFault>;
    /// Set the read-noise dial. Argument: `milli` (thousandths of sigma).
    fn set_noise_dial(&mut self, auth: &Mandate) -> Result<(), BpuFault>;
    /// Run one tile on the staged input. Arguments: `tile`, `passes`,
    /// `shift`.
    fn infer(&mut self, auth: &Mandate) -> Result<(), BpuFault>;
}
