//! After something goes wrong.
//!
//! A robot works a shift in a workshop it shares with people. At the end
//! of the day somebody says: *your machine damaged my equipment at
//! around two o'clock.* Three parties now want different things. The
//! owner wants to know what actually happened. The insurer wants
//! evidence that is not simply the vendor's word for it. And the vendor
//! — us — would very much like the answer not to depend on being
//! believed.
//!
//! §4 makes KIRA's eighth stage audit, and says it always runs: a
//! refusal is recorded as carefully as a grant. On its own that is worth
//! very little. An audit log held by the party it might incriminate is a
//! diary. What makes it evidence is that somebody who assumes we are
//! lying can check it.
//!
//! So this module does the whole thing. A shift is worked, an incident
//! window is named, a bundle is sealed, and then the bundle is verified
//! *by a party holding nothing but the bundle and thirty-two bytes of
//! public key* — no log, no entity, no us. Then four attempts to lie
//! with it, each of which fails in a different way.
//!
//! The finding worth stating in advance, because it is the part people
//! do not expect: **the refusals are the evidence.** What exonerates a
//! machine is not a record of it behaving well. It is a record of the
//! gate having refused, at a named stage, the specific thing it is being
//! accused of — recorded at the time, before anyone was accused of
//! anything, and provable afterwards.

use brainos_abi::ids::BodyId;
use brainos_abi::time::{Instant, Tick};
use brainos_abi::verb::Verb;
use brainos_adapters::native::mini_blur;
use brainos_body::calibration::{Calibration, JointTrim};
use brainos_body::hal::Telemetry;
use brainos_body::policy::Arg;
use brainos_crypto::key::BrainKey;
use brainos_entity::entity::Entity;
use brainos_kira::action::Request;
use brainos_kira::audit::Decision;
use brainos_kira::evidence::{Bundle, EvidenceError};
use brainos_kira::gate::Verdict;

fn v(s: &str) -> Verb {
    Verb::parse(s).expect("a well-formed verb")
}

fn calibration() -> Calibration {
    let mut c = Calibration::new(BodyId::ZERO, Tick(1));
    for ch in 0..10u8 {
        c.trim(JointTrim {
            channel: ch,
            offset_deg: 0.0,
            scale: 1.0,
            min_deg: -28.0,
            max_deg: 28.0,
            probed: true,
        })
        .expect("a sane trim");
    }
    c
}

fn fit(at: u64) -> Telemetry {
    Telemetry {
        at: Instant(at),
        link_alive: true,
        rail_v: Some(7.4),
        current_a: Some(0.5),
        tilt_deg: Some(1.0),
        hottest_c: Some(38.0),
        stops: 0,
        overruns: 0,
    }
}

