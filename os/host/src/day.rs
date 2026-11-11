//! BFS  LEVEL    Host - the only crate with an operating system (CLI, scenarios, OS nerves)  [crate brainos-host]
//! BFS  PURPOSE  Simulates a full working day of telemetry, faults and human speech to measure whether an entity interrupts only for what matters and stays cheap
//! BFS  DEFINES  Happening, Day, run, Body, run_on, narrate
//! BFS  USES     brainos: abi, adapters, body, cognition, crypto, entity, memory
//! BFS  USED BY  host/tests/day_suite
//!
//! A day in the life.
//!
//! §2.3: *"A sense → predict → error → attend loop replaces turn-based
//! interaction. The entity runs continuously and surfaces only what
//! needs a human."*
//!
//! Every other test in this workspace asks whether one thing works. This
//! one asks whether an entity can be *left running*. It simulates a
//! working day — tens of thousands of ticks — with a body reporting
//! telemetry the whole time, occasional genuine faults, a human who
//! speaks now and then, and quiet periods in which the entity
//! consolidates.
//!
//! The thing being measured is a balance that is easy to get wrong in
//! both directions:
//!
//! * An entity that surfaces everything is a notification firehose
//!   nobody keeps.
//! * An entity that surfaces nothing is a machine that quietly fails.
//!
//! So the numbers that matter are not "did it work" but *how often did
//! it interrupt*, *did it interrupt for the things that mattered*, and
//! *did it stay cheap while nothing was happening*. A day that ends with
//! the escalation budget exhausted is an entity that would have stopped
//! thinking by mid-afternoon.
//!
//! **The honest limit**, as with the compounding experiment: the day is
//! one I wrote. It can show the loop is stable, affordable and
//! attentive to the right things in a world with a known answer key. It
//! cannot show what a real workshop is like.

use brainos_abi::ids::BodyId;
use brainos_abi::time::{Instant, Tick};
use brainos_abi::verb::Verb;
use brainos_adapters::native::mini_blur;
use brainos_body::calibration::{Calibration, JointTrim};
use brainos_body::hal::Telemetry;
use brainos_cognition::ambient::{Intent, SenseFrame};
use brainos_crypto::key::BrainKey;
use brainos_crypto::rng::Drbg;
use brainos_entity::entity::Entity;
use brainos_memory::consolidate::Dream;
use brainos_memory::episode::Outcome;

/// Something that happens during the day, and whether the entity ought
/// to have noticed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Happening {
    /// nothing; the rail is steady and the body is level
    Nothing,
    /// the rail sags under a heavy motion. Normal, recoverable, and not
    /// worth interrupting a human for.
    RailDip,
    /// a leg stalls. Genuinely worth knowing about.
    LegStall,
    /// the body tips past its envelope and a reflex stops it. The one
    /// thing that must always reach the human.
    Stopped,
    /// the human says something. Always attended.
    HumanSpoke,
}

impl Happening {
    /// Should this have interrupted the human? The answer key.
    pub const fn deserves_interruption(self) -> bool {
        matches!(self, Happening::LegStall | Happening::Stopped)
    }

    pub const fn name(self) -> &'static str {
        match self {
            Happening::Nothing => "quiet",
            Happening::RailDip => "rail dip",
            Happening::LegStall => "leg stall",
            Happening::Stopped => "reflex stop",
            Happening::HumanSpoke => "the human spoke",
        }
    }
}

