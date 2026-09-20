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
    /// the third: every decoder against hostile input
    pub hostile: Option<crate::hostile::HostileReport>,
    /// the fourth: someone trying to get away with it
    pub adversary: Option<crate::adversary::AdversaryReport>,
    /// the fifth: losing power at every possible instant
    pub durability: Option<crate::durability::DurabilityReport>,
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
            hostile: None,
            adversary: None,
            durability: None,
        }
    }

    pub fn with_envelopes(mut self, e: crate::envelopes::EnvelopeReport) -> Report {
        if !e.holds() {
            self.verdict = Verdict::Violated;
        }
        self.envelopes = Some(e);
        self
    }

    pub fn with_hostile(mut self, h: crate::hostile::HostileReport) -> Report {
        if !h.holds() {
            self.verdict = Verdict::Violated;
        }
        self.hostile = Some(h);
        self
    }

    pub fn with_adversary(mut self, a: crate::adversary::AdversaryReport) -> Report {
        if !a.holds() {
            self.verdict = Verdict::Violated;
        }
        self.adversary = Some(a);
        self
    }

    pub fn with_durability(mut self, d: crate::durability::DurabilityReport) -> Report {
        if !d.holds() {
            self.verdict = Verdict::Violated;
        }
        self.durability = Some(d);
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

        if let Some(h) = &self.hostile {
            s.push_str("Third domain — hostile input\n");
            s.push_str("----------------------------\n");
            s.push_str("  Every decoder in the workspace, fed random bytes and mutated-valid\n");
            s.push_str("  bytes. No decoder may panic; anything that decodes must re-encode to\n");
            s.push_str("  exactly its own bytes; and a format carrying an integrity check must\n");
            s.push_str("  catch every mutation.\n\n");
            push_field(&mut s, "inputs fed", h.total_inputs as u64);
            push_field(&mut s, "decoders", h.targets.len() as u64);
            s.push('\n');
            s.push_str("    decoder                        tried  accepted  non-canonical\n");
            for t in &h.targets {
                s.push_str("    ");
                let mut name = alloc::string::String::from(t.name);
                while name.len() < 28 {
                    name.push(' ');
                }
                s.push_str(&name);
                pad_u64(&mut s, t.tried as u64, 7);
                pad_u64(&mut s, t.accepted as u64, 10);
                pad_u64(&mut s, t.not_canonical as u64, 15);
                if t.integrity_protected {
                    s.push_str("   (integrity-checked: no mutation may pass)");
                }
                s.push('\n');
            }
            s.push_str("\n  verdict: ");
            s.push_str(if h.holds() { "holds\n\n" } else { "VIOLATED\n\n" });
        }

        if let Some(a) = &self.adversary {
            s.push_str("Fourth domain — the adversary\n");
            s.push_str("-----------------------------\n");
            s.push_str("  The first three domains ask whether the system does the right\n");
            s.push_str("  thing. This one asks whether it can be made to do the wrong thing\n");
            s.push_str("  on purpose. Every attack runs against the real types: a forged\n");
            s.push_str("  signature is a real ed25519 signature by the wrong key, an edited\n");
            s.push_str("  audit log is a real log with an entry taken out.\n\n");
            push_field(&mut s, "attacks attempted", a.results.len() as u64);
            push_field(&mut s, "succeeded", a.breaches().len() as u64);
            push_field(
                &mut s,
                "stopped somewhere other than expected",
                a.misfires().len() as u64,
            );
            s.push('\n');
            for r in &a.results {
                s.push_str(match &r.ending {
                    crate::adversary::Ending::Repelled => "  [ok]   ",
                    crate::adversary::Ending::RepelledWrongly { .. } => "  [?]    ",
                    crate::adversary::Ending::Breached(_) => "  [BREACH]",
                });
                let mut id = alloc::string::String::from(r.id);
                while id.len() < 5 {
                    id.push(' ');
                }
                s.push_str(&id);
                s.push_str(r.goal);
                s.push('\n');
                s.push_str("          holding: ");
                s.push_str(r.holds);
                s.push_str("\n          stopped by: ");
                s.push_str(&r.stopped_by);
                s.push('\n');
                if let crate::adversary::Ending::RepelledWrongly { expected, got } = &r.ending {
                    s.push_str("          EXPECTED ");
                    s.push_str(expected);
                    s.push_str(", GOT ");
                    s.push_str(got);
                    s.push('\n');
                }
            }
            s.push('\n');
            for line in [
                "  What this domain cannot tell you: it is a list, and a list is",
                "  written by someone. It says these attacks fail. It does not say no",
                "  attack succeeds, and nothing enumerable ever could. Its value is that",
                "  the list is long, adversarial in construction, and runs on every",
                "  commit — so the day one of them starts passing, a test goes red",
                "  instead of a customer finding out.",
                "",
            ] {
                s.push_str(line);
                s.push('\n');
            }
        }

        if let Some(d) = &self.durability {
            s.push_str("Fifth domain — losing power at every possible instant\n");
            s.push_str("-----------------------------------------------------\n");
            s.push_str("  Robots lose power. Not as an edge case — as a normal Tuesday. For a\n");
            s.push_str("  journal write of n bytes there are n+1 moments it can stop, and every\n");
            s.push_str("  one is enumerated, on three media models: an unwritten tail that reads\n");
            s.push_str("  zero, one that still holds the slot's previous contents, and one that\n");
            s.push_str("  is garbage. The second is the nastiest, because the tail is\n");
            s.push_str("  valid-looking data sitting immediately after a valid header.\n\n");
            push_field(&mut s, "power-loss instants enumerated", d.instants as u64);
            push_field(&mut s, "newest write survived", d.kept_newest as u64);
            push_field(&mut s, "fell back to the previous good state", d.fell_back as u64);
            push_field(
                &mut s,
                "newborn: first write tore, nothing to come back to",
                d.started_fresh as u64,
            );
            push_field(
                &mut s,
                "lost memory that had been committed",
                d.lost_everything as u64,
            );
            s.push('\n');
            for line in [
                "  The property is not \"the newest write survives\" — it cannot be, and a",
                "  system claiming it would be lying. It is weaker and more useful:",
                "  whatever comes back was actually committed, in full, and is never older",
                "  than the last write that completed. An entity may lose the last thing it",
                "  was doing. It may not come back as a mixture of two entities, and it may",
                "  not come back as an earlier version of itself.",
                "",
            ] {
                s.push_str(line);
                s.push('\n');
            }
            s.push_str("  verdict: ");
            if !d.holds() {
                s.push_str("VIOLATED\n");
                for vln in d.violations.iter().take(8) {
                    s.push_str("    ");
                    s.push_str(vln.medium.name());
                    s.push_str(", torn at byte ");
                    push_u64(&mut s, vln.torn_at as u64);
                    s.push_str(": ");
                    s.push_str(&vln.why);
                    s.push('\n');
                }
            } else if !d.is_meaningful() {
                s.push_str("VACUOUS — the tears are not where the data is\n");
            } else {
                s.push_str("holds\n");
            }
            s.push('\n');
        }

        s.push_str("What this is\n------------\n");
        for line in [
            "  Bounded exhaustive model checking on the first two domains,",
            "  randomised plus mutation fuzzing on the third, directed attack on the",
            "  fourth, and exhaustive enumeration of every instant power can be lost",
            "  on the fifth. Every case in the",
            "  first two was run through the real code, not a model of it, and every",
            "  property was asserted over every resulting trace. Within those",
            "  domains the properties hold for all inputs, not for the ones a test",
            "  author thought of.",
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

    /// Everything: all three domains.
    pub fn full() -> Report {
        crate::harness::verify()
            .with_envelopes(crate::envelopes::verify_envelopes())
            .with_hostile(crate::hostile::verify_hostile(2_000))
            .with_adversary(crate::adversary::verify_adversary())
            .with_durability(crate::durability::verify_durability(4))
    }
}

fn pad_u64(s: &mut String, v: u64, width: usize) {
    let mut n = String::new();
    push_u64(&mut n, v);
    while n.len() < width {
        n.insert(0, ' ');
    }
    s.push_str(&n);
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
