//! BFS  LEVEL    Host - the only crate with an operating system (CLI, scenarios, OS nerves)  [crate brainos-host]
//! BFS  PURPOSE  Experiment in a simulated world measuring whether an entity's choices to attempt or decline improve with experience, with a no-structure control
//! BFS  DEFINES  Situation, World, Policy, Window, Run, run, Experiment, experiment, Regime, Curiosity, Adaptation, world_changes, +2 more
//! BFS  USES     brainos: abi, cognition, crypto, entity, memory
//! BFS  USED BY  host/fleet_experiment, host/tests/compound_suite
//!
//! Does an entity with experience actually get better?
//!
//! §2.4 says the moat becomes visible as a published curve: *entity age
//! against task success rate*. §1.5 is blunt that the moat is made of
//! accumulated experience and that today there is none. This module is
//! the measurement apparatus for that curve, run against a simulated
//! world so the mechanism can be tested before there is real data.
//!
//! **What this can show and what it cannot.** It can show whether the
//! consolidation mechanism recovers latent structure from experience —
//! whether an entity that has acted a thousand times makes better
//! decisions than one that has acted ten times, using only what it
//! learned. It **cannot** show that a real robot gets better, because
//! the world here is one I wrote and it has exactly the structure I put
//! in it. The honest claim is about the mechanism, not about robots.
//!
//! **The control that makes it mean anything.** The same experiment runs
//! in a world with *no* hidden structure, where success is a coin flip.
//! If the learner appears to improve there too, the metric is measuring
//! something other than learning and the whole result is worthless. That
//! run is reported alongside, not hidden, and a difference that does not
//! survive it is reported as a null.
//!
//! **The decision the experience is for.** Not "do it better" — the
//! entity has no motor skill to improve. It is *whether to attempt at
//! all*. An entity that has learned that walking fails on a wet floor
//! with a low battery declines and says why, and declining correctly is
//! worth as much as succeeding. That is measured separately, because an
//! entity that simply attempts less would look better on success rate
//! while being useless.

use brainos_abi::ids::BodyId;
use brainos_abi::time::{Instant, Tick};
use brainos_cognition::ambient::SenseFrame;
use brainos_abi::verb::Verb;
use brainos_crypto::key::BrainKey;
use brainos_crypto::rng::Drbg;
use brainos_entity::entity::Entity;
use brainos_memory::consolidate::Dream;
use brainos_memory::episode::Outcome;

/// The context an attempt happens in. Three binary conditions the entity
/// observes, and which the world uses to decide the outcome.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Situation {
    pub carpet: bool,
    pub battery_ok: bool,
    pub loaded: bool,
}

impl Situation {
    pub fn tags(&self) -> [&'static str; 3] {
        [
            if self.carpet { "carpet" } else { "tile" },
            if self.battery_ok { "battery_ok" } else { "battery_low" },
            if self.loaded { "loaded" } else { "unloaded" },
        ]
    }

    pub fn from_bits(n: u64) -> Situation {
        Situation {
            carpet: n & 1 != 0,
            battery_ok: n & 2 != 0,
            loaded: n & 4 != 0,
        }
    }
}

/// What the world is like.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum World {
    /// walking succeeds on carpet with a good battery, and fails
    /// otherwise. Structure the entity can in principle find.
    Structured,
    /// success is a coin flip, uncorrelated with anything observable.
    /// The control: an entity that "improves" here is not learning.
    Structureless,
}

impl World {
    /// Does an attempt succeed? The entity never sees this function.
    ///
    /// The structured rule deliberately uses two of the three conditions
    /// and ignores the third, so a learner that latches onto `loaded`
    /// has found a correlate rather than the cause — and the experiment
    /// reports whether it did.
    pub fn outcome(&self, s: Situation, rng: &mut Drbg) -> bool {
        match self {
            World::Structured => {
                let ought = s.carpet && s.battery_ok;
                // ten percent noise, so nothing is perfectly learnable
                // and a claim of a hundred percent would be a bug
                if rng.below(100) < 10 {
                    !ought
                } else {
                    ought
                }
            }
            World::Structureless => rng.below(100) < 50,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            World::Structured => "structured",
            World::Structureless => "structureless (control)",
        }
    }
}

