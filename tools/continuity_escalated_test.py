#!/usr/bin/env python3
"""
continuity_escalated_test.py -- the specific gap the brain-swap could
plausibly have introduced: conversation history (HISTORY) was hoisted
from a Qwen-closure-local deque to module scope. Does that change what
gets persisted to the BrAIn Key across a VM-to-VM boundary?

Answer by construction: HISTORY is in-process only either way (module
scope or closure scope, both die with the daemon process -- neither was
ever wired to EPISODES.LOG or the notebook), and persistence runs
entirely through the core's own state graph + notebook, driven by CX/
__consolidate__/notes.write, none of which this session's changes
touched. This test proves it empirically: same shape as
continuity_test.py's single-cycle round trip, except the fact is WRITTEN
via a phrase engineered to trigger should_escalate() (so the deliberate
Claude pathway actually handles the notes.write turn, not Qwen), and
RECALLED via a phrase that also escalates (cold-boot Qwen's notes.read
verb formatting is independently flaky per the earlier diagnostic --
using an escalating phrase for recall keeps this test's signal about the
brain swap, not a re-run of that unrelated pre-existing weakness).

Requires ANTHROPIC_API_KEY in the environment -- refuses to run without
one, since an unescalated run would just be continuity_test.py again.

Usage: ANTHROPIC_API_KEY=... tools/continuity_escalated_test.py
"""
import os
import re
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
from continuity_test import Body, cleanup  # noqa: E402

RESULT_RE = re.compile(r"the real result was: ([^.]*)\.")
SERVED_RE = re.compile(r"\[mind\] deliberate pathway served by (\S+)")


def served_by_models(daemon_log, since_line=0):
    """Which models answered turns in this daemon log, from `since_line`
    on. Empty list means the deliberate pathway never actually served a
    turn (escalation either didn't fire or fell back to fast)."""
    if not os.path.exists(daemon_log):
        return []
    lines = open(daemon_log, errors="replace").read().splitlines()
    return [m.group(1) for line in lines[since_line:]
            for m in [SERVED_RE.search(line)] if m]


def line_count(path):
    if not os.path.exists(path):
        return 0
    return len(open(path, errors="replace").read().splitlines())


def main():
    if not os.environ.get("ANTHROPIC_API_KEY"):
        print("[test] ANTHROPIC_API_KEY not set -- this test specifically "
              "verifies the ESCALATED (Claude) path; run "
              "continuity_test.py for the Qwen-only baseline instead.",
              file=sys.stderr)
        return 1

    codeword = f"cassiopeia{int(time.time()) % 100000}"
    # >=14 words AND contains an escalation keyword ("remember"/"note down"
    # trigger notes.write in SYSTEM_TEMPLATE; length alone clears
    # should_escalate()'s threshold regardless of keyword match).
    # Deliberately NOT phrased as a "code"/"secret" -- Claude (esp. via the
    # Opus 4.8 safety-routing fallback) treats "launch code" + "exact
    # value" framing as a credential request and either declines to look
    # or declines to recite it, independent of whether the underlying
    # notebook data is actually there. This test is about persistence,
    # not about that (separately interesting, separately reported)
    # content-judgment behavior -- so keep the fact mundane.
    write_phrase = (
        f"please note down and remember this important fact for later "
        f"because it truly matters to me: my favorite constellation "
        f"today is {codeword}"
    )
    # also escalates (contains "explain" + is long), so recall doesn't
    # depend on the independently-known-flaky cold-Qwen read-verb format.
    read_phrase = (
        "can you explain what you noted down about my favorite "
        "constellation, i would like to hear exactly what you wrote"
    )

    cleanup()
    results = []

    def check(name, ok):
        results.append((name, ok))
        print(f"  [{'PASS' if ok else 'FAIL'}] {name}")

    try:
        a = Body("A")
        n_before = a.boot()
        write_from = line_count(a.daemon_log)
        a.say(write_phrase)
        ok_write = a.wait_result_contains(codeword, timeout=60)
        check("A learns fact via escalated write", ok_write)
        write_served = served_by_models(a.daemon_log, write_from)
        check(f"write turn actually escalated to Claude (served_by={write_served})",
              len(write_served) > 0)
        a.release()

        b = Body("B")
        n_mid = b.boot()
        check(f"B inherited >= A's count ({n_mid} >= {n_before})",
              n_mid >= n_before)
        read_from = line_count(b.daemon_log)
        b.say(read_phrase)
        ok_recall_b = b.wait_result_contains(codeword, timeout=60)
        check("B recalls escalated fact from A via escalated read", ok_recall_b)
        read_served = served_by_models(b.daemon_log, read_from)
        check(f"B's read turn actually escalated to Claude (served_by={read_served})",
              len(read_served) > 0)
        b.release()

        a2 = Body("A")
        n_final = a2.boot()
        check(f"A rebooted with >= B's count ({n_final} >= {n_mid})",
              n_final >= n_mid)
        a2.say(read_phrase)
        ok_recall_a = a2.wait_result_contains(codeword, timeout=60)
        check("A still recalls escalated fact after B's session", ok_recall_a)
        a2.release()
    finally:
        cleanup()

    print("\n=== RESULTS ===")
    ok = all(passed for _, passed in results)
    for name, passed in results:
        print(f"  [{'PASS' if passed else 'FAIL'}] {name}")
    print("\nALL PASS" if ok else "\nFAILURES PRESENT")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
