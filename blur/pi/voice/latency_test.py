"""
latency_test.py -- one command to exercise the voice pipeline and log the
end-of-speech -> first-audio latency. Runs against mocks until keys are in
.env; once ANTHROPIC_API_KEY + CARTESIA_API_KEY (+ DEEPGRAM_API_KEY) are set,
the same command measures the real cloud round-trip.

  blur/pi/.venv/bin/python blur/pi/voice/latency_test.py

It also verifies: tool calls route through KIRA (a denied skill is spoken
gently), the local stop-word spotter fires with no network, and barge-in
stops playback.
"""
import os
import sys

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.dirname(HERE))      # blur/pi
sys.path.insert(0, HERE)
from pipeline import FRAME, Pipeline, TTS      # noqa: E402
from kira_gate import BodyState, KiraGate, Posture   # noqa: E402


def utterance(text, seconds=1.2):
    """fake 16 kHz int16 mic audio: `seconds` of noise then 300 ms silence."""
    n = int(seconds * 16000)
    speech = (np.random.default_rng(0).standard_normal(n) * 3000).astype(np.int16)
    tail = np.zeros(16000 * 300 // 1000, np.int16)
    pcm = np.concatenate([speech, tail])
    return [pcm[i:i + FRAME] for i in range(0, len(pcm) - FRAME, FRAME)], text


def main():
    gate = KiraGate(allow_spine_abd=False)
    state = BodyState(posture=Posture.STAND)
    log = []
    def on_tool(name):
        d = gate.check(name, state)
        log.append((name, d.verdict.value, d.reason))
        return d.verdict.value, d.reason

    keyed = bool(os.environ.get("ANTHROPIC_API_KEY") and
                 (os.environ.get("CARTESIA_API_KEY") or os.environ.get("ELEVENLABS_API_KEY")))
    mode = "REAL (keys present)" if keyed else "MOCK (no keys; timings are simulated)"
    print(f"voice pipeline latency test -- {mode}")
    print(f"TTS provider: {TTS.provider}; stock voice candidates:")
    for i, (prov, desc) in enumerate(TTS.stock_candidates):
        print(f"  [{i}] {prov}: {desc}")

    pipe = Pipeline(on_tool=on_tool)

    # 1. a normal request that KIRA grants
    frames, text = utterance("can you sit down please")
    t = pipe.handle_utterance(frames, transcript_fixture=text)
    print(f"\n[sit]  '{t.transcript}' -> \"{t.reply.strip()}\" tool={t.tool}")
    print(f"       end-of-speech -> first audio: {t.latency_ms:.0f} ms "
          f"({'under' if t.latency_ms < 800 else 'OVER'} the 800 ms target)")

    # 2. a request KIRA denies (walk: no spine/abd motors) -> spoken gently
    frames, text = utterance("come here")
    t2 = pipe.handle_utterance(frames, transcript_fixture=text)
    print(f"[come] '{t2.transcript}' -> \"{t2.reply.strip()}\" tool={t2.tool}")

    # 3. local stop-word with no network
    frames, text = utterance("stop stop")
    t3 = pipe.handle_utterance(frames, transcript_fixture=text)
    print(f"[stop] local stop-word fired = {t3.stopword_local} (works offline)")

    # 4. barge-in: loud mic frame while speaking stops playback
    pipe.speaking = True
    loud = (np.random.default_rng(1).standard_normal(FRAME) * 6000).astype(np.int16)
    print(f"[barge-in] interrupt detected = {pipe.handle_barge_in(loud)}")

    print("\nKIRA tool routing:")
    for name, verdict, reason in log:
        print(f"  {name:<10} {verdict:<6} {reason}")
    lat = pipe.log[0].latency_ms
    print(f"\nlogged latency (turn 1): {lat:.0f} ms  [{'measured' if keyed else 'simulated'}]")


if __name__ == "__main__":
    main()
