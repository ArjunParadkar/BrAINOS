"""
mind.py -- BLUR's conversational mind, built the way the brain runs: a fast
reflex loop and a cognitive path in parallel, with a slow "thinking" path that
shows itself in the body.

This is inside BrAInOS: it answers from the entity's PERSONA and its MEMORY of
this person (memory.py, Domain 5 rules), gates every body act through KIRA,
and streams speech + gesture together. It is not a voice script bolted on.

Paths (cf. the coordinator's brief):
  * Reflex (<150 ms, local): gaze, backchannels, turn-taking prediction,
    instant name/stop/greeting, prosody emotion. -> reflex.py
  * Fast path (target first audio <= 500-700 ms): a SPECULATIVE Claude call is
    started on the partial transcript when the turn is predicted to end, with a
    tiny prompt carrying only retrieved memory snippets (not full history); at
    end-of-turn it commits (hiding the LLM latency behind the user's last
    words) or cancels.
  * Slow path (thinking): on a recall miss or a complex question she SHOWS
    thinking (head tilt + gaze aversion + a soft "hmm", a KIRA-gated gesture)
    and runs a deeper call / memory search, then speaks.
  * Expressive: per-turn emotion -> ElevenLabs voice settings; gestures synced
    with speech.

"Like ChatGPT's newest voice" is a native speech-to-speech model; we don't
have one with Claude. This parallel design gets as close as the stack allows;
the gap and the measured numbers are reported honestly.
"""
from __future__ import annotations

import os
import sys
import threading
import time

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.dirname(HERE))      # blur/pi
sys.path.insert(0, os.path.join(os.path.dirname(HERE), "voice"))
sys.path.insert(0, HERE)
from pipeline import LLM, TTS, _phrases       # noqa: E402
from memory import Memory                      # noqa: E402
from reflex import Reflex, SR, syntactic_complete   # noqa: E402
from gestures import GesturePlanner, ConsoleSink  # noqa: E402

# The system prompt is pipeline.PERSONA (short, block-4 tuned: warm/shy/deadpan,
# "start with a few words"); the fast path adds only a retrieved memory snippet
# to the user message (not the whole history), per the brief.

# emotion -> ElevenLabs voice settings (expressive, per turn, low-latency lever)
EMOTION_VOICE = {
    "warm":   {"stability": 0.55, "style": 0.35, "similarity_boost": 0.75, "speed": 0.96},
    "bright": {"stability": 0.40, "style": 0.55, "similarity_boost": 0.75, "speed": 1.0},
    "low":    {"stability": 0.80, "style": 0.15, "similarity_boost": 0.8, "speed": 0.9},
    "tense":  {"stability": 0.65, "style": 0.3, "similarity_boost": 0.75, "speed": 1.0},
    "serious":{"stability": 0.85, "style": 0.1, "similarity_boost": 0.8, "speed": 0.9},
}

# recall = a QUESTION about something she may know (not a statement like
# "my name is ..."); complex = needs real thought.
RECALL_CUES = ("what is my", "what's my", "who am i", "do you remember", "what did i",
               "did i tell you", "remember when")
COMPLEX_CUES = ("why", "how would", "how do", "explain", "plan", "what should", "decide", "think about")


def classify(text, tools):
    t = text.lower()
    if tools:
        return "tool"
    if any(c in t for c in RECALL_CUES):
        return "recall"
    if any(c in t for c in COMPLEX_CUES) or len(text.split()) > 14:
        return "complex"
    return "chat"


def extract_facts(text):
    """Cheap local fact extraction (name / likes / simple 'I am/have')."""
    t = text.strip().rstrip(".!?")
    tl = t.lower()
    out = []
    for cue in ("my name is ", "i'm ", "i am ", "call me "):
        if cue in tl:
            val = t[tl.index(cue) + len(cue):].split()[0].strip(",.")
            if val and val[0].isalpha():
                out.append(("name", val.capitalize()))
            break
    for cue in ("i like ", "i love ", "i enjoy ", "my favorite ", "my favourite "):
        if cue in tl:
            out.append(("likes", t[tl.index(cue) + len(cue):].strip(",.")))
            break
    return out


