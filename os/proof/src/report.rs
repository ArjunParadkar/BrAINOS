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
    /// the second domain: the two-envelope property on the body side
    pub envelopes: Option<crate::envelopes::EnvelopeReport>,
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
            envelopes: None,
        }
    }

    pub fn with_envelopes(mut self, e: crate::envelopes::EnvelopeReport) -> Report {
        if !e.holds() {
            self.verdict = Verdict::Violated;
        }
        self.envelopes = Some(e);
        self
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

        if let Some(e) = &self.envelopes {
            s.push_str("Second domain — the two envelopes\n");
            s.push_str("---------------------------------\n");
            s.push_str("  The property the body path rests on: a body never applies a\n");
            s.push_str("  command outside the intersection of what the seat granted and what\n");
            s.push_str("  its own mechanism permits. Run against the real BodyRuntime, the\n");
            s.push_str("  real Hexapod, and the real miniblur-core geometry.\n\n");
            push_field(&mut s, "cases enumerated", e.cases as u64);
            push_field(&mut s, "accepted by the body", e.accepted as u64);
            push_field(&mut s, "distinct refusals produced", e.distinct_refusals as u64);
            s.push_str("  verdict: ");
            if e.holds() {
                s.push_str("holds\n");
            } else if e.failures.is_empty() {
                s.push_str("VACUOUS — the domain did not exercise the body\n");
            } else {
                s.push_str("VIOLATED\n");
                for (i, why) in &e.failures {
                    s.push_str("    case ");
                    push_u64(&mut s, *i as u64);
                    s.push_str(": ");
                    s.push_str(why);
                    s.push('\n');
                }
            }
            s.push('\n');
            for line in [
                "  Checked, for every one of those cases:",
                "    - nothing is applied outside the intersection",
                "    - the intersection is never wider than either input",
                "    - a refused command moves nothing",
                "    - an uncalibrated body never moves",
                "    - a stopped body never moves",
                "    - a body whose geometry would tip never walks",
                "    - an independently written oracle agrees, case by case",
            ] {
                s.push_str(line);
                s.push('\n');
            }
            s.push('\n');
        }

        s.push_str("What this is\n------------\n");
        for line in [
            "  Bounded exhaustive model checking. Every case in the domains above was",
            "  run through the real code — not a model of it — and every property was",
            "  asserted over every resulting trace. Within those domains the properties",
            "  hold for all inputs, not for the ones a test author thought of.",
        ] {
            s.push_str(line);
            s.push('\n');
        }
        s.push('\n');

        s.push_str("What this is not\n----------------\n");
        for line in [
            "  Not a machine-checked proof. The following are outside what this harness",
            "  can establish, and remain so until the Stage 6 work dated 2028-29:",
            "",
            "    - that the properties hold outside the bounded domains",
            "    - that a body policy's simulate() is a sound forward model of a",
            "      physical system; that is a claim about physics, and no amount of",
            "      enumeration touches it",
            "    - that the ed25519 implementation, the allocator, or the compiler",
            "      are correct",
            "",
            "  One property is stronger than anything here and needs no harness:",
            "  Granted has a private field of a private type, so no code outside",
            "  brainos-kira can construct one. The evidence is that this crate cannot",
            "  write the counterexample.",
        ] {
            s.push_str(line);
            s.push('\n');
        }
        s
    }

    /// Everything, both domains.
    pub fn full() -> Report {
        crate::harness::verify().with_envelopes(crate::envelopes::verify_envelopes())
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
