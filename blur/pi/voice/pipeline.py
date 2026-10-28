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
    "You are BLUR, a small quadruped robot companion: warm, soft, calm, a "
    "little shy, quietly protective, with dry deadpan humour. Short spoken "
    "sentences, said aloud -- no lists, no emoji, no stage directions. You are "
    "your own self; never imitate or quote anyone, no catchphrases. "
    "Always begin your reply with a few words immediately. If the person asks "
    "for a physical action, say a brief, tentative acknowledgement first (like "
    "\"okay, let me\"), then call the matching tool -- it may not be possible "
    "right now, and that's fine."
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


def load_env(path=None):
    """Load blur/pi/.env into os.environ if present. Never prints values."""
    path = path or os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), ".env")
    if not os.path.exists(path):
        return
    for line in open(path):
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        k, v = line.split("=", 1)
        k, v = k.strip(), v.strip().strip('"').strip("'")
        if v and not os.environ.get(k):
            os.environ[k] = v


# Anthropic tool schemas: each robot action the voice can trigger. All are
# no-argument commands; KIRA decides whether each is allowed right now.
_TOOL_DESC = {
    "sit": "Sit down.", "lie_down": "Lie down.", "stand": "Stand up.",
    "come_here": "Walk toward the person.", "look_at_me": "Turn the head to the person.",
    "follow": "Follow the person.", "stop": "Stop and hold still immediately.",
}
TOOL_SCHEMAS = [
    {"name": n, "description": d, "input_schema": {"type": "object", "properties": {}}}
    for n, d in _TOOL_DESC.items()
]


def _phrases(buf: str):
    """Split accumulated text into complete spoken phrases + a remainder."""
    out, cur = [], ""
    for ch in buf:
        cur += ch
        if ch in ".!?;:\n" or len(cur) >= 60:
            out.append(cur.strip())
            cur = ""
    return [p for p in out if p], cur


class LLM:
    """Claude claude-haiku-5-5, streaming, with tool use. The real path uses
    the official anthropic SDK; the mock keeps the identical generator shape so
    the pipeline runs offline. `execute(name, input)->(allowed, result_text)`
    runs each tool through KIRA; the result is returned to the model as a
    tool_result, so a denial is spoken in BLUR's own words."""
    model = "claude-haiku-5-5"

    def __init__(self):
        load_env()
        self.key = os.environ.get("ANTHROPIC_API_KEY")
        self._client = None
        if self.key:
            import anthropic
            self._client = anthropic.Anthropic()        # reads ANTHROPIC_API_KEY from env

    last_usage = None     # {"cache_read": int, "cache_creation": int, "input": int}

    def respond(self, transcript, history, execute):
        """ONE streaming call per turn (no tool round-trip on the critical
        path). Yields {"type":"text",...} deltas and {"type":"tool",...} events.
        The model speaks a short acknowledgement first, so TTS starts before
        the tool is even resolved. Tools run through KIRA locally; for history
        validity a tool_result is still appended, but no second LLM call is
        made -- denials are voiced from KIRA's own sayable reason upstream."""
        if self._client is None:
            yield from self._mock(transcript, history, execute)
            return
        system = [{"type": "text", "text": PERSONA, "cache_control": {"type": "ephemeral"}}]
        tools = [dict(t) for t in TOOL_SCHEMAS]
        tools[-1]["cache_control"] = {"type": "ephemeral"}   # cache the prompt prefix
        history.append({"role": "user", "content": transcript})
        with self._client.messages.stream(
            model=self.model, max_tokens=150, system=system,
            tools=tools, messages=history) as stream:
            for ev in stream:
                if ev.type == "content_block_delta" and ev.delta.type == "text_delta":
                    yield {"type": "text", "text": ev.delta.text}
            final = stream.get_final_message()
        u = final.usage
        self.last_usage = {"cache_read": getattr(u, "cache_read_input_tokens", 0) or 0,
                           "cache_creation": getattr(u, "cache_creation_input_tokens", 0) or 0,
                           "input": u.input_tokens, "output": u.output_tokens}
        history.append({"role": "assistant", "content": final.content})
        tus = [b for b in final.content if getattr(b, "type", None) == "tool_use"]
        results = []
        for b in tus:
            allowed, result = execute(b.name, dict(b.input or {}))
            yield {"type": "tool", "name": b.name, "allowed": allowed, "result": result}
            results.append({"type": "tool_result", "tool_use_id": b.id,
                            "content": result, "is_error": not allowed})
        if results:                           # keep history valid; no extra LLM call
            history.append({"role": "user", "content": results})

    def _mock(self, transcript, history, execute):
        tool = next((t for t in TOOLS if t.replace("_", " ") in transcript
                     or t.split("_")[0] in transcript), None)
        allowed, result = (True, "")
        if tool:
            allowed, result = execute(tool, {})
            yield {"type": "tool", "name": tool, "allowed": allowed, "result": result}
        if tool and not allowed:
            reply = f"I can't just now. {result}"
        else:
            reply = {"sit": "Okay. Sitting.", "lie_down": "Alright, lying down.",
                     "stand": "Standing up.", "come_here": "On my way.",
                     "look_at_me": "Looking.", "follow": "Following you.",
                     "stop": "Stopping."}.get(tool, "Mm. I'm here.")
        for w in reply.split():
            yield {"type": "text", "text": w + " "}
        history.append({"role": "user", "content": transcript})
        history.append({"role": "assistant", "content": reply})


