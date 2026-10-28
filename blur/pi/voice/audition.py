"""
audition.py -- find and compare stock TTS voices for BLUR.

Given a TTS provider key (Cartesia or ElevenLabs, from blur/pi/.env), it:
  1. searches the provider's STOCK voice library for 5-6 voices matching the
     brief (soft-spoken, gentle, calm, warm, young adult female);
  2. renders the same BLUR lines in each, stability fairly high and speed a
     little below 1.0;
  3. writes the audio + a local HTML page to listen and pick;
  4. saves the pick to blur/pi/voice/voice_config.json (via `--set`).

Never clones or imitates a real voice actor; these are the provider's own
stock library voices. The audio/key stay local; nothing is uploaded beyond
the provider's own TTS request. There is no TTS key yet -- run this once Arjun
provides CARTESIA_API_KEY or ELEVENLABS_API_KEY.

  blur/pi/.venv/bin/python blur/pi/voice/audition.py --provider cartesia
  blur/pi/.venv/bin/python blur/pi/voice/audition.py --set cartesia <voice_id>
"""
import argparse
import json
import os
import sys

import httpx

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
from pipeline import load_env   # noqa: E402

CONFIG = os.path.join(HERE, "voice_config.json")
KEYWORDS = ["soft", "gentle", "calm", "warm", "young", "soothing", "breathy",
            "soft-spoken", "mellow", "pleasant"]
# short lines (character budget): ~5 lines, warm/shy/deadpan/protective persona
BLUR_LINES = [
    "Hi. I'm BLUR. Good to meet you.",
    "Okay. Sitting now.",
    "I can't do that just yet. Soon.",
    "I'll keep watch. You rest.",
    "I fell over twice today. We don't talk about it.",
]
STABILITY = 0.7     # fairly high
SPEED = 0.95        # a little below 1.0
MODEL = "eleven_flash_v2_5"


# ------------------------- Cartesia -------------------------
def cartesia_voices(key, n):
    h = {"X-API-Key": key, "Cartesia-Version": "2024-11-13"}
    r = httpx.get("https://api.cartesia.ai/voices/", headers=h, timeout=30)
    r.raise_for_status()
    voices = r.json()
    voices = voices.get("data", voices) if isinstance(voices, dict) else voices
    scored = []
    for v in voices:
        text = " ".join(str(v.get(k, "")) for k in ("name", "description")).lower()
        if "female" in text or "woman" in text or v.get("gender") == "feminine":
            score = sum(k in text for k in KEYWORDS)
            scored.append((score, v))
    scored.sort(key=lambda s: -s[0])
    return [v for _, v in scored[:n]] or [v for _, v in [(0, x) for x in voices][:n]]

def cartesia_render(key, voice_id, text, path):
    h = {"X-API-Key": key, "Cartesia-Version": "2024-11-13", "Content-Type": "application/json"}
    body = {"model_id": "sonic-2", "transcript": text,
            "voice": {"mode": "id", "id": voice_id,
                      "__experimental_controls": {"speed": "slow"}},
            "language": "en",
            "output_format": {"container": "wav", "encoding": "pcm_s16le", "sample_rate": 44100}}
    r = httpx.post("https://api.cartesia.ai/tts/bytes", headers=h, json=body, timeout=60)
    r.raise_for_status()
    open(path, "wb").write(r.content)


# ------------------------- ElevenLabs -------------------------
def _score(name, desc, labels):
    labs = " ".join(str(x) for x in (labels or {}).values()).lower()
    text = f"{name} {desc} {labs}".lower()
    s = sum(k in text for k in KEYWORDS)
    if (labels or {}).get("gender") == "female" or "female" in text:
        s += 1
    if (labels or {}).get("age") in ("young", "young_adult"):
        s += 2
    return s, text

def eleven_candidates(key, n):
    """Return (renderable account voices, listed-only shared-library voices),
    both female and ranked by the persona keywords."""
    h = {"xi-api-key": key}
    acct = httpx.get("https://api.elevenlabs.io/v2/voices", headers=h,
                     params={"page_size": 100}, timeout=30).json().get("voices", [])
    ascored = []
    for v in acct:
        s, text = _score(v.get("name", ""), v.get("description", ""), v.get("labels"))
        if "female" in text or (v.get("labels") or {}).get("gender") == "female":
            ascored.append((s, v))
    ascored.sort(key=lambda x: -x[0])
    shared = []
    try:
        sv = httpx.get("https://api.elevenlabs.io/v1/shared-voices", headers=h,
                       params={"gender": "female", "page_size": 100}, timeout=30).json().get("voices", [])
        sscored = sorted(((_score(x.get("name",""), x.get("description",""), x)[0], x) for x in sv),
                         key=lambda x: -x[0])
        shared = [x for _, x in sscored[:6]]
    except Exception:
        pass
    return [v for _, v in ascored[:n]], shared

