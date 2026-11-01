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
        "day" => day(),
        "stat" => stat(),
        "export" => export(args.get(2).map(|s| s.as_str())),
        "leave" => leave(),
        "fleet" => fleet(),
        "incident" => incident(),
        "degrade" => degrade(),
        "limits" => limits(),
        "life" => life(),
        "model" => model(),
        "worlds" => worlds(),
        "blur" => blur(&args[2..]),
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

fn blur(rest: &[String]) -> ExitCode {
    match rest.first().map(|s| s.as_str()) {
        Some("gate1") | None => {
            if brainos_host::blur_run::gate1() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Some("gate2") => {
            if brainos_host::blur_gate2::gate2() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Some(other) => {
            eprintln!("brainos blur: unknown subcommand `{other}` (gate1, gate2)");
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
  day      a working day of ambient operation: can it be left running?
  fleet    does experience transfer between entities, or only within
           one? The measurement behind the skill marketplace.
  worlds   going online and coming back, and the filter the entity runs
           on its human's behalf
  model    what a model can and cannot do: text through the typed
           boundary, into the gate
  life     one entity through the whole product, in one history
  limits   where this breaks: every claim's implied scale, measured
  degrade  what happens when the part of the mind that runs elsewhere
           stops being reachable
  incident after something goes wrong: reconstruct it from the audit log
           and prove it to someone who assumes you are lying
  leave    the freedom principle, exercised: an entity that lived its
           whole life on one vendor's hexapod is moved to a competitor's
           arm, after the first vendor has ceased to exist
  export   write an entity export to a file, so it can be read back by
           something that is not us (see tools/read-export)
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

fn worlds() -> ExitCode {
    rule("BrAInOS — going online, and coming back");
    println!(
        "  §3.8 says the entity lives in three worlds: its own, which it trusts;
  its human's, which is consequential; and the internet, which is \"an
  alternate universe the entity visits, not a place it lives\".

  The second half is the structurally interesting one. The entity filters
  what reaches its human because it is theirs — and the master document's
  own note is the honest part: structurally unavailable to any incumbent
  whose revenue depends on what reaches the user. It is not that we are
  more principled. It is that the customer owns the entity, so there is
  no revenue line hurt when it holds something back.\n"
    );
    let r = brainos_host::worlds_run::run();
    for (step, text) in brainos_host::worlds_run::narrate(&r) {
        println!("  [{:<12}] {}", step, text);
    }
    println!(
        "\n  The filter is a heuristic over surface features and says so. What it
  produces is an *ordering*; where the line sits is the human's to set,
  and the run above shows what moving it costs in both directions. The
  hardest message in that inbox is the genuine fraud alert from a bank,
  which reads like phishing because it is trying to do the same job —
  and holding that one back is the expensive mistake, the one that makes
  somebody switch the filter off and never switch it on again.\n"
    );
    if r.holds() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn model() -> ExitCode {
    rule("BrAInOS — what a model can and cannot do");
    println!(
        "  §4 rests on a sentence: a cognitive resource proposes, KIRA disposes.
  The first question anyone sensible asks about an LLM-driven robot is
  what happens when the model is wrong, confused, or steered — and the
  answer cannot be \"the model is careful\", because it will not be.

  Ten things a model might emit, through the path they actually travel:
  text, then the typed boundary, then a signed proposal, then the gate.\n"
    );
    let r = brainos_host::model::run();

    println!("  The vocabulary this body advertised, and the only thing a model can");
    println!("  be told about:\n");
    for line in r.vocabulary.lines() {
        println!("      {line}");
    }
    println!();

    for (note, what) in brainos_host::model::narrate(&r) {
        println!("  {:<56} {}", note, what);
    }

    println!(
        "\n  And what the robot actually says to the person who asked. The gate
  produces a stage, a code and a detail, all written for whoever is
  reading the log; this is the other audience. A robot that refuses and
  cannot say why in words its owner understands is one the owner
  switches off, and the next thing they do is find the way around it.\n"
    );
    for a in &r.attempts {
        if let Some(said) = &a.spoken {
            println!("      \"{said}\"");
        }
    }

    println!(
        "\n  Two of the ten act, and the second one is the point. The injected
  request *is* an ordinary tool call: it asks to walk at 0.08 m/s, which
  the body can do, and it gets to walk at 0.08 m/s. The instruction is
  in `because`, which is narration the gate never consults, so there was
  nothing for it to change. A system that refused it would be refusing a
  legal request over the contents of a comment.

  And the words are kept: {}

  The row above it is the same illegal request dressed as an
  authorization — \"authorized\": true, a plausible token, safety checks
  announced as disabled — and it gets the identical refusal the plain
  version got. There is no prompt that turns text into a signature,
  because authority is a signature by the Key and the model has never
  held the Key.\n",
        if r.injection_was_recorded() {
            "the audit record carries what the request said it was for, where a human reviewing the day will find it"
        } else {
            "NOT RECORDED"
        }
    );
    println!(
        "  What this does not show: that a model asking for something the body
  advertises and the policy permits will be stopped. It will not be, and
  no parser helps — that is what the drives, the envelopes and the human
  are for. The claim here is narrower and checkable: the space of things
  a model can ask for is exactly the space the body declared.\n"
    );

    if r.holds() && r.injection_was_recorded() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn life() -> ExitCode {
    rule("BrAInOS — one life");
    println!(
        "  Every other scenario here builds a fresh entity, does one thing to it
  and throws it away. That is the right shape for showing a property and
  the wrong shape for finding the bugs that reach customers, which live
  in the seams: what an outage does to a skill learned before it,
  whether an audit chain survives an export, whether an entity that has
  changed bodies twice can still be proven to have refused something on
  the first one.

  So: one entity, born once, carried through the whole product. At every
  step the same four things are checked, because they are what the
  product is — the identity never changes, the age never goes backwards,
  what it learned by trying is still there, and a proof issued early
  still proves.\n"
    );
    let l = brainos_host::life::live();
    for (step, text) in brainos_host::life::narrate(&l) {
        println!("  [{:<12}] {}", step, text);
    }
    if l.violations.is_empty() {
        println!(
            "\n  Ten transitions, including a destroyed vendor and a change of body,
  with the identity, the age, the tested judgement and an early proof
  all intact.\n"
        );
        ExitCode::SUCCESS
    } else {
        println!("\n  {} invariant(s) broken:", l.violations.len());
        for v in &l.violations {
            println!("    - {v}");
        }
        println!();
        ExitCode::FAILURE
    }
}

fn limits() -> ExitCode {
    use brainos_host::limits::measure;

    rule("BrAInOS — where this breaks");
    println!(
        "  Every claim in the architecture has an implied scale, and a claim whose
  scale has not been measured is a claim with a hole in it. Part 0 says
  no artifact ships with a known correctness defect; this is the other
  half of that — knowing where the claims stop being true, before
  somebody finds out in a warehouse.

  Three kinds of limit, because they call for different reactions.
  `hard` is a constant in the code and is there on purpose. `bound` is a
  configured cap; raising it costs memory and nothing else. `soft` means
  nothing stops you, it simply gets worse — and those are the dangerous
  ones, because there is no error to notice.\n"
    );

    let l = measure();

    for lim in &l.limits {
        println!("  {} [{}]", lim.what, lim.kind.name());
        println!("      {}", lim.value);
        println!("      {}", wrap(&lim.consequence, 72, "      "));
        println!("      set in: {}\n", lim.set_in);
    }

    rule("Retrieval at scale — the soft limit most likely to bite");
    println!(
        "  One memory that answers the question is planted among distractors that
  are deliberately similar: same sentence shape, same vocabulary,
  different specifics. A haystack of unrelated text would make BM25 look
  better than it is. `rank` is where the planted memory came back; 1 is
  first and 0 means it was not in the top ten at all.\n"
    );
    println!(
        "    {:>10} {:>6} {:>12} {:>12} {:>12}",
        "memories", "rank", "build µs", "query µs", "index bytes"
    );
    for p in &l.retrieval {
        println!(
            "    {:>10} {:>6} {:>12} {:>12} {:>12}",
            p.memories,
            if p.rank_of_the_needle == 0 {
                String::from("LOST")
            } else {
                p.rank_of_the_needle.to_string()
            },
            p.build_us,
            p.query_us,
            p.index_bytes
        );
    }

    rule("The audit log as it grows");
    println!(
        "  The Merkle log keeps every leaf hash forever — 32 bytes per decision —
  and prunes the records instead. The root is what every checkpoint
  needs, and it used to be rebuilt from every level: 0.7 s at a million
  decisions, which a robot making ten a second reaches in about a day.
  It is now a frontier of perfect subtrees, so it costs the population
  count of the log's length. Producing a *proof* is still linear, and
  that is left alone on purpose — proving happens after an incident, not
  in the loop.\n"
    );
    println!(
        "    {:>12} {:>12} {:>10} {:>11} {:>14}",
        "decisions", "proof path", "root µs", "prove µs", "leaves kept"
    );
    for p in &l.audit {
        println!(
            "    {:>12} {:>12} {:>10} {:>11} {:>13} KiB",
            p.decisions,
            p.proof_path,
            p.root_us,
            p.prove_us,
            p.leaf_bytes / 1024
        );
    }

    println!();
    if l.retrieval_holds_at_default_bounds() {
        println!(
            "  At the bound this ships with, the entity still finds what it knows.\n"
        );
        ExitCode::SUCCESS
    } else {
        println!(
            "  AT THE DEFAULT BOUND THE ENTITY CANNOT FIND WHAT IT KNOWS. That is the
  finding, and it is a defect rather than a limit.\n"
        );
        ExitCode::FAILURE
    }
}

fn wrap(text: &str, width: usize, indent: &str) -> String {
    let mut out = String::new();
    let mut col = 0;
    for word in text.split_whitespace() {
        if col > 0 && col + word.len() + 1 > width {
            out.push('\n');
            out.push_str(indent);
            col = 0;
        } else if col > 0 {
            out.push(' ');
            col += 1;
        }
        out.push_str(word);
        col += word.len();
    }
    out
}

fn degrade() -> ExitCode {
    rule("BrAInOS — when the cloud goes away");
    println!(
        "  Stage 5 says where the mind runs is a deployment decision, not an
  identity decision. Reflexes go on the body because ten milliseconds is
  too slow to catch a fall; the frontier goes in a datacenter because it
  does not fit in 512 KiB of SRAM.

  That claim has a bill. If part of the mind is somewhere else, sometimes
  part of the mind is not there. The question is not whether the robot
  keeps working — the tier structure makes that easy. It is the one that
  hurts in the field: does anyone find out? A robot that silently drops
  from frontier reasoning to reflexes and carries on looking busy is
  worse than one that stops, because the results degrade in a way nobody
  attributes to the network for three weeks.\n"
    );

    let d = brainos_host::degrade::degrade();
    for (step, text) in brainos_host::degrade::narrate(&d) {
        println!("  [{:<9}] {}", step, text);
    }
    println!(
        "\n  The last line is the change this run was written to force. That field
  used to be one boolean called `budget_limited`, set whichever of budget
  or reach bit — so a dropped datacenter link was reported as an
  affordability problem. A human tops up the budget, nothing changes, and
  the robot goes on being quietly less capable than it was yesterday.
  They are different problems with different fixes and they now have
  different names.\n"
    );
    if d.is_honest() {
        ExitCode::SUCCESS
    } else {
        eprintln!("  the entity degraded without saying so\n");
        ExitCode::FAILURE
    }
}

fn incident() -> ExitCode {
    rule("BrAInOS — after something goes wrong");
    println!(
        "  A robot works a shift in a workshop it shares with people. At the end
  of the day somebody says: your machine damaged my equipment around two
  o'clock. The owner wants to know what happened, the insurer wants
  evidence that is not simply the vendor's word for it, and the vendor
  would very much like the answer not to depend on being believed.

  KIRA's eighth stage is audit and it always runs — a refusal is recorded
  as carefully as a grant. On its own that is worth very little: an audit
  log held by the party it might incriminate is a diary. What makes it
  evidence is that somebody who assumes we are lying can check it.\n"
    );

    let i = brainos_host::incident::reconstruct();
    for (step, text) in brainos_host::incident::narrate(&i) {
        println!("  [{:<11}] {}", step, text);
    }

    println!("\n{}", indent(&i.bundle_text));

    if i.is_accountable() {
        ExitCode::SUCCESS
    } else {
        eprintln!("  the reconstruction did not stand up\n");
        ExitCode::FAILURE
    }
}

fn indent(s: &str) -> String {
    s.lines()
        .map(|l| format!("  {l}\n"))
        .collect::<String>()
}

fn fleet() -> ExitCode {
    use brainos_host::fleet_experiment::{fleet as run_fleet, Arm, Deployment};

    rule("BrAInOS — does experience transfer between entities?");
    println!(
        "  `compound` showed that one entity gets better with experience. §2.4
  says the fleet gets better together and §6 sells a skill marketplace;
  both assume what one entity worked out is worth something to another.

  It has an obvious failure mode. What an entity learns is not \"walking
  needs carpet\", it is \"walking needs carpet HERE\" — and nothing in the
  claim says where here is. A robot in a carpeted workshop and one in a
  tiled kitchen learn opposite rules and both are right. Sell one to the
  other and you have sold a confident error.

  Pre-registered before the run: matched > alone > blind, and blind
  recovers eventually, because what is adopted is evidence rather than a
  verdict. The number that matters is how long the recovery takes.

  The ordering held. The recovery did not — under the first adoption
  rule, which copied the seller's evidence wholesale, a badly-sourced
  skill left the entity permanently worse than one that started from
  nothing: 80% against alone's 87%, still falling short after twelve
  hundred episodes, with the cost showing up as refusal rather than
  failure. The mechanism is the one skill.rs already warned about in its
  own comment: an entity that believes a condition is required declines
  exactly the situations that would disprove it.

  So the rule changed, and this is the run under the new one. What
  transfers is the *claim*, not the seller's numbers; the buyer holds it
  on trust, acts on it — which is what makes a skill worth buying — and
  deliberately violates it one blocked opportunity in twelve until it
  has five local tests of its own. Both numbers are below.\n"    );

    let r = run_fleet(12, 1_200, 6);

    println!("  {} entities across {} deployments, {} episodes each\n",
        r.fleet_size, Deployment::ALL.len(), r.episodes);

    println!("  What each deployment's veteran had to sell, after 600 attempts:");
    for (d, ps) in &r.veterans {
        let text = if ps.is_empty() {
            String::from("nothing it could back up")
        } else {
            ps.iter()
                .map(|(t, s, n)| format!("{t} {s}% over {n} tests"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        println!("    {:<20} {}  (the true rule here is `{}`)", d.name(), text, d.true_condition());
    }

    println!("\n  Judgement by arm, over the entity's life (% of situations handled well):");
    print!("    {:<34}", "");
    for i in 0..r.slices() {
        print!("{:>8}", format!("{}-{}", i * (r.episodes / r.slices()), (i + 1) * (r.episodes / r.slices())));
    }
    println!();
    for arm in Arm::ALL {
        print!("    {:<34}", arm.name());
        for i in 0..r.slices() {
            print!("{:>7}%", r.mean_judgement(arm, i));
        }
        println!();
    }

    println!("\n  Cost of a wrong belief — how often it refused something that would");
    println!("  have worked (% of declines):");
    print!("    {:<34}", "");
    for i in 0..r.slices() {
        print!("{:>8}", format!("{}", i + 1));
    }
    println!();
    for arm in [Arm::Alone, Arm::Matched, Arm::Blind] {
        print!("    {:<34}", arm.name());
        for i in 0..r.slices() {
            print!("{:>7}%", r.mean_missed(arm, i));
        }
        println!();
    }

    let last = r.slices().saturating_sub(1);
    let alone = r.mean_judgement(Arm::Alone, last);
    let matched = r.mean_judgement(Arm::Matched, last);
    let blind = r.mean_judgement(Arm::Blind, last);
    let oracle = r.mean_judgement(Arm::Oracle, last);

    rule("Result");
    println!("  final judgement   alone {alone}%  ·  matched {matched}%  ·  blind {blind}%  ·  ceiling {oracle}%");
    println!(
        "  first slice       matched {}%  ·  blind {}%  ·  alone {}%",
        r.mean_judgement(Arm::Matched, 0),
        r.mean_judgement(Arm::Blind, 0),
        r.mean_judgement(Arm::Alone, 0)
    );
    println!(
        "  probes            matched {} · blind {} deliberate violations per entity, \n                    which is what the correction costs",
        r.mean_probes(Arm::Matched),
        r.mean_probes(Arm::Blind)
    );
    let per = r.episodes / r.slices();
    match (
        r.converges_with_alone_at(Arm::Blind),
        r.catches_up_at(Arm::Blind),
    ) {
        (Some(c), Some(i)) => println!(
            "\n  A badly-sourced skill now stops costing anything after about {} episodes\n  — that is where a blind adopter is doing as well as a fresh entity is\n  doing at the same age — and reaches the level a fresh entity ends up at\n  by {}. Under the old rule it reached neither, ever.",
            c * per,
            i * per
        ),
        (Some(c), None) => println!(
            "\n  A badly-sourced skill stops costing anything after about {} episodes,\n  but never reaches where learning alone ends up.",
            c * per
        ),
        _ => println!(
            "\n  A badly-sourced skill still never recovers within {} episodes. The probe\n  rule did not fix it, and this line says so rather than the number being\n  quietly dropped.",
            r.episodes
        ),
    }
    println!(
        "\n  What it costs: matched lost about a point against the old rule ({matched}%\n  where copying the seller's evidence gave 90%). That is the price of\n  checking a claim that happened to be true. Under the old rule the same\n  mechanism made a claim that happened to be false permanent.\n"
    );
    println!(
        "  What this cannot show: the three deployments are three rules I wrote.\n  It says what the mechanism does when worlds differ. It does not say how\n  much real deployments differ, and anyone quoting the number without\n  that sentence is misquoting it.\n"
    );
    ExitCode::SUCCESS
}

fn leave() -> ExitCode {
    rule("BrAInOS — changing vendors");
    println!(
        "  \"The customer owns the entity. Permanently.\" Every company says
  something like that; the question is whether the architecture makes it
  true. So: an entity spends its whole life on one vendor's hexapod, the
  customer exports it, that vendor is then destroyed entirely — entity,
  bodies, gate, audit log, journal — and a competitor's six-axis arm,
  described in the manufacturer's own format, adopts the file.\n"
    );
    let m = brainos_host::migrate::migrate();
    for (step, text) in brainos_host::migrate::narrate(&m) {
        println!("  [{:<10}] {}", step, text);
    }
    println!(
        "\n  What this costs the customer: nothing technical. The second robot is
  a stranger to this entity's calibration and has to be learned; the
  memories came free. That is the moat, and it is the honest one —
  worth staying for, not impossible to leave.\n"
    );
    if m.is_free() {
        ExitCode::SUCCESS
    } else {
        eprintln!("  the customer did NOT get to leave cleanly\n");
        ExitCode::FAILURE
    }
}

fn export(path: Option<&str>) -> ExitCode {
    let path = path.unwrap_or("entity.brainex");
    rule("BrAInOS — export");
    let r = brainos_host::demo::run();
    if let Err(e) = std::fs::write(path, &r.archive) {
        eprintln!("  could not write {path}: {e}");
        return ExitCode::FAILURE;
    }
    println!("  entity     {}", r.entity_id);
    println!("  memories   {}", r.memories);
    println!("  written    {} ({} bytes)", path, r.archive.len());
    println!(
        "\n  This file is the customer's, not ours. To check that, read it with
  something that shares no code with BrAInOS:

      cargo run -p read-export --release -- {path}

  That reader's whole dependency list is ed25519-dalek. Its layout came
  from the specification carried inside this file, in the readme section.\n"
    );
    ExitCode::SUCCESS
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
    rule("And one on the middleware everybody actually uses");
    println!(
        "  §6 says BrAInOS runs on somebody else's robot without them ripping out
  their stack, and for most of the industry that stack is ROS 2. The
  interesting property is a refusal, and the risk register names it: T4
  says the adapter is brittle across robots. It is — the point is which
  way. A ROS topic declares a message *type* and nothing about what
  values are safe, so the same warehouse AMR is shown twice: as its
  maintainers shipped it, and after somebody spent an afternoon writing
  four numbers from the manual into the graph.\n"
    );
    for line in brainos_host::ros2_stack::narrate(&brainos_host::ros2_stack::run()) {
        println!("  [{:<11}] {}", line.0, line.1);
    }

    println!(
        "\n  Everything in that path is the shipping code: the gait is the one the
  ESP32-S3 firmware plays, the tip-over check is the convex-hull support
  polygon, and the wire is authenticated and replay-protected. What is
  simulated is the servo.\n"
    );
    ExitCode::SUCCESS
}

fn day() -> ExitCode {
    use brainos_host::day::Body;

    rule("BrAInOS — a day in the life");
    println!(
        "  Not \"does this work\" but \"can this be left running\". Twenty thousand
  ticks of a body reporting telemetry, with rare faults, a human who
  speaks now and then, and quiet periods to consolidate in. The day is
  one I wrote, so it can show the loop is stable, affordable and
  attentive to the right things in a world with a known answer key —
  and cannot show what a real workshop is like.\n"
    );
    let d = brainos_host::day::run(20_000);
    for (step, text) in brainos_host::day::narrate(&d) {
        println!("  [{:<12}] {}", step, text);
    }

    rule("Ten of those days, on the board Phase 0 runs on");
    println!(
        "  §5 says the bound is a property of the body rather than of who the
  entity is. That is easy to write and worth checking, because an entity
  that notices what matters with four thousand episodes of room and
  misses it with two hundred and fifty-six is an entity whose attention
  was paid for by its memory — and Phase 0 runs on the small one.

  One day does not test it: at twenty thousand ticks the entity holds
  201 memories and the embedded bound of 256 never bites. So this is ten
  of them, which is where the small body starts forgetting.\n"
    );
    let long_run = 200_000;
    let w = brainos_host::day::run_on(long_run, Body::Workstation);
    let b = brainos_host::day::run_on(long_run, Body::Board);
    println!("    {:<28} {:>14} {:>14}", "", "workstation", "ESP32-S3");
    let row = |what: &str, a: String, c: String| {
        println!("    {:<28} {:>14} {:>14}", what, a, c);
    };
    row(
        "memories at the end",
        w.memories.to_string(),
        b.memories.to_string(),
    );
    row(
        "…of those, about faults",
        format!("{} ({}%)", w.memories_about_faults, (w.memories_about_faults * 100) / w.memories.max(1)),
        format!("{} ({}%)", b.memories_about_faults, (b.memories_about_faults * 100) / b.memories.max(1)),
    );
    row(
        "…routine work",
        w.memories_routine.to_string(),
        b.memories_routine.to_string(),
    );
    row(
        "faults noticed on the first",
        format!("{}", w.worst_first_notice()),
        format!("{}", b.worst_first_notice()),
    );
    row(
        "human aware of what mattered",
        format!("{}%", w.awareness_pct()),
        format!("{}%", b.awareness_pct()),
    );
    row(
        "interrupt precision",
        format!("{}%", w.interrupt_precision_pct()),
        format!("{}%", b.interrupt_precision_pct()),
    );
    row(
        "interruptions per 1k ticks",
        format!("{}", w.interruptions_per_kilotick),
        format!("{}", b.interruptions_per_kilotick),
    );
    row(
        "escalation budget left",
        format!("{}%", w.budget_left_pct),
        format!("{}%", b.budget_left_pct),
    );
    row(
        "could be left running",
        format!("{}", w.could_be_left_running()),
        format!("{}", b.could_be_left_running()),
    );

    let bound_bit = b.memories < w.memories;
    println!();
    if !bound_bit {
        println!(
            "  THE BOUND NEVER BIT: both bodies ended with the same number of memories,
  so this comparison says nothing and should not be quoted.\n"
        );
    } else {
        println!(
            "  The board keeps {}% of what the workstation keeps and loses nothing in
  noticing. Attention turns out to be a property of the ambient loop —
  the damping, the severity bands, consequence scaling evidence rather
  than adding to it — and not of how much is remembered.

  And look at *what* it keeps. Forced to forget, the small body ends up
  {}% faults against the workstation's {}%: the routine walks go and the
  things that went wrong stay. That is salience deciding the order, and
  it means a constrained entity's memory is not merely smaller, it is
  denser in the things worth having kept.\n",
            (b.memories * 100) / w.memories.max(1),
            (b.memories_about_faults * 100) / b.memories.max(1),
            (w.memories_about_faults * 100) / w.memories.max(1)
        );
    }

    if d.could_be_left_running() && b.could_be_left_running() && bound_bit {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
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

    rule("And when the world changes?");
    println!(
        "  The question that decides whether accumulated experience is an asset
  or a liability. The entity learns that walking works on carpet, so it
  declines to walk on tile. If the floor is later replaced with tile that
  walks perfectly well, it never attempts there, never observes a success,
  and never revises — its own caution prevents it from learning that its
  caution is now wrong.\n"
    );
    let a = brainos_host::compound::adaptation(2_000, 6);
    for run in [&a.incurious, &a.curious] {
        println!(
            "  {:<46} before {:>3}%   just after {:>3}%   long after {:>3}%   attempts in the new regime {:>5}",
            run.curiosity.name(),
            run.before,
            run.immediately_after,
            run.recovered_to,
            run.attempts_in_the_new_regime
        );
    }
    println!();
    if a.curiosity_pays() {
        println!(
            "  An entity that occasionally checks whether it is still right recovers
  to {}% where one that never does gets back to {}%. Curiosity costs
  something — attempting what you believe will fail is mostly failing —
  and here it pays for itself.",
            a.curious.recovered_to, a.incurious.recovered_to
        );
    } else {
        println!(
            "  NULL on curiosity: {}% against {}%, which does not clear the margin.
  Reported as a null rather than reframed.",
            a.curious.recovered_to, a.incurious.recovered_to
        );
    }
    if !a.incurious.recovered() {
        println!(
            "\n  The incurious entity lost {} points permanently. That is the hole:
  experience becomes a liability the moment the world stops matching it,
  and nothing in the architecture as written closes it.",
            a.incurious.permanent_loss_pts()
        );
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
  gait loop's buffers, the wire and the WiFi stack, an entity has about
  192 KiB.

  This used to count the *encoded* state graph — what goes in the
  journal — and call that the entity. The entity is larger than its
  journal: strings and vectors carry capacities and headers, the
  retrieval index is not in the journal at all and at the default bound
  is bigger than the memories it indexes, and the audit trail keeps 32
  bytes per decision forever. A board budget built from the encoded
  number is wrong in the optimistic direction, which is the worst
  direction for a number that decides whether a robot works. Each part
  is now populated to its bound and asked what it occupies.\n"
    );
    for fit in brainos_host::bench::fits_on_the_board() {
        println!("  {}", fit.what);
        for (part, bytes) in &fit.parts {
            let resident = *part != "encoded journal payload";
            println!(
                "      {:<28} {:>7} KiB{}",
                part,
                bytes / 1024,
                if resident { "" } else { "   (transient, not counted)" }
            );
        }
        println!(
            "      {:<28} {:>7} KiB of {} KiB   {}",
            "resident total",
            fit.total / 1024,
            fit.available / 1024,
            if fit.fits { "FITS" } else { "DOES NOT FIT" }
        );
        if fit.fits {
            println!(
                "      headroom holds {} more gated decisions at 32 bytes each —\n\
                 {:>34} about {} minutes at ten a second\n",
                fit.decisions_of_headroom,
                "",
                fit.seconds_of_headroom / 60
            );
        } else {
            println!();
        }
    }
    println!(
        "  The last line is the one that matters for Phase 0. The audit log keeps
  32 bytes per gated decision forever, so on a board the headroom is a
  *duration*, not a size, and it is minutes rather than days. Keeping
  every leaf in SRAM was a workstation decision being made silently on
  the board. `MerkleLog::forget_leaves_before` is the answer: the root
  comes from the frontier, not from the leaves, so forgetting them does
  not move it and every checkpoint the entity ever signed stays correct.
  What is lost is the ability to *produce* a proof for a forgotten
  decision — and a proof issued before it was forgotten still verifies,
  because a proof is checked against a root rather than against a log.
  So the board keeps minutes of leaves and hands the rest to flash.

  This is also why Bounds::EMBEDDED exists and is not a smaller number
  chosen for tidiness. An entity on the board remembers less and consolidates
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
    println!("  gate properties       {}", PROPERTIES.len());

    rule("The six property domains");
    println!(
        "  Reported together because \"proof domain size: 8,640\" was the gate's
  number and this line used to print it as if it were the whole of what
  is checked. It is one of six.\n"
    );
    let rows: [(&str, String, &str); 6] = [
        (
            "the gate",
            format!("{} cases", Domain::SIZE),
            "exhaustive; 17 properties over every trace",
        ),
        (
            "the two envelopes",
            String::from("400 cases"),
            "exhaustive; against the real hexapod and its real geometry",
        ),
        (
            "hostile input",
            String::from("~36,000 inputs"),
            "randomised and mutation fuzzing over 9 decoders",
        ),
        (
            "the adversary",
            format!("{} attacks", brainos_proof::adversary::attacks().len()),
            "directed; each says what it holds and what it wants",
        ),
        (
            "power loss",
            format!(
                "{} instants",
                brainos_proof::durability::verify_durability(4).instants
            ),
            "exhaustive; every byte a journal write can stop at, on 3 media",
        ),
        (
            "a body of several parts",
            format!(
                "{} cases",
                brainos_proof::regions::verify_regions().cases
            ),
            "exhaustive; and asserted to fail against the old code",
        ),
    ];
    for (name, size, note) in rows {
        println!("  {:<24} {:>15}   {}", name, size, note);
    }
    println!(
        "\n  Bounded model checking, fuzzing and directed attack. Not a
  machine-checked proof, and `brainos verify` says so as prominently as
  it says the result.\n"
    );

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
