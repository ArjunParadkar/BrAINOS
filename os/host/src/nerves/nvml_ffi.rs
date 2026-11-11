//! BFS  LEVEL    Host - the only crate with an operating system (CLI, scenarios, OS nerves)  [crate brainos-host]
//! BFS  PURPOSE  The only unsafe code for NVML: loads libnvidia-ml at run time with dlopen and wraps the dozen C calls the GPU nerve needs
//! BFS  DEFINES  THRESHOLD_SHUTDOWN, THRESHOLD_SLOWDOWN, Nvml, Gpu
//! BFS  USES     external: std
//! BFS  USED BY  host/nerves/nvml
//!
//! NVML, loaded at run time. **This file is where the `unsafe` is.**
//!
//! Hardware forces it: NVIDIA publishes GPU telemetry through a C
//! library (`libnvidia-ml.so.1`) and nowhere else. Everything unsafe
//! about talking to it is in this file and nothing unsafe leaves it —
//! `nvml.rs` sees only safe functions returning `Option`.
//!
//! No crate is added for this. `dlopen` and `dlsym` are in the C
//! library every Linux Rust program already links; they are declared
//! here by hand.
//!
//! The library is looked up when `Nvml::load` is called, not at link
//! time, so a machine with no NVIDIA driver runs the same binary and
//! `load` returns `None`.
//!
//! Signatures are from NVIDIA's `nvml.h`. Every call returns
//! `nvmlReturn_t` (an `int`, 0 = success) and writes its result
//! through an out-pointer. Each wrapper checks the return code and
//! gives `None` for anything else, including "not supported on this
//! GPU", which consumer cards return for ECC counters.

#![allow(unsafe_code)]

use std::ffi::{c_char, c_int, c_uint, c_ulonglong, c_void, CStr, CString};