/// What the day looked like.
#[derive(Clone, Debug, PartialEq)]
pub struct Day {
    pub ticks: u64,
    /// ticks where nothing salient happened
    pub quiet: u64,
    /// times the entity escalated above Tier 0
    pub escalations: u64,
    /// times it surfaced something to the human at all
    pub surfaced: u64,
    /// times it interrupted — surfaced something salient enough to break
    /// into what the human was doing
    pub interrupted: u64,
    /// of the things that deserved attention, how many reached the human
    /// in some form
    pub caught: u64,
    pub deserved: u64,
    /// for each kind that deserved attention: how many occurrences
    /// passed before the first one reached the human. One is ideal.
    pub first_notice: Vec<(Happening, u64)>,
    /// of the windows in which a deserving kind occurred, in how many
    /// did it reach the human at least once
    pub windows_aware: u64,
    pub windows_with_a_fault: u64,
    /// of the interruptions raised, how many were for something that
    /// deserved one
    pub interrupts_deserved: u64,
    /// consolidation passes once the body was put away for the night
    pub dreams_when_idle: u64,
    /// times it surfaced something that did not deserve it
    pub false_alarms: u64,
    /// consolidation passes run in quiet periods
    pub dreams: u64,
    /// memories at the end
    pub memories: usize,
    /// of those, how many are about something that went wrong — a
    /// reading out of family, or a body stopping itself
    pub memories_about_faults: usize,
    /// and how many are the routine work
    pub memories_routine: usize,
    /// what the escalation budget had left at the end, as a percentage
    /// of its cap
    pub budget_left_pct: u8,
    /// times the budget refused an escalation the entity wanted
    pub budget_refusals: u64,
    /// the entity's surprise rate at the end — how well calibrated it is
    pub surprise_rate_pct: u8,
    /// the human interrupted this many times per thousand ticks
    pub interruptions_per_kilotick: u64,
    /// per kind: (occurrences, reached the human, interrupted)
    pub by_kind: Vec<(Happening, u64, u64, u64)>,
}

impl Day {
    /// Of what deserved the human's attention, how much reached them —
    /// whether by interrupting or by going in the pile they read later.
    ///
    /// Deliberately not "how often did it interrupt". A leg stalling
    /// sixty times in a day should be *known about*, once, prominently,
    /// and then counted. Measuring interruptions would reward the
    /// firehose.
    pub fn recall_pct(&self) -> u8 {
        if self.deserved == 0 {
            return 100;
        }
        ((self.caught * 100) / self.deserved) as u8
    }

    /// Of what it surfaced, how much deserved it?
    pub fn precision_pct(&self) -> u8 {
        if self.surfaced == 0 {
            return 100;
        }
        ((self.caught * 100) / self.surfaced) as u8
    }

    /// How many occurrences of the worst-noticed fault passed before the
    /// entity said anything. One is ideal.
    pub fn worst_first_notice(&self) -> u64 {
        self.first_notice
            .iter()
            .map(|(_, n)| *n)
            .max()
            .unwrap_or(0)
    }

    /// Of the windows in which something went wrong, in how many did the
    /// human hear about it?
    pub fn awareness_pct(&self) -> u8 {
        if self.windows_with_a_fault == 0 {
            return 100;
        }
        ((self.windows_aware * 100) / self.windows_with_a_fault) as u8
    }

    /// Of the interruptions raised, how many deserved one?
    pub fn interrupt_precision_pct(&self) -> u8 {
        if self.interrupted == 0 {
            return 100;
        }
        ((self.interrupts_deserved * 100) / self.interrupted) as u8
    }

    /// Could this entity be left running?
    ///
    /// Deliberately **not** "did every occurrence reach the human". A
    /// leg stalling fifty times in a day should be known about — once,
    /// promptly, and then counted. Measuring per-occurrence recall would
    /// score the firehose highest, which is the behaviour the damping
    /// exists to prevent.
    ///
    /// Five conditions:
    ///
    /// * it noticed each kind of fault within its first few occurrences
    /// * it stayed aware: in most windows where something went wrong,
    ///   the human heard about it
    /// * most of what it interrupted for deserved it
    /// * it did not become a firehose
    /// * it still had budget at the end. An entity that scores well on
    ///   the others by spending everything by lunchtime has not been
    ///   left running, it has been left switched off.
    pub fn could_be_left_running(&self) -> bool {
        self.worst_first_notice() > 0
            && self.worst_first_notice() <= 3
            && self.awareness_pct() >= 70
            && self.interrupt_precision_pct() >= 60
            && self.interruptions_per_kilotick <= 40
            && self.budget_left_pct >= 20
    }

    /// Of everything it surfaced, how much of it interrupted? A high
    /// number here with a high recall is an entity that has not learned
    /// the difference between "worth knowing" and "worth stopping you".
    pub fn interrupt_share_pct(&self) -> u8 {
        if self.surfaced == 0 {
            return 0;
        }
        ((self.interrupted * 100) / self.surfaced) as u8
    }
}

