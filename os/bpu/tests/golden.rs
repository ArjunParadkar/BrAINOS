//! BFS  LEVEL    Domain 4 substrate - the BPU seam (descriptor, mock, nerve, planner)  [crate brainos-bpu]
//! BFS  PURPOSE  Reproduces the bpusim reference's outputs bit for bit: the FPGA gate's tile-read vectors, the noise generator, and the write path
//! BFS  DEFINES  -
//! BFS  USES     brainos: bpu | external: std
//! BFS  USED BY  (entry point or test)
//!
//! Golden vectors: the mock against the reference, bit for bit.
//!
//! The files under `tests/golden/` were produced by
//! `tools/gen_golden.py` from `bpusim/fixed.py` — the integer contract
//! the KV260 FPGA tile was checked against. `REFERENCE.txt` records
//! which commit. Nothing here is approximate: every comparison is
//! integer equality.
//!
//! * `tile_read.txt` — the FPGA gate's own generator, seeds 0..47, plus
//!   every contract corner case: 69 vectors, 42 one-pass and 27
//!   two-pass, five noise levels, masked tiles, ADC rails.
//! * `prng.txt` — two consecutive noise blocks for five seeds and dials.
//! * `write_path.txt` — each write-path function on 400-device arrays.
//! * `program_target.txt` — five whole write-verify runs.

use brainos_bpu::contract::*;
use brainos_bpu::prng::{eta_from_word, EtaPrng, Xoshiro128ss};
use std::collections::HashMap;

fn golden(name: &str) -> String {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden").join(name);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn nums(rest: &str) -> Vec<i64> {
    rest.split_whitespace().map(|x| x.parse().unwrap()).collect()
}

/// Split a file into records, each starting at a line with `head`.
fn records(text: &str, head: &str) -> Vec<Vec<(String, String)>> {
    let mut out: Vec<Vec<(String, String)>> = Vec::new();
    for line in text.lines() {
        let (k, v) = line.split_once(' ').unwrap_or((line, ""));
        if k == head {
            out.push(Vec::new());
        }
        out.last_mut().expect("a record header first").push((k.to_string(), v.to_string()));
    }
    out
}

fn field<'a>(r: &'a [(String, String)], k: &str) -> &'a str {
    &r.iter().find(|(key, _)| key == k).unwrap_or_else(|| panic!("no field {k}")).1
}

#[test]
fn the_reference_commit_is_recorded() {
    let r = golden("REFERENCE.txt");
    assert!(r.starts_with("bpusim "), "{r}");
    assert!(!r.contains("modified"), "vectors were generated from an uncommitted contract");
}

#[test]
fn every_fpga_gate_vector_reads_to_the_same_bits_one_pass_and_two() {
    let recs = records(&golden("tile_read.txt"), "V");
    assert_eq!(recs.len(), 69);
    let (mut one, mut two, mut clipped_some, mut masked_some) = (0, 0, 0, 0);
    for (i, r) in recs.iter().enumerate() {
        let head = nums(field(r, "V"));
        let (passes, shift) = (head[0] as u32, head[1] as u32);
        let wp: Vec<u8> = nums(field(r, "wp")).iter().map(|v| *v as u8).collect();
        let wm: Vec<u8> = nums(field(r, "wm")).iter().map(|v| *v as u8).collect();
        let mask: Vec<bool> = nums(field(r, "mask")).iter().map(|v| *v != 0).collect();
        let x: Vec<i32> = nums(field(r, "x")).iter().map(|v| *v as i32).collect();
        let eta: Vec<i16> = nums(field(r, "eta")).iter().map(|v| *v as i16).collect();
        let got = tile_read(32, 32, &wp, &wm, &mask, &x, &eta, shift, passes);
        let want_y: Vec<i8> = nums(field(r, "y")).iter().map(|v| *v as i8).collect();
        assert_eq!(got.acc, nums(field(r, "Y")), "vector {i}: column sums");
        assert_eq!(got.y, want_y, "vector {i}: ADC codes");
        assert_eq!(got.clipped as i64, nums(field(r, "clipped"))[0], "vector {i}: clip count");
        assert_eq!(got.beyond_40_bits, 0, "vector {i}: a 32-row tile left the 40-bit range");
        if passes == 1 { one += 1 } else { two += 1 }
        clipped_some += (got.clipped > 0) as u32;
        masked_some += mask.iter().any(|m| !m) as u32;
    }
    assert_eq!((one, two), (42, 27));
    assert!(clipped_some > 0 && masked_some > 0, "the vectors exercise neither rails nor masks");
}