extern "C" {
    fn dlopen(filename: *const c_char, flag: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}
const RTLD_NOW: c_int = 2;

type Dev = *mut c_void;

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Utilization {
    gpu: c_uint,
    memory: c_uint,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Memory {
    total: c_ulonglong,
    free: c_ulonglong,
    used: c_ulonglong,
}

/// `nvmlTemperatureThresholds_t`
pub const THRESHOLD_SHUTDOWN: c_int = 0;
pub const THRESHOLD_SLOWDOWN: c_int = 1;

/// The loaded library and the functions taken from it.
pub struct Nvml {
    shutdown: unsafe extern "C" fn() -> c_int,
    count: unsafe extern "C" fn(*mut c_uint) -> c_int,
    by_index: unsafe extern "C" fn(c_uint, *mut Dev) -> c_int,
    name: unsafe extern "C" fn(Dev, *mut c_char, c_uint) -> c_int,
    uuid: unsafe extern "C" fn(Dev, *mut c_char, c_uint) -> c_int,
    temperature: unsafe extern "C" fn(Dev, c_int, *mut c_uint) -> c_int,
    threshold: unsafe extern "C" fn(Dev, c_int, *mut c_uint) -> c_int,
    utilization: unsafe extern "C" fn(Dev, *mut Utilization) -> c_int,
    memory: unsafe extern "C" fn(Dev, *mut Memory) -> c_int,
    power: unsafe extern "C" fn(Dev, *mut c_uint) -> c_int,
    ecc: unsafe extern "C" fn(Dev, c_int, c_int, *mut c_ulonglong) -> c_int,
}

/// One GPU. The pointer is NVML's and is valid until `nvmlShutdown`,
/// which `Nvml` calls only when it is dropped; a `Gpu` borrows nothing
/// but is only ever used through the `Nvml` that produced it.
#[derive(Clone, Copy)]
pub struct Gpu(Dev);

impl Nvml {
    /// Load the library and initialise it. `None` if there is no
    /// NVIDIA driver, or it will not initialise (no GPU, driver
    /// mismatch).
    pub fn load() -> Option<Nvml> {
        // SAFETY: dlopen with a NUL-terminated name; a null return is
        // handled. Each dlsym result is checked for null before being
        // transmuted to the function type nvml.h declares for that
        // symbol. nvmlInit_v2 takes no arguments.
        unsafe {
            let lib = CString::new("libnvidia-ml.so.1").ok()?;
            let h = dlopen(lib.as_ptr(), RTLD_NOW);
            if h.is_null() {
                return None;
            }
            let sym = |n: &str| -> Option<*mut c_void> {
                let c = CString::new(n).ok()?;
                let p = dlsym(h, c.as_ptr());
                (!p.is_null()).then_some(p)
            };
            let init: unsafe extern "C" fn() -> c_int = std::mem::transmute(sym("nvmlInit_v2")?);
            let me = Nvml {
                shutdown: std::mem::transmute(sym("nvmlShutdown")?),
                count: std::mem::transmute(sym("nvmlDeviceGetCount_v2")?),
                by_index: std::mem::transmute(sym("nvmlDeviceGetHandleByIndex_v2")?),
                name: std::mem::transmute(sym("nvmlDeviceGetName")?),
                uuid: std::mem::transmute(sym("nvmlDeviceGetUUID")?),
                temperature: std::mem::transmute(sym("nvmlDeviceGetTemperature")?),
                threshold: std::mem::transmute(sym("nvmlDeviceGetTemperatureThreshold")?),
                utilization: std::mem::transmute(sym("nvmlDeviceGetUtilizationRates")?),
                memory: std::mem::transmute(sym("nvmlDeviceGetMemoryInfo")?),
                power: std::mem::transmute(sym("nvmlDeviceGetPowerUsage")?),
                ecc: std::mem::transmute(sym("nvmlDeviceGetTotalEccErrors")?),
            };
            if init() != 0 {
                return None;
            }
            Some(me)
        }
    }

    pub fn gpus(&self) -> Vec<Gpu> {
        let mut n: c_uint = 0;
        // SAFETY: out-pointer to a live local.
        if unsafe { (self.count)(&mut n) } != 0 {
            return Vec::new();
        }
        (0..n)
            .filter_map(|i| {
                let mut d: Dev = std::ptr::null_mut();
                // SAFETY: out-pointer to a live local; index < count.
                (unsafe { (self.by_index)(i, &mut d) } == 0 && !d.is_null()).then_some(Gpu(d))
            })
            .collect()
    }

    fn string(&self, f: unsafe extern "C" fn(Dev, *mut c_char, c_uint) -> c_int, g: Gpu) -> Option<String> {
        let mut buf = [0 as c_char; 96];
        // SAFETY: the buffer is 96 bytes and its length is passed; NVML
        // NUL-terminates within it on success. The last byte is forced
        // to NUL before reading in case it did not.
        unsafe {
            if f(g.0, buf.as_mut_ptr(), buf.len() as c_uint) != 0 {
                return None;
            }
            buf[buf.len() - 1] = 0;
            Some(CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned())
        }
    }

    pub fn name(&self, g: Gpu) -> Option<String> {
        self.string(self.name, g)
    }

    pub fn uuid(&self, g: Gpu) -> Option<String> {
        self.string(self.uuid, g)
    }

    /// Degrees Celsius at the GPU's own sensor.
    pub fn temperature_c(&self, g: Gpu) -> Option<f64> {
        let mut t: c_uint = 0;
        // SAFETY: out-pointer to a live local; sensor 0 is NVML_TEMPERATURE_GPU.
        (unsafe { (self.temperature)(g.0, 0, &mut t) } == 0).then_some(t as f64)
    }

    /// One of the GPU's own temperature limits, degrees Celsius.
    pub fn threshold_c(&self, g: Gpu, which: c_int) -> Option<f64> {
        let mut t: c_uint = 0;
        // SAFETY: out-pointer to a live local.
        (unsafe { (self.threshold)(g.0, which, &mut t) } == 0).then_some(t as f64)
    }

    /// (GPU utilisation, memory-controller utilisation), each 0..1.
    pub fn utilization(&self, g: Gpu) -> Option<(f64, f64)> {
        let mut u = Utilization::default();
        // SAFETY: out-pointer to a live repr(C) struct of the declared layout.
        (unsafe { (self.utilization)(g.0, &mut u) } == 0)
            .then_some((u.gpu as f64 / 100.0, u.memory as f64 / 100.0))
    }

    /// (used, total) bytes.
    pub fn memory(&self, g: Gpu) -> Option<(f64, f64)> {
        let mut m = Memory::default();
        // SAFETY: out-pointer to a live repr(C) struct of the declared layout.
        (unsafe { (self.memory)(g.0, &mut m) } == 0).then_some((m.used as f64, m.total as f64))
    }

    /// Watts.
    pub fn power_w(&self, g: Gpu) -> Option<f64> {
        let mut mw: c_uint = 0;
        // SAFETY: out-pointer to a live local.
        (unsafe { (self.power)(g.0, &mut mw) } == 0).then_some(mw as f64 / 1000.0)
    }

    /// Uncorrected ECC errors over the GPU's lifetime. `None` on cards
    /// without ECC memory.
    pub fn ecc_uncorrected(&self, g: Gpu) -> Option<f64> {
        let mut n: c_ulonglong = 0;
        // SAFETY: out-pointer to a live local. 1 = uncorrected, 1 = aggregate.
        (unsafe { (self.ecc)(g.0, 1, 1, &mut n) } == 0).then_some(n as f64)
    }
}

impl Drop for Nvml {
    fn drop(&mut self) {
        // SAFETY: nvmlShutdown takes no arguments and pairs with the
        // successful nvmlInit_v2 in `load`.
        unsafe {
            (self.shutdown)();
        }
    }
}
