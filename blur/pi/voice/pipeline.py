"""
pipeline.py -- BLUR's full-duplex voice pipeline.

Architecture A (built here): streaming STT -> Claude claude-haiku-5-5
(streaming, tool use) -> streaming TTS, with VAD, barge-in, a local stop-word
keyword spotter, an offline fallback, and latency logging. Architecture B
(a speech-to-speech realtime API) plugs in behind the same Pipeline interface
for a head-to-head latency comparison once keys exist.

TTS provider: **Cartesia Sonic** (primary) for its very low streaming
time-to-first-audio over a WebSocket, which is what the < 800 ms budget and
barge-in need; ElevenLabs Flash v2.5 is the quality fallback. STT: a streaming
provider (Deepgram-class) with endpointing; local Whisper is the offline
fallback.

Everything here runs against MOCKS/fixtures until API keys are in .env, so the
orchestration, VAD, barge-in, keyword spotting, tool routing and latency
accounting are all exercised offline. Real latency is measured once keys land
(voice/latency_test.py). The LLM's tool calls (sit, come_here, look_at_me,
follow, lie_down, stop) are routed through KIRA -> the skill runtime; "stop"
and "freeze" are also matched locally so they work with no network.

Persona: polite, warm, a little shy, quietly protective, dry deadpan humour,
short spoken sentences. BLUR is herself -- no copied catchphrases or lines,
and the voice is a stock-library voice, never a clone of a real voice actor.
"""
from __future__ import annotations

import os
import time
from dataclasses import dataclass, field

import numpy as np

SAMPLE_RATE = 16000
FRAME_MS = 20
FRAME = SAMPLE_RATE * FRAME_MS // 1000          # 320 samples

TOOLS = ["sit", "lie_down", "stand", "come_here", "look_at_me", "follow", "stop"]
STOP_WORDS = {"stop", "freeze", "halt", "wait"}

PERSONA = (
    "You are BLUR, a small quadruped robot. You are polite, warm, a little shy "
    "and quietly protective, with dry deadpan humour. Speak in short spoken "
    "sentences. You are yourself -- never copy a character's catchphrases. When "
    "the person asks you to do something physical, call the matching tool; a "
    "tool may be refused by your safety policy, and that is fine, say so gently."
)


# ---------------- VAD ----------------
class VAD:
    """Energy VAD with hangover, on 20 ms int16 frames. Returns speech on/off
    and fires an end-of-utterance after `hang_ms` of trailing silence."""
    def __init__(self, on_db=-45.0, hang_ms=300):
        self.on = 10 ** (on_db / 20) * 32768
        self.hang = hang_ms // FRAME_MS
        self.active = False
        self.silence = 0

    def push(self, frame: np.ndarray):
        rms = float(np.sqrt(np.mean((frame.astype(np.float64)) ** 2)) + 1e-9)
        speech = rms > self.on
        started = ended = False
        if speech:
            if not self.active:
                started = True
            self.active = True
            self.silence = 0
        elif self.active:
            self.silence += 1
            if self.silence >= self.hang:
                self.active = False
                ended = True
        return speech, started, ended


# ---------------- provider interfaces (mock + real, key-guarded) ----------------
class STT:
    """Streaming STT. Mock replays a fixture transcript with word timing."""
    def __init__(self, fixture_text="can you sit down please"):
        self.key = os.environ.get("DEEPGRAM_API_KEY")
        self.fixture = fixture_text

    def stream(self, audio_frames):
        if self.key:
            raise NotImplementedError("real Deepgram streaming goes here (keyed)")
        # mock: emit partials as the audio plays, final on end
        words = self.fixture.split()
        for i, w in enumerate(words):
            yield {"partial": " ".join(words[: i + 1]), "final": False}
        yield {"partial": self.fixture, "final": True}


class LLM:
    """Claude claude-haiku-5-5, streaming, tool use. Mock streams a short
    reply and may emit a tool call parsed from the transcript."""
    model = "claude-haiku-5-5"

    def __init__(self):
        self.key = os.environ.get("ANTHROPIC_API_KEY")

    def respond(self, transcript: str):
        tool = next((t for t in TOOLS if t.replace("_", " ") in transcript
                     or t.split("_")[0] in transcript), None)
        if self.key:
            raise NotImplementedError(
                "real anthropic streaming goes here: client.messages.stream("
                "model=self.model, system=PERSONA, tools=[...], messages=[...]) "
                "yielding text deltas and tool_use blocks (keyed)")
        # mock: a warm, short reply + the detected tool call
        reply = {"sit": "Okay. Sitting.", "lie_down": "Alright, lying down.",
                 "stand": "Standing up.", "come_here": "On my way.",
                 "look_at_me": "Looking.", "follow": "Following you.",
                 "stop": "Stopping."}.get(tool, "Mm. I'm here.")
        for chunk in reply.split():
            yield {"text": chunk + " ", "tool": None}
        if tool:
            yield {"text": "", "tool": tool}