/// How an entity decides whether to attempt.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Policy {
    /// always attempt. The control entity.
    Always,
    /// consult the skill graph; decline when a learned precondition is
    /// missing. The entity with experience.
    Learned,
    /// never attempt. Not a real policy — it exists to show the metric
    /// cannot be gamed by refusing everything, which is the obvious way
    /// a "success rate" would be cheated.
    Never,
}

impl Policy {
    pub const fn name(self) -> &'static str {
        match self {
            Policy::Always => "always attempts",
            Policy::Learned => "consults what it learned",
            Policy::Never => "declines everything (metric check)",
        }
    }
}

/// One window of the entity's life.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Window {
    pub from_episode: usize,
    pub to_episode: usize,
    /// attempts made
    pub attempted: u32,
    /// attempts that succeeded
    pub succeeded: u32,
    /// times it declined to attempt
    pub declined: u32,
    /// of the declines, how many the world would have failed anyway —
    /// the ones where declining was right
    pub declined_rightly: u32,
    /// how many preconditions the entity holds as *binding* at the end of
    /// the window — the learning signal itself.
    ///
    /// Not "practiced skills": a skill becomes practiced at twenty
    /// attempts and eighty percent lifetime success, and an entity that
    /// learned to decline correctly has a lifetime success rate dragged
    /// down by the failures it needed in order to learn. Counting
    /// practiced skills here would report zero throughout and look like
    /// nothing happened.
    pub binding_preconditions: usize,
}

impl Window {
    /// Of what it attempted, how much worked.
    pub fn success_rate_pct(&self) -> u8 {
        if self.attempted == 0 {
            return 0;
        }
        ((self.succeeded as u64 * 100) / self.attempted as u64) as u8
    }

    /// Of every situation it faced, how many it handled well — where
    /// handling well is either succeeding or correctly declining.
    ///
    /// This is the metric that matters, and it is deliberately not
    /// success rate. An entity that declined everything would have a
    /// perfect success rate and be useless; this counts the declines it
    /// got right and none of the ones it got wrong.
    pub fn judgement_pct(&self) -> u8 {
        let faced = self.attempted as u64 + self.declined as u64;
        if faced == 0 {
            return 0;
        }
        let good = self.succeeded as u64 + self.declined_rightly as u64;
        ((good * 100) / faced) as u8
    }

    /// How often it declined something that would have worked. The cost
    /// of caution, reported because caution has one.
    pub fn missed_pct(&self) -> u8 {
        if self.declined == 0 {
            return 0;
        }
        let wrongly = self.declined - self.declined_rightly;
        ((wrongly as u64 * 100) / self.declined as u64) as u8
    }
}

/// A whole run.
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub world: World,
    pub policy: Policy,
    pub episodes: usize,
    pub windows: Vec<Window>,
    /// the preconditions the entity ended up believing
    pub learned: Vec<(String, u8)>,
}

impl Run {
    pub fn early(&self) -> Window {
        self.windows.first().copied().unwrap_or_default()
    }

    pub fn late(&self) -> Window {
        self.windows.last().copied().unwrap_or_default()
    }

    /// Did judgement improve from the first window to the last?
    pub fn improvement_pts(&self) -> i16 {
        self.late().judgement_pct() as i16 - self.early().judgement_pct() as i16
    }
}

fn v(s: &str) -> Verb {
    Verb::parse(s).expect("a well-formed verb")
}

