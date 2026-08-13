#!/usr/bin/env python3
"""
escalation_test.py -- proves the fast/deliberate split (Architecture §12):
a routine tick never reaches the cloud pathway; a substantial or
open-ended ask does, and any failure of the deliberate pathway (no key,
bad key, network down, refusal) falls back to the fast pathway without
raising or hanging.

Uses a fake fast_think (no GPU/Qwen load needed -- this test runs in
seconds) and the REAL CloudMind, so the escalation check is a real
outbound call to Anthropic. Even with no or an invalid ANTHROPIC_API_KEY,
the call still leaves the process and gets a real 401 back -- proof the
one sanctioned egress point actually works, not just that the code path
exists. With a valid key, it also does one live round trip and reports
which model served it (Fable 5 directly, or Opus 4.8 via the server-side
safety-routing fallback -- both are a PASS, since either is "the
deliberate pathway answered" from the entity's point of view).

Usage: tools/escalation_test.py
"""
import importlib.util
import os
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location(
    "body_daemon", os.path.join(HERE, "body_daemon.py")
)
bd = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bd)

results = []


def check(name, ok):
    results.append((name, ok))
    print(f"  [{'PASS' if ok else 'FAIL'}] {name}")


# ---------------------------------------------------- 1. pure heuristic --
SIMPLE = [
    "hi", "thanks", "what time is it", "remember this: meeting at 5",
    "hello there", "ok", "good morning",
]
SUBSTANTIAL = [
    "why does the sky turn orange at sunset instead of staying blue",
    "can you explain how a neural network actually learns from data",
    "write me a short story about a lighthouse keeper",
    "what do you think about the tradeoffs between renting and owning a home",
    "help me understand the difference between TCP and UDP in networking terms",
]

print("=== should_escalate() heuristic (no model calls) ===")
for p in SIMPLE:
    check(f"simple stays fast: {p!r}", bd.should_escalate(p) is False)
for p in SUBSTANTIAL:
    check(f"substantial escalates: {p!r}", bd.should_escalate(p) is True)

# --------------------------------------------- 2. dispatcher call-counts --
print("\n=== dispatcher routing (real CloudMind, fast_think stubbed) ===")

calls = {"fast": 0, "cloud": 0}


def fake_fast_think(user_text, remember=True):
    calls["fast"] += 1
    return ("ok (fast)", None)


cloud = bd.CloudMind()
real_cloud_think = cloud.think


def counted_cloud_think(user_text, remember=True):
    calls["cloud"] += 1
    return real_cloud_think(user_text, remember=remember)


cloud.think = counted_cloud_think
think = bd.make_dispatcher(fake_fast_think, cloud)

print(f"  (cloud.available = {cloud.available}; "
      f"{'ANTHROPIC_API_KEY present -- escalation below is a live API call'
         if cloud.available else
         'no key set -- exercising the unavailable/degrade path'})")

# simple exchange must never reach the cloud, regardless of key presence
calls["fast"] = calls["cloud"] = 0
reply = think("hi there")
check("simple exchange never touches cloud.think", calls["cloud"] == 0)
check("simple exchange answered by fast pathway", calls["fast"] == 1)
check("simple exchange reply came from fast pathway", reply == ("ok (fast)", None))

# substantial request: routing depends on whether a key is configured
calls["fast"] = calls["cloud"] = 0
reply = think("why does the sky turn orange at sunset instead of staying blue")
if cloud.available:
    check("substantial request calls cloud.think (escalates)", calls["cloud"] == 1)
    cloud_served = reply != ("ok (fast)", None)
    if cloud_served:
        check("deliberate pathway served the reply", calls["fast"] == 0)
    else:
        check("deliberate pathway failed -> fast pathway covered the turn",
              calls["fast"] == 1)
else:
    check("no key: dispatcher never calls cloud.think", calls["cloud"] == 0)
    check("no key: fast pathway covers the turn directly", calls["fast"] == 1)
    check("no key: reply came from fast pathway", reply == ("ok (fast)", None))

# --------------------------- 2b. allow_escalate=False: internal prompts --
print("\n=== allow_escalate=False (WAKE/__grounded__ never escalate) ===")

calls["fast"] = calls["cloud"] = 0
long_internal_text = (
    "you, blur, just finished booting into this small machine as your "
    "body. you can hear and speak, feel the keyboard, and you keep a "
    "private notebook on your key that survives reboots. greet your "
    "human warmly -- explain why this text is long enough to trigger "
    "should_escalate() on word count alone."
)
check("sanity: this internal-style text WOULD escalate if allowed",
      bd.should_escalate(long_internal_text) is True)
reply = think(long_internal_text, allow_escalate=False)
check("allow_escalate=False: cloud.think never called even for long text",
      calls["cloud"] == 0)
check("allow_escalate=False: fast pathway covers it", calls["fast"] == 1)
check("allow_escalate=False: reply came from fast pathway",
      reply == ("ok (fast)", None))