fn alloc_kinds() -> Vec<(Happening, u64, u64, u64)> {
    [
        Happening::Nothing,
        Happening::RailDip,
        Happening::LegStall,
        Happening::Stopped,
        Happening::HumanSpoke,
    ]
    .into_iter()
    .map(|k| (k, 0, 0, 0))
    .collect()
}

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

/// Decide what happens on one tick.
///
/// The mix is meant to be a working day rather than a stress test: the
/// overwhelming majority of ticks are nothing, faults are rare, and the
/// human speaks a few times an hour.
fn happening(rng: &mut Drbg) -> Happening {
    match rng.below(1_000) {
        0..=2 => Happening::LegStall,
        3..=4 => Happening::Stopped,
        5..=29 => Happening::RailDip,
        30..=39 => Happening::HumanSpoke,
        _ => Happening::Nothing,
    }
}

/// Run a day.
pub fn run(ticks: u64) -> Day {
    run_on(ticks, Body::Workstation)
}

/// Which body the entity is thinking in.
///
/// The same day, on two very different budgets. §5 says the bound is a
/// property of the body rather than of who the entity is, and that is
/// easy to write and worth checking: an entity that notices what
/// matters with four thousand episodes of room and misses it with two
/// hundred and fifty-six is an entity whose attention was paid for by
/// its memory, and Phase 0 runs on the small one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Body {
    /// default bounds — a laptop, a workstation, a robot with a real
    /// computer in it
    Workstation,
    /// what actually fits in an ESP32-S3's SRAM: 256 episodes, a
    /// 32-record audit window, and leaves handed off every couple of
    /// minutes
    Board,
}

impl Body {
    pub const fn name(self) -> &'static str {
        match self {
            Body::Workstation => "workstation bounds",
            Body::Board => "ESP32-S3 bounds",
        }
    }
}

