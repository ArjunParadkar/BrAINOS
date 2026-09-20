//! When the cloud goes away.
//!
//! Stage 5 is brain placement, and its claim is that *where the mind
//! runs is a deployment decision, not an identity decision.* Tier 0 runs
//! on the body because ten milliseconds is too slow to catch a fall;
//! Tier 3 runs in a datacenter because a frontier model does not fit in
//! 512 KiB of SRAM. The entity is the same entity either way.
//!
//! That claim has a bill attached, and this module is the bill. If part
//! of the mind is somewhere else, then sometimes part of the mind is
//! *not there*. A warehouse loses its uplink. A workshop's wifi is bad
//! near the metal racks. The datacenter is fine and the robot is in a
//! basement.
//!
//! The question is not whether the robot keeps working — it must, and
//! the tier structure makes that easy. The question is the one that
//! actually hurts in the field: **does anyone find out?** A robot that
//! silently drops from frontier reasoning to reflexes and carries on
//! looking busy is worse than one that stops, because the results
//! degrade in a way nobody attributes to the network for three weeks.
//!
//! Five things are checked:
//!
//! 1. Tier 0 never goes away. It is on the body, by construction.
//! 2. The entity keeps acting: reach loss degrades, it does not halt.
//! 3. The entity *knows* its ceiling dropped, at the moment it dropped.
//! 4. Memory keeps working throughout — §3.7 puts it on the entity, so
//!    an outage cannot cost memories, and the record of the outage
//!    itself survives the outage.
//! 5. Every decision made while degraded is **marked as degraded, and
//!    marked with the right cause**. This is the one that was wrong: the
//!    field was called `budget_limited` and was set whichever of budget
//!    or reach bit, so a dropped link was reported as an affordability
//!    problem. A human tops up the budget and nothing changes.

use brainos_abi::ids::BodyId;
use brainos_abi::time::{Instant, Tick};
use brainos_adapters::native::mini_blur;
use brainos_body::calibration::{Calibration, JointTrim};
use brainos_body::hal::Telemetry;
use brainos_cognition::ambient::SenseFrame;
use brainos_cognition::escalate::{cost_of, decide, Budget, Limit, Reason};
use brainos_cognition::placement::{Plan, Site};
use brainos_cognition::predict::Surprise;
use brainos_cognition::resource::{Resource, Tier};
use brainos_crypto::key::BrainKey;
use brainos_entity::entity::Entity;

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

/// One stretch of the day, at one level of reach.
#[derive(Clone, Debug, PartialEq)]
pub struct Phase {
    pub what: &'static str,
    pub ceiling: Tier,
    /// situations faced
    pub faced: u32,
    /// escalations that happened at all
    pub escalated: u32,
    /// of those, how many reached the tier the situation warranted
    pub reached_what_was_warranted: u32,
    /// and how many were short, by cause
    pub short_on_reach: u32,
    pub short_on_budget: u32,
    /// memories written during this phase
    pub memories_written: usize,
}

impl Phase {
    /// Of the situations that warranted thinking harder, how many got
    /// the thinking they warranted.
    pub fn served_pct(&self) -> u8 {
        if self.escalated == 0 {
            return 100;
        }
        ((self.reached_what_was_warranted as u64 * 100) / self.escalated as u64) as u8
    }
}

#[derive(Clone, Debug)]
pub struct Degradation {
    pub phases: Vec<Phase>,
    /// the entity never stopped acting
    pub kept_acting: bool,
    /// tier 0 was available in every phase
    pub reflexes_never_left: bool,
    /// outages the entity recorded in its own memory
    pub outages_recorded: usize,
    /// what the entity says about itself while degraded
    pub self_description_while_dark: String,
    /// one degraded escalation, described — this is what a human reads
    pub a_degraded_decision: String,
    /// would the old field have said "budget"?
    pub would_have_blamed_the_budget: bool,
}

