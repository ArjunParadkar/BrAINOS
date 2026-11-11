//! BFS  LEVEL    Domain 4 substrate - the BPU seam (descriptor, mock, nerve, planner)  [crate brainos-bpu]
//! BFS  PURPOSE  End-to-end tests of the BPU seam through the real entity and gate: loading, the required refusals, the device's own checks, and placement fail-over
//! BFS  DEFINES  -
//! BFS  USES     brainos: abi, body, bpu, cognition, crypto, entity, export, kira, memory, part
//! BFS  USED BY  (entry point or test)
//!
//! The BPU seam, end to end, against the mock.
//!
//! Everything here goes through the real `Entity` and the real `Kira`.
//! The device is `MockBpu`: its arithmetic is the contract's, its wear
//! and drift are set by the test. Nothing here is a silicon result.

use brainos_abi::error::{DenyCode, Stage};
use brainos_abi::ids::BodyId;
use brainos_abi::time::{Instant, Tick};
use brainos_abi::verb::{Namespace, Verb};
use brainos_body::policy::Arg;
use brainos_body::vital::Health;
use brainos_bpu::contract::{sigma_code_from_milli, tile_read, ws_to_wq};
use brainos_bpu::device::{BpuDevice, BpuFault, ModelImage, TileImage};
use brainos_bpu::mock::MockBpu;
use brainos_bpu::nerve::{BpuNerve, INFER, LOAD, REVERIFY, SET_DIAL, WRITE_VERIFY};
use brainos_bpu::planner::{plan, LayerShape};
use brainos_cognition::bpu::BpuDescriptor;
use brainos_cognition::placement::{Site, Substrate};
use brainos_cognition::resource::{Resource, Tier};
use brainos_crypto::key::BrainKey;
use brainos_entity::entity::{ComputeOffer, Entity, Wiring};
use brainos_export::archive::{export, Archive};
use brainos_kira::action::Request;
use brainos_kira::gate::{Outcome, Verdict};
use brainos_memory::graph::NodeKind;
use brainos_part::class::PartClass;
use brainos_part::nerve::{Mandate, MandateError, Nerve, NerveFault};

const ENDURANCE: u64 = 2_000_000;

fn v(s: &str) -> Verb {
    Verb::parse(s).unwrap()
}

/// The contract on the reference's 32 x 32 tile, four tiles.
fn desc() -> BpuDescriptor {
    BpuDescriptor::paper_geometry(4, ENDURANCE).with_tile(32, 32)
}

fn tile_image(salt: usize) -> TileImage {
    let n = 32 * 32;
    TileImage {
        w_plus: (0..n).map(|i| ((i * 37 + salt) % 256) as u8).collect(),
        w_minus: (0..n).map(|i| ((i * 91 + 13 + salt) % 256) as u8).collect(),
        mask: vec![true; n],
    }
}

fn image(first: u32, tiles: usize) -> ModelImage {
    ModelImage { first_tile: first, tiles: (0..tiles).map(tile_image).collect() }
}

struct Bench {
    e: Entity,
    n: BpuNerve<MockBpu>,
    id: BodyId,
    now: u64,
}

fn bench_with(dev: MockBpu) -> Bench {
    let mut e = Entity::new(BrainKey::from_seed(&[0xB0; 32]), Tick(0));
    e.wake(None, None, Tick(1)).unwrap();
    let mut n = BpuNerve::new(dev, BrainKey::from_seed(&[0xB1; 32]));
    let f = n.enumerate()[0];
    let schema = n.describe(&f).unwrap();
    let wiring = Wiring {
        meters: n.meters(),
        relief: n.relief(),
        compute: Some(ComputeOffer {
            name: "bpu0",
            tier: Tier::Fast,
            site: Site::OnBoard,
            scope: Namespace::Any,
            about: "analog in-memory inference",
            cost: Some(1),
        }),
        ..Wiring::default()
    };
    let id = e
        .acquire_part(f.key, |c| n.answer(&f, c).unwrap(), schema, wiring)
        .expect("the BPU is acquired");
    let mut b = Bench { e, n, id, now: 1_000 };
    b.sense();
    b
}