/// What the reconstruction found.
#[derive(Clone, Debug)]
pub struct Incident {
    pub shift_decisions: usize,
    pub shift_granted: u64,
    pub shift_refused: u64,
    /// the window the complaint names
    pub from: Tick,
    pub to: Tick,
    pub in_window: usize,
    pub window_granted: usize,
    pub window_refused: usize,
    /// what the machine is accused of, and what the log says about it
    pub accusation: String,
    pub the_answer: Option<String>,
    /// a third party, holding only the bundle and the public key
    pub third_party_verdict: Result<(), EvidenceError>,
    /// four ways of lying with it, and how each one fails
    pub forgeries: Vec<(&'static str, String)>,
    /// the honest bundle has no holes
    pub honest_bundle_is_contiguous: bool,
    /// the curated one does
    pub curated_bundle_gaps: usize,
    pub bundle_text: String,
    pub bundle_bytes: usize,
}

pub fn reconstruct() -> Incident {
    let key = BrainKey::from_seed(&[0x1C; 32]);
    let device = BrainKey::from_seed(&[0x2C; 32]);
    let mut e = Entity::new(BrainKey::from_seed(&[0x1C; 32]), Tick(0));
    e.wake(None, None, Tick(1)).expect("a fresh entity wakes");
    let body = e
        .acquire(&device, mini_blur(), calibration(), 10)
        .expect("the hexapod joins");
    e.observe_body(&body, &fit(0));

    // --- the shift ---
    //
    // Ordinary work, with the speed varying the way a real shift does.
    // The schema caps `legs.walk` at 0.12 m/s; anything above that is
    // refused at validate, and those refusals are the ones that will
    // matter in a moment.
    let mut asked_fast = 0;
    let mut ticks: Vec<u64> = Vec::new();
    for i in 0..180u64 {
        e.tick(brainos_cognition::ambient::SenseFrame::quiet(Instant(i)));
        // the request carries the entity's own tick. Using a counter of
        // the loop's own instead made every single request arrive from
        // the future and die at authenticate — which the gate was right
        // about and the scenario was wrong about.
        let tick = e.tick;
        ticks.push(tick.0);
        let speed = match i % 9 {
            // somebody, or something, asks for a sprint
            4 => {
                asked_fast += 1;
                0.9
            }
            _ => 0.06 + (i % 3) as f64 * 0.02,
        };
        let r = Request::on_body(v("legs.walk"), body, tick)
            .arg("speed", Arg::Scalar(speed))
            .arg("heading", Arg::Scalar(0.0))
            .because(if speed > 0.5 {
                "asked to hurry"
            } else {
                "crossing the workshop"
            });
        let _ = e.propose(r);
    }

    let shift_decisions = e.kira.trail.len();
    let shift_granted = e.kira.trail.granted_count();
    let shift_refused = e.kira.trail.denied_count();

    // --- the complaint ---
    //
    // "Around two o'clock" is a range, not a moment, and the window is
    // deliberately generous: a bundle that covered only the tick the
    // complainant named would be the vendor choosing the evidence.
    let mid = ticks.len() / 2;
    let from = Tick(ticks[mid.saturating_sub(20)]);
    let to = Tick(ticks[(mid + 20).min(ticks.len() - 1)]);
    let accusation = String::from(
        "the machine was moving too fast across the workshop around this time \
         and struck equipment standing near its path",
    );

    let sealing_key = BrainKey::from_seed(&[0x1C; 32]);
    let bundle: Bundle = e.kira.trail.bundle(&sealing_key, from, to);
    let in_window = bundle.exhibits.len();
    let window_granted = bundle.granted();
    let window_refused = bundle.denied();

    // --- what the log actually says about the accusation ---
    //
    // Not "the robot behaved well". Something narrower and much more
    // useful: every request to exceed the schema's speed was refused, at
    // a named stage, at the time, before anyone had made a complaint.
    let fast_refusals: Vec<&brainos_kira::evidence::Exhibit> = bundle
        .exhibits
        .iter()
        .filter(|x| matches!(x.record.decision, Decision::Denied { .. }))
        .collect();
    let the_answer = fast_refusals.first().map(|x| {
        let d = x.record.denial.expect("a denial");
        format!(
            "{} of the {} decisions in the window were refusals — every one a request to \
             move faster than the body's schema allows, refused at {:?} with {:?}, \
             recorded at the time and before anyone had complained. The machine was asked \
             to hurry and the gate said no, in writing, {} times.",
            window_refused, in_window, d.stage, d.code, window_refused
        )
    });
    let _ = asked_fast;

    // --- the third party ---
    //
    // Holds the bundle and the entity's public key. Not the log, not the
    // entity, not us.
    let third_party_verdict = bundle.verify(&sealing_key.public());

    // --- four ways of lying with it ---
    let mut forgeries = Vec::new();

    // 1. edit a record to say something else
    {
        let mut tampered = bundle.clone();
        if let Some(x) = tampered.exhibits.first_mut() {
            x.record.reason = String::from("the human explicitly authorised this");
        }
        forgeries.push((
            "edit what a decision says it was for",
            describe(tampered.verify(&sealing_key.public())),
        ));
    }

    // 2. edit the bytes to match the edited record, so the two agree
    {
        let mut tampered = bundle.clone();
        if let Some(x) = tampered.exhibits.first_mut() {
            x.record.reason = String::from("the human explicitly authorised this");
            x.bytes = x.record.canonical();
        }
        forgeries.push((
            "edit the record and re-encode it so the two agree",
            describe(tampered.verify(&sealing_key.public())),
        ));
    }

    // 3. drop the inconvenient decisions and present the rest
    {
        let mut trimmed = bundle.clone();
        trimmed
            .exhibits
            .retain(|x| x.record.decision == Decision::Granted);
        let n = trimmed.exhibits.len();
        let gaps = trimmed.gaps();
        let verdict = trimmed.verify(&sealing_key.public());
        forgeries.push((
            "leave the refusals out and present only the grants",
            match verdict {
                Ok(()) => format!(
                    "verifies, and every exhibit in it is genuine — a bundle cannot be \
                     forged, but it can be curated, and nothing in the cryptography stops \
                     its producer choosing what to include. What does stop them getting \
                     away with it: the log is append-only and its indices are \
                     consecutive, so this bundle has {} visible holes in it ({}) across \
                     {n} exhibits. The insurer does not need to know what #{} says to ask \
                     for it",
                    gaps.len(),
                    gaps.iter()
                        .take(4)
                        .map(|g| format!("#{g}"))
                        .collect::<Vec<_>>()
                        .join(" "),
                    gaps.first().copied().unwrap_or(0)
                ),
                other => describe(other),
            },
        ));
    }

    // 4. present a genuine proof next to a different decision
    {
        let mut swapped = bundle.clone();
        if swapped.exhibits.len() >= 2 {
            let other = swapped.exhibits[1].record.clone();
            swapped.exhibits[0].record = other.clone();
            swapped.exhibits[0].bytes = other.canonical();
        }
        forgeries.push((
            "pair a real proof with a different decision",
            describe(swapped.verify(&sealing_key.public())),
        ));
    }

    let honest_bundle_is_contiguous = bundle.gaps().is_empty();
    let curated_bundle_gaps = {
        let mut trimmed = bundle.clone();
        trimmed
            .exhibits
            .retain(|x| x.record.decision == Decision::Granted);
        trimmed.gaps().len()
    };

    let bundle_text = bundle.render();
    let bundle_bytes = bundle
        .exhibits
        .iter()
        .map(|x| x.bytes.len() + x.proof.path_len() * 32 + 40)
        .sum::<usize>()
        + 128;

    Incident {
        shift_decisions,
        shift_granted,
        shift_refused,
        from,
        to,
        in_window,
        window_granted,
        window_refused,
        accusation,
        the_answer,
        third_party_verdict,
        forgeries,
        honest_bundle_is_contiguous,
        curated_bundle_gaps,
        bundle_text,
        bundle_bytes,
    }
}

fn describe(r: Result<(), EvidenceError>) -> String {
    match r {
        Ok(()) => String::from("VERIFIES — the forgery was accepted"),
        Err(e) => format!("refused: {e:?}"),
    }
}

impl Incident {
    pub fn is_accountable(&self) -> bool {
        self.third_party_verdict.is_ok()
            && self.in_window > 0
            && self.the_answer.is_some()
            // the first, second and fourth forgeries must be refused;
            // the third is expected to verify and to be visibly partial
            && self.honest_bundle_is_contiguous
            && self.curated_bundle_gaps > 0
            && self.forgeries.iter().enumerate().all(|(i, (_, how))| {
                if i == 2 {
                    how.contains("verifies")
                } else {
                    how.starts_with("refused")
                }
            })
    }
}

/// The narrated run.
pub fn narrate(i: &Incident) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    out.push((
        "the shift",
        format!(
            "{} gated decisions across the day: {} granted, {} refused. Stage 8 runs on \
             every one of them, refusals included",
            i.shift_decisions, i.shift_granted, i.shift_refused
        ),
    ));
    out.push((
        "complaint",
        format!("\"{}\" — said to be around tick {} to {}", i.accusation, i.from.0, i.to.0),
    ));
    out.push((
        "bundle",
        format!(
            "{} decisions sealed for that window ({} granted, {} refused), about {} bytes \
             — the records, an inclusion proof for each, and the root they recompute to, \
             signed by the entity's own Key",
            i.in_window, i.window_granted, i.window_refused, i.bundle_bytes
        ),
    ));
    out.push((
        "the answer",
        i.the_answer
            .clone()
            .unwrap_or_else(|| String::from("the log has nothing to say about this window")),
    ));
    out.push((
        "third party",
        format!(
            "verified by someone holding the bundle and 32 bytes of public key — no log, \
             no entity, no us: {}",
            match &i.third_party_verdict {
                Ok(()) => String::from("stands up"),
                Err(e) => format!("FAILED ({e:?})"),
            }
        ),
    ));
    for (what, how) in &i.forgeries {
        out.push(("forgery", format!("{what} — {how}")));
    }
    out
}