class TTS:
    """Cartesia Sonic streaming TTS. Mock yields silent PCM, first chunk after
    a realistic time-to-first-audio so the latency harness is meaningful."""
    provider = "cartesia-sonic"
    # three warm, gentle, soft-spoken young-adult-female STOCK candidates for
    # Arjun to pick by ear (exact ids confirmed against the live library):
    stock_candidates = [
        ("cartesia", "soft warm conversational (slightly breathy)"),
        ("cartesia", "calm even gentle assistant"),
        ("elevenlabs", "youthful bright-but-soft (Flash v2.5)"),
    ]
    mock_ttfa_ms = 90    # Cartesia Sonic's published streaming TTFB ballpark

    def __init__(self, voice_id="stock:0"):
        self.key = os.environ.get("CARTESIA_API_KEY") or os.environ.get("ELEVENLABS_API_KEY")
        self.voice_id = voice_id

    def stream(self, text_chunks):
        if self.key:
            raise NotImplementedError("real Cartesia Sonic WebSocket goes here (keyed)")
        first = True
        for _ in text_chunks:
            if first:
                time.sleep(self.mock_ttfa_ms / 1000)   # simulated TTFA
                first = False
            yield np.zeros(FRAME, np.int16)             # silent mock audio


# ---------------- the orchestrator ----------------
@dataclass
class Turn:
    transcript: str = ""
    reply: str = ""
    tool: str | None = None
    t_end_of_speech: float = 0.0
    t_first_audio: float = 0.0
    barged_in: bool = False
    stopword_local: bool = False
    @property
    def latency_ms(self):
        return (self.t_first_audio - self.t_end_of_speech) * 1e3 if self.t_first_audio else float("nan")


class Pipeline:
    def __init__(self, on_tool=None, stt=None, llm=None, tts=None):
        self.vad = VAD()
        self.stt = stt or STT()
        self.llm = llm or LLM()
        self.tts = tts or TTS()
        self.on_tool = on_tool or (lambda name: ("grant", ""))  # -> KIRA/skill runtime
        self.speaking = False
        self.log: list[Turn] = []

    def local_stopword(self, partial: str) -> bool:
        return any(w in STOP_WORDS for w in partial.lower().split())

    def handle_utterance(self, audio_frames, transcript_fixture=None) -> Turn:
        """One user utterance -> BLUR's spoken reply. audio_frames is an
        iterable of 20 ms int16 frames (mic, with the speaker's echo already
        cancelled upstream)."""
        turn = Turn()
        stt = STT(transcript_fixture) if transcript_fixture else self.stt
        # STT streaming with local stop-word spotting + end-of-speech from VAD
        for ev in stt.stream(audio_frames):
            if self.local_stopword(ev["partial"]) and not turn.stopword_local:
                turn.stopword_local = True
                self._do_tool(turn, "stop")           # acts with no network
            if ev["final"]:
                turn.transcript = ev["partial"]
        turn.t_end_of_speech = time.perf_counter()

        # offline fallback: no keys -> canned reply via the mock LLM/TTS (no hang)
        # LLM streaming -> start TTS on the first phrase
        text_chunks = []
        for ev in self.llm.respond(turn.transcript):
            if ev["text"]:
                text_chunks.append(ev["text"])
                turn.reply += ev["text"]
            if ev["tool"]:
                turn.tool = ev["tool"]
        # TTS: first audio chunk time = our measured latency
        self.speaking = True
        for i, _pcm in enumerate(self.tts.stream(text_chunks)):
            if i == 0:
                turn.t_first_audio = time.perf_counter()
            # barge-in: a real pipeline checks the mic VAD here each chunk and
            # stops on detected speech; see handle_barge_in().
        self.speaking = False
        if turn.tool and not turn.stopword_local:
            self._do_tool(turn, turn.tool)
        self.log.append(turn)
        return turn

    def handle_barge_in(self, mic_frame: np.ndarray) -> bool:
        """Call while speaking; returns True (and stops TTS) if the user
        interrupts."""
        _, started, _ = self.vad.push(mic_frame)
        if self.speaking and started:
            self.speaking = False
            return True
        return False

    def _do_tool(self, turn: Turn, name: str):
        verdict, reason = self.on_tool(name)
        if verdict != "grant" and reason:
            turn.reply += f" ({reason})"
