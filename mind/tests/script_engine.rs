//! Stage 1.1 — the entity's own compute: bounded, deterministic, honest
//! about every failure mode.

use brainos_mind::script::run;

#[test]
fn arithmetic_variables_and_repeat() {
    let (ok, out) = run("let x = 6\nlet y = x * 7\nprint \"answer:\", y");
    assert!(ok, "{out}");
    assert!(out.contains("answer: 42"));

    let (ok, out) = run("let s = 0; repeat 10 { let s = s + 3 }; print s");
    assert!(ok, "{out}");
    assert!(out.contains("30"));
}

#[test]
fn step_budget_stops_runaway_programs() {
    // nested repeats trying to burn ~10^10 steps
    let (ok, out) = run("repeat 100000 { repeat 100000 { let x = 1 } }");
    assert!(!ok);
    assert!(out.contains("step budget"), "must say WHY it stopped: {out}");
}

#[test]
fn output_is_bounded() {
    let (ok, out) = run("repeat 1000 { print \"aaaaaaaaaaaaaaaaaaaa\" }");
    // may finish or hit the budget; either way the digest stays bounded
    let _ = ok;
    assert!(out.len() < 400, "digest leaked past its cap: {} bytes", out.len());
}

#[test]
fn arithmetic_failures_are_reported_not_wrapped() {
    let (ok, out) = run("print 9223372036854775807 + 1");
    assert!(!ok);
    assert!(out.contains("overflow"));

    let (ok, out) = run("print 1 / 0");
    assert!(!ok);
    assert!(out.contains("divide by zero"));

    let (ok, out) = run("print 1 % 0");
    assert!(!ok);
    assert!(out.contains("modulo by zero"));
}

#[test]
fn bad_programs_fail_to_parse_not_to_run() {
    for src in ["let = 5", "print \"unterminated", "repeat { }", "}", "let x = (1 + "] {
        let (ok, out) = run(src);
        assert!(!ok, "'{src}' should not run");
        assert!(
            out.contains("parse") || out.contains("unexpected"),
            "'{src}' should fail as a parse error: {out}"
        );
    }
    let (ok, _) = run("");
    assert!(!ok, "an empty program is not a success");
}

#[test]
fn unknown_variables_are_errors() {
    let (ok, out) = run("print ghost");
    assert!(!ok);
    assert!(out.contains("unknown variable"));
    // assigning to a name that was never bound is the same mistake
    let (ok, out) = run("ghost = 3");
    assert!(!ok);
    assert!(out.contains("unknown variable"));
}

// ---- Stage 2.4: the language grown up enough to write real programs ----

#[test]
fn conditionals_and_comparison_operators() {
    let (ok, out) = run(
        "let n = 17\n\
         if n % 2 == 0 { print \"even\" } else if n > 10 { print \"big odd\" } \
         else { print \"small odd\" }",
    );
    assert!(ok, "{out}");
    assert!(out.contains("big odd"), "{out}");

    let (ok, out) = run("print 3 < 4, 4 <= 4, 5 > 6, 2 != 2, 1 && 0, 1 || 0, !0");
    assert!(ok, "{out}");
    assert!(out.contains("1 1 0 0 0 1 1"), "{out}");
}

#[test]
fn while_loops_run_and_terminate() {
    let (ok, out) = run(
        "let i = 0\nlet total = 0\nwhile i < 10 { total = total + i; i = i + 1 }\n\
         print \"total:\", total",
    );
    assert!(ok, "{out}");
    assert!(out.contains("total: 45"), "{out}");
}

#[test]
fn an_infinite_loop_dies_on_the_budget_not_the_clock() {
    let (ok, out) = run("let i = 0\nwhile 1 { i = i + 1 }");
    assert!(!ok);
    assert!(out.contains("step budget"), "must say WHY it stopped: {out}");
}

#[test]
fn functions_with_arguments_and_recursion() {
    let (ok, out) = run(
        "fn fact(n) { if n <= 1 { return 1 }; return n * fact(n - 1) }\n\
         print \"5! =\", fact(5)",
    );
    assert!(ok, "{out}");
    assert!(out.contains("5! = 120"), "{out}");

    // defined below the call site: helpers may come last, as people write
    let (ok, out) = run("print double(21)\nfn double(x) { return x * 2 }");
    assert!(ok, "{out}");
    assert!(out.contains("42"), "{out}");
}

#[test]
fn runaway_recursion_is_bounded_and_says_so() {
    let (ok, out) = run("fn forever(n) { return forever(n + 1) }\nprint forever(0)");
    assert!(!ok);
    assert!(out.contains("recursion"), "{out}");
}

#[test]
fn wrong_arity_is_refused_not_guessed() {
    let (ok, out) = run("fn add(a, b) { return a + b }\nprint add(1)");
    assert!(!ok);
    assert!(out.contains("argument"), "{out}");
    let (ok, out) = run("print nosuchfn(1)");
    assert!(!ok);
    assert!(out.contains("no function"), "{out}");
}

#[test]
fn strings_are_real_values() {
    let (ok, out) = run(
        "let who = \"blur\"\nlet greet = \"hello, \" + who\n\
         print upper(greet), len(who), contains(greet, \"blur\")",
    );
    assert!(ok, "{out}");
    assert!(out.contains("HELLO, BLUR 4 1"), "{out}");
}

#[test]
fn lists_index_mutate_and_grow() {
    let (ok, out) = run(
        "let xs = [3, 1, 2]\nxs[0] = 9\nlet xs = push(xs, 7)\n\
         print xs, len(xs), xs[-1]",
    );
    assert!(ok, "{out}");
    assert!(out.contains("[9, 1, 2, 7] 4 7"), "{out}");

    let (ok, out) = run("let xs = [1]\nprint xs[5]");
    assert!(!ok);
    assert!(out.contains("outside a list"), "{out}");
}

#[test]
fn a_real_algorithm_end_to_end() {
    // bubble sort: lists, nested loops, indexed writes, comparisons
    let (ok, out) = run(
        "let xs = [5, 3, 8, 1, 9, 2]\n\
         let n = len(xs)\n\
         let i = 0\n\
         while i < n {\n\
           let j = 0\n\
           while j < n - 1 {\n\
             if xs[j] > xs[j + 1] {\n\
               let t = xs[j]\n\
               xs[j] = xs[j + 1]\n\
               xs[j + 1] = t\n\
             }\n\
             j = j + 1\n\
           }\n\
           i = i + 1\n\
         }\n\
         print \"sorted:\", xs",
    );
    assert!(ok, "{out}");
    assert!(out.contains("sorted: [1, 2, 3, 5, 8, 9]"), "{out}");
}

#[test]
fn type_errors_are_explained_not_coerced() {
    let (ok, out) = run("print [1, 2] - 3");
    assert!(!ok);
    assert!(out.contains("cannot use"), "{out}");
    let (ok, out) = run("print 1 < \"two\"");
    assert!(!ok);
    assert!(out.contains("cannot compare"), "{out}");
}

#[test]
fn comments_are_ignored() {
    let (ok, out) = run("# a note to myself\nlet x = 4 # trailing\nprint x");
    assert!(ok, "{out}");
    assert!(out.contains("4"), "{out}");
}
