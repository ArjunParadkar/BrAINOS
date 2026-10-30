"""
demo.py -- talk to BLUR's conversational mind on the laptop.

Runs a short scripted conversation (or your own line with --say) through the
full parallel mind: reflexes while you "speak", a speculative fast path that
answers from persona + BrAInOS memory, a slow "thinking" path that shows itself
in the body, expressive ElevenLabs speech and synced gestures. Prints the
reflex/gesture/thinking timeline and reports measured numbers.

  # headless (measures latency, prints gestures, no audio):
  blur/pi/.venv/bin/python blur/pi/mind/demo.py

  # with voice on the laptop speaker:
  blur/pi/.venv/bin/python blur/pi/mind/demo.py --speak

  # one line of your own:
  blur/pi/.venv/bin/python blur/pi/mind/demo.py --say "hi, my name is Arjun"
"""
import argparse
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.dirname(HERE))
sys.path.insert(0, HERE)
from mind import ConversationalMind         # noqa: E402
from gestures import CollectSink, write_view_html   # noqa: E402
from kira_gate import BodyState, KiraGate, Posture   # noqa: E402

MEM_PATH = os.path.join(HERE, "demo_memory.json")
VIEW_PATH = os.path.join(HERE, "web", "reacts.html")

SCRIPT = [
    "hi there",
    "my name is Arjun",
    "what is my name again",
    "can you sit down please",
    "now come here",
    "why do you fall over so much",
    "tell me a tiny joke",
    "okay good night Blur",
]


def partials(utterance):
    """word-by-word partial transcripts (as a streaming STT would emit)."""
    w = utterance.split()
    return [" ".join(w[: i + 1]) for i in range(len(w))]


def pct(xs, q):
    xs = sorted(x for x in xs if x == x)
    return xs[min(len(xs) - 1, int(q * len(xs)))] if xs else float("nan")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--speak", action="store_true", help="play audio on the laptop speaker")
    ap.add_argument("--say", help="one line instead of the script")
    ap.add_argument("--fresh", action="store_true", help="forget demo memory first")
    ap.add_argument("--view", action="store_true", help="write a gesture-replay page for the laptop")
    args = ap.parse_args()
    if args.fresh and os.path.exists(MEM_PATH):
        os.unlink(MEM_PATH)

    gate = KiraGate(allow_spine_abd=False)
    state = BodyState(posture=Posture.STAND)
    def on_tool(name):
        d = gate.check(name, state)
        return d.verdict.value, d.reason

    sink = CollectSink() if args.view else None
    mind = ConversationalMind(on_tool, memory_path=MEM_PATH, play=args.speak, twin_sink=sink)
    lines = [args.say] if args.say else SCRIPT

    for utt in lines:
        print(f"\nyou: {utt}")
        r = mind.process(partials(utt))
        if sink is not None:
            sink.mark(utt, r["reply"])
        flags = []
        if r["committed"]:
            flags.append("speculative-commit")
        if r["thinking"]:
            flags.append("THINKING")
        if r["reactions"]:
            flags.append("reflex:" + "/".join(r["reactions"]))
        if r["backchannels"]:
            flags.append(f"{r['backchannels']}x backchannel")
        tag = f"  [{', '.join(flags)}]" if flags else ""
        print(f"BLUR ({r['kind']}/{r['emotion']}): {r['reply']}")
        print(f"  first audio {r['first_audio_ms']:.0f} ms{tag}")
    mind.mem.save()
    if sink is not None:
        os.makedirs(os.path.dirname(VIEW_PATH), exist_ok=True)
        write_view_html(VIEW_PATH, sink.events)
        print(f"\ngesture replay page: {VIEW_PATH}")

    m = mind.metrics
    rus = m["reflex_us"]
    print("\n================ measured ================")
    print(f"reflex latency per frame: mean {sum(rus)/len(rus):.0f} us  max {max(rus):.0f} us "
          f"(budget 150000 us)")
    for kind in ("chat", "recall", "tool", "complex"):
        xs = m["first_audio"].get(kind, [])
        if xs:
            print(f"end-of-turn -> first audio [{kind:<7}]: p50 {pct(xs,.5):.0f} ms  "
                  f"p95 {pct(xs,.95):.0f} ms  (n={len(xs)})")
    print(f"thinking display triggered: {m['thinking']}/{m['turns']} turns "
          f"({100*m['thinking']/max(1,m['turns']):.0f}%)")
    print(f"speculative commits: {m['speculative_commits']}/{m['turns']}")
    print(f"memory: knows name = {mind.mem.name_of()!r}; {len(mind.mem.episodes)} episodes stored "
          f"-> {MEM_PATH}")


if __name__ == "__main__":
    main()
