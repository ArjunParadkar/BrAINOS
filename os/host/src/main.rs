//! `brainos` — run it and see.
//!
//! Four subcommands, each of which produces an artifact rather than a
//! claim:
//!
//!   demo    the Phase 0 demo in simulation, end to end
//!   verify  the Stage 6 property report over the whole bounded domain
//!   wire    the transport under a deliberately bad link
//!   stat    what this build actually contains

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(|s| s.as_str()).unwrap_or("demo");

    match cmd {
        "demo" => demo(),
        "verify" => verify(),
        "wire" => wire(),
        "stack" => stack(),
        "bench" => bench(),
        "compound" => compound(),
        "stat" => stat(),
        "help" | "-h" | "--help" => {
            usage();
            ExitCode::SUCCESS
        }
        other => {
            eprintln!("brainos: unknown command `{other}`\n");
            usage();
            ExitCode::FAILURE
        }
    }
}

fn usage() {
    println!(
        "brainos — the identity, memory and policy layer for embodied AI

usage: brainos <command>

  demo     the Phase 0 demo in simulation: two bodies, one memory,
           one gate, end to end
  verify   the Stage 6 property report: every property over the whole
           bounded domain. Run this with --release; it is ~26,000
           ed25519 operations and debug builds take minutes.
  wire     the transport carrying commands over a deliberately bad link
  stack    the whole path: entity → gate → wire → body → the real gait,
           including a subverted seat that the body refuses anyway
  bench    measured latencies and memory footprint, against the budgets
           the architecture's tiers imply
  compound the experiment behind the moat claim: does an entity with
           experience make better decisions than one without?
  stat     what this build contains
"
    );
}

fn rule(title: &str) {
    println!("\n{title}");
    println!("{}", "─".repeat(title.chars().count().max(20)));
}