/// Run one entity through one world.
///
/// Deterministic: the same seed gives the same run, so a result can be
/// re-checked rather than re-rolled.
pub fn run(world: World, policy: Policy, episodes: usize, windows: usize, seed: &[u8]) -> Run {
    let walk = v("legs.walk");
    let body = BodyId::from_bytes([0xB2; 32]);
    let mut world_rng = Drbg::deterministic(seed);
    let mut situation_rng = Drbg::deterministic(&[seed, b"-situations"].concat());

    let mut e = Entity::new(BrainKey::from_seed(&[0x5A; 32]), Tick(0));
    e.wake(None, None, Tick(1)).expect("a fresh entity wakes");
    // the entity knows it has the verb; what it does not know is when it
    // works
    e.learn_skill(walk, "walk");

    let per_window = episodes / windows.max(1);
    let mut out_windows = Vec::with_capacity(windows);
    let mut w = Window {
        from_episode: 0,
        to_episode: per_window,
        ..Default::default()
    };

    for i in 0..episodes {
        // the entity is running its loop; without this its tick never
        // advances and nothing it believes can ever go stale
        e.tick(SenseFrame::quiet(Instant(i as u64 * 1_000)));

        let situation = Situation::from_bits(situation_rng.below(8));
        let tags = situation.tags();

        // --- decide ---
        let attempt = match policy {
            Policy::Always => true,
            Policy::Never => false,
            Policy::Learned => e
                .skills
                .for_verb(&walk)
                .map(|s| s.applicable_at(&tags, e.tick))
                .unwrap_or(true),
        };

        // --- the world's answer, drawn whether or not we attempt, so
        //     that declining and attempting see the same world ---
        let would_succeed = world.outcome(situation, &mut world_rng);

        if attempt {
            w.attempted += 1;
            if would_succeed {
                w.succeeded += 1;
            }
            e.outcome(
                walk,
                Some(body),
                if would_succeed {
                    Outcome::Succeeded
                } else {
                    Outcome::Failed
                },
                "forward",
                &tags,
            );
        } else {
            w.declined += 1;
            if !would_succeed {
                w.declined_rightly += 1;
            }
            // declining is also something that happened, and the entity
            // should remember having declined — but it is not an
            // attempt, so it does not teach the skill anything
            e.outcome(
                walk,
                Some(body),
                Outcome::Denied,
                "declined: a learned precondition was missing",
                &tags,
            );
        }

        // --- consolidate periodically, as an idle entity would ---
        if (i + 1) % 25 == 0 {
            e.dream(Dream::deep());
        }

        if (i + 1) % per_window == 0 && out_windows.len() < windows {
            w.binding_preconditions = e
                .skills
                .for_verb(&walk)
                .map(|s| s.preconditions.iter().filter(|p| p.is_well_supported()).count())
                .unwrap_or(0);
            out_windows.push(w);
            w = Window {
                from_episode: i + 1,
                to_episode: i + 1 + per_window,
                ..Default::default()
            };
        }
    }

    let learned = e
        .skills
        .for_verb(&walk)
        .map(|s| {
            s.preconditions
                .iter()
                .filter(|p| p.support >= 3)
                .map(|p| (p.tag.clone(), p.strength()))
                .collect()
        })
        .unwrap_or_default();

    Run {
        world,
        policy,
        episodes,
        windows: out_windows,
        learned,
    }
}

/// The whole experiment: both policies in both worlds.
#[derive(Clone, Debug)]
pub struct Experiment {
    pub structured_control: Run,
    pub structured_learner: Run,
    pub structureless_control: Run,
    pub structureless_learner: Run,
    pub episodes: usize,
}

impl Experiment {
    /// The headline, in the structured world: how much better the
    /// learner's judgement is than the control's, in percentage points.
    pub fn effect_pts(&self) -> i16 {
        self.structured_learner.late().judgement_pct() as i16
            - self.structured_control.late().judgement_pct() as i16
    }

    /// The same difference in the world with nothing to learn. This
    /// should be near zero; anything large means the metric is measuring
    /// something other than learning.
    pub fn null_effect_pts(&self) -> i16 {
        self.structureless_learner.late().judgement_pct() as i16
            - self.structureless_control.late().judgement_pct() as i16
    }

    /// Does the result survive its own control?
    ///
    /// The effect in the structured world must be substantially larger
    /// than the effect in the structureless one. "Substantially" is
    /// three times and at least ten points — a threshold chosen before
    /// looking, and stated so it can be argued with.
    pub fn survives_the_control(&self) -> bool {
        let effect = self.effect_pts();
        let null = self.null_effect_pts().abs();
        effect >= 10 && effect as i32 >= (null as i32) * 3
    }

    /// Did the learner find the real rule, or a correlate?
    ///
    /// The world's rule uses `carpet` and `battery_ok` and ignores
    /// `loaded`. A learner that ended up believing `loaded` matters has
    /// found a correlate, which is the honest limit of intersecting the
    /// context of successes and is stated in `Precondition`'s own
    /// documentation.
    pub fn found_the_real_rule(&self) -> (bool, bool, bool) {
        let has = |t: &str| {
            self.structured_learner
                .learned
                .iter()
                .any(|(tag, strength)| tag == t && *strength >= 80)
        };
        (has("carpet"), has("battery_ok"), has("loaded"))
    }
}