#[test]
fn the_noise_generator_produces_the_reference_stream() {
    let recs = records(&golden("prng.txt"), "P");
    assert_eq!(recs.len(), 5);
    for r in &recs {
        let head: Vec<u64> = field(r, "P").split_whitespace().map(|x| x.parse().unwrap()).collect();
        let mut p = EtaPrng::new(head[0], head[1] as i64, 32);
        for block in ["b0", "b1"] {
            let want: Vec<i16> = nums(field(r, block)).iter().map(|v| *v as i16).collect();
            assert_eq!(p.read_block(32), want, "seed {} {block}", head[0]);
        }
    }
}

#[test]
fn the_same_seed_gives_the_same_stream_and_a_different_seed_does_not() {
    let a = EtaPrng::new(9, 819, 32).read_block(32);
    let b = EtaPrng::new(9, 819, 32).read_block(32);
    let c = EtaPrng::new(10, 819, 32).read_block(32);
    assert_eq!(a, b);
    assert_ne!(a, c);
    // dial at zero is silence
    assert!(EtaPrng::new(9, 0, 32).read_block(32).iter().all(|e| *e == 0));
}

#[test]
fn each_write_path_function_matches_the_reference() {
    let text = golden("write_path.txt");
    let mut single: HashMap<String, Vec<i64>> = HashMap::new();
    let mut shifts: Vec<(u32, Vec<i64>)> = Vec::new();
    let mut rounds: Vec<Vec<i64>> = Vec::new();
    for line in text.lines() {
        let (k, v) = line.split_once(' ').unwrap();
        match k {
            "rne_shift" => {
                let (s, rest) = v.split_once(' ').unwrap();
                shifts.push((s.parse().unwrap(), nums(rest)));
            }
            "c2c_round" => rounds.push(nums(v)),
            _ => {
                single.insert(k.to_string(), nums(v));
            }
        }
    }
    let g = |k: &str| single.get(k).unwrap_or_else(|| panic!("no {k}")).clone();
    let (ws, gain, c2c, tgt) = (g("ws"), g("gain"), g("c2c"), g("tgt"));
    assert_eq!(ws.len(), 400);

    assert_eq!(ws.iter().map(|w| ws_to_wq(*w)).collect::<Vec<_>>(), g("ws_to_wq"));
    let step = |dir: i64| -> Vec<i64> {
        (0..400).map(|i| step_q(ws[i], gain[i], c2c[i], dir)).collect()
    };
    assert_eq!(step(1), g("step_up"));
    assert_eq!(step(-1), g("step_dn"));
    assert_eq!(
        (0..400).map(|i| pulses_for(ws[i], tgt[i])).collect::<Vec<_>>(),
        g("pulses_for")
    );
    assert_eq!(g("gsum").iter().map(|s| w_meas_from_sum(*s)).collect::<Vec<_>>(), g("w_meas"));

    let xs = g("rne_x");
    assert_eq!(shifts.len(), 5);
    for (s, want) in &shifts {
        assert_eq!(&xs.iter().map(|x| rne_shift(*x, *s)).collect::<Vec<_>>(), want, "shift {s}");
    }

    // pulses, with a fresh c2c sample per device per round
    let dead: Vec<bool> = g("dead").iter().map(|d| *d != 0).collect();
    assert_eq!(rounds.len() as i64, g("c2c_rounds")[0]);
    let mut it = rounds.into_iter();
    let out = pulse(&ws, &g("n"), &gain, |_| it.next().expect("more rounds than the reference"), &dead);
    assert!(it.next().is_none(), "fewer rounds than the reference");
    assert_eq!(out, g("pulse"));
}

/// The harness the generator documents: one xoshiro generator for read
/// noise, eight reads summed, devices in index order, no c2c.
fn run_program(seed: u64, sigma_q: i64, w0: &[i64], tgt: &[i64], gain: &[i64], dead: &[bool]) -> Programmed {
    let mut g = Xoshiro128ss::seeded(seed, 0);
    let read = |w: &[i64]| {
        let mut s = vec![0i64; w.len()];
        for _ in 0..VERIFY_READS {
            for (i, wi) in w.iter().enumerate() {
                s[i] += verify_current(ws_to_wq(*wi), eta_from_word(g.next(), sigma_q));
            }
        }
        s
    };
    program_target(w0, tgt, gain, read, |n| vec![0; n], dead)
}