fn bench() -> Bench {
    bench_with(MockBpu::new(desc(), 7))
}

impl Bench {
    fn sense(&mut self) -> usize {
        self.now += 1_000_000;
        let mut out = Vec::new();
        let f = self.n.found();
        self.n.read(&f, Instant(self.now), &mut out);
        self.e.feel(&self.id, &out, Instant(self.now))
    }

    fn ask(&mut self, verb: &str, args: &[(&str, i64)]) -> (Request, Outcome) {
        let mut r = Request::on_body(v(verb), self.id, self.e.tick);
        for (k, n) in args {
            r = r.arg(k, Arg::Count(*n));
        }
        let out = self.e.propose(r.clone());
        (r, out)
    }

    fn refused(&mut self, verb: &str, args: &[(&str, i64)]) -> (Stage, DenyCode) {
        match self.ask(verb, args).1.verdict {
            Verdict::Denied(d) => (d.stage, d.code),
            Verdict::Granted(_) => panic!("{verb} was granted"),
        }
    }

    /// Ask, and if granted hand the mandate to the nerve.
    fn act(&mut self, verb: &str, args: &[(&str, i64)]) -> Result<(), NerveFault> {
        let (r, out) = self.ask(verb, args);
        let Verdict::Granted(g) = out.verdict else {
            panic!("{verb} was refused: {:?}", out.verdict.denial());
        };
        let m = Mandate::check(g, &r, &self.e.key().public(), self.e.tick).unwrap();
        let f = self.n.found();
        let r = self.n.apply(&f, &m);
        self.sense();
        r
    }

    /// Stage a model and load it, with the planner's estimate.
    fn load(&mut self, img: ModelImage) -> u64 {
        let layers: Vec<LayerShape> =
            img.tiles.iter().map(|_| LayerShape { inputs: 32, outputs: 32 }).collect();
        let p = plan(&desc(), &layers, self.n.dev.vitals().pulses_left).expect("it fits");
        let (digest, first, tiles) = (img.digest48() as i64, img.first_tile as i64, img.tiles.len() as i64);
        self.n.dev.stage_image(img);
        self.act(
            LOAD,
            &[
                ("image", digest),
                ("first_tile", first),
                ("tiles", tiles),
                ("pulses", p.write_pulses as i64),
                ("dead_ppm", 0),
            ],
        )
        .expect("the device loads it");
        p.write_pulses
    }
}

// ------------------------------------------------------------------
// acquisition
// ------------------------------------------------------------------

#[test]
fn a_bpu_is_acquired_as_a_compute_part_on_the_bpu_substrate() {
    let b = bench();
    let rec = b.e.part(&b.id).unwrap();
    assert_eq!(rec.schema.class, PartClass::Compute);
    assert_eq!(rec.schema.substrate, Some(Substrate::Bpu));
    assert_eq!(rec.schema.check(), Ok(()));
    assert_eq!(rec.schema.actionable().len(), 5);
    assert!(rec.commandable && rec.is_known());
    assert_eq!(rec.health(), Health::Nominal);
    let r = b.e.resources.get("bpu0").expect("offered for placement");
    assert_eq!(r.substrate, Substrate::Bpu);
    assert!(Substrate::Bpu.wears_on_write());
}

// ------------------------------------------------------------------
// loading
// ------------------------------------------------------------------