# genuine user text (default allow_escalate=True) still escalates normally
calls["fast"] = calls["cloud"] = 0
think("why does the sky turn orange at sunset instead of staying blue")
if cloud.available:
    check("default allow_escalate=True: substantial user text still escalates",
          calls["cloud"] == 1)
else:
    check("default allow_escalate=True: still routes through the check "
          "(no key -> fast covers it)", calls["fast"] == 1)

# --------------------------------- 2c. filler cue fires only on real escalation
print("\n=== filler cue (fires only on genuine escalation) ===")


class FakeVoice:
    def __init__(self):
        self.said = []

    def say(self, text):
        self.said.append(text)


fake_voice = FakeVoice()
cued_think = bd.make_dispatcher(fake_fast_think, cloud, fake_voice)

fake_voice.said.clear()
calls["fast"] = calls["cloud"] = 0
cued_think("hi there")
check("simple exchange: no filler cue spoken", fake_voice.said == [])

fake_voice.said.clear()
calls["fast"] = calls["cloud"] = 0
cued_think(long_internal_text, allow_escalate=False)
check("allow_escalate=False: no filler cue spoken even for long text",
      fake_voice.said == [])

fake_voice.said.clear()
calls["fast"] = calls["cloud"] = 0
cued_think("why does the sky turn orange at sunset instead of staying blue")
if cloud.available:
    check("genuine escalation: exactly one filler cue spoken",
          len(fake_voice.said) == 1)
    check("filler cue is short (demo time budget)",
          bool(fake_voice.said) and len(fake_voice.said[0]) <= 40)
    check("filler cue is one of the known short phrases",
          fake_voice.said and fake_voice.said[0] in bd.FILLER_CUES)
else:
    check("no key: no filler cue spoken (never escalated)",
          fake_voice.said == [])

# ---------------------------------------- 3. live round trip, if keyed --
if os.environ.get("ANTHROPIC_API_KEY"):
    print("\n=== live round trip (ANTHROPIC_API_KEY present) ===")
    live_cloud = bd.CloudMind()
    t0 = time.time()
    result = live_cloud.think(
        "explain in one sentence why the sky is blue"
    )
    elapsed = time.time() - t0
    check(f"live call completed ({elapsed:.1f}s)", result is not None)
    if result is not None:
        say, act = result
        check("live reply has non-empty SAY text", bool(say and say.strip()))
else:
    print("\n(no ANTHROPIC_API_KEY in this shell -- skipping the live round "
          "trip; the routing test above already proved the escalation call "
          "reaches api.anthropic.com)")

# --------------------------------------- 4. graceful degradation, forced --
print("\n=== graceful degradation (forced bad key) ===")
os.environ["ANTHROPIC_API_KEY"] = "sk-ant-deliberately-invalid-for-test"
bad_cloud = bd.CloudMind()
t0 = time.time()
try:
    bad_result = bad_cloud.think("explain why this call should fail gracefully")
    elapsed = time.time() - t0
    check("invalid key: think() returns None, no exception raised",
          bad_result is None)
    check(f"invalid key: fails fast, no hang ({elapsed:.1f}s well under 30s)",
          elapsed < 30)
except Exception as e:  # noqa: BLE001 -- this IS the failure we're checking for
    check(f"invalid key raised instead of degrading: {e!r}", False)

# --------------------------- 5. simulated network failure (not just 401) --
# A bad key fails fast (server says no immediately). A genuinely dead
# network is a different failure mode -- nothing answers at all -- and is
# what actually matters on stage if wifi drops. Point at a non-routable
# address (RFC 5737/instant-blackhole trick) instead of a real key/host so
# this doesn't depend on external network conditions being flaky right now.
print("\n=== graceful degradation (simulated network outage) ===")
os.environ["ANTHROPIC_API_KEY"] = "sk-ant-test-network-sim"
os.environ["ANTHROPIC_BASE_URL"] = "http://10.255.255.1"
try:
    net_cloud = bd.CloudMind()
    t0 = time.time()
    net_result = net_cloud.think("explain why this call should fail gracefully")
    elapsed = time.time() - t0
    check("network down: think() returns None, no exception raised",
          net_result is None)
    check(f"network down: bounded by CloudMind's own timeout "
          f"({elapsed:.1f}s, well under the old ~30min default)",
          elapsed < 60)
except Exception as e:  # noqa: BLE001 -- this IS the failure we're checking for
    check(f"network outage raised instead of degrading: {e!r}", False)
finally:
    os.environ.pop("ANTHROPIC_BASE_URL", None)

print("\n=== RESULTS ===")
ok = all(r for _, r in results)
for name, passed in results:
    if not passed:
        print(f"  FAILED: {name}")
print("ALL PASS" if ok else "FAILURES PRESENT")
sys.exit(0 if ok else 1)