pub fn run_on(ticks: u64, body_kind: Body) -> Day {
    let mut rng = Drbg::deterministic(b"a working day");
    let mut e = Entity::new(BrainKey::from_seed(&[0x5A; 32]), Tick(0));
    if body_kind == Body::Board {
        e.for_embedded_body();
    }
    e.wake(None, None, Tick(1)).expect("a fresh entity wakes");

    let body = e
        .acquire(
            &BrainKey::from_seed(&[0xB2; 32]),
            mini_blur(),
            calibration(),
            10,
        )
        .expect("mini-blur joins");
    e.observe_body(
        &body,
        &Telemetry {
            at: Instant(0),
            link_alive: true,
            rail_v: Some(7.4),
            current_a: Some(0.4),
            tilt_deg: Some(1.0),
            hottest_c: Some(38.0),
            stops: 0,
            overruns: 0,
        },
    );
    e.learn_skill(v("legs.walk"), "walk");

    let budget_cap = e.ambient.budget.available();
    let mut day = Day {
        ticks,
        quiet: 0,
        escalations: 0,
        surfaced: 0,
        interrupted: 0,
        caught: 0,
        deserved: 0,
        false_alarms: 0,
        dreams: 0,
        memories: 0,
        memories_about_faults: 0,
        memories_routine: 0,
        budget_left_pct: 0,
        budget_refusals: 0,
        surprise_rate_pct: 0,
        interruptions_per_kilotick: 0,
        by_kind: alloc_kinds(),
        first_notice: Vec::new(),
        windows_aware: 0,
        windows_with_a_fault: 0,
        interrupts_deserved: 0,
        dreams_when_idle: 0,
    };

    // per-window awareness: 2,000 ticks, about twenty seconds
    const WINDOW: u64 = 2_000;
    let mut window_had_fault = false;
    let mut window_was_aware = false;
    // occurrences of each deserving kind before it was first noticed
    let mut unnoticed: Vec<(Happening, u64)> = vec![
        (Happening::LegStall, 0),
        (Happening::Stopped, 0),
    ];
    let mut noticed: Vec<Happening> = Vec::new();

    for i in 0..ticks {
        let at = Instant(i * 10_000); // a tick every 10 ms
        let what = happening(&mut rng);
        if what.deserves_interruption() {
            day.deserved += 1;
        }

        // --- what the bodies delivered ---
        let mut frame = SenseFrame::quiet(at);
        frame.ceiling = brainos_cognition::resource::Tier::Frontier;

        let (rail, tilt, current) = match what {
            Happening::RailDip => (6.9, 1.0, 1.9),
            Happening::LegStall => (7.3, 1.0, 2.8),
            Happening::Stopped => (7.2, 26.0, 0.9),
            _ => (
                7.4 + (rng.below(20) as f32 - 10.0) * 0.002,
                1.0 + (rng.below(20) as f32 - 10.0) * 0.02,
                0.42 + (rng.below(20) as f32 - 10.0) * 0.004,
            ),
        };
        frame = frame
            .from_body(body, "rail.volts", rail)
            .from_body(body, "tilt", tilt)
            .from_body(body, "leg.current", current);

        if what == Happening::HumanSpoke {
            frame = frame.saying("how is the workshop looking");
        }
        if what == Happening::Stopped {
            frame.stops.push((body, "stability-lost"));
        }

        // --- one pass ---
        let before_refusals = e.ambient.budget.refused;
        let intent = e.tick(frame);
        day.budget_refusals += e.ambient.budget.refused - before_refusals;

        let mut interrupted = false;
        let reached_the_human = match &intent {
            Intent::Idle { .. } => {
                day.quiet += 1;
                false
            }
            Intent::Think { escalation, .. } => {
                if escalation.happened() {
                    day.escalations += 1;
                }
                false
            }
            Intent::Surface { items } => {
                day.surfaced += 1;
                interrupted = items.iter().any(|i| i.salience.should_interrupt());
                true
            }
            Intent::Attend { escalation, .. } => {
                if escalation.happened() {
                    day.escalations += 1;
                }
                // a stopped body always reaches the human
                day.surfaced += 1;
                interrupted = true;
                true
            }
        };
        if interrupted {
            day.interrupted += 1;
            if what.deserves_interruption() {
                day.interrupts_deserved += 1;
            }
        }

        if what.deserves_interruption() {
            window_had_fault = true;
            if reached_the_human {
                window_was_aware = true;
                if !noticed.contains(&what) {
                    noticed.push(what);
                    let before = unnoticed
                        .iter()
                        .find(|(k, _)| *k == what)
                        .map(|(_, n)| *n)
                        .unwrap_or(0);
                    day.first_notice.push((what, before + 1));
                }
            } else if !noticed.contains(&what) {
                if let Some(e) = unnoticed.iter_mut().find(|(k, _)| *k == what) {
                    e.1 += 1;
                }
            }
        }

        if (i + 1) % WINDOW == 0 {
            if window_had_fault {
                day.windows_with_a_fault += 1;
                if window_was_aware {
                    day.windows_aware += 1;
                }
            }
            window_had_fault = false;
            window_was_aware = false;
        }

        if let Some(entry) = day.by_kind.iter_mut().find(|(k, _, _, _)| *k == what) {
            entry.1 += 1;
            if reached_the_human {
                entry.2 += 1;
            }
            if interrupted {
                entry.3 += 1;
            }
        }

        if reached_the_human {
            if what.deserves_interruption() {
                day.caught += 1;
            } else {
                day.false_alarms += 1;
            }
        }

        // --- acting, occasionally ---
        if what == Happening::Nothing && i % 97 == 0 {
            let out = e.attempt(v("legs.walk"), body, &[("speed", 0.08), ("heading", 0.0)]);
            if out.verdict.is_granted() {
                e.outcome(
                    v("legs.walk"),
                    Some(body),
                    Outcome::Succeeded,
                    "forward",
                    &["workshop", "carpet"],
                );
            }
        }

        // --- consolidating while nothing is happening ---
        if e.ambient.should_consolidate(200) && e.dream(Dream::light()).is_some() {
            day.dreams += 1;
        }
    }

    // --- the body is put away for the night ---
    //
    // A busy workshop never gives an entity two hundred consecutive
    // quiet ticks, so consolidation does not run during the day. That is
    // not a scheduling accident: rewriting memory while a gait loop is
    // running is the kind of thing that is fine in testing and
    // catastrophic once, and the quiet check is what enforces it. The
    // dreaming happens when the robot is switched off, which is where
    // the architecture's word for it came from.
    for night in 0..1_000u64 {
        let at = Instant((ticks + night) * 10_000);
        e.tick(SenseFrame::quiet(at).from_body(body, "rail.volts", 7.4));
        if e.ambient.should_consolidate(200) && e.dream(Dream::deep()).is_some() {
            day.dreams_when_idle += 1;
        }
    }

    day.memories = e.graph.len();
    day.memories_about_faults = e
        .graph
        .nodes()
        .iter()
        .filter(|n| {
            n.content.contains("sigmas)") || n.content.contains("a body stopped")
        })
        .count();
    day.memories_routine = e
        .graph
        .nodes()
        .iter()
        .filter(|n| n.content.contains("legs.walk"))
        .count();
    day.budget_left_pct = ((e.ambient.budget.available() as u64 * 100) / budget_cap.max(1) as u64)
        .min(100) as u8;
    day.surprise_rate_pct = e.ambient.forward.surprise_rate_pct();
    day.interruptions_per_kilotick = (day.interrupted * 1_000) / ticks.max(1);
    day
}