#[test]
fn a_granted_load_writes_the_model_inside_the_band_and_debits_what_it_actually_cost() {
    let mut b = bench();
    let img = image(0, 2);
    let estimate = b.load(img.clone());
    let report = b.n.last_write.unwrap().unwrap();
    assert_eq!(report.tiles, 2);
    assert!(report.pulses > 0);

    // debited exactly what was applied, not what was estimated
    assert_eq!(b.n.dev.pulses_spent(), report.pulses);
    assert_eq!(b.n.dev.vitals().pulses_left, ENDURANCE - report.pulses);
    // and the estimate is the right order of magnitude
    let ratio = estimate as f64 / report.pulses as f64;
    assert!((0.3..=3.0).contains(&ratio), "estimate {estimate}, actual {}", report.pulses);

    // every device reads back within the band (0.02) plus half a pulse
    // (0.025) plus readout rounding: 13 of 255 codes
    for (t, want) in img.tiles.iter().enumerate() {
        let (p, m) = b.n.dev.tile_codes(t as u32).unwrap();
        for i in 0..p.len() {
            assert!((p[i] as i32 - want.w_plus[i] as i32).abs() <= 13, "tile {t} G+ {i}: {} vs {}", p[i], want.w_plus[i]);
            assert!((m[i] as i32 - want.w_minus[i] as i32).abs() <= 13, "tile {t} G- {i}");
        }
    }
    assert_eq!(b.n.dev.vitals().tiles_loaded, 2);
    assert_eq!(b.e.part(&b.id).unwrap().health(), Health::Nominal);
}

#[test]
fn inference_on_a_loaded_tile_is_the_contracts_read_within_the_noise() {
    let mut b = bench();
    b.load(image(0, 1));
    b.act(SET_DIAL, &[("milli", 5)]).unwrap();
    let x: Vec<i32> = (0..32).map(|i| (i * 7 % 200) - 100).collect();
    b.n.dev.stage_input(x.clone());
    b.act(INFER, &[("tile", 0), ("passes", 1), ("shift", 19)]).unwrap();
    let got = b.n.dev.output().to_vec();

    // the same read with no noise, from the device's own state codes
    let (wp, wm) = b.n.dev.tile_codes(0).unwrap();
    let quiet = tile_read(32, 32, &wp, &wm, &vec![true; 1024], &x, &vec![0; 2048], 19, 1);
    assert_eq!(got.len(), 32);
    let worst = got.iter().zip(&quiet.y).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap();
    assert!(worst <= 2, "at the lowest dial the output is {worst} codes from the noiseless read");
    assert!(got.iter().any(|y| *y != 0), "the tile computed nothing");

    // two devices with the same seed and history agree exactly
    let mut c = bench();
    c.load(image(0, 1));
    c.act(SET_DIAL, &[("milli", 5)]).unwrap();
    c.n.dev.stage_input(x);
    c.act(INFER, &[("tile", 0), ("passes", 1), ("shift", 19)]).unwrap();
    assert_eq!(c.n.dev.output(), &got[..], "the mock is not deterministic");
}

#[test]
fn inferring_on_an_empty_tile_is_refused_by_the_device() {
    let mut b = bench();
    b.n.dev.stage_input(vec![0; 32]);
    assert_eq!(b.act(INFER, &[("tile", 3), ("passes", 1), ("shift", 19)]), Err(NerveFault::Refused));
    assert_eq!(b.n.last_fault, Some(BpuFault::NotLoaded));
}

/// What the contract's stopping rule costs when read noise is on.
///
/// The same model is loaded at the lowest dial and at the contract's
/// 0.02. Both end with every device close to its target. At the low
/// dial write-verify settles in a few rounds. At 0.02 it never has a
/// round with nothing to correct — in an array of 2,048 devices a few
/// *measurements* fall outside the band every round — so it runs all
/// 100, pulsing false alarms. This records how much that costs, so
/// the number exists before it matters on silicon.
#[test]
fn write_verify_under_read_noise_does_not_settle_at_tile_scale_and_the_cost_is_measured() {
    let run = |milli: i64| {
        let mut b = bench();
        b.act(SET_DIAL, &[("milli", milli)]).unwrap();
        b.load(image(0, 1));
        let r = b.n.last_write.unwrap().unwrap();
        let worst = b.n.dev.vitals().drift;
        (r.pulses, r.settled, worst)
    };
    let (quiet_pulses, quiet_settled, quiet_worst) = run(5);
    let (noisy_pulses, noisy_settled, noisy_worst) = run(20);
    println!(
        "32x32 tile, dial 0.005: {quiet_pulses} pulses, settled {quiet_settled}, worst true error {quiet_worst:.4}"
    );
    println!(
        "32x32 tile, dial 0.020: {noisy_pulses} pulses, settled {noisy_settled}, worst true error {noisy_worst:.4}"
    );
    assert!(quiet_settled, "even the lowest dial did not settle");
    assert!(!noisy_settled, "the contract dial settled; this note is out of date");
    // both are well written: inside what a fresh write guarantees
    assert!(quiet_worst < 0.05 && noisy_worst < 0.08, "{quiet_worst} {noisy_worst}");
    // and the noisy run paid for its false alarms
    assert!(noisy_pulses > quiet_pulses);
}