/// Run it.
pub fn experiment(episodes: usize, windows: usize) -> Experiment {
    Experiment {
        structured_control: run(World::Structured, Policy::Always, episodes, windows, b"s-c"),
        structured_learner: run(World::Structured, Policy::Learned, episodes, windows, b"s-l"),
        structureless_control: run(
            World::Structureless,
            Policy::Always,
            episodes,
            windows,
            b"n-c",
        ),
        structureless_learner: run(
            World::Structureless,
            Policy::Learned,
            episodes,
            windows,
            b"n-l",
        ),
        episodes,
    }
}

// =====================================================================
// The harder question: what happens when the world changes?
// =====================================================================

/// A world whose rule changes partway through.
///
/// Everything above shows an entity finding structure. This asks the
/// question that decides whether accumulated experience is an asset or a
/// liability: **when the world stops working the way it used to, can the
/// entity notice?**
///
/// It matters because the failure mode is specific and nasty. The
/// entity learns that walking works on carpet. It therefore *declines*
/// to walk on tile. If the floor is later replaced with tile that walks
/// perfectly well, the entity never attempts on tile, never observes a
/// success there, and never revises. Its own caution prevents it from
/// learning that its caution is now wrong — and the more experience it
/// has, the more confident the mistake.
///
/// That is not a bug in the code so much as a hole in the design, and
/// naming it is worth more than a graph.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Regime {
    /// the original rule: carpet and a good battery
    Original,
    /// the floor was replaced. Now it is the *battery* that matters and
    /// the surface does not.
    Changed,
}

impl Regime {
    pub fn outcome(&self, s: Situation, rng: &mut Drbg) -> bool {
        let ought = match self {
            Regime::Original => s.carpet && s.battery_ok,
            Regime::Changed => s.battery_ok,
        };
        if rng.below(100) < 10 {
            !ought
        } else {
            ought
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Regime::Original => "carpet and a good battery",
            Regime::Changed => "a good battery; the floor was replaced",
        }
    }
}

/// How the entity behaves when a learned precondition is missing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Curiosity {
    /// never attempt when a precondition is missing. The entity as
    /// built before this experiment.
    None,
    /// attempt anyway, rarely, precisely to find out whether the world
    /// still works the way it did. One in `1/rate` of the situations it
    /// would otherwise decline.
    Occasional { per_mille: u64 },
}

impl Curiosity {
    pub const fn name(self) -> &'static str {
        match self {
            Curiosity::None => "never tries what it believes will fail",
            Curiosity::Occasional { .. } => "occasionally checks whether it is still right",
        }
    }
}

/// What happened when the world changed under an entity.
#[derive(Clone, Debug, PartialEq)]
pub struct Adaptation {
    pub curiosity: Curiosity,
    /// judgement in the last window before the change
    pub before: u8,
    /// judgement in the first window after
    pub immediately_after: u8,
    /// judgement in the last window, long after
    pub recovered_to: u8,
    /// what it believed at the end
    pub learned: Vec<(String, u8)>,
    /// did it ever attempt in the situations the old rule told it to
    /// avoid?
    pub attempts_in_the_new_regime: u32,
    /// how many episodes it spent in each regime
    pub episodes_per_regime: usize,
}

impl Adaptation {
    /// Did it get back to something like its old competence?
    pub fn recovered(&self) -> bool {
        self.recovered_to as i16 >= self.before as i16 - 10
    }

    /// How much it lost and did not get back.
    pub fn permanent_loss_pts(&self) -> i16 {
        self.before as i16 - self.recovered_to as i16
    }
}

