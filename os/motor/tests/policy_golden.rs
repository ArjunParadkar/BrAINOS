//! BFS  LEVEL    Domain 2 Autonomic - motor memory (self-model, mandates, primitives, reflexes)  [crate brainos-motor]
//! BFS  PURPOSE  Tests that the int8 policy runner gives the Python reference's integers bit for bit on recorded observations, and refuses malformed blobs
//! BFS  DEFINES  -
//! BFS  USES     brainos: motor
//! BFS  USED BY  (entry point or test)
//!
//! The golden files in `tests/golden/` come from `motor/gait/make_golden.py`
//! and `motor/gait/int8_ref.py`: a seeded, untrained 36-64-64-10 network,
//! 1,000 observations recorded from the generated hexapod, and the integer
//! actions the Python reference computes for them. `tests/golden/edge/`
//! holds crafted rows (rounding ties, NaN, infinities, huge values, -0.0,
//! a subnormal) through the same blob.
//!
//! Set `BRAINOS_POLICY_GOLDEN_DIR` to a directory in the same format
//! (`motor/gait/export_int8.py --golden-dir` writes one for a trained
//! policy) and `trained_policy_from_env` checks that one too. Unset, that
//! test checks nothing and says so.
//!
//!     cd os && cargo test --release -p brainos-motor --test policy_golden -- --nocapture

use brainos_motor::policy::{Int8Mlp, PolicyError, MAGIC};

const BLOB: &[u8] = include_bytes!("golden/blob.bin");
const OBS: &[u8] = include_bytes!("golden/obs.f32");
const OUT: &[u8] = include_bytes!("golden/out.i8");
const ACT_FNV: &[u8] = include_bytes!("golden/act.fnv");
const EDGE_OBS: &[u8] = include_bytes!("golden/edge/obs.f32");
const EDGE_OUT: &[u8] = include_bytes!("golden/edge/out.i8");
const EDGE_FNV: &[u8] = include_bytes!("golden/edge/act.fnv");

fn f32s(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
}

