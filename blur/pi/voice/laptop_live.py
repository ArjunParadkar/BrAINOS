"""
laptop_live.py -- talk to BLUR on the laptop, before the Pi exists.

laptop mic -> local Whisper STT -> Claude (claude-haiku-5-5, streaming, tools
through KIRA) -> TTS -> laptop speaker. STT is local (faster-whisper tiny/base)
so no STT key is needed; TTS is ElevenLabs Flash v2.5 (the pipeline's streaming
WebSocket backend) when ELEVENLABS_API_KEY is set, else the local piper binary
in tools/piper, else text only.

Modes:
  --mic                 live: record until you stop talking, then BLUR replies
  --wav FILE            transcribe a wav through the full pipeline (headless)
  --say "text"          skip STT, send text straight in (headless)
  --no-audio            synthesize replies to wav files instead of playing
  --self-test           piper says a line -> whisper -> pipeline -> reply wav

  blur/pi/.venv/bin/python blur/pi/voice/laptop_live.py --say "can you sit down"
"""
import argparse
import os
import sys
import time
import wave

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(os.path.dirname(HERE)))
sys.path.insert(0, os.path.dirname(HERE))      # blur/pi
sys.path.insert(0, HERE)
from pipeline import FRAME, Pipeline, load_env   # noqa: E402
from kira_gate import BodyState, KiraGate, Posture   # noqa: E402

PIPER = os.path.join(REPO, "tools", "piper", "piper", "piper")
PIPER_VOICE = None  # auto-found below


def find_piper_voice():
    import glob
    for d in (os.path.join(REPO, "tools", "piper"), os.path.join(REPO, "tools")):
        hits = glob.glob(os.path.join(d, "**", "*.onnx"), recursive=True)
        if hits:
            return hits[0]
    return None


def read_wav(path):
    with wave.open(path, "rb") as w:
        sr = w.getframerate()
        pcm = np.frombuffer(w.readframes(w.getnframes()), np.int16)
        if w.getnchannels() == 2:
            pcm = pcm[::2]
    return pcm, sr


def write_wav(path, pcm, sr):
    with wave.open(path, "wb") as w:
        w.setnchannels(1); w.setsampwidth(2); w.setframerate(sr)
        w.writeframes(pcm.astype(np.int16).tobytes())


def resample(pcm, sr, target=16000):
    if sr == target:
        return pcm
    n = int(len(pcm) * target / sr)
    return np.interp(np.linspace(0, len(pcm), n, endpoint=False),
                     np.arange(len(pcm)), pcm).astype(np.int16)


# ------------------- local STT (faster-whisper) -------------------
class WhisperSTT:
    def __init__(self, size="base"):
        from faster_whisper import WhisperModel
        self.m = WhisperModel(size, device="cpu", compute_type="int8")

    def transcribe(self, pcm16, sr):
        audio = resample(pcm16, sr, 16000).astype(np.float32) / 32768.0
        segs, _ = self.m.transcribe(audio, language="en", beam_size=1)
        return " ".join(s.text for s in segs).strip()