/// The narrated version.
pub fn narrate(day: &Day) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    out.push((
        "ran",
        format!(
            "{} ticks, {}% of them quiet — the entity was not being asked questions, it was running",
            day.ticks,
            (day.quiet * 100) / day.ticks.max(1)
        ),
    ));
    out.push((
        "first notice",
        {
            let mut t = String::new();
            for (kind, before) in &day.first_notice {
                if !t.is_empty() {
                    t.push_str(" · ");
                }
                t.push_str(&format!("{}: noticed on occurrence {before}", kind.name()));
            }
            if t.is_empty() {
                t.push_str("nothing was ever noticed");
            }
            t
        },
    ));
    out.push((
        "awareness",
        format!(
            "of {} windows in which something went wrong, the human heard about it in {} ({}%) — not every occurrence, which would be the firehose, but every stretch of time",
            day.windows_with_a_fault,
            day.windows_aware,
            day.awareness_pct()
        ),
    ));
    out.push((
        "precision",
        format!(
            "{} interruptions, {} of them ({}%) for something that deserved one",
            day.interrupted,
            day.interrupts_deserved,
            day.interrupt_precision_pct()
        ),
    ));
    out.push((
        "quiet",
        format!(
            "{} things surfaced, {} of which ({}%) actually interrupted — {} interruptions per thousand ticks. An entity that surfaces everything is a firehose nobody keeps; one that surfaces nothing is a machine that quietly fails",
            day.surfaced,
            day.interrupted,
            day.interrupt_share_pct(),
            day.interruptions_per_kilotick
        ),
    ));
    out.push((
        "thinking",
        format!(
            "{} escalations above the reflex tier, with {}% of the budget left at the end and {} escalations the budget refused",
            day.escalations, day.budget_left_pct, day.budget_refusals
        ),
    ));
    out.push((
        "dreaming",
        format!(
            "{} consolidation passes during the day and {} once the body was put away — a busy workshop never gives two hundred consecutive quiet ticks, and rewriting memory while a gait loop runs is the thing the quiet check exists to prevent. {} memories at the end",
            day.dreams, day.dreams_when_idle, day.memories
        ),
    ));
    out.push((
        "by kind",
        {
            let mut t = String::new();
            for (kind, seen, reached, interrupted) in &day.by_kind {
                if *seen == 0 {
                    continue;
                }
                if !t.is_empty() {
                    t.push_str(" · ");
                }
                t.push_str(&format!(
                    "{}: {seen} seen, {reached} reached, {interrupted} interrupted",
                    kind.name()
                ));
            }
            t
        },
    ));
    out.push((
        "calibration",
        format!(
            "surprise rate {}% — a well-calibrated entity in a familiar place should be low single digits, and one that is surprised half the time is somewhere it does not understand and should say so",
            day.surprise_rate_pct
        ),
    ));
    out.push((
        "verdict",
        if day.could_be_left_running() {
            "this entity could be left running: it caught what mattered, stayed quiet, and still had budget at the end".to_string()
        } else {
            format!(
                "NOT fit to be left running — first notice on occurrence {}, awareness {}%, interrupt precision {}%, {} interruptions per kilotick, {}% budget left",
                day.worst_first_notice(),
                day.awareness_pct(),
                day.interrupt_precision_pct(),
                day.interruptions_per_kilotick,
                day.budget_left_pct
            )
        },
    ));
    out
}