fn demo() -> ExitCode {
    rule("BrAInOS — Phase 0, in simulation");
    let r = brainos_host::demo::run();
    for line in &r.lines {
        println!("  [{:<10}] {}", line.step, line.text);
    }

    rule("Result");
    println!("  entity        {}", r.entity_id);
    println!("  bodies        {}", r.bodies);
    println!("  memories      {}", r.memories);
    println!("  skills        {}", r.skills);
    println!("  gated         {} granted, {} refused", r.granted, r.denied);
    println!("  audit root    {}", r.audit_root);
    println!("  export        {} bytes", r.archive.len());
    println!(
        "  continuity    {}",
        if r.memory_survived {
            "memory outlived the process and the bodies"
        } else {
            "FAILED"
        }
    );

    println!(
        "\n  What this does not show: nothing here has touched a motor.
  Mini-Blur is simulated. Everything between the human and the servo
  is done; the servo is Phase 0's job.\n"
    );

    if r.memory_survived {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn verify() -> ExitCode {
    rule("BrAInOS — KIRA property verification");
    let report = brainos_proof::report::Report::full();
    println!("{}", report.render());
    if report.holds() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn wire() -> ExitCode {
    rule("BrAInOS — the wire, under load");
    for line in brainos_host::demo::wire_demo() {
        println!("  [{:<10}] {}", line.step, line.text);
    }
    println!();
    ExitCode::SUCCESS
}

fn stack() -> ExitCode {
    rule("BrAInOS — the whole stack");
    for line in brainos_host::fullstack::run() {
        println!("  [{:<10}] {}", line.step, line.text);
    }

    rule("One self, two bodies");
    for line in brainos_host::fullstack::two_bodies_demo() {
        println!("  [{:<10}] {}", line.step, line.text);
    }

    rule("A body we did not build");
    for line in brainos_host::fullstack::manufacturer_demo() {
        println!("  [{:<10}] {}", line.step, line.text);
    }
    println!(
        "\n  Everything in that path is the shipping code: the gait is the one the
  ESP32-S3 firmware plays, the tip-over check is the convex-hull support
  polygon, and the wire is authenticated and replay-protected. What is
  simulated is the servo.\n"
    );
    ExitCode::SUCCESS
}

fn compound() -> ExitCode {
    use brainos_host::compound::{experiment, Run};

    rule("BrAInOS — does experience compound?");
    println!(
        "  §2.4 says the moat becomes visible as a published curve: entity age
  against task success. This is the measurement apparatus for that curve,
  run against a simulated world — so it can say whether the mechanism
  recovers latent structure from experience, and cannot say that a real
  robot gets better. The world here has exactly the structure I put in it.\n"
    );

    let e = experiment(2_000, 8);

    let table = |r: &Run| {
        println!(
            "\n  {} · {} · {} episodes",
            r.world.name(),
            r.policy.name(),
            r.episodes
        );
        println!(
            "    {:>10} {:>10} {:>10} {:>10} {:>10} {:>9}",
            "episodes", "attempted", "succeeded", "declined", "judgement", "learned"
        );
        for w in &r.windows {
            println!(
                "    {:>10} {:>10} {:>10} {:>10} {:>9}% {:>9}",
                format!("{}-{}", w.from_episode, w.to_episode),
                w.attempted,
                w.succeeded,
                w.declined,
                w.judgement_pct(),
                w.binding_preconditions
            );
        }
    };

    table(&e.structured_control);
    table(&e.structured_learner);
    table(&e.structureless_control);
    table(&e.structureless_learner);

    rule("What it found");
    println!(
        "  In the structured world, the entity that consults what it learned ends
  with judgement {}% against the control's {}% — a difference of {} points.",
        e.structured_learner.late().judgement_pct(),
        e.structured_control.late().judgement_pct(),
        e.effect_pts()
    );
    println!(
        "\n  In the world with nothing to learn, the same comparison gives {} points.
  That is the number that decides whether the first one means anything:
  an entity that 'improves' where there is no structure is not learning,
  and the metric would be measuring something else.",
        e.null_effect_pts()
    );

    let (carpet, battery, loaded) = e.found_the_real_rule();
    println!(
        "\n  The world's rule was: walking works on carpet with a good battery, and
  ignores whether the body is loaded. The entity ended up believing:"
    );
    println!("    carpet matters      {}", yes_no(carpet));
    println!("    battery matters     {}", yes_no(battery));
    println!(
        "    loaded matters      {}   <- the world ignores this; believing it
                              would be a correlate rather than a cause",
        yes_no(loaded)
    );
    if !e.structured_learner.learned.is_empty() {
        println!("\n  Every precondition it ended up holding, with its strength:");
        for (tag, strength) in &e.structured_learner.learned {
            println!("    {:<16} {}%", tag, strength);
        }
    }

    rule("Verdict");
    if e.survives_the_control() {
        println!(
            "  The effect survives its control: {} points where there is structure,
  {} where there is not. The consolidation mechanism recovers latent
  structure from experience.",
            e.effect_pts(),
            e.null_effect_pts()
        );
    } else {
        println!(
            "  NULL. The effect is {} points where there is structure and {} where
  there is not, which does not clear the threshold set before running
  (at least 10 points, and at least three times the control). Reported
  as a null rather than reframed.",
            e.effect_pts(),
            e.null_effect_pts()
        );
    }
    println!(
        "\n  What this does not show: that a robot gets better. The world is
  simulated and its structure is one I wrote. What it shows is that the
  mechanism can find structure when there is some, and — the control —
  does not manufacture an improvement when there is none.\n"
    );

    ExitCode::SUCCESS
}

fn yes_no(b: bool) -> &'static str {
    if b {
        "yes"
    } else {
        "no"
    }
}

fn bench() -> ExitCode {
    rule("BrAInOS — measured");
    println!(
        "  Release build on this machine. These are NOT worst-case execution times
  on an ESP32-S3: an x86 core is one to two orders of magnitude faster
  than Xtensa at 240 MHz, so a number here is a lower bound on the board,
  not a prediction for it. What it is good for is the shape.\n"
    );

    println!(
        "  {:<52} {:>9} {:>9} {:>9} {:>6}  {}",
        "operation", "min", "median", "budget", "spread", "fits"
    );
    println!("  {}", "─".repeat(104));

    let mut all_fit = true;
    let mut noisy = 0;
    let measurements = brainos_host::bench::run();
    for m in &measurements {
        let fits = match m.fits() {
            None => "—".to_string(),
            Some(true) => format!("yes, {}% of budget", m.headroom_pct().unwrap_or(0)),
            Some(false) => {
                all_fit = false;
                "NO".to_string()
            }
        };
        if !m.measurement_is_clean() {
            noisy += 1;
        }
        println!(
            "  {:<52} {:>9} {:>9} {:>9} {:>5.1}x  {}",
            m.what,
            human_ns(m.min_ns),
            human_ns(m.median_ns),
            m.budget
                .map(|b| human_ns(b.as_micros() * 1_000))
                .unwrap_or_else(|| "—".into()),
            m.spread(),
            fits
        );
    }
    if noisy > 0 {
        println!(
            "\n  {noisy} of {} measurements have a p99 more than 3x their min, which means
  this machine was busy while they ran. The min is the number to believe;
  the spread is a reading about the machine, not about the code.",
            measurements.len()
        );
    }

    rule("What the numbers mean");
    for line in brainos_host::bench::implications(&measurements) {
        // wrap at a comfortable width
        let mut col = 0;
        print!("  ");
        for word in line.split_whitespace() {
            if col + word.len() > 74 {
                print!("\n  ");
                col = 0;
            }
            print!("{word} ");
            col += word.len() + 1;
        }
        println!("\n");
    }

    rule("What each one actually does");
    for m in &measurements {
        println!("  {}", m.what);
        println!("      {}", m.note);
    }

    rule("Footprint");
    for (what, bytes, note) in brainos_host::bench::footprint() {
        println!("  {:<28} {:>5} B   {}", what, bytes, note);
    }

    rule("Does it fit on the board?");
    println!(
        "  An ESP32-S3 has 512 KiB of SRAM. Allowing 320 KiB for the firmware, the
  gait loop's buffers and the wire, a state graph has about 192 KiB.\n"
    );
    for (what, needs, available, fits) in brainos_host::bench::fits_on_the_board() {
        println!(
            "  {:<36} needs {:>4} KiB of {:>4} KiB   {}",
            what,
            needs / 1024,
            available / 1024,
            if fits { "fits" } else { "DOES NOT FIT" }
        );
    }
    println!(
        "\n  This is why Bounds::EMBEDDED exists and is not a smaller number chosen
  for tidiness. An entity on the board remembers less and consolidates
  more often; the same entity on a workstation body remembers more. It is
  the same entity either way — the bound is a property of the body it is
  currently thinking in, not of who it is.\n"
    );

    let (empty, full, nodes) = brainos_host::bench::memory_per_episode();
    let per = if nodes > 1 {
        (full - empty) / (nodes - 1).max(1)
    } else {
        0
    };
    println!();
    println!(
        "  a state graph with {nodes} memories encodes to {full} bytes — about {per} B per memory,
  so an entity at its default episodic bound of 4,096 episodes is roughly
  {} KiB of durable state.",
        (per * 4096) / 1024
    );

    println!();
    if all_fit {
        ExitCode::SUCCESS
    } else {
        eprintln!("  something did not fit its budget.");
        ExitCode::FAILURE
    }
}

fn human_ns(ns: u64) -> String {
    if ns >= 1_000_000_000 {
        format!("{:.2}s", ns as f64 / 1e9)
    } else if ns >= 1_000_000 {
        format!("{:.2}ms", ns as f64 / 1e6)
    } else if ns >= 1_000 {
        format!("{:.1}us", ns as f64 / 1e3)
    } else {
        format!("{ns}ns")
    }
}

fn stat() -> ExitCode {
    use brainos_abi::error::{DenyCode, Stage};
    use brainos_crypto::domain::Tag;
    use brainos_proof::domain::Domain;
    use brainos_proof::properties::PROPERTIES;

    rule("BrAInOS — this build");
    println!("  abi version           {}", brainos_abi::version::ABI);
    println!("  export format         v{}", brainos_export::archive::FORMAT_VERSION);
    println!("  gate stages           {}", Stage::ALL.len());
    println!("  refusal codes         {}", DenyCode::ALL.len());
    println!("  domain separation tags {}", Tag::ALL.len());
    println!("  checked properties    {}", PROPERTIES.len());
    println!("  proof domain size     {} cases", Domain::SIZE);

    rule("The eight stages");
    for s in Stage::ALL {
        println!("  {}  {}", s as u8, s.name());
    }

    rule("The Level-0 drives");
    for d in brainos_kira::rules::level_zero() {
        println!("  {:<24} {}", d.forbids.to_string(), d.because);
    }

    rule("The properties");
    for p in PROPERTIES {
        println!("  {:<4} {}", p.id, p.statement);
    }
    println!();
    ExitCode::SUCCESS
}