class ConversationalMind:
    def __init__(self, on_tool, memory_path=None, play=False, twin_sink=None):
        self.on_tool = on_tool
        self.mem = Memory(memory_path)
        self.reflex = Reflex()
        self.gestures = GesturePlanner(on_tool, sink=twin_sink or ConsoleSink())
        self.llm = LLM()
        self.tts = TTS()
        self.play = play
        self.history = []
        self.turn_i = 0
        self.partial_delay = 0.12      # per-partial pacing (~speech), so speculation overlaps
        self.metrics = {"reflex_us": [], "first_audio": {"chat": [], "recall": [], "tool": [], "complex": []},
                        "thinking": 0, "turns": 0, "speculative_commits": 0}

    # ---- draining one LLM response to text + tool intents (one call) ----
    def _drain(self, text, history, record, tries=2):
        """One Claude call -> (reply, tool intents, first-token time). Resilient
        to a transient network blip: retry once, rolling back the half-appended
        turn so history stays valid."""
        for attempt in range(tries):
            base = len(history)
            reply, tools, ftt = "", [], None
            try:
                for ev in self.llm.respond(text, history, record):
                    if ev["type"] == "text" and ev["text"]:
                        if ftt is None:
                            ftt = time.perf_counter()
                        reply += ev["text"]
                    elif ev["type"] == "tool":
                        tools.append(ev["name"])
                return reply.strip(), tools, ftt
            except Exception:
                del history[base:]           # undo the partial append, then retry
        return "", [], None

    def _speak(self, text, emotion, first_audio_ref):
        vs = EMOTION_VOICE.get(emotion, EMOTION_VOICE["warm"])
        sess = self.tts.session(play=self.play, voice_settings=vs)
        for ph in _phrases(text)[0] or [text]:
            sess.feed(ph + " ")
        sess.close()
        ts = sess.first_audio_ts()
        if ts and first_audio_ref["t"] is None:
            first_audio_ref["t"] = ts

    # ---- one user turn, with partials for speculative start ----
    def process(self, partials, audio_frames=None):
        self.turn_i += 1
        self.metrics["turns"] += 1
        final = partials[-1].strip()
        rec = {"log": []}
        record_exec = lambda n, i: (rec["log"].append(n) or (True, ""))  # noqa: E731

        # --- reflex loop over the incoming partials (+ audio) ---
        reactions, backchannels, spec_thread, spec = [], 0, None, {}
        frames = audio_frames or _synth_audio(final)
        fi = 0
        for pi, partial in enumerate(partials):
            # feed a slice of audio for this partial
            for _ in range(max(1, len(frames) // len(partials))):
                if fi < len(frames):
                    self.metrics["reflex_us"].append(self.reflex.on_frame(frames[fi])); fi += 1
            r = self.reflex.instant_reaction(partial)
            if r:
                reactions.append(r)
                self.gestures.emit(self.gestures.perk_up() if r != "stop" else self.gestures.head_tilt())
            if self.reflex.backchannel_due(partial):
                backchannels += 1
                self.gestures.emit(self.gestures.nod())
            # speculative start: the moment the partial is a complete thought
            # (turn predicted to end), fire the LLM on it -- its latency then
            # hides behind the person's last word(s).
            likely_end = self.reflex.turn_end_prob(partial) >= 0.5 or syntactic_complete(partial) >= 0.5
            if spec_thread is None and likely_end and pi < len(partials) - 1:
                spec["partial"] = partial
                def _spec(p=partial):
                    h = list(self.history)
                    r, t, f = self._drain(self._fast_prompt(p), h, record_exec)
                    spec.update(reply=r, tools=t, h=h, ok=bool(r) or bool(t))
                spec_thread = threading.Thread(target=_spec, daemon=True); spec_thread.start()
            if self.partial_delay and pi < len(partials) - 1:
                time.sleep(self.partial_delay)
        self.gestures.emit(self.gestures.gaze(0.2, 0.0))   # keep looking at them

        t_end = time.perf_counter()
        emotion = self.reflex.prosody.emotion()
        first_audio = {"t": None}

        def _commits(sp, fin):
            if not sp:
                return False
            return fin == sp or (fin.startswith(sp) and (len(fin.split()) - len(sp.split())) <= 2)

        # --- 0. instant local answer from what she already knows (no cloud) ---
        local = self.fast_local(final)
        committed = False
        thinking = False
        tools = []
        if local is not None:
            reply, kind = local, classify(final, [])
        else:
            kind = classify(final, [])
            thinking = kind in ("complex", "recall") and not self.mem.is_recall_hit(final)
            if thinking:
                # --- slow path: SHOW thinking first (that is the first audio),
                # then run the deeper call while the gesture + "hmm" play ---
                self.metrics["thinking"] += 1
                self.gestures.emit(self.gestures.thinking())
                self._speak("Hmm, let me think.", "serious", first_audio)
                reply, tools, _ = self._drain(self._deep_prompt(final), self.history, record_exec)
                emotion = "serious" if emotion == "warm" else emotion
            else:
                # --- fast path: commit the speculative reply if the final only
                # extended it and it succeeded; otherwise answer now ---
                if spec_thread is not None and _commits(spec.get("partial", ""), final):
                    spec_thread.join(timeout=8)
                if spec.get("ok") and _commits(spec.get("partial", ""), final):
                    reply = spec.get("reply", ""); tools = spec.get("tools", [])
                    self.history = spec.get("h", self.history)
                    committed = True
                    self.metrics["speculative_commits"] += 1
                else:
                    reply, tools, _ = self._drain(self._fast_prompt(final), self.history, record_exec)
            kind = classify(final, tools)

        if not reply:
            reply = "Okay." if tools else "Sorry, I drifted off for a second. Say again?"

        # --- KIRA on the (deferred) tool intents; voice a denial locally ---
        for name in tools:
            verdict, reason = self.on_tool(name)
            if verdict != "grant" and reason:
                reply += " " + reason
            act = "affirm" if verdict == "grant" else "question"
            for g in self.gestures.for_dialogue_act(act, emotion):
                self.gestures.emit(g)
        if any(w in final.lower() for w in ("hi", "hello", "hey", "morning")):
            for g in self.gestures.for_dialogue_act("greeting", emotion):
                self.gestures.emit(g)
        self.gestures.emit(self.gestures.jaw_talk())

        # --- speak the reply (fast path) ---
        self._speak(reply, emotion, first_audio)
        fa_ms = (first_audio["t"] - t_end) * 1e3 if first_audio["t"] else float("nan")
        self.metrics["first_audio"].setdefault(kind, []).append(fa_ms)

        # --- memory write-back (bounded, provenance) ---
        prov = f"turn{self.turn_i}"
        for key, val in extract_facts(final):
            self.mem.remember_fact(key, val, prov)
        self.mem.add_turn(final, reply)
        if self.turn_i % 3 == 0:
            self.mem.consolidate()

        return {"reply": reply, "kind": kind, "emotion": emotion, "thinking": thinking,
                "committed": committed, "reactions": reactions, "backchannels": backchannels,
                "first_audio_ms": fa_ms, "tools": tools}

    def fast_local(self, text):
        """Answer instantly from persona + memory + body state, with no cloud
        call, when she already knows the answer. Returns a line or None."""
        t = text.lower().strip().rstrip("?.!")
        name = self.mem.name_of()
        words = t.split()
        if any(c in t for c in ("my name", "who am i", "do you remember me")) and name:
            return f"Of course. You're {name}."
        if len(words) <= 4 and any(w in words for w in ("hi", "hello", "hey")):
            return f"Hi{', ' + name if name else ''}. Good to see you."
        if any(w in t for w in ("good night", "goodnight", "goodbye", "bye", "good bye")):
            return f"Good night{', ' + name if name else ''}. I'll stay right here and keep things calm."
        if words and words[0] in ("stop", "freeze", "halt"):
            return "Okay. Stopping."
        return None

    def _fast_prompt(self, text):
        snip = self.mem.snippet(text)
        pre = f"(You know: {snip}.) " if snip else ""
        return pre + text

    def _deep_prompt(self, text):
        snip = self.mem.snippet(text, k=6)
        recent = "; ".join(e["user"] for e in self.mem.episodes[-4:])
        return (f"(You know: {snip}. Earlier they said: {recent}.) Take a breath and "
                f"answer thoughtfully in one or two short sentences: {text}")


def _synth_audio(text, seconds_per_word=0.25):
    """Fake 16 kHz int16 mic audio for the reflex loop: noise during the words,
    then 300 ms of trailing silence with a falling tail."""
    n = int(max(0.4, len(text.split()) * seconds_per_word) * SR)
    rng = np.random.default_rng(len(text))
    speech = (rng.standard_normal(n) * 2500).astype(np.int16)
    tail = np.zeros(SR * 300 // 1000, np.int16)
    pcm = np.concatenate([speech, tail])
    F = SR * 20 // 1000
    return [pcm[i:i + F] for i in range(0, len(pcm) - F, F)]
