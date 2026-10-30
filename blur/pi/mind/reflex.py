"""
reflex.py -- BLUR's reflex layer: the fast, on-device loop that runs WHILE the
person is talking. No cloud. Target < 150 ms (in practice microseconds per
frame). It produces the quick, human reactions the slow cognitive path can't:

  * turn-taking prediction -- not just silence, but pause + falling pitch +
    syntactic completeness, so she knows the turn is ending before it ends;
  * backchannels ("mm", "mhm") at clause boundaries;
  * gaze to the speaker and emotional mirroring from the voice's prosody;
  * instant reaction to her name, "stop", or a greeting.

Everything here is deterministic and local; the gestures it requests are still
KIRA-gated downstream (gestures.py). Input is the streaming partial transcript
plus 20 ms / 16 kHz int16 mic frames.
"""
from __future__ import annotations

import time
from dataclasses import dataclass, field

import numpy as np

SR = 16000
NAME_WORDS = {"blur"}
STOP_WORDS = {"stop", "freeze", "halt", "wait"}
GREETINGS = {"hi", "hello", "hey", "morning", "evening", "yo"}
NON_FINAL = {"and", "but", "so", "or", "the", "a", "to", "of", "with", "um", "uh",
             "because", "that", "if", "when", "my", "your", "i", "you"}


@dataclass
class Prosody:
    """Cheap running prosody: energy, pitch (f0) and its slope, trailing
    silence. Updated per 20 ms frame."""
    energy_db: float = -90.0
    f0: float = 0.0
    f0_slope: float = 0.0           # Hz per frame, + rising / - falling
    silence_ms: float = 0.0
    _f0_hist: list = field(default_factory=list)
    _e_hist: list = field(default_factory=list)

    def push(self, frame: np.ndarray):
        x = frame.astype(np.float64)
        rms = float(np.sqrt(np.mean(x * x)) + 1e-9)
        self.energy_db = 20 * np.log10(rms / 32768 + 1e-9)
        voiced = self.energy_db > -45
        self.silence_ms = 0.0 if voiced else self.silence_ms + 20
        f0 = self._pitch(x) if voiced else 0.0
        if f0:
            self._f0_hist.append(f0)
            self._f0_hist = self._f0_hist[-8:]
            self.f0 = float(np.median(self._f0_hist))
            if len(self._f0_hist) >= 4:
                self.f0_slope = float(np.polyfit(range(len(self._f0_hist)), self._f0_hist, 1)[0])
        self._e_hist.append(self.energy_db); self._e_hist = self._e_hist[-25:]

    @staticmethod
    def _pitch(x):
        """autocorrelation f0 over 70-350 Hz, cheap."""
        x = x - x.mean()
        if len(x) < 160:
            return 0.0
        ac = np.correlate(x, x, "full")[len(x) - 1:]
        lo, hi = SR // 350, SR // 70
        if hi >= len(ac):
            return 0.0
        seg = ac[lo:hi]
        if seg.size == 0 or ac[0] <= 0:
            return 0.0
        lag = lo + int(np.argmax(seg))
        if ac[lag] / ac[0] < 0.3:
            return 0.0
        return SR / lag

    def emotion(self):
        """coarse emotional mirror from prosody -> one of calm/warm/bright/
        tense/low."""
        e = np.std(self._e_hist) if len(self._e_hist) > 3 else 0.0
        if self.energy_db < -55:
            return "low"
        if self.f0 > 220 and e > 4:
            return "bright"
        if e > 6:
            return "tense"
        if self.f0 and self.f0 < 150:
            return "low"
        return "warm"


def syntactic_complete(text: str) -> float:
    """0..1 that the utterance is a complete thought."""
    t = text.strip().lower()
    if not t:
        return 0.0
    if t[-1] in ".?!":
        return 1.0
    words = t.split()
    if len(words) < 2:
        return 0.2
    if words[-1] in NON_FINAL:
        return 0.1
    # a verb-ish token present is a weak completeness cue
    return 0.6 if len(words) >= 3 else 0.4


class Reflex:
    def __init__(self):
        self.prosody = Prosody()
        self._last_backchannel = 0.0
        self._last_cost_us = 0.0

    def on_frame(self, frame):
        t0 = time.perf_counter()
        self.prosody.push(frame)
        self._last_cost_us = (time.perf_counter() - t0) * 1e6
        return self._last_cost_us

    def reflex_cost_us(self):
        return self._last_cost_us

    def instant_reaction(self, partial: str):
        """Fires before cognition: name / stop / greeting."""
        t0 = time.perf_counter()
        toks = set(w.strip(".,?!").lower() for w in partial.split())
        react = None
        if toks & STOP_WORDS:
            react = "stop"
        elif toks & NAME_WORDS:
            react = "name"           # perk up, ears forward, gaze lock
        elif toks & GREETINGS:
            react = "greeting"
        self._last_cost_us = (time.perf_counter() - t0) * 1e6
        return react

    def backchannel_due(self, partial: str) -> bool:
        """A quiet 'mm' at a clause boundary during a long turn."""
        now = time.time()
        if now - self._last_backchannel < 2.5:
            return False
        if self.prosody.silence_ms >= 120 and len(partial.split()) >= 4:
            self._last_backchannel = now
            return True
        return False

    def turn_end_prob(self, partial: str) -> float:
        """pause + falling pitch + syntactic completeness -> P(end of turn)."""
        pause = min(1.0, self.prosody.silence_ms / 500.0)
        falling = 1.0 if self.prosody.f0_slope < -0.5 else (0.5 if self.prosody.f0_slope < 0 else 0.0)
        syntax = syntactic_complete(partial)
        p = 0.5 * pause + 0.2 * falling + 0.3 * syntax
        return max(0.0, min(1.0, p))
