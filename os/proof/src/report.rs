//! The report.
//!
//! A machine-generated statement of what was checked and what held. It
//! exists so that "Stage 6, partially delivered" is a claim with an
//! artifact behind it rather than a status in a document — and so that
//! the *limits* travel with the result. Every report states its domain
//! size and what it did not check.

use crate::properties::PropertyResult;
use alloc::string::String;
use alloc::vec::Vec;

/// What the harness covered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Coverage {
    /// how many cases were enumerated
    pub cases: usize,
    /// how many distinct refusal stages were actually exercised
    pub stages_exercised: usize,
    /// how many distinct deny codes were actually produced
    pub codes_exercised: usize,
    /// how many cases ended in a grant
    pub grants: usize,
}

impl Coverage {
    /// Did the domain actually exercise the pipeline, or did everything
    /// refuse in the same place?
    ///
    /// A harness that enumerates ten thousand cases which all die at
    /// authenticate has checked one thing ten thousand times. This is
    /// the number that says whether the domain is doing its job.
    pub fn is_meaningful(&self) -> bool {
        self.stages_exercised >= 5 && self.grants > 0 && self.codes_exercised >= 6
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Verdict {
    /// every property held over the whole domain
    Holds,
    /// at least one property failed
    Violated,
    /// the properties held but the domain did not exercise the pipeline
    Vacuous,
}

#[derive(Clone, Debug)]
pub struct Report {
    pub coverage: Coverage,
    pub results: Vec<PropertyResult>,
    pub verdict: Verdict,
}

impl Report {
    pub fn new(coverage: Coverage, results: Vec<PropertyResult>) -> Report {
        let all_hold = results.iter().all(|r| r.holds());
        let verdict = if !all_hold {
            Verdict::Violated
        } else if !coverage.is_meaningful() {
            Verdict::Vacuous
        } else {
            Verdict::Holds
        };
        Report {
            coverage,
            results,
            verdict,
        }
    }

    pub fn holds(&self) -> bool {
        self.verdict == Verdict::Holds
    }

    pub fn failures(&self) -> Vec<&PropertyResult> {
        self.results.iter().filter(|r| !r.holds()).collect()
    }

    /// The written report, including what was not checked.
    ///
    /// The limits section is not boilerplate. A report that stated only
    /// what held would be read as a proof, and it is not one.
    pub fn render(&self) -> String {
        let mut s = String::new();
        s.push_str("BrAInOS — KIRA property report\n");
        s.push_str("==============================\n\n");

        s.push_str("Verdict: ");
        s.push_str(match self.verdict {
            Verdict::Holds => "every property holds over the whole bounded domain",
            Verdict::Violated => "VIOLATED",
            Verdict::Vacuous => {
                "properties hold, but the domain did not exercise the pipeline — treat as no result"
            }
        });
        s.push_str("\n\n");

        s.push_str("Domain\n------\n");
        push_field(&mut s, "cases enumerated", self.coverage.cases as u64);
        push_field(&mut s, "ended in a grant", self.coverage.grants as u64);
        push_field(
            &mut s,
            "distinct refusal stages exercised",
            self.coverage.stages_exercised as u64,
        );
        push_field(
            &mut s,
            "distinct deny codes produced",
            self.coverage.codes_exercised as u64,
        );
        s.push('\n');

        s.push_str("Properties\n----------\n");
        for (p, r) in crate::properties::PROPERTIES.iter().zip(&self.results) {
            s.push_str(if r.holds() { "  [ok]   " } else { "  [FAIL] " });
            s.push_str(p.id);
            s.push_str("  ");
            s.push_str(p.statement);
            s.push('\n');
            if !r.holds() {
                for (i, why) in &r.failures {
                    s.push_str("          case ");
                    push_u64(&mut s, *i as u64);
                    s.push_str(": ");
                    s.push_str(why);
                    s.push('\n');
                }
            }
        }
        s.push('\n');

        s.push_str("What this is\n------------\n");
        s.push_str(
            "  Bounded exhaustive model checking. Every case in the domain above was run\n\
             through the real gate — not a model of it — and every property was asserted\n\
             over every resulting trace. Within that domain the properties hold for all\n\
             inputs, not for the ones a test author thought of.\n\n",
        );

        s.push_str("What this is not\n----------------\n");
        s.push_str(
            "  Not a machine-checked proof. The following are outside what this harness\n\
             can establish, and remain so until the Stage 6 work dated 2028-29:\n\n\
               - that the properties hold outside the bounded domain\n\
               - that a body policy's simulate() is a sound forward model of a physical\n\
                 system; that is a claim about physics, and no amount of enumeration\n\
                 touches it\n\
               - that the ed25519 implementation, the allocator, or the compiler are\n\
                 correct\n\n\
             One property is stronger than anything here and needs no harness: Granted\n\
             has a private field of a private type, so no code outside brainos-kira can\n\
             construct one. The evidence is that this crate cannot write the\n\
             counterexample.\n",
        );
        s
    }
}

fn push_field(s: &mut String, label: &str, value: u64) {
    s.push_str("  ");
    s.push_str(label);
    s.push_str(": ");
    push_u64(s, value);
    s.push('\n');
}

fn push_u64(s: &mut String, mut v: u64) {
    if v == 0 {
        s.push('0');
        return;
    }
    let mut buf = [0u8; 20];
    let mut n = 0;
    while v > 0 {
        buf[n] = b'0' + (v % 10) as u8;
        v /= 10;
        n += 1;
    }
    while n > 0 {
        n -= 1;
        s.push(buf[n] as char);
    }
}