#[test]
fn whole_write_verify_runs_match_the_reference_state_for_state() {
    let recs = records(&golden("program_target.txt"), "W");
    assert_eq!(recs.len(), 5);
    for r in &recs {
        let head: Vec<u64> = field(r, "W").split_whitespace().map(|x| x.parse().unwrap()).collect();
        let dead: Vec<bool> = nums(field(r, "dead")).iter().map(|d| *d != 0).collect();
        let got = run_program(
            head[0],
            head[1] as i64,
            &nums(field(r, "w0")),
            &nums(field(r, "tgt")),
            &nums(field(r, "gain")),
            &dead,
        );
        assert_eq!(got.w_s, nums(field(r, "w")), "seed {}: final states", head[0]);
        assert_eq!(got.iterations as i64, nums(field(r, "iters"))[0], "seed {}: iterations", head[0]);
        assert_eq!(got.pulses as i64, nums(field(r, "total"))[0], "seed {}: pulses", head[0]);
    }
}

#[test]
fn write_verify_lands_inside_the_band_with_no_noise_and_charges_for_every_pulse() {
    let n = 64;
    let tgt: Vec<i64> = (0..n).map(|i| (i as i64 * 500) % (WS_ONE + 1)).collect();
    let got = run_program(1, 0, &vec![0; n], &tgt, &vec![GAIN_ONE; n], &vec![false; n]);
    assert!(got.settled);
    assert!(got.pulses > 0);
    for i in 0..n {
        // measured through the 8-bit read side, as the device would be
        let seen = w_meas_from_sum(VERIFY_READS * verify_current(ws_to_wq(got.w_s[i]), 0));
        assert!((tgt[i] - seen).abs() <= TOL_Q.max(900), "device {i}: {} vs {}", tgt[i], seen);
    }
}

#[test]
fn a_dead_device_is_never_pulsed_and_does_not_stop_the_rest_converging() {
    let n = 8;
    let mut dead = vec![false; n];
    dead[3] = true;
    let got = run_program(2, 0, &vec![0; n], &vec![20_000; n], &vec![GAIN_ONE; n], &dead);
    assert_eq!(got.w_s[3], 0, "a dead device changed state");
    assert!(got.settled, "one dead device kept the whole run from finishing");
}

#[test]
fn the_noise_dial_endpoints_are_exact_integers() {
    assert_eq!(sigma_code_from_milli(5), 82); // 0.005
    assert_eq!(sigma_code_from_milli(20), 328); // 0.02, the contract value
    assert_eq!(sigma_code_from_milli(200), 3277); // 0.2
    assert_eq!(sigma_code_from_milli(0), 0);
}

#[test]
fn a_256_row_tile_fits_40_bits_without_noise_and_can_leave_them_with_it() {
    // the worst column: every plus device at full conductance, every
    // minus device at zero, every row at the largest two-pass code
    let (rows, cols) = (256, 4);
    let wp = vec![255u8; rows * cols];
    let wm = vec![0u8; rows * cols];
    let mask = vec![true; rows * cols];
    let x = vec![32767i32; rows];
    let lim = 1i64 << (ACC_BITS - 1);

    // no noise: exact, and inside by about one percent
    let quiet = tile_read(rows, cols, &wp, &wm, &mask, &x, &vec![0i16; rows * cols * 2], 31, 2);
    assert_eq!(quiet.acc[0], (GMAX_Q - GMIN_Q) * 32767 * rows as i64);
    assert_eq!(quiet.beyond_40_bits, 0);
    assert!(quiet.acc[0] > lim * 98 / 100, "the margin is wider than documented");

    // the largest sample the generator can produce at the top of the
    // dial (sigma = 0.2), with the sign that makes it worst
    let top = (0..=u32::MAX >> 22)
        .map(|k| eta_from_word(k << 22, sigma_code_from_milli(200)))
        .max()
        .unwrap();
    assert!((10_000..=11_000).contains(&top), "{top}");
    let mut eta = vec![0i16; rows * cols * 2];
    for pair in eta.chunks_mut(2) {
        pair[0] = top as i16;
        pair[1] = -(top as i16);
    }
    let noisy = tile_read(rows, cols, &wp, &wm, &mask, &x, &eta, 31, 2);
    assert_eq!(noisy.beyond_40_bits, cols as u32, "{:?}", noisy.acc);
    assert!(noisy.acc[0] > lim, "and the 64-bit sum is still exact, not wrapped");

    // the same worst case on the reference's 32 rows stays inside
    let rows = 32;
    let mut eta = vec![0i16; rows * cols * 2];
    for pair in eta.chunks_mut(2) {
        pair[0] = top as i16;
        pair[1] = -(top as i16);
    }
    let r = tile_read(rows, cols, &vec![255; rows * cols], &vec![0; rows * cols],
                      &vec![true; rows * cols], &vec![32767; rows], &eta, 31, 2);
    assert_eq!(r.beyond_40_bits, 0);
}