fn fnv1a64(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in data {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Runs every row; returns (rows whose integers all match, rows, whether
/// the float outputs hash to the reference's).
fn check(blob: &[u8], obs: &[u8], out: &[u8], fnv: &[u8]) -> (usize, usize, bool) {
    let p = Int8Mlp::from_bytes(blob).expect("golden blob parses");
    let (n_in, n_out) = (p.inputs(), p.outputs());
    let obs = f32s(obs);
    assert_eq!(obs.len() % n_in, 0, "obs file is not whole rows");
    let n = obs.len() / n_in;
    assert_eq!(out.len(), n * n_out, "out file does not match obs rows");
    let mut q = vec![0i32; n_out];
    let mut y = vec![0f32; n_out];
    let mut bytes = Vec::with_capacity(n * n_out * 4);
    let mut matched = 0;
    for r in 0..n {
        let x = &obs[r * n_in..(r + 1) * n_in];
        assert!(p.act_int(x, &mut q));
        assert!(p.act(x, &mut y));
        let want = &out[r * n_out..(r + 1) * n_out];
        if q.iter().zip(want).all(|(a, b)| *a == *b as i8 as i32) {
            matched += 1;
        } else if n - matched <= 3 {
            eprintln!("row {r}: rust {q:?}");
            eprintln!("row {r}: want {:?}", want.iter().map(|b| *b as i8).collect::<Vec<_>>());
        }
        for v in &y {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
    }
    let fnv_ok = fnv.len() == 8 && fnv1a64(&bytes).to_le_bytes() == fnv[..8];
    (matched, n, fnv_ok)
}

#[test]
fn recorded_observations_bit_exact() {
    let (m, n, fnv) = check(BLOB, OBS, OUT, ACT_FNV);
    eprintln!("golden: {m}/{n} rows bit-exact, float outputs hash {}", if fnv { "equal" } else { "DIFFERENT" });
    assert_eq!(n, 1000);
    assert_eq!(m, n);
    assert!(fnv, "act() float outputs differ from the reference's");
}

#[test]
fn crafted_edge_rows_bit_exact() {
    let (m, n, fnv) = check(BLOB, EDGE_OBS, EDGE_OUT, EDGE_FNV);
    eprintln!("edge: {m}/{n} rows bit-exact");
    assert_eq!(m, n);
    assert!(fnv);
}

#[test]
fn trained_policy_from_env() {
    let Ok(dir) = std::env::var("BRAINOS_POLICY_GOLDEN_DIR") else {
        eprintln!("BRAINOS_POLICY_GOLDEN_DIR not set: no trained policy checked");
        return;
    };
    let rd = |f: &str| std::fs::read(format!("{dir}/{f}")).unwrap_or_else(|e| panic!("{dir}/{f}: {e}"));
    let (m, n, fnv) = check(&rd("blob.bin"), &rd("obs.f32"), &rd("out.i8"), &rd("act.fnv"));
    eprintln!("{dir}: {m}/{n} rows bit-exact, float outputs hash {}", if fnv { "equal" } else { "DIFFERENT" });
    assert_eq!(m, n);
    assert!(fnv);
}

// ---------------------------------------------------------------- refusals

/// Byte offset of layer `k`'s header (rows u16, cols u16, mult i32, shift u8).
fn layer_at(blob: &[u8], k: usize) -> usize {
    let u16_at = |i: usize| u16::from_le_bytes([blob[i], blob[i + 1]]) as usize;
    let n_in = u16_at(8);
    let mut at = 12 + 4 * n_in;
    for _ in 0..k {
        let (rows, cols) = (u16_at(at), u16_at(at + 2));
        at += 9 + rows * cols + 4 * rows;
    }
    at
}

#[test]
fn the_golden_blob_is_what_the_doc_says() {
    assert_eq!(&BLOB[..8], MAGIC);
    let p = Int8Mlp::from_bytes(BLOB).unwrap();
    assert_eq!((p.inputs(), p.outputs()), (36, 10));
    // three layers end exactly 4 bytes (out_scale) before the end
    assert_eq!(layer_at(BLOB, 3) + 4, BLOB.len());
}

#[test]
fn bad_magic_is_refused() {
    for i in 0..8 {
        let mut b = BLOB.to_vec();
        b[i] ^= 0x01;
        assert_eq!(Int8Mlp::from_bytes(&b).unwrap_err(), PolicyError::BadMagic, "byte {i}");
    }
}

#[test]
fn every_truncation_is_refused() {
    for len in 0..BLOB.len() {
        let r = Int8Mlp::from_bytes(&BLOB[..len]);
        assert!(r.is_err(), "a blob cut to {len} bytes parsed");
        if len >= 8 {
            assert_eq!(r.unwrap_err(), PolicyError::Truncated, "cut to {len}");
        }
    }
}

#[test]
fn trailing_bytes_are_refused() {
    for extra in [&[0u8][..], &[0, 0, 0, 0], &BLOB[..16]] {
        let mut b = BLOB.to_vec();
        b.extend_from_slice(extra);
        assert_eq!(Int8Mlp::from_bytes(&b).unwrap_err(), PolicyError::TrailingBytes);
    }
}

#[test]
fn width_mismatch_is_refused() {
    for k in 0..3 {
        let at = layer_at(BLOB, k) + 2; // cols
        let mut b = BLOB.to_vec();
        let cols = u16::from_le_bytes([b[at], b[at + 1]]);
        b[at..at + 2].copy_from_slice(&(cols - 1).to_le_bytes());
        assert_eq!(Int8Mlp::from_bytes(&b).unwrap_err(), PolicyError::Mismatch, "layer {k}");
    }
}

#[test]
fn bad_shift_and_widths_are_refused() {
    let at = layer_at(BLOB, 1);
    for shift in [0u8, 41, 255] {
        let mut b = BLOB.to_vec();
        b[at + 8] = shift;
        assert_eq!(Int8Mlp::from_bytes(&b).unwrap_err(), PolicyError::BadShift, "shift {shift}");
    }
    let mut b = BLOB.to_vec();
    b[at..at + 2].copy_from_slice(&0u16.to_le_bytes()); // rows = 0
    assert_eq!(Int8Mlp::from_bytes(&b).unwrap_err(), PolicyError::TooWide);
    let mut b = BLOB.to_vec();
    b[at..at + 2].copy_from_slice(&257u16.to_le_bytes()); // rows > MAX_WIDTH
    assert_eq!(Int8Mlp::from_bytes(&b).unwrap_err(), PolicyError::TooWide);
    let mut b = BLOB.to_vec();
    b[8..10].copy_from_slice(&0u16.to_le_bytes()); // n_in = 0
    assert_eq!(Int8Mlp::from_bytes(&b).unwrap_err(), PolicyError::TooWide);
    let mut b = BLOB.to_vec();
    b[10..12].copy_from_slice(&0u16.to_le_bytes()); // no layers
    assert_eq!(Int8Mlp::from_bytes(&b).unwrap_err(), PolicyError::TooWide);
}

#[test]
fn wrong_lengths_are_refused_at_run_time() {
    let p = Int8Mlp::from_bytes(BLOB).unwrap();
    let x = vec![0f32; 36];
    assert!(!p.act_int(&x[..35], &mut [0i32; 10]));
    assert!(!p.act_int(&x, &mut [0i32; 9]));
    assert!(!p.act(&x, &mut [0f32; 11]));
}
