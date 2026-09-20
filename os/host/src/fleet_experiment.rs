//! Does experience transfer between entities, or only within one?
//!
//! `compound` answered the first half of §2.4: an entity with experience
//! makes better decisions than one without, 61 points against a 3-point
//! null control. This is the second half, and it is the half the
//! business case actually rests on. §2.4 says the fleet gets better
//! together and §6 sells a skill marketplace; both assume that what one
//! entity worked out is worth something to another.
//!
//! That assumption has an obvious failure mode and it is worth stating
//! before the numbers rather than after. What an entity learns is not
//! "walking needs carpet". It is "walking needs carpet *here*" — and
//! nothing in the claim says where here is. A robot in a carpeted
//! workshop and a robot in a tiled one learn opposite rules and both are
//! right. Sell one to the other and you have sold a confident error.
//!
//! So: a fleet of entities in three different deployments, learning the
//! same verb under three different hidden rules, and four arms.
//!
//!   alone     learns from its own experience only. The baseline.
//!   matched   adopts a veteran's evidence from the same deployment
//!   blind     adopts a veteran's evidence from a random deployment —
//!             the marketplace with no notion of where a skill came from
//!   oracle    starts with the true rule handed to it. Not achievable;
//!             it is the ceiling, so the other three can be read against
//!             something other than each other.
//!
//! **Pre-registered, before the run.** matched > alone > blind, and
//! blind recovers eventually because adopted evidence is evidence rather
//! than a verdict — the buyer's own attempts accumulate on top of it and
//! contradict it. The interesting number is *how long* the recovery
//! takes, because that is the cost of a marketplace that does not track
//! provenance, and it is a number rather than an opinion.
//!
//! **What this cannot show.** The three deployments are three rules I
//! wrote. It says what the mechanism does when worlds differ; it does
//! not say how much real deployments differ. Anyone quoting the number
//! without that sentence is misquoting it.

use brainos_abi::ids::BodyId;
use brainos_abi::time::{Instant, Tick};
use brainos_abi::verb::Verb;
use brainos_cognition::ambient::SenseFrame;
use brainos_crypto::key::BrainKey;
use brainos_crypto::rng::Drbg;
use brainos_entity::entity::Entity;
use brainos_memory::consolidate::Dream;
use brainos_memory::episode::Outcome;
use brainos_memory::skill::{Decision, Precondition};

use crate::compound::Situation;

/// One deployment. Each has a different hidden rule, and every rule is
/// as true as the others — that is the whole difficulty.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Deployment {
    /// a carpeted workshop: the gait needs carpet and a good battery
    Carpeted,
    /// a tiled kitchen: the gait needs the *tile* — the rubber feet grip
    /// it and sink into pile — and a good battery
    Tiled,
    /// a loading bay: floor is irrelevant, but the gait fails under load
    LoadingBay,
}