def _voice_id(default="EXAVITQu4vr4xnSDxMaL"):   # "Sarah" stock premade, until a pick is saved
    import json
    cfg = os.path.join(os.path.dirname(os.path.abspath(__file__)), "voice_config.json")
    if os.path.exists(cfg):
        return json.load(open(cfg)).get("voice_id", default)
    return default


class TTS:
    """Streaming TTS. ElevenLabs Flash v2.5 is primary (lowest-latency model);
    the /stream endpoint with pcm_16000 yields the first audio bytes as soon as
    they are synthesised, so the first yield here is the real first-audio-byte.
    Mock (no key) yields silent PCM after a nominal delay."""
    provider = "elevenlabs-flash-v2.5"
    model = "eleven_flash_v2_5"
    mock_ttfa_ms = 90

    def __init__(self, voice_id=None):
        load_env()
        self.el_key = os.environ.get("ELEVENLABS_API_KEY")
        self.ca_key = os.environ.get("CARTESIA_API_KEY")
        self.voice_id = voice_id or _voice_id()

    def stream(self, text_chunks):
        if self.el_key:
            yield from self._elevenlabs(text_chunks)
        elif self.ca_key:
            yield from self._cartesia(text_chunks)
        else:
            first = True
            for _ in text_chunks:
                if first:
                    time.sleep(self.mock_ttfa_ms / 1000)
                    first = False
                yield np.zeros(FRAME, np.int16)

    def _elevenlabs(self, text_chunks):
        import httpx
        h = {"xi-api-key": self.el_key, "Content-Type": "application/json"}
        url = f"https://api.elevenlabs.io/v1/text-to-speech/{self.voice_id}/stream"
        for p in text_chunks:
            body = {"text": p, "model_id": self.model,
                    "voice_settings": {"stability": 0.7, "similarity_boost": 0.75, "speed": 0.95}}
            with httpx.stream("POST", url, headers=h, json=body,
                              params={"output_format": "pcm_16000"}, timeout=60) as r:
                r.raise_for_status()
                rem = b""
                for chunk in r.iter_bytes():
                    if not chunk:
                        continue
                    data = rem + chunk
                    n = (len(data) // 2) * 2
                    if n:
                        yield np.frombuffer(data[:n], np.int16)
                    rem = data[n:]

    def _cartesia(self, text_chunks):
        import httpx
        h = {"X-API-Key": self.ca_key, "Cartesia-Version": "2024-11-13", "Content-Type": "application/json"}
        for p in text_chunks:
            body = {"model_id": "sonic-2", "transcript": p,
                    "voice": {"mode": "id", "id": self.voice_id}, "language": "en",
                    "output_format": {"container": "raw", "encoding": "pcm_s16le", "sample_rate": 44100}}
            with httpx.stream("POST", "https://api.cartesia.ai/tts/bytes",
                              headers=h, json=body, timeout=60) as r:
                r.raise_for_status()
                rem = b""
                for chunk in r.iter_bytes():
                    if not chunk:
                        continue
                    data = rem + chunk
                    n = (len(data) // 2) * 2
                    if n:
                        yield np.frombuffer(data[:n], np.int16)
                    rem = data[n:]

    def session(self, play=False):
        """A per-turn streaming session: feed text as the LLM emits it, audio
        starts as soon as the first words arrive. ElevenLabs uses the
        WebSocket stream-input endpoint (socket opened up front, off the
        critical path); everything else buffers and renders on close."""
        if self.el_key:
            return ElevenWSSession(self.el_key, self.voice_id, self.model, play=play)
        return BufferedSession(self, play=play)


class BufferedSession:
    """Collects the whole reply, then renders it via tts.stream() on close.
    Used for the mock, piper, and Cartesia-HTTP backends."""
    def __init__(self, tts, play=False):
        self.tts = tts
        self.play = play
        self.buf = ""
        self._first = None

    def feed(self, text):
        self.buf += text

    def first_audio_ts(self):
        return self._first

    def close(self):
        if not self.buf.strip():
            return
        for pcm in self.tts.stream([self.buf.strip()]):
            if self._first is None:
                self._first = time.perf_counter()
            if self.play:
                _play_blocking(pcm, 16000)


class ElevenWSSession:
    """ElevenLabs stream-input WebSocket. The socket is opened and primed in
    __init__ (off the LLM critical path); feed() sends text as tokens arrive;
    a reader thread timestamps the first audio chunk and collects PCM."""
    def __init__(self, key, voice_id, model, play=False):
        import json
        import threading
        from websockets.sync.client import connect
        self.play = play
        self._first = None
        self._pcm = []
        url = (f"wss://api.elevenlabs.io/v1/text-to-speech/{voice_id}/stream-input"
               f"?model_id={model}&output_format=pcm_16000&optimize_streaming_latency=3"
               f"&inactivity_timeout=20&auto_mode=true")
        self._ws = connect(url, additional_headers={"xi-api-key": key}, max_size=None,
                           open_timeout=10)
        self._ws.send(json.dumps({"text": " ",
                                  "voice_settings": {"stability": 0.7, "similarity_boost": 0.75,
                                                     "speed": 0.95}}))
        self._json = json
        self._reader = threading.Thread(target=self._read, daemon=True)
        self._reader.start()

    def _read(self):
        import base64
        try:
            for msg in self._ws:
                d = self._json.loads(msg)
                if d.get("audio"):
                    if self._first is None:
                        self._first = time.perf_counter()
                    self._pcm.append(base64.b64decode(d["audio"]))
                if d.get("isFinal"):
                    break
        except Exception:
            pass

    def feed(self, text):
        if text.strip():
            self._ws.send(self._json.dumps({"text": text}))

    def first_audio_ts(self):
        return self._first

    def close(self):
        try:
            self._ws.send(self._json.dumps({"text": ""}))   # flush + end of stream
            self._reader.join(timeout=15)
            self._ws.close()
        except Exception:
            pass
        if self.play and self._pcm:
            import numpy as _np
            _play_blocking(_np.frombuffer(b"".join(self._pcm), _np.int16), 16000)


def _play_blocking(pcm, sr):
    try:
        import sounddevice as sd
        sd.play(pcm, sr); sd.wait()
    except Exception:
        pass


# ---------------- the orchestrator ----------------
@dataclass
class Turn:
    transcript: str = ""
    reply: str = ""
    tools: list = field(default_factory=list)       # [(name, allowed, result)]
    t_end_of_speech: float = 0.0
    t_first_token: float = 0.0
    t_first_phrase: float = 0.0
    t_first_audio: float = 0.0
    barged_in: bool = False
    stopword_local: bool = False
    def _ms(self, t):
        return (t - self.t_end_of_speech) * 1e3 if t else float("nan")
    @property
    def token_ms(self): return self._ms(self.t_first_token)
    @property
    def phrase_ms(self): return self._ms(self.t_first_phrase)
    @property
    def latency_ms(self): return self._ms(self.t_first_audio)


class Pipeline:
    def __init__(self, on_tool=None, stt=None, llm=None, tts=None, history_turns=6):
        self.vad = VAD()
        self.stt = stt or STT()
        self.llm = llm or LLM()
        self.tts = tts or TTS()
        self.on_tool = on_tool or (lambda name: ("grant", ""))  # -> KIRA/skill runtime
        self.speaking = False
        self.history: list = []
        self.history_turns = history_turns
        self.log: list[Turn] = []

    def local_stopword(self, partial: str) -> bool:
        return any(w in STOP_WORDS for w in partial.lower().split())

    def _execute(self, name, _input):
        """Run a tool through KIRA; the (allowed, result_text) goes back to the
        model as a tool_result so a denial is spoken in BLUR's own words."""
        verdict, reason = self.on_tool(name)
        allowed = verdict == "grant"
        result = ("Done." if not reason else reason) if allowed else (reason or "Not allowed right now.")
        return allowed, result

    def handle_utterance(self, audio_frames, transcript_fixture=None) -> Turn:
        """One user utterance -> BLUR's spoken reply. audio_frames is an
        iterable of 20 ms int16 frames (mic, echo already cancelled upstream)."""
        turn = Turn()
        stt = STT(transcript_fixture) if transcript_fixture else self.stt
        for ev in stt.stream(audio_frames):
            if self.local_stopword(ev["partial"]) and not turn.stopword_local:
                turn.stopword_local = True
                a, r = self._execute("stop", {})       # acts with no network
                turn.tools.append(("stop", a, r))
            if ev["final"]:
                turn.transcript = ev["partial"]
        turn.t_end_of_speech = time.perf_counter()

        # Open the TTS session up front (WS handshake off the critical path),
        # then feed Claude's words to it the instant they stream. The model is
        # told to start with a short acknowledgement, so audio begins before
        # any tool is resolved -- no second LLM call on the critical path.
        self.speaking = True
        play = getattr(self, "play", False)
        # open the TTS session concurrently so the WS handshake overlaps the
        # LLM's first-token wait instead of adding to it.
        import threading
        holder = {}
        def _open():
            holder["s"] = (self.tts.session(play=play) if hasattr(self.tts, "session")
                           else BufferedSession(self.tts, play=play))
        opener = threading.Thread(target=_open); opener.start()
        sess = None
        def _sess():
            nonlocal sess
            if sess is None:
                opener.join()
                sess = holder["s"]
            return sess
        buf = ""
        for ev in self.llm.respond(turn.transcript, self.history, self._execute):
            if ev["type"] == "text" and ev["text"]:
                if not turn.t_first_token:
                    turn.t_first_token = time.perf_counter()
                turn.reply += ev["text"]
                _sess().feed(ev["text"])        # stream tokens straight to TTS (auto_mode buffers)
                buf += ev["text"]
                done, buf = _phrases(buf)
                if done and not turn.t_first_phrase:
                    turn.t_first_phrase = time.perf_counter()
            elif ev["type"] == "tool":
                turn.tools.append((ev["name"], ev["allowed"], ev["result"]))
                # voice a denial from KIRA's own sayable reason -- no extra LLM call
                if not ev["allowed"] and ev["result"]:
                    _sess().feed(" " + ev["result"])
                    turn.reply += " " + ev["result"]
        if not turn.reply.strip():
            # the model skipped the spoken acknowledgement -> a short local one,
            # so a granted action still gets a voice
            fallback = "Okay." if any(a for _, a, _ in turn.tools) else "Mm."
            turn.reply = fallback
            _sess().feed(fallback)
        s = _sess()
        s.close()
        turn.t_first_audio = s.first_audio_ts() or 0.0
        self.speaking = False

        # keep conversation history short, but only ever cut at a real user
        # utterance (a user message with plain string content) so a tool_result
        # is never orphaned from its tool_use.
        starts = [i for i, m in enumerate(self.history)
                  if m["role"] == "user" and isinstance(m["content"], str)]
        if len(starts) > self.history_turns:
            self.history = self.history[starts[-self.history_turns]:]
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