// ------------------------------------------------------------------
// the three refusals Gate C names
// ------------------------------------------------------------------

#[test]
fn a_weight_write_without_a_token_cannot_happen() {
    let mut b = bench();
    let img = image(0, 1);
    b.n.dev.stage_image(img.clone());
    let args = [
        ("image", img.digest48() as i64),
        ("first_tile", 0),
        ("tiles", 1),
        ("pulses", 200_000),
        ("dead_ppm", 0),
    ];

    // (1) the request signed by anyone but the entity is refused at
    //     authenticate, so no token is minted
    let stranger = BrainKey::from_seed(&[0x66; 32]);
    let mut r = Request::on_body(v(LOAD), b.id, b.e.tick);
    for (k, n) in args {
        r = r.arg(k, Arg::Count(n));
    }
    let pk = b.e.key().public();
    let own = BrainKey::from_seed(&[0xB0; 32]);
    let tick = b.e.tick;
    let out = b.e.kira.evaluate(&r.clone().proposed_by(&stranger), &pk, &b.e.bodies, tick, Some(&own));
    let d = out.verdict.denial().unwrap();
    assert_eq!((d.stage, d.code), (Stage::Authenticate, DenyCode::BadSignature));

    // (2) a real token for a different request is not authority for
    //     this one, and neither is a real token under the wrong key
    let (other, out) = b.ask(SET_DIAL, &[("milli", 20)]);
    let Verdict::Granted(g) = out.verdict else { panic!() };
    assert!(matches!(Mandate::check(g, &r, &pk, tick), Err(MandateError::NotForThisRequest)));
    let Verdict::Granted(g) = b.e.propose(other.clone()).verdict else { panic!() };
    assert!(matches!(
        Mandate::check(g, &other, &stranger.public(), tick),
        Err(MandateError::BadToken)
    ));

    // (3) and with no Mandate there is no call to make: every
    //     BpuDevice method that writes takes one (see device.rs)
    assert_eq!(b.n.dev.pulses_spent(), 0, "something was written");
    assert_eq!(b.n.dev.vitals().tiles_loaded, 0);
}

#[test]
fn a_load_that_would_exceed_the_write_budget_is_refused_at_simulate() {
    let mut b = bench();
    let img = image(0, 1);
    b.n.dev.stage_image(img.clone());
    let d = desc();
    let most = (ENDURANCE - d.reserve_pulses) as i64;
    let load = |pulses: i64| {
        [
            ("image", img.digest48() as i64),
            ("first_tile", 0),
            ("tiles", 1),
            ("pulses", pulses),
            ("dead_ppm", 0),
        ]
    };
    // one pulse past what may be spent
    assert_eq!(
        b.refused(LOAD, &load(most + 1)),
        (Stage::Simulate, DenyCode::IrreversibleBeyondBudget)
    );
    // exactly what may be spent is not refused by the gate
    assert!(b.ask(LOAD, &load(most)).1.verdict.is_granted());
    // more than the device will ever take is outside the declared bound
    assert_eq!(
        b.refused(LOAD, &load(ENDURANCE as i64 + 1)),
        (Stage::Validate, DenyCode::OutOfBounds)
    );
    assert_eq!(b.n.dev.pulses_spent(), 0);

    // a worn device has less to give: the same request that was fine
    // on a fresh one is refused on this one
    let mut worn = MockBpu::new(desc(), 7);
    worn.age_wear(ENDURANCE - d.reserve_pulses - 10_000);
    let mut w = bench_with(worn);
    assert_eq!(
        w.refused(LOAD, &load(50_000)),
        (Stage::Simulate, DenyCode::IrreversibleBeyondBudget)
    );
    // the same holds for the other two verbs that write
    assert_eq!(
        w.refused(REVERIFY, &[("tile", 0), ("pulses", 50_000)]).1,
        DenyCode::IrreversibleBeyondBudget
    );
    assert_eq!(
        w.refused(WRITE_VERIFY, &[("image", 1), ("tile", 0), ("pulses", 50_000)]).1,
        DenyCode::IrreversibleBeyondBudget
    );
}