/// Run an entity through a world that changes underneath it.
pub fn world_changes(
    curiosity: Curiosity,
    episodes_per_regime: usize,
    windows_per_regime: usize,
    seed: &[u8],
) -> Adaptation {
    let walk = v("legs.walk");
    let body = BodyId::from_bytes([0xB2; 32]);
    let mut world_rng = Drbg::deterministic(seed);
    let mut situation_rng = Drbg::deterministic(&[seed, b"-sit"].concat());
    let mut curiosity_rng = Drbg::deterministic(&[seed, b"-cur"].concat());

    let mut e = Entity::new(BrainKey::from_seed(&[0x5A; 32]), Tick(0));
    e.wake(None, None, Tick(1)).expect("a fresh entity wakes");
    e.learn_skill(walk, "walk");

    let total = episodes_per_regime * 2;
    let per_window = episodes_per_regime / windows_per_regime.max(1);
    let mut windows: Vec<Window> = Vec::new();
    let mut w = Window::default();
    let mut attempts_in_the_new_regime = 0u32;

    for i in 0..total {
        let regime = if i < episodes_per_regime {
            Regime::Original
        } else {
            Regime::Changed
        };
        e.tick(SenseFrame::quiet(Instant(i as u64 * 1_000)));

        let situation = Situation::from_bits(situation_rng.below(8));
        let tags = situation.tags();

        let believes_it_will_work = e
            .skills
            .for_verb(&walk)
            .map(|s| s.applicable_at(&tags, e.tick))
            .unwrap_or(true);

        let attempt = believes_it_will_work
            || match curiosity {
                Curiosity::None => false,
                Curiosity::Occasional { per_mille } => curiosity_rng.below(1_000) < per_mille,
            };

        let would_succeed = regime.outcome(situation, &mut world_rng);

        if attempt {
            w.attempted += 1;
            if would_succeed {
                w.succeeded += 1;
            }
            // the situations the *old* rule said to avoid, attempted in
            // the new regime
            if regime == Regime::Changed && !situation.carpet {
                attempts_in_the_new_regime += 1;
            }
            e.outcome(
                walk,
                Some(body),
                if would_succeed {
                    Outcome::Succeeded
                } else {
                    Outcome::Failed
                },
                "forward",
                &tags,
            );
        } else {
            w.declined += 1;
            if !would_succeed {
                w.declined_rightly += 1;
            }
            e.outcome(
                walk,
                Some(body),
                Outcome::Denied,
                "declined: a learned precondition was missing",
                &tags,
            );
        }

        if (i + 1) % 25 == 0 {
            e.dream(Dream::deep());
        }
        if (i + 1) % per_window == 0 {
            w.binding_preconditions = e
                .skills
                .for_verb(&walk)
                .map(|s| s.preconditions.iter().filter(|p| p.is_well_supported()).count())
                .unwrap_or(0);
            windows.push(w);
            w = Window {
                from_episode: i + 1,
                to_episode: i + 1 + per_window,
                ..Default::default()
            };
        }
    }

    let learned = e
        .skills
        .for_verb(&walk)
        .map(|s| {
            s.preconditions
                .iter()
                .filter(|p| p.support >= 3)
                .map(|p| (p.tag.clone(), p.strength()))
                .collect()
        })
        .unwrap_or_default();

    let before = windows
        .get(windows_per_regime.saturating_sub(1))
        .map(|w| w.judgement_pct())
        .unwrap_or(0);
    let immediately_after = windows
        .get(windows_per_regime)
        .map(|w| w.judgement_pct())
        .unwrap_or(0);
    let recovered_to = windows.last().map(|w| w.judgement_pct()).unwrap_or(0);

    Adaptation {
        curiosity,
        before,
        immediately_after,
        recovered_to,
        learned,
        attempts_in_the_new_regime,
        episodes_per_regime,
    }
}

/// Both behaviours, through the same change.
#[derive(Clone, Debug)]
pub struct AdaptationExperiment {
    pub incurious: Adaptation,
    pub curious: Adaptation,
}

impl AdaptationExperiment {
    /// Does curiosity earn its place?
    ///
    /// It has a cost — attempting things you believe will fail is, by
    /// construction, mostly failing — so it has to pay for itself when
    /// the world changes or it is just noise.
    pub fn curiosity_pays(&self) -> bool {
        self.curious.recovered_to > self.incurious.recovered_to + 5
    }
}

pub fn adaptation(episodes_per_regime: usize, windows_per_regime: usize) -> AdaptationExperiment {
    AdaptationExperiment {
        incurious: world_changes(
            Curiosity::None,
            episodes_per_regime,
            windows_per_regime,
            b"incurious",
        ),
        curious: world_changes(
            Curiosity::Occasional { per_mille: 80 },
            episodes_per_regime,
            windows_per_regime,
            b"curious",
        ),
    }
}