# ------------------- TTS backends (play or write) -------------------
class PiperTTS:
    provider = "piper(local)"
    sr = 22050
    def __init__(self, play=True):
        self.play = play
        self.voice = find_piper_voice()
        self.ok = os.path.exists(PIPER) and self.voice

    def synth(self, text):
        import subprocess, tempfile
        out = tempfile.mktemp(suffix=".wav")
        subprocess.run([PIPER, "-m", self.voice, "-f", out], input=text.encode(),
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        pcm, sr = read_wav(out)
        os.unlink(out)
        return pcm, sr

    def stream(self, phrases):
        for p in phrases:
            if not self.ok:
                yield np.zeros(FRAME, np.int16); continue
            pcm, sr = self.synth(p)
            if self.play:
                _play(pcm, sr)
            yield pcm


class CartesiaTTS:
    provider = "cartesia-sonic"
    def __init__(self, play=True):
        import httpx
        self.httpx = httpx
        self.key = os.environ.get("CARTESIA_API_KEY")
        self.play = play
        cfg = os.path.join(HERE, "voice_config.json")
        import json
        self.voice = (json.load(open(cfg)).get("voice_id")
                      if os.path.exists(cfg) else None) or "a0e99841-438c-4a64-b679-ae501e7d6091"

    def stream(self, phrases):
        h = {"X-API-Key": self.key, "Cartesia-Version": "2024-11-13", "Content-Type": "application/json"}
        for p in phrases:
            body = {"model_id": "sonic-2", "transcript": p,
                    "voice": {"mode": "id", "id": self.voice},
                    "language": "en",
                    "output_format": {"container": "raw", "encoding": "pcm_s16le", "sample_rate": 44100}}
            r = self.httpx.post("https://api.cartesia.ai/tts/bytes", headers=h, json=body, timeout=60)
            r.raise_for_status()
            pcm = np.frombuffer(r.content, np.int16)
            if self.play:
                _play(pcm, 44100)
            yield pcm


def _play(pcm, sr):
    try:
        import sounddevice as sd
        sd.play(pcm, sr); sd.wait()
    except Exception as e:
        print(f"  (playback unavailable: {e})")


def pick_tts(play):
    load_env()
    if os.environ.get("ELEVENLABS_API_KEY") or os.environ.get("CARTESIA_API_KEY"):
        from pipeline import TTS            # ElevenLabs Flash v2.5 WS (primary), Cartesia secondary
        return TTS()
    p = PiperTTS(play=False)                 # BufferedSession does the playback
    if p.ok:
        return p
    print("  (no TTS key and no local piper; replies will be text only)")
    return PiperTTS(play=False)   # yields silence


def record_until_silence(max_s=10):
    import sounddevice as sd
    from pipeline import VAD
    vad = VAD()
    buf, sr = [], 16000
    print("listening... (speak, then pause)")
    with sd.InputStream(samplerate=sr, channels=1, dtype="int16") as stream:
        spoke = False
        t0 = time.time()
        while time.time() - t0 < max_s:
            data, _ = stream.read(FRAME)
            frame = data[:, 0]
            buf.append(frame)
            _, started, ended = vad.push(frame)
            spoke = spoke or started
            if spoke and ended:
                break
    return np.concatenate(buf), sr


def build_pipeline(tts, play=True):
    gate = KiraGate(allow_spine_abd=False)
    state = BodyState(posture=Posture.STAND)
    def on_tool(name):
        d = gate.check(name, state)
        return d.verdict.value, d.reason
    pipe = Pipeline(on_tool=on_tool, tts=tts)
    pipe.play = play            # the TTS session plays audio on the laptop speaker
    return pipe


def one_turn(pipe, text):
    t0 = time.perf_counter()
    turn = pipe.handle_utterance([], transcript_fixture=text)
    dt = (time.perf_counter() - t0) * 1e3
    tools = ", ".join(f"{n}:{'ok' if a else 'deny'}" for n, a, _ in turn.tools) or "-"
    print(f"you: {text}\nBLUR: {turn.reply.strip()}\n  [tools {tools}; first token {turn.token_ms:.0f} ms; "
          f"turn {dt:.0f} ms]")
    return turn


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--mic", action="store_true")
    ap.add_argument("--wav")
    ap.add_argument("--say")
    ap.add_argument("--no-audio", action="store_true")
    ap.add_argument("--self-test", action="store_true")
    ap.add_argument("--whisper", default="base")
    args = ap.parse_args()
    load_env()
    play = not args.no_audio and not args.wav and not args.self_test
    tts = pick_tts(play=play)
    pipe = build_pipeline(tts, play=play)
    print(f"STT: local whisper({args.whisper})  LLM: claude-haiku-5-5  TTS: {tts.provider}")

    if args.say:
        one_turn(pipe, args.say)
    elif args.wav:
        pcm, sr = read_wav(args.wav)
        text = WhisperSTT(args.whisper).transcribe(pcm, sr)
        one_turn(pipe, text)
    elif args.self_test:
        # piper speaks a line -> wav -> whisper -> pipeline (fully local loop)
        p = PiperTTS(play=False)
        if not p.ok:
            print("self-test needs local piper (tools/piper)."); return
        pcm, sr = p.synth("can you sit down please")
        text = WhisperSTT(args.whisper).transcribe(pcm, sr)
        print(f"self-test: piper->whisper heard: '{text}'")
        one_turn(pipe, text)
    elif args.mic:
        print("Ctrl-C to quit.")
        try:
            while True:
                pcm, sr = record_until_silence()
                text = WhisperSTT(args.whisper).transcribe(pcm, sr)
                if text:
                    one_turn(pipe, text)
        except KeyboardInterrupt:
            print("\nbye")
    else:
        print("pick a mode: --mic / --wav FILE / --say TEXT / --self-test")


if __name__ == "__main__":
    main()