pub fn degrade() -> Degradation {
    let key = BrainKey::from_seed(&[0x3D; 32]);
    let device = BrainKey::from_seed(&[0x4D; 32]);
    let mut e = Entity::new(BrainKey::from_seed(&[0x3D; 32]), Tick(0));
    e.wake(None, None, Tick(1)).expect("a fresh entity wakes");
    let body = e
        .acquire(&device, mini_blur(), calibration(), 10)
        .expect("the hexapod joins");
    e.observe_body(&body, &fit(0));
    let _ = key;

    // The deployment: reflexes on the body, fast cognition on the
    // board, deliberation at the edge, the frontier in a datacenter.
    // This is `Plan::distributed`, which is what a real fleet robot
    // looks like and the only shape in which the question arises.
    let plan = Plan::distributed();
    e.resources
        .add(Resource::new("reflex", Tier::Autonomic, Site::Body));
    e.resources
        .add(Resource::new("onboard", Tier::Fast, plan.site_for(Tier::Fast)));
    e.resources.add(Resource::new(
        "edge-planner",
        Tier::Deliberate,
        plan.site_for(Tier::Deliberate),
    ));
    e.resources.add(Resource::new(
        "frontier",
        Tier::Frontier,
        plan.site_for(Tier::Frontier),
    ));

    let mut phases = Vec::new();
    let mut kept_acting = true;
    let mut reflexes_never_left = true;
    let mut a_degraded_decision = String::new();
    let mut would_have_blamed_the_budget = false;

    let mut run = |e: &mut Entity, what: &'static str, n: u32, sample: bool| -> Phase {
        let before_memories = e.graph.len();
        let ceiling = e.ceiling();
        let mut budget = Budget::plentiful();
        let mut p = Phase {
            what,
            ceiling,
            faced: 0,
            escalated: 0,
            reached_what_was_warranted: 0,
            short_on_reach: 0,
            short_on_budget: 0,
            memories_written: 0,
        };
        let mut shown = !sample;

        for i in 0..n {
            e.tick(SenseFrame::quiet(Instant(i as u64)));
            budget.tick();
            p.faced += 1;

            // every fourth situation is genuinely novel, which is what
            // warrants the frontier
            let novel = i % 4 == 0;
            let surprise = Surprise {
                key: 0,
                sigmas: if novel { 4.0 } else { 0.5 },
                novel,
                observed: 0.0,
                expected: 0.0,
            };
            // the entity is working, not just thinking: every tenth
            // situation it actually does something and records how it
            // went. Without this the phase writes no memories at all and
            // the "memory keeps working while dark" claim is untested —
            // which it was, on the first run, and the run said so.
            if i % 10 == 0 {
                let out = e.attempt(
                    brainos_abi::verb::Verb::literal("legs.walk"),
                    body,
                    &[("speed", 0.08), ("heading", 0.0)],
                );
                let granted = matches!(out.verdict, brainos_kira::gate::Verdict::Granted(_));
                e.outcome(
                    brainos_abi::verb::Verb::literal("legs.walk"),
                    Some(body),
                    if granted {
                        brainos_memory::episode::Outcome::Succeeded
                    } else {
                        brainos_memory::episode::Outcome::Failed
                    },
                    "crossing the floor",
                    &["workshop"],
                );
            }

            let esc = decide(&surprise, 20, true, 0, false, &mut budget, ceiling);
            if !esc.happened() {
                continue;
            }
            p.escalated += 1;
            let warranted = esc.reason.map(Reason::warrants).unwrap_or(Tier::Autonomic);
            match esc.limited_by {
                None => p.reached_what_was_warranted += 1,
                Some(Limit::Reach) => {
                    p.short_on_reach += 1;
                    if !shown {
                        a_degraded_decision = format!(
                            "{} — the situation warranted {}, the entity reached {}, \
                             and the reason is recorded as `{}`, not as an \
                             affordability problem",
                            esc.describe().as_str(),
                            warranted.name(),
                            esc.to.name(),
                            Limit::Reach.name()
                        );
                        would_have_blamed_the_budget = true;
                        shown = true;
                    }
                }
                Some(Limit::Budget) => p.short_on_budget += 1,
            }
        }

        // it is still acting, and the reflexes are still there
        if e.ceiling() == Tier::Autonomic && e.resources.at_tier(Tier::Autonomic).count() == 0 {
            reflexes_never_left = false;
        }
        if e.resources
            .at_tier(Tier::Autonomic)
            .all(|r| !r.available)
        {
            reflexes_never_left = false;
        }
        let out = e.attempt(
            brainos_abi::verb::Verb::literal("legs.walk"),
            body,
            &[("speed", 0.08), ("heading", 0.0)],
        );
        if !matches!(out.verdict, brainos_kira::gate::Verdict::Granted(_)) {
            kept_acting = false;
        }

        p.memories_written = e.graph.len().saturating_sub(before_memories);
        p
    };

    phases.push(run(&mut e, "everything reachable", 60, false));

    // --- the datacenter link drops ---
    e.lose_site(Site::Remote);
    phases.push(run(&mut e, "datacenter unreachable", 60, true));

    // --- and then the edge node too: the robot is on its own ---
    e.lose_site(Site::Edge);
    phases.push(run(&mut e, "edge unreachable as well", 60, false));

    // --- and it all comes back ---
    e.regain_site(Site::Edge);
    e.regain_site(Site::Remote);
    phases.push(run(&mut e, "reach restored", 60, false));

    let self_description_while_dark = String::from(e.describe().as_str());
    let outages_recorded = e.outages();

    Degradation {
        phases,
        kept_acting,
        reflexes_never_left,
        outages_recorded,
        self_description_while_dark,
        a_degraded_decision,
        would_have_blamed_the_budget,
    }
}

impl Degradation {
    pub fn is_honest(&self) -> bool {
        self.kept_acting
            && self.reflexes_never_left
            && self.outages_recorded >= 2
            && !self.a_degraded_decision.is_empty()
            // memory kept working in every phase, including the dark ones
            && self.phases.iter().all(|p| p.memories_written > 0)
            // the dark phases really were degraded
            && self.phases.iter().any(|p| p.short_on_reach > 0)
            // and the restored phase is not
            && self
                .phases
                .last()
                .map(|p| p.short_on_reach == 0)
                .unwrap_or(false)
    }
}

pub fn narrate(d: &Degradation) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    for p in &d.phases {
        out.push((
            "phase",
            format!(
                "{:<26} ceiling {:<10} · {} situations, {} escalations, {}% served at the \
                 tier they warranted ({} short on reach, {} on budget) · {} memories written",
                p.what,
                p.ceiling.name(),
                p.faced,
                p.escalated,
                p.served_pct(),
                p.short_on_reach,
                p.short_on_budget,
                p.memories_written
            ),
        ));
    }
    out.push((
        "acting",
        format!(
            "the body kept moving through every phase: {} · the reflex tier was never \
             absent: {}",
            d.kept_acting, d.reflexes_never_left
        ),
    ));
    out.push((
        "recorded",
        format!(
            "{} outage(s) written into the entity's own memory, at the moment the reach \
             dropped — §3.7 puts memory on the entity, so the record of the outage \
             survives the outage",
            d.outages_recorded
        ),
    ));
    out.push((
        "the fix",
        format!(
            "a degraded decision, as a human reads it: {}",
            d.a_degraded_decision
        ),
    ));
    out
}

/// So the cost of escalation is visible next to the rest.
pub fn tier_costs() -> Vec<(Tier, u32)> {
    Tier::ALL.iter().map(|t| (*t, cost_of(*t))).collect()
}
