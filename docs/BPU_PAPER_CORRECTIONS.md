# BPU paper: two corrections, with replacement text

**2026-11-12.** Found by building the BPU mock in `os/bpu` against the
integer contract (`bpusim/fixed.py`, commit `0af2b7aebd8c`). Neither is a
problem with the FPGA result: the FPGA tile is 32 × 32, where both
statements in the paper are true. They are problems with what the paper
says about the **256 × 256 tile**.

The paper's source is not on this machine (the PDF was built with XeTeX
elsewhere), so the paper itself has **not been edited**. This file is the
replacement text and the evidence. Page and section numbers are from
`Paradkar_BPU.pdf` built 2026-11-09.

Reproduce every number here:

```sh
cd os && cargo test --release -p brainos-bpu --test golden -- --nocapture
```

Copies checked: the two 9 November PDFs in `~/Downloads` contain both
passages. The 17 October paper and the MIT portfolio copies
(`22_bpu_technical_paper.pdf`, `BPU_Technical_Paper_2026-11.pdf`) do not
contain the "never overflow" sentence.

---

## 1. Section 4.1 and Table 2: the 40-bit column sum

**The paper says** (§4.1, p. 9): "the column sum is kept at 40 bits, which
is wide enough that it can never overflow, so no hidden rounding happens
before the ADC." Table 2: "Column sum — 40-bit, exact".

**What is true.** The largest column sum is
(largest noisy conductance) × (largest two-pass activation, 32,767) × rows.

| tile rows | read noise | worst column sum | 40-bit signed limit 2³⁹ = 5.50 × 10¹¹ |
|---|---|---|---|
| 32 | none | 6.79 × 10¹⁰ | fits, 8× margin |
| 32 | sample at its clamp | 1.37 × 10¹¹ | fits, 4× margin |
| 256 | none | 5.43 × 10¹¹ | fits, by **1.2 %** |
| 256 | σ = 0.2, largest sample (+0.658) | 9.08 × 10¹¹ | **does not fit** |
| 256 | sample at its clamp | 1.098 × 10¹² | **does not fit**; fits 41 bits (2⁴⁰ = 1.0995 × 10¹²) by 0.2 % |

So "can never overflow" is true of the tile that was built and false of
the tile the paper is about.

**Fix (pick one and state it):** widen the accumulator to **41 bits
signed**, which is exact for 256 rows at every noise sample the contract
can represent; or keep 40 bits and state a saturation rule.

**Replacement text for §4.1** (41-bit option):

> First, the column sum is wide enough that it can never overflow, so no
> hidden rounding happens before the ADC. The width depends on the number
> of rows: 40 bits is exact for the 32 × 32 tile used in the FPGA tests,
> and the 256 × 256 tile of the architecture needs 41, because 256 rows at
> full-scale drive and the largest read-noise sample reach 1.10 × 10¹²,
> just above the 40-bit limit of 5.5 × 10¹¹.

**Table 2, replacement row:**

> Column sum — 40-bit for 32 rows, 41-bit for 256 rows; exact; read out by
> an 8-bit SAR ADC

If the paper keeps 40 bits instead, the sentence must say what happens at
the limit (for example, "saturating at ±2³⁹"), say when that can happen —
which needs working out properly: the no-noise margin is 1.2 %, so it
takes near-full-scale activations on every row together with noise that
is high on most of them — and the reference contract and the RTL have to
implement it. The
contract currently says nothing: `docs/reference_contract.md` line 142
reads "widths chosen so nothing can overflow", which has the same scope
problem and should be corrected with the paper.

---

## 2. Section 4.2: the write-verify stopping rule

**The paper says** (§4.2): "It stops when the average lands in the band,
or gives up after 100 iterations."

That sentence describes one device, and for one device it is right. What
the contract implements for an *array* is different, and the paper does
not say so: every round, **every** device is measured again — including
the ones already in the band — and the procedure stops only when a whole
round finds nothing to correct.

**Why that matters.** Each measurement carries fresh read noise. Equation
6 puts the band at two standard deviations of the measurement, so a
device sitting exactly on target is still flagged about one time in
twenty at the limit. With thousands of devices, some are flagged every
round; each gets a corrective pulse it did not need; and the run goes to
the 100-round cap.

**Measured.** 2,048 devices written from empty, same tables, same noise
generator, same seed. "Per-device" means a device that lands in the band
is done and is not measured again.

| σ | rule | rounds | write pulses | reached a quiet round | worst true error |
|---|---|---|---|---|---|
| 0.005 | every device every round | 29 | 69,002 | yes | 0.025 |
| 0.005 | per-device | 4 | 68,335 | yes | 0.029 |
| 0.02 | every device every round | 100 | 73,273 | no | 0.042 |
| 0.02 | per-device | 6 | 68,455 | yes | 0.034 |
| 0.05 | every device every round | 100 | 148,816 | no | 0.049 |
| 0.05 | per-device | 10 | 69,639 | yes | 0.057 |
| 0.10 | every device every round | 100 | 297,480 | no | 0.074 |
| 0.10 | per-device | 16 | 73,106 | yes | 0.083 |

* At the contract's σ = 0.02 the extra wear is **7 %**.
* At σ = 0.05 — the noise §4.2 says the rule "allows" — it is **2.1×**.
* At σ = 0.10 it is **4.1×**.
* In every noisy case the procedure also runs all 100 rounds of verify
  reads where 6 to 16 would do.
* The devices end up written about equally well. The per-device rule's
  worst error is up to 0.01 of full scale higher, because it accepts a
  device on a single in-band measurement. That is the honest cost of the
  fix, and it is small next to the wear.

Endurance is the constraint the paper itself names as unproven. A
procedure that spends two to four times the pulses it needs is worth
fixing before silicon.

**Replacement text for §4.2**, after "…or gives up after 100 iterations.":

> For an array, the stopping rule has to be per device: a device whose
> average lands in the band is finished and is not measured again. The
> alternative — re-measuring every device each round and stopping only
> when none is outside the band — does not terminate at tile scale under
> read noise, because fresh noise on every measurement keeps flagging a
> few devices that are already well written. On 2,048 devices at σ = 0.05
> it ran to the 100-iteration limit and applied 2.1 times as many write
> pulses as the per-device rule (4.1 times at σ = 0.1), for the same
> final accuracy to within 0.01 of full scale.

**Two things to check before that paragraph goes in:**

1. **The contract and the RTL.** `bpusim/fixed.py::program_target`
   implements the every-device rule, and the FPGA firmware was verified
   bit-exact against it. If the paper says per-device, the contract has
   to change to match, and the write-verify gate has to be re-run. The
   proposal measured above is `os/bpu/src/contract.rs::
   program_target_per_device`; it is labelled as not part of the
   contract and has no golden vectors.
2. **Experiment B4.** §4.2 says B4 tests Equation 6 "on 1,296 cells". If
   a cell there is an array judged by "did the run stop before 100
   iterations", then some cells counted as not converging may be this
   effect rather than a failure of Equation 6. I have not read B4's
   code; this is a question, not a finding.

---

## What does not change

The FPGA results. The tile there is 32 × 32: the 40-bit sum cannot
overflow, and the write-verify gate compared the FPGA with the reference
bit for bit, which holds whatever the stopping rule costs. Both
corrections make the paper's claims about the 256 × 256 tile true, and
both were found by the rest of the stack running against the contract —
which is an argument for the contract, and worth a sentence in the paper.