impl Deployment {
    pub const ALL: [Deployment; 3] = [
        Deployment::Carpeted,
        Deployment::Tiled,
        Deployment::LoadingBay,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Deployment::Carpeted => "carpeted workshop",
            Deployment::Tiled => "tiled kitchen",
            Deployment::LoadingBay => "loading bay",
        }
    }

    /// The tag whose absence actually causes failure here.
    pub const fn true_condition(self) -> &'static str {
        match self {
            Deployment::Carpeted => "carpet",
            Deployment::Tiled => "tile",
            Deployment::LoadingBay => "unloaded",
        }
    }

    /// Does an attempt succeed? The entity never sees this.
    pub fn outcome(&self, s: Situation, rng: &mut Drbg) -> bool {
        let ought = match self {
            Deployment::Carpeted => s.carpet && s.battery_ok,
            Deployment::Tiled => !s.carpet && s.battery_ok,
            Deployment::LoadingBay => !s.loaded && s.battery_ok,
        };
        // the same ten percent noise as `compound`, so nothing is
        // perfectly learnable and a hundred percent would be a bug
        if rng.below(100) < 10 {
            !ought
        } else {
            ought
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Arm {
    Alone,
    Matched,
    Blind,
    Oracle,
}

impl Arm {
    pub const ALL: [Arm; 4] = [Arm::Alone, Arm::Matched, Arm::Blind, Arm::Oracle];

    pub const fn name(self) -> &'static str {
        match self {
            Arm::Alone => "alone",
            Arm::Matched => "adopts from the same deployment",
            Arm::Blind => "adopts from a random deployment",
            Arm::Oracle => "handed the true rule (ceiling)",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Slice {
    pub from: usize,
    pub to: usize,
    pub attempted: u32,
    pub succeeded: u32,
    pub declined: u32,
    pub declined_rightly: u32,
    /// attempts made *specifically* to check a borrowed claim. A subset
    /// of `attempted`, counted separately because probing has a cost and
    /// the cost should be visible rather than buried in the total.
    pub probed: u32,
}

impl Slice {
    /// Of every situation faced, how many were handled well — succeeded,
    /// or correctly declined. The same metric `compound` uses, and for
    /// the same reason: an entity that declines everything scores
    /// perfectly on success rate and is useless.
    pub fn judgement_pct(&self) -> u8 {
        let faced = self.attempted as u64 + self.declined as u64;
        if faced == 0 {
            return 0;
        }
        ((self.succeeded as u64 + self.declined_rightly as u64) * 100 / faced) as u8
    }

    /// How often it refused something that would have worked. The cost
    /// of a wrong belief, specifically.
    pub fn missed_pct(&self) -> u8 {
        if self.declined == 0 {
            return 0;
        }
        (((self.declined - self.declined_rightly) as u64 * 100) / self.declined as u64) as u8
    }
}

/// One entity's life.
#[derive(Clone, Debug)]
pub struct Career {
    pub deployment: Deployment,
    pub arm: Arm,
    pub adopted_from: Option<Deployment>,
    pub slices: Vec<Slice>,
    /// what it ended up holding as binding
    pub believes: Vec<(String, u8, u16)>,
    /// the first slice in which its judgement reached the baseline's
    /// final level — `None` if it never did
    pub caught_up_at: Option<usize>,
}

fn v(s: &str) -> Verb {
    Verb::parse(s).expect("a well-formed verb")
}

const WALK: &str = "legs.walk";

/// Run one entity through one deployment, optionally starting with
/// somebody else's evidence.
#[allow(clippy::too_many_arguments)]
fn career(
    deployment: Deployment,
    arm: Arm,
    adopted: Option<(&[Precondition], Deployment)>,
    episodes: usize,
    slices: usize,
    seed: &[u8],
) -> Career {
    let walk = v(WALK);
    let body = BodyId::from_bytes([0xB2; 32]);
    let mut world_rng = Drbg::deterministic(seed);
    let mut situation_rng = Drbg::deterministic(&[seed, b"-situations"].concat());

    let mut e = Entity::new(BrainKey::from_seed(&[0x5A; 32]), Tick(0));
    e.wake(None, None, Tick(1)).expect("a fresh entity wakes");
    e.learn_skill(walk, "walk");

    if let Some((taught, _)) = adopted {
        let (h, graph, skills) = e.memory_mut();
        skills.adopt(h, graph, &walk, taught, Tick(1));
    }

    let per = episodes / slices.max(1);
    let mut out = Vec::with_capacity(slices);
    let mut s = Slice {
        from: 0,
        to: per,
        ..Default::default()
    };

    for i in 0..episodes {
        e.tick(SenseFrame::quiet(Instant(i as u64 * 1_000)));
        let situation = Situation::from_bits(situation_rng.below(8));
        let tags = situation.tags();

        let (attempt, probing) = match arm {
            Arm::Oracle => (
                tags.contains(&deployment.true_condition()) && situation.battery_ok,
                false,
            ),
            _ => {
                let now = e.tick;
                match e
                    .skills
                    .for_verb_mut(&walk)
                    .map(|s| s.decide_at(&tags, now))
                {
                    Some(Decision::Attempt) => (true, false),
                    Some(Decision::Probe(_)) => (true, true),
                    Some(Decision::Decline(_)) => (false, false),
                    None => (true, false),
                }
            }
        };

        let would_succeed = deployment.outcome(situation, &mut world_rng);

        if probing {
            s.probed += 1;
        }
        if attempt {
            s.attempted += 1;
            if would_succeed {
                s.succeeded += 1;
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
            s.declined += 1;
            if !would_succeed {
                s.declined_rightly += 1;
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
        if (i + 1) % per == 0 && out.len() < slices {
            out.push(s);
            s = Slice {
                from: i + 1,
                to: i + 1 + per,
                ..Default::default()
            };
        }
    }

    let believes = e
        .skills
        .for_verb(&walk)
        .map(|s| {
            s.preconditions
                .iter()
                .filter(|p| p.is_binding_at(e.tick))
                .map(|p| (p.tag.clone(), p.strength(), p.tests))
                .collect()
        })
        .unwrap_or_default();

    Career {
        deployment,
        arm,
        adopted_from: adopted.map(|(_, d)| d),
        slices: out,
        believes,
        caught_up_at: None,
    }
}

/// What a veteran of one deployment has to sell.
fn veteran(deployment: Deployment, episodes: usize, seed: &[u8]) -> Vec<Precondition> {
    let walk = v(WALK);
    let body = BodyId::from_bytes([0xB2; 32]);
    let mut world_rng = Drbg::deterministic(seed);
    let mut situation_rng = Drbg::deterministic(&[seed, b"-situations"].concat());

    let mut e = Entity::new(BrainKey::from_seed(&[0x5A; 32]), Tick(0));
    e.wake(None, None, Tick(1)).expect("a fresh entity wakes");
    e.learn_skill(walk, "walk");

    for i in 0..episodes {
        e.tick(SenseFrame::quiet(Instant(i as u64 * 1_000)));
        let situation = Situation::from_bits(situation_rng.below(8));
        let tags = situation.tags();
        // a veteran that already declines would stop generating the
        // tests its own evidence is made of, so the seller here attempts
        // everything: what is for sale is the record of an entity that
        // actually found out
        let ok = deployment.outcome(situation, &mut world_rng);
        e.outcome(
            walk,
            Some(body),
            if ok { Outcome::Succeeded } else { Outcome::Failed },
            "forward",
            &tags,
        );
        if (i + 1) % 25 == 0 {
            e.dream(Dream::deep());
        }
    }

    // What is for sale is what the veteran can back up, not every tag it
    // ever tracked. Selling the hunches too made matched and blind
    // adopt near-identical lists and the two arms came out to the digit
    // — which is what a bug in an experiment looks like when it is
    // lucky enough to be obvious.
    e.skills
        .for_verb(&walk)
        .map(|s| {
            s.preconditions
                .iter()
                .filter(|p| p.is_well_supported())
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

#[derive(Clone, Debug)]
pub struct FleetResult {
    pub episodes: usize,
    pub fleet_size: usize,
    pub careers: Vec<Career>,
    /// what each deployment's veteran had to sell
    pub veterans: Vec<(Deployment, Vec<(String, u8, u16)>)>,
}

impl FleetResult {
    pub fn arm(&self, arm: Arm) -> Vec<&Career> {
        self.careers.iter().filter(|c| c.arm == arm).collect()
    }

    /// Mean judgement across the fleet, in one slice, for one arm.
    pub fn mean_judgement(&self, arm: Arm, slice: usize) -> u8 {
        let cs = self.arm(arm);
        let vals: Vec<u32> = cs
            .iter()
            .filter_map(|c| c.slices.get(slice))
            .map(|s| s.judgement_pct() as u32)
            .collect();
        if vals.is_empty() {
            return 0;
        }
        (vals.iter().sum::<u32>() / vals.len() as u32) as u8
    }

    pub fn mean_missed(&self, arm: Arm, slice: usize) -> u8 {
        let cs = self.arm(arm);
        let vals: Vec<u32> = cs
            .iter()
            .filter_map(|c| c.slices.get(slice))
            .map(|s| s.missed_pct() as u32)
            .collect();
        if vals.is_empty() {
            return 0;
        }
        (vals.iter().sum::<u32>() / vals.len() as u32) as u8
    }

    pub fn mean_probes(&self, arm: Arm) -> u32 {
        let cs = self.arm(arm);
        if cs.is_empty() {
            return 0;
        }
        let total: u32 = cs
            .iter()
            .map(|c| c.slices.iter().map(|s| s.probed).sum::<u32>())
            .sum();
        total / cs.len() as u32
    }

    pub fn slices(&self) -> usize {
        self.careers.first().map(|c| c.slices.len()).unwrap_or(0)
    }

    /// The first slice in which `arm` is doing at least as well as the
    /// baseline *is doing at the time*. This is the honest convergence
    /// number: it asks when the handicap stops mattering, rather than
    /// when the handicapped entity catches a moving target's finish
    /// line.
    pub fn converges_with_alone_at(&self, arm: Arm) -> Option<usize> {
        (0..self.slices()).find(|&i| self.mean_judgement(arm, i) >= self.mean_judgement(Arm::Alone, i))
    }

    /// The first slice in which `arm` reaches `alone`'s final level.
    /// `None` if it never does — which for the blind arm would mean a
    /// bad skill is permanent, and is the finding that would matter most.
    pub fn catches_up_at(&self, arm: Arm) -> Option<usize> {
        let target = self.mean_judgement(Arm::Alone, self.slices().saturating_sub(1));
        (0..self.slices()).find(|&i| self.mean_judgement(arm, i) >= target)
    }
}

/// The whole thing.
pub fn fleet(per_deployment: usize, episodes: usize, slices: usize) -> FleetResult {
    let veteran_episodes = 600;

    let experience: Vec<(Deployment, Vec<Precondition>)> = Deployment::ALL
        .iter()
        .map(|d| {
            (
                *d,
                veteran(*d, veteran_episodes, &[b"veteran-", d.name().as_bytes()].concat()),
            )
        })
        .collect();

    let mut careers = Vec::new();
    let mut pick = Drbg::deterministic(b"fleet-pairings");

    for (di, deployment) in Deployment::ALL.iter().enumerate() {
        for n in 0..per_deployment {
            let seed = alloc_seed(di, n);
            for arm in Arm::ALL {
                let adopted: Option<(&[Precondition], Deployment)> = match arm {
                    Arm::Alone | Arm::Oracle => None,
                    Arm::Matched => experience
                        .iter()
                        .find(|(d, _)| d == deployment)
                        .map(|(d, p)| (p.as_slice(), *d)),
                    Arm::Blind => {
                        // a marketplace that does not know where a skill
                        // came from: pick a seller at random
                        let k = pick.below(Deployment::ALL.len() as u64) as usize;
                        experience.get(k).map(|(d, p)| (p.as_slice(), *d))
                    }
                };
                careers.push(career(*deployment, arm, adopted, episodes, slices, &seed));
            }
        }
    }

    FleetResult {
        episodes,
        fleet_size: per_deployment * Deployment::ALL.len(),
        careers,
        veterans: experience
            .into_iter()
            .map(|(d, ps)| {
                (
                    d,
                    ps.into_iter()
                        .filter(|p| p.is_well_supported())
                        .map(|p| (p.tag.clone(), p.strength(), p.tests))
                        .collect(),
                )
            })
            .collect(),
    }
}

fn alloc_seed(deployment: usize, n: usize) -> Vec<u8> {
    let mut s = Vec::from(b"entity-".as_slice());
    s.push(b'0' + deployment as u8);
    s.push(b'-');
    s.extend_from_slice(&(n as u32).to_le_bytes());
    s
}