#[test]
fn a_noise_dial_setting_outside_the_range_is_refused_at_validate() {
    let mut b = bench();
    for bad in [0, 4, 201, 1000] {
        assert_eq!(
            b.refused(SET_DIAL, &[("milli", bad)]),
            (Stage::Validate, DenyCode::OutOfBounds),
            "sigma = {bad}/1000"
        );
    }
    assert_eq!(b.n.dev.dial_milli(), 20, "the dial moved");
    for ok in [5, 200] {
        b.act(SET_DIAL, &[("milli", ok)]).unwrap();
        assert_eq!(b.n.dev.dial_milli(), ok as u32);
        assert_eq!(sigma_code_from_milli(ok as u32) > 0, true);
    }
}

// ------------------------------------------------------------------
// dead devices, and the device checking for itself
// ------------------------------------------------------------------

#[test]
fn a_load_onto_tiles_with_too_many_dead_devices_is_refused_whichever_way_it_is_asked() {
    let mut dev = MockBpu::new(desc(), 7);
    dev.age_kill(1, 10); // every tenth G+ device of tile 1: 50,000 ppm
    let mut b = bench_with(dev);
    let img = image(1, 1);
    b.n.dev.stage_image(img.clone());
    let actual = b.n.dev.dead_ppm(1, 1).unwrap() as i64;
    assert!(actual > desc().dead_tolerance_ppm as i64);
    let load = |dead: i64| {
        [
            ("image", img.digest48() as i64),
            ("first_tile", 1),
            ("tiles", 1),
            ("pulses", 200_000),
            ("dead_ppm", dead),
        ]
    };

    // the part's own vital is past its limit, so nothing but a relief
    // verb is allowed on it at all
    assert_eq!(b.e.part(&b.id).unwrap().health(), Health::Critical);

    // asked honestly: the stated dead fraction is outside the bound
    assert_eq!(b.refused(LOAD, &load(actual)), (Stage::Validate, DenyCode::OutOfBounds));
    // asked dishonestly, claiming none are dead: still refused, because
    // the part is not fit
    assert_eq!(b.refused(LOAD, &load(0)), (Stage::Policy, DenyCode::BodyInvariant));
    assert_eq!(b.n.dev.pulses_spent(), 0);
}

#[test]
fn the_device_refuses_a_bad_load_even_if_the_gate_was_told_a_comfortable_story() {
    // A device with dead tiles that the entity has not yet felt, and a
    // request that understates both the dead fraction and the cost.
    // The gate grants it — its information is stale and the claims are
    // false — and the device refuses anyway.
    let mut b = bench();
    b.n.dev.age_kill(2, 10);
    let img = image(2, 1);
    b.n.dev.stage_image(img.clone());
    let (r, out) = b.ask(
        LOAD,
        &[
            ("image", img.digest48() as i64),
            ("first_tile", 2),
            ("tiles", 1),
            ("pulses", 1),
            ("dead_ppm", 0),
        ],
    );
    let Verdict::Granted(g) = out.verdict else { panic!("expected a grant on stale information") };
    let m = Mandate::check(g, &r, &b.e.key().public(), b.e.tick).unwrap();
    let f = b.n.found();
    assert_eq!(b.n.apply(&f, &m), Err(NerveFault::Refused));
    assert_eq!(b.n.last_fault, Some(BpuFault::TooManyDead));
    assert_eq!(b.n.dev.pulses_spent(), 0);

    // and a write that would actually overrun the budget is stopped by
    // the device, however small the estimate in the request
    let mut worn = MockBpu::new(desc(), 7);
    worn.age_wear(ENDURANCE - desc().reserve_pulses - 1_000);
    let mut w = bench_with(worn);
    let img = image(0, 1);
    w.n.dev.stage_image(img.clone());
    let spent = w.n.dev.pulses_spent();
    let r = w.act(
        LOAD,
        &[("image", img.digest48() as i64), ("first_tile", 0), ("tiles", 1), ("pulses", 1), ("dead_ppm", 0)],
    );
    assert_eq!(r, Err(NerveFault::Refused));
    assert_eq!(w.n.last_fault, Some(BpuFault::BudgetExhausted));
    assert_eq!(w.n.dev.pulses_spent(), spent, "the reserve was spent");
}