def eleven_render(key, voice_id, text, path):
    h = {"xi-api-key": key, "Content-Type": "application/json"}
    body = {"text": text, "model_id": MODEL,
            "voice_settings": {"stability": STABILITY, "similarity_boost": 0.75, "speed": SPEED}}
    r = httpx.post(f"https://api.elevenlabs.io/v1/text-to-speech/{voice_id}",
                   headers=h, params={"output_format": "mp3_44100_128"}, json=body, timeout=60)
    r.raise_for_status()
    open(path, "wb").write(r.content)


def write_html(outdir, provider, items, ext, shared=None):
    rows = []
    for vid, name in items:
        clips = "".join(
            f'<audio controls preload="none" src="{vid}_{i}.{ext}"></audio>' for i in range(len(BLUR_LINES)))
        rows.append(f'<div class="v"><b>{name}</b> <code>{vid}</code><br>{clips}'
                    f'<br><button onclick="pick(\'{vid}\')">pick this one</button></div>')
    extra = ""
    if shared:
        s = "".join(f'<li>{n} <code>{i}</code></li>' for i, n in shared)
        extra = (f'<h3>Shared-library matches (add to the account to render)</h3><ul>{s}</ul>')
    html = f"""<!doctype html><meta charset=utf-8><title>BLUR voice audition</title>
<style>body{{font:14px system-ui;background:#0d0f12;color:#d7dce1;padding:16px}}
.v{{border:1px solid #232a32;border-radius:8px;padding:10px;margin:8px 0}}
audio{{height:30px;margin:2px}} code{{color:#8fb7ff}}</style>
<h2>BLUR voice audition &mdash; {provider}</h2>
<p>Listen, then click pick. Then run:
<code>audition.py --set {provider} &lt;id&gt;</code></p>
{''.join(rows)}
{extra}
<script>function pick(id){{navigator.clipboard?.writeText(id);
document.title='picked '+id; alert('picked '+id+'  (id copied). Save it with --set '+id);}}</script>"""
    open(os.path.join(outdir, "index.html"), "w").write(html)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--provider", choices=["cartesia", "elevenlabs"], default="cartesia")
    ap.add_argument("--n", type=int, default=6)
    ap.add_argument("--out", default=os.path.join(HERE, "audition"))
    ap.add_argument("--set", nargs=2, metavar=("PROVIDER", "VOICE_ID"),
                    help="save the chosen voice to voice_config.json")
    args = ap.parse_args()
    load_env()

    if args.set:
        json.dump({"provider": args.set[0], "voice_id": args.set[1],
                   "stability": STABILITY, "speed": SPEED}, open(CONFIG, "w"), indent=1)
        print(f"saved voice pick -> {CONFIG}")
        return

    keyvar = "CARTESIA_API_KEY" if args.provider == "cartesia" else "ELEVENLABS_API_KEY"
    key = os.environ.get(keyvar)
    if not key:
        print(f"no {keyvar} in blur/pi/.env yet. Add it, then re-run to audition "
              f"{args.provider} stock voices. (This tool is ready; it just needs the key.)")
        return

    os.makedirs(args.out, exist_ok=True)
    if args.provider == "cartesia":
        voices = cartesia_voices(key, args.n)
        items = [(v["id"], v.get("name", v["id"])) for v in voices]
        ext = "wav"
        for vid, _ in items:
            for i, line in enumerate(BLUR_LINES):
                cartesia_render(key, vid, line, os.path.join(args.out, f"{vid}_{i}.{ext}"))
    else:
        acct, shared = eleven_candidates(key, args.n)
        items, ext = [], "mp3"
        for v in acct:
            vid, name = v["voice_id"], v.get("name", v["voice_id"])
            try:
                for i, line in enumerate(BLUR_LINES):
                    eleven_render(key, vid, line, os.path.join(args.out, f"{vid}_{i}.{ext}"))
                items.append((vid, name))
            except Exception as e:
                print(f"  skip {name}: {e}")
        shared_list = [(x.get("voice_id"), x.get("name", "")) for x in (shared or [])]
        write_html(args.out, args.provider, items, ext, shared=shared_list)
    print(f"auditioned {len(items)} {args.provider} voices -> open {args.out}/index.html")
    for vid, name in items:
        print(f"  {name}  {vid}")


if __name__ == "__main__":
    main()
