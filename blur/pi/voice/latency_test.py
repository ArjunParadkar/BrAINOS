"""
latency_test.py -- exercise the voice pipeline and log latency.

With ANTHROPIC_API_KEY in blur/pi/.env it runs the REAL Claude
(claude-haiku-5-5) streaming path with mock STT/TTS, and reports, over ~10
short turns, end-of-speech -> first LLM text token and -> first complete
phrase as p50/p95. It also checks that tool calls route through KIRA (a
denied skill comes back as a tool_result and is spoken). Without the key it
runs fully on mocks. Never prints the key.

  blur/pi/.venv/bin/python blur/pi/voice/latency_test.py
"""
import os
import sys

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.dirname(HERE))      # blur/pi
sys.path.insert(0, HERE)
from pipeline import FRAME, Pipeline, TTS, load_env   # noqa: E402
from kira_gate import BodyState, KiraGate, Posture    # noqa: E402

TURNS = [
    "can you sit down please", "lie down for me", "okay stand back up",
    "good girl, look at me", "come here", "follow me please",
    "what are you", "are you doing okay", "tell me a tiny joke",
    "alright, stop",
]


def utterance(text, seconds=1.0):
    n = int(seconds * 16000)
    speech = (np.random.default_rng(len(text)).standard_normal(n) * 3000).astype(np.int16)
    tail = np.zeros(16000 * 300 // 1000, np.int16)
    pcm = np.concatenate([speech, tail])
    return [pcm[i:i + FRAME] for i in range(0, len(pcm) - FRAME, FRAME)], text


def pct(xs, q):
    xs = sorted(x for x in xs if x == x)
    return xs[min(len(xs) - 1, int(q * len(xs)))] if xs else float("nan")


def main():
    load_env()
    gate = KiraGate(allow_spine_abd=False)
    state = BodyState(posture=Posture.STAND)
    routed = []
    def on_tool(name):
        d = gate.check(name, state)
        routed.append((name, d.verdict.value, d.code))
        return d.verdict.value, d.reason

    llm = bool(os.environ.get("ANTHROPIC_API_KEY"))
    tts_real = bool(os.environ.get("ELEVENLABS_API_KEY") or os.environ.get("CARTESIA_API_KEY"))
    print(f"voice latency test -- STT: fixture ; "
          f"LLM: {'REAL claude-haiku-5-5' if llm else 'mock'} ; "
          f"TTS: {'REAL '+TTS.provider if tts_real else 'mock'}")

    pipe = Pipeline(on_tool=on_tool)
    tok, phr, aud = [], [], []
    for text in TURNS:
        frames, t = utterance(text)
        turn = pipe.handle_utterance(frames, transcript_fixture=t)
        tok.append(turn.token_ms); phr.append(turn.phrase_ms); aud.append(turn.latency_ms)
        tools = ", ".join(f"{n}:{'ok' if a else 'deny'}" for n, a, _ in turn.tools) or "-"
        print(f"  '{text[:26]:<26}' tok {turn.token_ms:5.0f}  phrase {turn.phrase_ms:5.0f}  "
              f"audio {turn.latency_ms:5.0f}ms  [{tools}] \"{turn.reply.strip()[:40]}\"")

    print(f"\nend-of-speech -> first LLM token : p50 {pct(tok,.5):.0f} ms  p95 {pct(tok,.95):.0f} ms")
    print(f"end-of-speech -> first phrase    : p50 {pct(phr,.5):.0f} ms  p95 {pct(phr,.95):.0f} ms")
    print(f"end-of-speech -> first AUDIO byte: p50 {pct(aud,.5):.0f} ms  p95 {pct(aud,.95):.0f} ms  "
          f"({'real LLM+TTS' if (llm and tts_real) else 'partly mocked'}; target < 800 ms)")

    # barge-in
    pipe.speaking = True
    loud = (np.random.default_rng(1).standard_normal(FRAME) * 6000).astype(np.int16)
    print(f"\nbarge-in interrupt detected: {pipe.handle_barge_in(loud)}")
    print("KIRA tool routing:")
    for n, v, c in routed:
        print(f"  {n:<10} {v:<6} {c}")


if __name__ == "__main__":
    main()