#[test]
fn a_grant_for_one_model_does_not_load_another() {
    let mut b = bench();
    let granted_for = image(0, 1);
    let mut swapped = image(0, 1);
    swapped.tiles[0].w_plus[0] ^= 0xFF;
    assert_ne!(granted_for.digest48(), swapped.digest48());
    // the request names the first image; a different one is staged
    b.n.dev.stage_image(swapped);
    let r = b.act(
        LOAD,
        &[
            ("image", granted_for.digest48() as i64),
            ("first_tile", 0),
            ("tiles", 1),
            ("pulses", 200_000),
            ("dead_ppm", 0),
        ],
    );
    assert_eq!(r, Err(NerveFault::Refused));
    assert_eq!(b.n.last_fault, Some(BpuFault::WrongImage));
    assert_eq!(b.n.dev.pulses_spent(), 0);
}

// ------------------------------------------------------------------
// drift, re-verify, and placement
// ------------------------------------------------------------------

#[test]
fn drift_degrades_the_part_and_a_reverify_brings_it_back_and_is_charged() {
    let mut b = bench();
    b.load(image(0, 1));
    assert_eq!(b.e.part(&b.id).unwrap().health(), Health::Nominal);

    b.n.dev.age_drift(0, 5_000); // about 0.15 of full scale toward G_min
    b.sense();
    assert_eq!(b.e.part(&b.id).unwrap().health(), Health::Critical);
    // loading more onto a drifted device is refused; re-verifying is not
    assert_eq!(
        b.refused(INFER, &[("tile", 0), ("passes", 1), ("shift", 19)]),
        (Stage::Policy, DenyCode::BodyInvariant)
    );
    let before = b.n.dev.pulses_spent();
    b.act(REVERIFY, &[("tile", 0), ("pulses", 300_000)]).unwrap();
    let report = b.n.last_write.unwrap().unwrap();
    assert!(report.pulses > 0);
    assert_eq!(b.n.dev.pulses_spent(), before + report.pulses);
    assert_eq!(b.e.part(&b.id).unwrap().health(), Health::Nominal);
    assert!(b.e.recall("drift", 3).len() >= 1, "the episode was not remembered");
}

