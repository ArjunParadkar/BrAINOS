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
    let report = brainos_proof::verify();
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
