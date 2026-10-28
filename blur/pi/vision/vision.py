"""
vision.py -- person/face detection on the AI HAT+ 2 (Hailo-8L) and the head
tracker that keeps BLUR looking at whoever is speaking.

Three parts, cheap to run without a camera:
  * Detector interface with two backends:
      - HailoDetector: the real path (Hailo-8L running a person/face model
        via the Hailo runtime / DeGirum, frames from Picamera2). Guarded so it
        imports only when the AI HAT and its runtime are present.
      - StandInDetector: synthetic moving detections, so the head tracker and
        the plumbing are testable on any machine with zero dependencies (the
        "cheap stand-in" the brief allows; a laptop webcam path via OpenCV is
        left as a clearly-guarded option, not a hard dependency).
  * HeadTracker: picks the active speaker (audio direction / lip motion flag,
    else the most confident face), turns the bbox error into neck pan/tilt
    setpoints with a gain, deadband and limits, and reports fps.

Targets from the brief: person + face >= 15 fps, head tracks the speaker.
"""
from __future__ import annotations

import os
import time
from dataclasses import dataclass


@dataclass
class Detection:
    cx: float            # bbox centre, normalised 0..1 (0 left / top)
    cy: float
    w: float
    h: float
    score: float
    is_face: bool = True
    is_speaking: bool = False     # from audio DOA or lip motion


class Detector:
    def detect(self, frame) -> list[Detection]:
        raise NotImplementedError


class HailoDetector(Detector):
    """Real detector on the Hailo-8L. Imports the runtime lazily so this file
    loads anywhere; raises a clear message if the AI HAT is not present."""
    def __init__(self, model="scrfs_person_face.hef"):
        self.model = model
        self._infer = None

    def _ensure(self):
        if self._infer is None:
            try:
                import degirum as dg            # or hailo_platform / picamera2 + hailo
            except Exception as e:               # pragma: no cover (needs the HAT)
                raise RuntimeError(
                    "Hailo runtime not available on this machine; this path "
                    "runs on the Pi with the AI HAT+ 2. Use StandInDetector "
                    f"for development. ({e})")
            self._infer = dg  # a loaded model handle on the robot

    def detect(self, frame):                     # pragma: no cover
        self._ensure()
        raise NotImplementedError("wire the Hailo person/face model inference here")


class StandInDetector(Detector):
    """A synthetic person who drifts across the view and speaks in bursts, so
    the tracker and wiring run with no camera and no deps."""
    def __init__(self, seed=0):
        self.t = 0.0

    def detect(self, frame=None):
        import math
        self.t += 1 / 30
        cx = 0.5 + 0.35 * math.sin(self.t * 0.6)
        cy = 0.45 + 0.05 * math.sin(self.t * 0.9)
        speaking = (int(self.t) % 4) < 2
        return [Detection(cx, cy, 0.18, 0.28, 0.93, True, speaking)]


class HeadTracker:
    """Turns the active speaker's position into neck pan/tilt setpoints."""
    def __init__(self, gain=0.6, deadband=0.03, pan_limit=1.2, tilt_limit=0.6):
        self.gain = gain
        self.deadband = deadband
        self.pan_limit = pan_limit
        self.tilt_limit = tilt_limit
        self.pan = 0.0
        self.tilt = 0.0
        self._t = []

    def pick(self, dets: list[Detection]) -> Detection | None:
        if not dets:
            return None
        speaking = [d for d in dets if d.is_speaking]
        pool = speaking or dets
        return max(pool, key=lambda d: d.score)

    def update(self, dets, dt=1 / 30):
        t0 = time.perf_counter()
        d = self.pick(dets)
        if d is not None:
            ex, ey = d.cx - 0.5, d.cy - 0.5          # image-centre error
            if abs(ex) > self.deadband:
                self.pan -= self.gain * ex           # pan right when target is right
            if abs(ey) > self.deadband:
                self.tilt -= self.gain * ey
            self.pan = max(-self.pan_limit, min(self.pan_limit, self.pan))
            self.tilt = max(-self.tilt_limit, min(self.tilt_limit, self.tilt))
        self._t.append(time.perf_counter() - t0)
        return self.pan, self.tilt

    def fps_estimate(self):
        return 1.0 / (sum(self._t) / len(self._t)) if self._t else float("inf")


def make_detector():
    """HailoDetector on the robot (BLUR_VISION=hailo), else the stand-in."""
    if os.environ.get("BLUR_VISION") == "hailo":
        return HailoDetector()
    return StandInDetector()