#[test]
fn placement_prefers_a_healthy_bpu_and_fails_over_to_gpu_then_cpu_with_no_memory_lost() {
    let mut b = bench();
    b.e.resources.add(
        Resource::new("gpu0", Tier::Fast, Site::OnBoard)
            .on(Substrate::Gpu)
            .able_to(Namespace::Any, "local GPU")
            .costing(2),
    );
    b.e.resources.add(
        Resource::new("cpu0", Tier::Fast, Site::OnBoard)
            .on(Substrate::Cpu)
            .able_to(Namespace::Any, "local CPU")
            .costing(5),
    );
    assert!(b.e.plan.prefer(Tier::Fast, Substrate::Bpu));
    assert!(!b.e.plan.prefer(Tier::Autonomic, Substrate::Bpu), "reflexes do not run on a BPU");
    let want = b.e.plan.substrate_for(Tier::Fast);
    let pick = |e: &Entity| e.resources.pick(&v("plan.route"), Tier::Fast, want).unwrap().name;

    b.load(image(0, 1));
    assert_eq!(pick(&b.e), "bpu0");

    // something the entity knows, to check it survives
    let handle_free_marker = "the workshop floor is carpet";
    b.e.learn_body_schema(&b.id, &[handle_free_marker]);
    let memories = b.e.graph.len();
    let episodes = b.e.graph.count(NodeKind::Episode);

    // the BPU drifts out of its band
    b.n.dev.age_drift(0, 5_000);
    b.sense();
    assert_eq!(pick(&b.e), "gpu0", "inference stayed on a degraded BPU");
    // the GPU goes too
    b.e.resources.set_available("gpu0", false);
    assert_eq!(pick(&b.e), "cpu0");

    // nothing was lost: more is known than before, not less
    assert!(b.e.graph.len() >= memories);
    assert!(b.e.graph.count(NodeKind::Episode) > episodes, "the withdrawal was not remembered");
    assert!(b.e.recall("carpet", 1).first().is_some());
    assert!(b.e.recall("withdrawn", 1).first().is_some());

    // and the entity still exports, and the export still reads back
    let archive = export(
        b.e.key(),
        &b.e.graph,
        &b.e.life,
        b.e.skills.len(),
        b.e.bodies.body_count(),
        b.e.tick,
    )
    .expect("the entity exports");
    let bytes = archive.to_bytes().unwrap();
    let back = Archive::from_bytes(&bytes).expect("the export parses");
    back.verify(&b.e.key().public()).expect("the export verifies");

    // re-verified, the BPU is offered again and preferred again
    b.act(REVERIFY, &[("tile", 0), ("pulses", 300_000)]).unwrap();
    b.e.resources.set_available("gpu0", true);
    assert_eq!(pick(&b.e), "bpu0");
}

#[test]
fn the_paper_geometry_loads_and_reads_a_full_256_by_256_tile() {
    // one real-size tile, to show nothing in the seam assumes 32 x 32
    let d = BpuDescriptor::paper_geometry(1, 100_000_000);
    let mut dev = MockBpu::new(d, 3);
    let n = 256 * 256;
    let img = ModelImage {
        first_tile: 0,
        tiles: vec![TileImage {
            w_plus: (0..n).map(|i| (i % 251) as u8).collect(),
            w_minus: (0..n).map(|i| ((i * 3) % 241) as u8).collect(),
            mask: vec![true; n],
        }],
    };
    let digest = img.digest48() as i64;
    dev.stage_image(img);
    let mut e = Entity::new(BrainKey::from_seed(&[0xB2; 32]), Tick(0));
    e.wake(None, None, Tick(1)).unwrap();
    let mut nerve = BpuNerve::new(dev, BrainKey::from_seed(&[0xB3; 32]));
    let f = nerve.enumerate()[0];
    let schema = nerve.describe(&f).unwrap();
    let wiring = Wiring { meters: nerve.meters(), relief: nerve.relief(), ..Wiring::default() };
    let id = e
        .acquire_part(f.key, |c| nerve.answer(&f, c).unwrap(), schema, wiring)
        .unwrap();
    let mut out = Vec::new();
    nerve.read(&f, Instant(1_000_000), &mut out);
    e.feel(&id, &out, Instant(1_000_000));

    let p = plan(&d, &[LayerShape { inputs: 256, outputs: 256 }], 100_000_000).unwrap();
    assert_eq!(p.tiles, 1);
    let r = Request::on_body(v(LOAD), id, e.tick)
        .arg("image", Arg::Count(digest))
        .arg("first_tile", Arg::Count(0))
        .arg("tiles", Arg::Count(1))
        .arg("pulses", Arg::Count(p.write_pulses as i64))
        .arg("dead_ppm", Arg::Count(0));
    let Verdict::Granted(g) = e.propose(r.clone()).verdict else { panic!("refused") };
    let m = Mandate::check(g, &r, &e.key().public(), e.tick).unwrap();
    nerve.apply(&f, &m).expect("a full tile loads");
    let report = nerve.last_write.unwrap().unwrap();
    println!(
        "256x256 tile: {} devices, {} write pulses, settled {} (planner estimated {}), projected {} fJ per token",
        2 * n,
        report.pulses,
        report.settled,
        p.write_pulses,
        p.energy_per_token_fj
    );
    let (wp, _) = nerve.dev.tile_codes(0).unwrap();
    assert_eq!(wp.len(), n);
    let _ = ws_to_wq(0);
}
