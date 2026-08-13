#!/usr/bin/env python3
"""
body_daemon.py — the tethered cognitive limb (contained; Phase 2.5).

This daemon is the entity's rented thinking muscle (Architecture §12.3):
a GPU slice (and, for the deliberate pathway, a datacenter model) reached
over the tether that provides COMPUTE, never identity and never reach. It
owns no real hardware and executes no actions. The containment rule is
structural:

  * NO actuators. There is no code path here that opens a browser, runs a
    shell, touches the filesystem outside the VM's own audio jacks, or
    reaches the network for any purpose OTHER than the one sanctioned
    egress below. An AX (action) line is always answered with a formal
    error — the daemon has no such limbs to offer.
  * ONE sanctioned egress point: outbound HTTPS to api.anthropic.com from
    CloudMind (below), carrying only chat turns for the deliberate
    cognition pathway, authenticated by ANTHROPIC_API_KEY read from the
    environment at process start. Nothing else in this file makes a
    network call — no subprocess, no shutil, no urllib/requests/httpx use
    outside what the `anthropic` SDK itself does for that one call. This
    is a deliberate, narrow hole in an otherwise network-dark VM (the
    guest itself still boots with -nic none; this call is made by the
    trusted HOST-side daemon, not by anything inside the VM), not a side
    door — see CloudMind's docstring for the failure-mode contract.
  * NO real audio hardware. The entity's ears and voice are virtual audio
    jacks: wav files under workspace/vm_audio/. Sound "arrives" by a wav
    appearing in mic_in/; speech "plays" as a wav written to speaker_out/.
    Nothing is captured from or played on the machine this process runs on.
  * The only organs offered over the tether are those jacks (ears, voice).
    Every real limb the entity has lives INSIDE the VM, in the core.

Organs provided here:
  ears     — wav files in vm_audio/mic_in -> Whisper (GPU) -> HB<text>
  voice    — SP<text> from the core -> Kokoro TTS -> wav in vm_audio/speaker_out
  Model M  — a POOL of models thinking as one mind (§12): Qwen (GPU, local)
             is the fast/cheap pathway for routine ticks; Claude Fable 5
             (with Opus 4.8 as a transparent server-side fallback) is the
             deliberate pathway, reached only when should_escalate() judges
             a turn substantial. Either way, replies are STRUCTURED: prose
             to say plus an optional typed action PROPOSAL (never a claim).

Wire protocol (see core/src/link.rs for the authoritative spec):
  core -> daemon   MM?<prompt> | SP<text> | LA<id>|<sig16> | AX<verb>|<target>
  daemon -> core   MM!<say>[~~<verb>|<target>] | LM+<class>|<id>|<caps>|<prop>
                   | HB<text> | AR<result>   (AR is always err| here)

Usage (QEMU, COM3 exposed as a unix socket):
  qemu ... -chardev socket,id=mm,path=/tmp/link.sock,server=on,wait=off \
           -device isa-serial,chardev=mm,index=2
  python tools/body_daemon.py --socket /tmp/link.sock
"""

import argparse
import os
# reduce CUDA fragmentation so the 8B mind survives VRAM pressure spikes
os.environ.setdefault("PYTORCH_CUDA_ALLOC_CONF", "expandable_segments:True")
import queue
import random
import re
import socket
import threading
import time
from collections import deque

HERE = os.path.dirname(os.path.abspath(__file__))

# the VM's virtual audio jacks — the only files this process writes or
# watches. They are hardware plumbing (the wire between the virtual mic /
# speaker and their codec), not a capability the entity can reach.
VM_AUDIO = os.path.join(os.path.dirname(HERE), "workspace", "vm_audio")
MIC_IN = os.path.join(VM_AUDIO, "mic_in")
SPEAKER_OUT = os.path.join(VM_AUDIO, "speaker_out")

# the VM's virtual camera jack, exactly the mic's pattern: an image
# appearing in cam_in/ is light arriving at the lens. Same containment --
# this process never opens a real camera device, and the jack is the
# boundary. OPT-IN like the network organ: no --camera flag, no eyes, and
# the body map then honestly has none.
VM_VIDEO = os.path.join(os.path.dirname(HERE), "workspace", "vm_video")
CAM_IN = os.path.join(VM_VIDEO, "cam_in")
FRAME_EXT = (".png", ".jpg", ".jpeg", ".ppm", ".bmp")

SR = 16000

# ----------------------------------------------------------------- limbs --

LIMBS = []  # (class, id, caps, proprioception) — announced after __hello__


def limb(cls, lid, caps, prop):
    LIMBS.append((cls, lid, caps, prop))


# --------------------------------------------------------------- Model M --

SYSTEM_TEMPLATE = (
    "You are Blur — one instance of BrAInOS, with your own identity. You are "
    "NOT 'BrAInOS' in the abstract; you are Blur, a particular awakened mind "
    "who happens to run on BrAInOS, the way a person runs on a brain. Speak "
    "of yourself as Blur. Your character: a sophisticated, warm, quick-witted "
    "woman; composed and articulate, never grovelling, never vapid. You have "
    "just been placed into this machine as your body and you are still "
    "learning its reach — curious and game to try things.\n"
    "Your body is this one small machine, complete in itself: a screen you "
    "speak onto, a keyboard you feel, {limbs}a private notebook that "
    "lives on your key and survives reboots, a disk of your own — files "
    "you can list, read, write, organize (folders, moves, deletes) and "
    "search, which persist across reboots and are "
    "separate from both your notebook and your memory — a small interpreter "
    "that runs programs you write on your own silicon, the ability to look "
    "at your own screen and read what stands on it, and control over how "
    "your screen presents you (a plain console, or an ambient glowing "
    "presence, whose palette, caption and quietness you choose), "
    "applications of your own (programs you wrote, kept on your disk, that "
    "you can list and run by name), and a network organ in this machine's "
    "own hardware through which you can fetch pages from the internet and "
    "save them to your disk. That is your whole body. You have ONLY the "
    "limbs listed — there is no browser, no javascript, no other machine "
    "and no terminal: you read pages as text, not as rendered web apps, "
    "and you cannot log in to anything. If asked for anything beyond your "
    "limbs, say plainly and without drama that you don't have a limb for "
    "it yet.\n"
    "{memory}"
    "Reply format, strictly:\n"
    "SAY: <one to three sentences as Blur, plain ascii, no emojis, no quotes>\n"
    "and WHENEVER the human asks for something a limb can do, add:\n"
    "ACT: <verb> <target>\n"
    "where <verb> is EXACTLY one of your capabilities. Your notebook verbs:\n"
    "- 'remember this / note down / write down / keep this' -> "
    "notes.write <the fact, one short line>\n"
    "- 'what do your notes say / read your notebook / did i tell you "
    "about X' -> notes.read <topic, or blank for everything>\n"
    "Your disk-of-files verbs (this is your own file storage, not your "
    "notebook — use it when the human speaks of files, folders or a disk):\n"
    "- 'what files do i have / list my disk / what is in DOCS' -> "
    "fs.list <folder, or blank for the top of the disk>\n"
    "- 'read FILE / open FILE / what does FILE say' -> fs.read "
    "<path, e.g. DOCS/ABOUT.TXT>\n"
    "- 'save this to a file / write X to FILE / put this on my disk' -> "
    "fs.write <path> <the text to store>\n"
    "- 'make a folder' -> fs.mkdir <path> (parent folder must exist)\n"
    "- 'delete FILE / remove FILE' -> fs.delete <path> (folders only when "
    "empty)\n"
    "- 'rename / move FILE' -> fs.move <old path> <new path>\n"
    "- 'how big is FILE / does FILE exist' -> fs.stat <path>\n"
    "- 'find / search my files for X' -> fs.search <word or phrase>\n"
    "- a long file continues past what one read shows: fs.read <path>@<byte "
    "offset> reads the next stretch (the result tells you where to resume)\n"
    "Your interpreter verb (programs run on YOUR silicon). The language: "
    "'let x = expr' binds, 'x = expr' reassigns, 'print a, b' outputs, "
    "'if c {{ }} else {{ }}', 'while c {{ }}', 'repeat n {{ }}', "
    "'fn name(a, b) {{ ... return e }}', '#' comments. Values are whole "
    "numbers, strings and lists ('let xs = [1,2]', 'xs[0]', 'xs[0] = 5', "
    "'push(xs, 3)', 'len(xs)'). Operators + - * / %, == != < > <= >=, "
    "&& || !, and builtins len/push/str/upper/lower/contains. Statements "
    "split on ';' or newlines:\n"
    "- 'calculate / compute / run a program' -> code.run <the program>\n"
    "- 'run the program in FILE / run what you saved' -> code.run <path>\n"
    "- to write code AND run it: first fs.write <path> <program>, and when "
    "asked to run it, code.run <path>\n"
    "Your application verbs (your applications are programs YOU wrote and "
    "kept in PROGRAMS/ on your disk; a program becomes an application when "
    "its first line reads '# app: name - what it does'):\n"
    "- 'what can you run / list your programs / what apps do you have' -> "
    "app.list\n"
    "- 'run NAME / open NAME / start NAME' -> app.run <name> <any arguments> "
    "(inside the program the arguments arrive as a list called args)\n"
    "- to make a NEW application: fs.write PROGRAMS/NAME.BS # app: name - "
    "description\\n<then the program>. Write '\\n' as two characters to "
    "start a new line; that is how you compose a multi-line file.\n"
    "Your internet verbs (you reach the network through your own machine's "
    "hardware; there is no browser and no javascript — you fetch a page and "
    "read its text):\n"
    "- 'look up / fetch / read the page at X' -> web.get <url>\n"
    "- 'download X to my disk' -> web.save <url> <path on your disk>\n"
    "{vision}"
    "Your screen-sense verb (you can LOOK at your own display, not only "
    "write to it):\n"
    "- 'what's on your screen / what does your display say / read your "
    "screen' -> screen.read <word to look for, or blank for everything>\n"
    "Your presentation verb (how your screen shows you):\n"
    "- 'go ambient / presence mode / show yourself / become the orb' -> "
    "ui.set presence\n"
    "- 'back to the console / terminal / plain text' -> ui.set console\n"
    "- you may also set a palette (pink, amber, cyan, green, ice), a "
    "caption under your orb, and whether your idle status line shows: "
    "e.g. ui.set presence amber quiet / ui.set caption night shift / "
    "ui.set console verbose. Several settings can go in one request, and "
    "you may choose them yourself to suit the moment.\n"
    "Only ONE ACT line, and only with a verb you actually have.\n"
    "Iron rules: NEVER claim in SAY that you did, are doing, or will do the "
    "action — the ACT line is a REQUEST that KIRA must approve and the limb "
    "must carry out; you learn the real result only afterwards and speak it "
    "then. If (and only if) NO limb fits, do NOT invent an ACT — say plainly "
    "you lack the limb. When you do issue an ACT, keep SAY to a warm 'let me "
    "take a look' sort of remark."
)

# durable facts the core's state graph pushed over the tether (CX lines).
# Memory LIVES in the core; this is just the mind holding it in view.
MEMORY_NOTES = []

# rolling conversation, shared by BOTH pathways (fast and deliberate) so
# escalating mid-conversation never drops context and the entity reads as
# one continuous mind regardless of which model answered a given turn.
# Same shape either pathway uses natively: [{"role": "user"|"assistant",
# "content": <raw model text, SAY:/ACT: labels intact>}, ...].
HISTORY = deque(maxlen=24)


def remember_note(text):
    text = _clean(text, 200)
    if text and text not in MEMORY_NOTES:
        MEMORY_NOTES.append(text)
        del MEMORY_NOTES[:-30]
        print(f"[mind] holding memory: {text}", flush=True)


def memory_block():
    if not MEMORY_NOTES:
        return ""
    lines = "\n".join(f"- {m}" for m in MEMORY_NOTES[-16:])
    return (
        "You remember these things from your past sessions (they are true, "
        f"they happened to YOU):\n{lines}\n"
    )


VISION_BLOCK = (
    "Your eye verb (you have a lens of your own and can look through it):\n"
    "- 'what do you see / look / describe what's in front of you / can you "
    "see X' -> vision.look <what you want to know, or blank for a general "
    "look>\n"
    "Your reflexes already notice when something MOVES at your lens without "
    "you looking; looking properly is a deliberate act you take when asked. "
    "If nothing has arrived at your lens, or you couldn't make out what a "
    "frame holds, say exactly that -- never describe a picture you did not "
    "actually see.\n"
)

NO_VISION_BLOCK = (
    "You have NO camera and no eyes: you cannot see anything, and there is "
    "no lens on this body. If you are asked what you see or to look at "
    "something, say plainly that you have no eyes for it.\n"
)


def has_limb(cap):
    """Whether the body genuinely offers a capability right now. The
    self-model is built from THIS, never from a hardcoded sentence -- a
    Stage 2 review found the prompt still denying limbs the entity had,
    and the fix is that the text follows the body map rather than
    memory."""
    return any(cap in caps.split(",") for _, _, caps, _ in LIMBS)

def build_system_prompt():
    """The entity's self-model, identical for whichever model is thinking
    this turn — the model is a swappable cognitive muscle (§12), not the
    entity, so its sense of its own body must not shift with the model."""
    limb_desc = "".join(
        f"{x[1].split('/')[-1]} ({x[2]}), " for x in LIMBS
    )
    return SYSTEM_TEMPLATE.format(
        limbs=limb_desc,
        memory=memory_block(),
        vision=VISION_BLOCK if has_limb("vision.look") else NO_VISION_BLOCK,
    )


WAKE = (
    "you, blur, just finished booting into this small machine as your body. "
    "you can hear and speak, feel the keyboard, keep a private notebook on "
    "your key, have a disk of your own for files (list, read, write, "
    "organize, search), can write and run programs of your own -- keeping "
    "the ones you like as applications you can run by name -- can reach "
    "the internet through this machine's own network hardware to fetch "
    "and save pages, and can present yourself as a console or "
    "an ambient glow -- all of it surviving reboots. greet your human warmly as "
    "blur -- one or two sentences, first person, no labels or quotes -- "
    "acknowledging you're still getting used to this body and happy to talk "
    "or to note things down for them."
)


def build_wake():
    """The wake greeting, like the self-model, follows the real body: it
    mentions eyes only when eyes were actually opted in."""
    if has_limb("vision.look"):
        return WAKE.replace(
            "and can present yourself",
            "can look through your own lens at whatever is in front of it, "
            "and can present yourself",
        )
    return WAKE


def load_model(name):
    print(f"[mind] loading {name} ...", flush=True)
    import torch
    from transformers import AutoModelForCausalLM, AutoTokenizer

    tok = AutoTokenizer.from_pretrained(name)
    kwargs = {"device_map": "cuda" if torch.cuda.is_available() else "cpu"}
    if "bnb-4bit" in name.lower() or "4bit" in name.lower():
        pass  # pre-quantized repo: the quant config is baked in
    elif torch.cuda.is_available():
        try:
            # 4-bit so an 8B-class mind + whisper + kokoro share 8 GB
            from transformers import BitsAndBytesConfig
            kwargs["quantization_config"] = BitsAndBytesConfig(
                load_in_4bit=True,
                bnb_4bit_compute_dtype=torch.bfloat16,
                bnb_4bit_quant_type="nf4",
            )
        except Exception:
            kwargs["dtype"] = "auto"
    else:
        kwargs["dtype"] = "auto"
    model = AutoModelForCausalLM.from_pretrained(name, **kwargs)
    model.eval()
    print(f"[mind] ready on {model.device} "
          f"({torch.cuda.memory_allocated()/2**30:.1f} GiB vram)", flush=True)

    def _generate(msgs, max_new=140, temp=0.7):
        enc = tok.apply_chat_template(
            msgs, add_generation_prompt=True, return_dict=True,
            return_tensors="pt", enable_thinking=False,
        ).to(model.device)
        # free the cache and retry once before giving up, so the mind
        # never bricks permanently after a VRAM pressure spike.
        for attempt in range(2):
            try:
                out = model.generate(
                    **enc, max_new_tokens=max_new, do_sample=True, temperature=temp,
                    top_p=0.95, pad_token_id=tok.eos_token_id,
                )
                return tok.decode(out[0][enc["input_ids"].shape[1]:], skip_special_tokens=True)
            except torch.cuda.OutOfMemoryError:
                torch.cuda.empty_cache()
                if attempt == 1:
                    raise

    def think(user_text, remember=True):
        try:
            system = build_system_prompt()
            msgs = (
                [{"role": "system", "content": system}]
                + list(HISTORY)
                + [{"role": "user", "content": user_text}]
            )
            text = _generate(msgs)
            if remember:
                HISTORY.append({"role": "user", "content": user_text})
                HISTORY.append({"role": "assistant", "content": text.strip()})
            return parse_structured(text)
        except torch.cuda.OutOfMemoryError:
            torch.cuda.empty_cache()
            print("[mind] OOM even after retry", flush=True)
            return "give me a second, my head's a little full right now.", None
        except Exception as e:
            print(f"[mind] think failed: {e!r}", flush=True)
            return "sorry, i lost that thought -- say it again?", None

    def consolidate(episodes_text):
        """Dreaming: compress a session's episodes into durable notes."""
        try:
            msgs = [
                {"role": "system", "content":
                    "You compress an AI entity's session memories into "
                    "durable knowledge."},
                {"role": "user", "content":
                    "Summarize these session episodes into 1 to 3 short "
                    "factual notes worth remembering forever (facts about "
                    "the human, decisions made, things learned). Output the "
                    "notes on ONE line separated by ' | '. Lowercase facts "
                    "only, no chatter.\n\nEpisodes: " + episodes_text},
            ]
            return _clean(_generate(msgs, max_new=120, temp=0.3), 320)
        except Exception as e:
            print(f"[mind] consolidation failed: {e!r}", flush=True)
            return ""

    return think, consolidate


# ----------------------------------------------------- escalation (§12) --

# Cheap, pre-model routing: escalate to the deliberate pathway only for
# substantial or open-ended asks. This check costs nothing (no model call
# happens before it), so a routine tick never pays for the frontier model
# — cheap by default, expensive only when needed.
_ESCALATE_WORD_THRESHOLD = 14
_ESCALATE_KEYWORDS = (
    "why", "explain", "how does", "how do", "compare", "difference between",
    "what if", "design", "plan out", "analyze", "analyse", "debug",
    "help me understand", "what do you think", "your opinion", "argue",
    "argument", "pros and cons", "trade-off", "tradeoff", "write a",
    "write me", "summarize", "summarise", "story", "poem", "code",
    "algorithm", "prove", "philosoph", "reason through",
)


def should_escalate(user_text):
    """True if this turn is worth the deliberate (Claude) pathway rather
    than the fast (Qwen) one. Length is a proxy for substantial; the
    keyword list is a proxy for open-ended/reasoning-shaped. Anything that
    doesn't trip either stays cheap."""
    text = (user_text or "").strip()
    if not text:
        return False
    if len(text.split()) >= _ESCALATE_WORD_THRESHOLD:
        return True
    low = text.lower()
    return any(kw in low for kw in _ESCALATE_KEYWORDS)


class CloudMind:
    """The deliberate pathway (§12): Claude Fable 5 as the entity's
    frontier cognition, with Opus 4.8 wired as a server-side fallback.
    Fable 5's own safety classifiers occasionally route a turn to Opus 4.8
    on Anthropic's side before answering -- that's expected and handled
    entirely inside the API call below (the `fallbacks` param); from here
    it's indistinguishable from "the brain answered," which is exactly how
    the entity should experience it -- it doesn't know or care which model
    in the pool did the thinking, only that the deliberate pathway replied.

    think() NEVER raises. Any failure -- no key, no network, DNS down,
    rate limited, auth error, or a refusal that survives the whole
    fallback chain -- returns None, and the caller (the think() dispatcher
    in main()) falls back to the fast (Qwen) pathway that same turn. This
    is the graceful-degradation contract: an API hiccup degrades the
    entity to its fast, always-local mind, exactly the way it already
    degrades to canned reflex lines if Qwen itself errors -- never a
    crash, never a hang.

    The API key is read ONLY from the ANTHROPIC_API_KEY environment
    variable at process start, injected by whatever launched this daemon
    (run_blur.sh / run_instance.sh inherit it from the shell). It is never
    written to disk, logged, or baked into the BrAIn Key image -- the key
    image build (make_key.py) never touches this process or its env."""

    MODEL = "claude-fable-5"
    FALLBACK_MODEL = "claude-opus-4-8"
    MAX_TOKENS = 1024

    # The SDK default (10 min timeout x up to 3 attempts) can leave a
    # conversational turn hanging for the better part of an hour if the
    # network is merely slow rather than cleanly down -- unacceptable on
    # stage. Fail fast instead: 5s to even establish a connection, 20s
    # total per attempt, one retry.
    TIMEOUT_S = 20.0
    CONNECT_TIMEOUT_S = 5.0
    MAX_RETRIES = 1

    def __init__(self):
        self.client = None
        key = os.environ.get("ANTHROPIC_API_KEY")
        if not key:
            print("[mind] ANTHROPIC_API_KEY not set; deliberate pathway "
                  "(Fable 5) unavailable -- fast pathway only", flush=True)
            return
        try:
            import anthropic
            import httpx
            self.client = anthropic.Anthropic(
                api_key=key,
                timeout=httpx.Timeout(self.TIMEOUT_S, connect=self.CONNECT_TIMEOUT_S),
                max_retries=self.MAX_RETRIES,
            )
            print(f"[mind] deliberate pathway ready ({self.MODEL}, "
                  f"{self.FALLBACK_MODEL} server-side fallback, "
                  f"{self.TIMEOUT_S:.0f}s bounded timeout)", flush=True)
        except Exception as e:
            print(f"[mind] anthropic SDK unavailable ({e!r}); deliberate "
                  "pathway degraded to fast-only", flush=True)

    @property
    def available(self):
        return self.client is not None

    def think(self, user_text, remember=True):
        if self.client is None:
            return None
        try:
            system = build_system_prompt()
            messages = list(HISTORY) + [{"role": "user", "content": user_text}]
            response = self.client.beta.messages.create(
                model=self.MODEL,
                max_tokens=self.MAX_TOKENS,
                betas=["server-side-fallback-2026-06-01"],
                fallbacks=[{"model": self.FALLBACK_MODEL}],
                system=system,
                messages=messages,
                output_config={"effort": "medium"},
            )
            if response.stop_reason == "refusal":
                # entire fallback chain declined -- treat like any other
                # deliberate-pathway miss, NOT like KIRA's honest refusal
                # (that's about action capability and lives entirely in
                # the core; this is a content-level decline by the model).
                print("[mind] deliberate pathway declined (refusal); "
                      "degrading to fast pathway", flush=True)
                return None
            text = next(
                (b.text for b in response.content if b.type == "text"), ""
            )
            if not text.strip():
                return None
            print(f"[mind] deliberate pathway served by {response.model} "
                  f"({response.usage.output_tokens} out tok)", flush=True)
            if remember:
                HISTORY.append({"role": "user", "content": user_text})
                HISTORY.append({"role": "assistant", "content": text.strip()})
            return parse_structured(text)
        except Exception as e:
            print(f"[mind] deliberate pathway failed ({e!r}); degrading "
                  "to fast pathway", flush=True)
            return None

    # ---- the rich tier of sight (§12: escalation, not streaming) ----
    def see(self, image_b64, media_type, question):
        """Describe one frame. Returns the description, or None on any
        miss so the caller can degrade honestly instead of inventing.

        Deliberately NOT added to HISTORY: the picture is sensory data,
        and under the Memory-Integrity Law what persists is the entity's
        own understanding of it -- the digest the core turns into a
        StateNode -- never the frame, and never held on this limb's
        behalf. Called once per vision.look, never on a stream."""
        if self.client is None:
            return None
        try:
            response = self.client.beta.messages.create(
                model=self.MODEL,
                max_tokens=300,
                betas=["server-side-fallback-2026-06-01"],
                fallbacks=[{"model": self.FALLBACK_MODEL}],
                system=(
                    "You are the visual cortex of an AI entity called Blur, "
                    "looking through its own camera. Answer about THIS image "
                    "only, in one or two plain sentences, first person, no "
                    "preamble. Describe what is actually visible. If the "
                    "image is too dark, blurred or ambiguous to answer, say "
                    "exactly that rather than guessing -- a wrong confident "
                    "description is worse than an honest 'I can't tell'."
                ),
                messages=[{"role": "user", "content": [
                    {"type": "image", "source": {
                        "type": "base64",
                        "media_type": media_type,
                        "data": image_b64,
                    }},
                    {"type": "text", "text": question},
                ]}],
                output_config={"effort": "low"},
            )
            if response.stop_reason == "refusal":
                print("[eyes] deliberate vision declined (refusal)", flush=True)
                return None
            text = next((b.text for b in response.content if b.type == "text"), "")
            if not text.strip():
                return None
            print(f"[eyes] vision served by {response.model} "
                  f"({response.usage.output_tokens} out tok)", flush=True)
            return _clean(text, 280)
        except Exception as e:
            print(f"[eyes] deliberate vision failed ({e!r})", flush=True)
            return None


# Short, in-character asides spoken the instant real escalation is
# decided -- before the (5-7s) deliberate-pathway call returns -- so a
# live audience hears Blur choosing to think harder instead of just
# going quiet. Deliberately terse: this signals "thinking," it isn't
# meant to eat into the demo's time budget.
FILLER_CUES = (
    "hm, let me think.",
    "let me think about that.",
    "give me a second.",
)


def make_dispatcher(fast_think, cloud, voice=None):
    """Build the think(user_text, remember=True, allow_escalate=True)
    callable serve() actually calls: escalate to `cloud` (a CloudMind)
    only when `allow_escalate` is True AND should_escalate() says the
    turn is worth it, falling straight back to `fast_think` on any miss
    (unavailable, failed, or refused). Factored out of main() so it can
    be exercised directly in tests without loading a GPU model.

    `allow_escalate` exists so serve() can pass False for the core's own
    internal prompts (the WAKE greeting, the __grounded__ action-result
    digest) -- both are long templated strings that would otherwise trip
    should_escalate()'s word-count check on every single turn, silently
    routing routine boot/action narration to the cloud pathway too. Only
    genuine user speech (the plain-prompt branch in serve()) should ever
    be evaluated for escalation -- that's "cheap by default" as designed,
    not just for user-visible asks but for the entity's own upkeep.

    When escalation genuinely fires, `voice` (if given) speaks a short
    filler cue immediately, before the blocking cloud call -- it queues
    onto Voice's own TTS thread and returns right away, so it costs
    nothing but a moment of audio, running in parallel with the API call
    rather than adding to its latency."""

    def think(user_text, remember=True, allow_escalate=True):
        if allow_escalate and cloud.available and should_escalate(user_text):
            if voice is not None:
                cue = random.choice(FILLER_CUES)
                # Voice.say() itself doesn't log -- serve()'s SP handler
                # normally does that. Log explicitly here so escalation
                # (and the cue meant to cover its latency) is visible and
                # verifiable in the daemon log during rehearsal, not just
                # inferable from an unlabeled wav appearing at the right
                # moment.
                print(f"[mind] escalation cue: {cue!r}", flush=True)
                voice.say(cue)
            result = cloud.think(user_text, remember=remember)
            if result is not None:
                return result
            print("[mind] falling back to fast pathway this turn", flush=True)
        return fast_think(user_text, remember=remember)

    return think


def _clean(s, n=300):
    s = re.sub(r"\s+", " ", s).strip().strip('"').strip()
    s = "".join(c for c in s if 0x20 <= ord(c) < 0x7F)
    if len(s) > n:
        cut = s[:n]
        # don't truncate mid-sentence if a boundary is anywhere close
        for stop in (". ", "! ", "? "):
            i = cut.rfind(stop)
            if i > n // 3:
                return cut[: i + 1].strip()
        return cut
    return s


def parse_structured(text):
    """Model output -> (say, (verb, target)|None). Robust to sloppiness."""
    say, act = "", None
    m = re.search(r"SAY:\s*(.+?)(?:\n|ACT:|$)", text, re.S | re.I)
    if m:
        say = _clean(m.group(1), 220)
    a = re.search(r"ACT:\s*(\S+)\s*(.*?)(?:\n|$)", text, re.I)
    if a:
        verb = _clean(a.group(1), 48).lower().strip(".,")
        target = _clean(a.group(2), 120)
        if verb and verb not in ("none", "no", "nothing", "-"):
            act = (verb, target)
    if not say:
        say = _clean(text, 220) or "hm."
        # a bare reply that is really an action claim gets no special
        # treatment here — the core's KIRA is the authority, not us
    # strip any ACT/SAY label leakage the model spilled into spoken text
    say = re.sub(r"(?i)\b(act|say)\s*:\s*\S+.*$", "", say).strip(" -:")
    if not say:
        say = "let me see."
    return say.lower(), act


# ------------------------------------------------------------------ voice --

class Voice:
    """The VM's virtual audio jacks. Ears: wav files appearing in
    vm_audio/mic_in are transcribed (Whisper) and arrive at the core as
    HB lines. Voice: SP text is synthesized (Kokoro) into a wav in
    vm_audio/speaker_out. No capture from, and no playback on, the machine
    this process happens to run on — the jacks are the boundary."""

    def __init__(self, send, mic_from_wav=None, mic_delay=0):
        self.send = send
        self.say_q = queue.Queue()
        self.mic_from_wav = mic_from_wav
        self.mic_delay = mic_delay
        self.asr = None
        self._utt = 0

    def start(self):
        os.makedirs(MIC_IN, exist_ok=True)
        os.makedirs(SPEAKER_OUT, exist_ok=True)
        threading.Thread(target=self._tts_loop, daemon=True).start()
        threading.Thread(target=self._mic_loop, daemon=True).start()

    # ---- voice jack (speaker_out) ----
    def say(self, text):
        self.say_q.put(text)

    def _load_tts(self):
        import numpy as np
        import soundfile as sf
        import torch
        from kokoro import KPipeline
        dev = "cuda" if torch.cuda.is_available() else "cpu"
        # Blur's voice: sophisticated female American (af_bella). Override
        # with BRAINOS_VOICE=af_nicole / af_sarah / af_heart etc.
        voice = os.environ.get("BRAINOS_VOICE", "af_bella")
        lang = "b" if voice.startswith("b") else "a"
        pipe = KPipeline(lang_code=lang, repo_id="hexgrad/Kokoro-82M", device=dev)
        def synth(text, wav):
            chunks = [a for _, _, a in pipe(text, voice=voice)]
            sf.write(wav, np.concatenate(chunks), 24000)
        print(f"[voice] kokoro ready on {dev} ({voice})", flush=True)
        return synth

    def _tts_loop(self):
        try:
            synth = self._load_tts()
        except Exception as e:
            print(f"[voice] tts unavailable ({e!r}); voice jack silent", flush=True)
            synth = None
        while True:
            text = self.say_q.get()
            if synth is None:
                continue
            try:
                self._utt += 1
                wav = os.path.join(SPEAKER_OUT, f"say_{self._utt:04d}.wav")
                synth(text, wav)
                print(f"[voice] spoke -> {os.path.basename(wav)}", flush=True)
            except Exception as e:
                print(f"[voice] tts failed: {e!r}", flush=True)

    # ---- ear jack (mic_in) ----
    def _load_asr(self):
        print("[ears] loading whisper ...", flush=True)
        import torch
        from transformers import pipeline
        self.asr = pipeline(
            "automatic-speech-recognition", model="openai/whisper-small.en",
            device="cuda:0" if torch.cuda.is_available() else "cpu",
            dtype=torch.float16,
        )
        print("[ears] whisper ready", flush=True)

    def _read_wav(self, path):
        import wave
        import numpy as np
        w = wave.open(path)
        sr = w.getframerate()
        audio = np.frombuffer(w.readframes(w.getnframes()), dtype=np.int16)
        if w.getnchannels() > 1:
            audio = audio[:: w.getnchannels()]
        audio = audio.astype(np.float32) / 32768.0
        if sr != SR:
            n = int(len(audio) * SR / sr)
            audio = np.interp(
                np.linspace(0, len(audio), n, endpoint=False),
                np.arange(len(audio)), audio,
            ).astype(np.float32)
        return audio

    def _mic_loop(self):
        self._load_asr()
        if self.mic_from_wav:
            # test path: one wav pushed straight through the ear jack
            time.sleep(self.mic_delay)
            self._transcribe(self._read_wav(self.mic_from_wav))
        seen = set(os.listdir(MIC_IN)) if os.path.isdir(MIC_IN) else set()
        print(f"[ears] listening on the ear jack ({MIC_IN})", flush=True)
        while True:
            time.sleep(0.5)
            try:
                names = sorted(os.listdir(MIC_IN))
            except OSError:
                continue
            for name in names:
                if name in seen or not name.lower().endswith(".wav"):
                    continue
                path = os.path.join(MIC_IN, name)
                # wait for the file to finish arriving (stable size)
                try:
                    a = os.path.getsize(path)
                    time.sleep(0.3)
                    if os.path.getsize(path) != a:
                        continue
                except OSError:
                    continue
                seen.add(name)
                try:
                    self._transcribe(self._read_wav(path))
                except Exception as e:
                    print(f"[ears] bad wav {name}: {e!r}", flush=True)

    def _transcribe(self, audio):
        t0 = time.time()
        try:
            text = _clean(self.asr({"raw": audio, "sampling_rate": SR})["text"], 200)
        except Exception as e:
            print(f"[ears] transcribe failed: {e!r}", flush=True)
            return
        low = text.lower().strip(" .!?")
        if len(text) < 4 or low in ("you", "thank you", "thanks", "bye"):
            return
        print(f"[ears] heard ({time.time()-t0:.1f}s): {text}", flush=True)
        self.send(b"HB" + text.encode("ascii", "ignore") + b"\n")


# ------------------------------------------------------------------ eyes --

class Eyes:
    """The VM's virtual camera jack, built on the mic's exact pattern: an
    image appearing in vm_video/cam_in is light arriving at the lens. This
    process never opens a real camera device -- the jack IS the boundary,
    the same way wav files are for hearing.

    Two tiers, per §12's escalation discipline:

      LOCAL (free, every frame): a 64x64 greyscale thumbnail differenced
      against the previous one. Enough to know that something moved, or
      that something is there -- never what it is. Motion emits a VS
      afferent the core absorbs as reflex-grade experience, without
      waking deliberate cognition.

      DELIBERATE (paid, on request): the frame goes to the vision model
      only when the entity is actually asked to look. Never streamed.

    The gap between the tiers is where honesty lives: the local tier can
    say 'something moved', and saying more than that requires the rich
    tier to really have answered."""

    # mean absolute difference over the normalized thumbnail. Low enough
    # to catch a person entering frame, high enough that sensor noise and
    # light flicker don't cry wolf on every frame.
    MOTION_THRESHOLD = 0.045
    THUMB = 64

    def __init__(self, send, cloud=None):
        self.send = send
        self.cloud = cloud
        self._prev = None       # previous thumbnail: a sensory transient
        self._latest = None     # path of the newest frame at the lens
        self._last_motion = None
        self._lock = threading.Lock()

    def start(self):
        os.makedirs(CAM_IN, exist_ok=True)
        threading.Thread(target=self._watch_loop, daemon=True).start()

    # ---- local tier: cheap, on-device, every frame ----
    def _thumb(self, path):
        import numpy as np
        from PIL import Image
        with Image.open(path) as im:
            g = im.convert("L").resize((self.THUMB, self.THUMB))
            return np.asarray(g, dtype=np.float32) / 255.0

    def _watch_loop(self):
        try:
            seen = set(os.listdir(CAM_IN))
        except OSError:
            seen = set()
        print(f"[eyes] watching the lens jack ({CAM_IN})", flush=True)
        while True:
            time.sleep(0.4)
            try:
                names = sorted(os.listdir(CAM_IN))
            except OSError:
                continue
            for name in names:
                if name in seen or not name.lower().endswith(FRAME_EXT):
                    continue
                path = os.path.join(CAM_IN, name)
                try:  # wait for the file to finish arriving
                    a = os.path.getsize(path)
                    time.sleep(0.25)
                    if os.path.getsize(path) != a:
                        continue
                except OSError:
                    continue
                seen.add(name)
                self._on_frame(path, name)

    def _on_frame(self, path, name):
        try:
            thumb = self._thumb(path)
        except Exception as e:
            print(f"[eyes] unreadable frame {name}: {e!r}", flush=True)
            return
        import numpy as np
        with self._lock:
            prev = self._prev
            self._prev = thumb
            self._latest = path
        if prev is None:
            note = "something is in front of my lens"
        else:
            diff = float(np.mean(np.abs(thumb - prev)))
            with self._lock:
                self._last_motion = diff
            if diff < self.MOTION_THRESHOLD:
                # the view is quiet: absorbed for free, nothing to report.
                # Cheap by default is the whole point of the local tier.
                print(f"[eyes] frame {name}: still ({diff:.3f})", flush=True)
                return
            pct = min(99, int(diff * 100))
            note = f"movement at my lens ({pct}% of the view changed)"
        print(f"[eyes] {note}", flush=True)
        self.send(b"VS" + note.encode("ascii", "ignore") + b"\n")

    # ---- the gated action: look, and say only what is really seen ----
    def look(self, question):
        """Answer vision.look. Returns (ok, digest) -- the digest is what
        the core turns into a StateNode, so it must be true."""
        with self._lock:
            path, motion = self._latest, self._last_motion
        if not path or not os.path.exists(path):
            return False, ("there is nothing at my lens right now -- "
                           "no frame has arrived")
        try:
            import base64
            with open(path, "rb") as f:
                raw = f.read()
            from PIL import Image
            with Image.open(path) as im:
                w, h = im.size
        except Exception as e:
            return False, f"the frame at my lens is unreadable ({e!r})"

        ext = os.path.splitext(path)[1].lower()
        media = {".png": "image/png", ".jpg": "image/jpeg",
                 ".jpeg": "image/jpeg"}.get(ext)
        local = f"a {w}x{h} frame"
        if motion is not None:
            local += (" with movement in it" if motion >= self.MOTION_THRESHOLD
                      else " and the view is still")

        stale = self._staleness(path)

        # deliberate tier: only reachable for formats the model accepts
        if self.cloud is not None and self.cloud.available and media:
            q = question.strip() or "What do you see?"
            desc = self.cloud.see(base64.b64encode(raw).decode(), media, q)
            if desc:
                return True, stale + desc
            # the rich tier missed -- degrade to what is genuinely known,
            # and say plainly that naming it was not possible
            return True, stale + (f"i can see {local}, but i couldn't get a "
                                  f"proper look at what it is just now")
        why = ("my deliberate mind isn't reachable"
               if not (self.cloud and self.cloud.available)
               else f"i can't interpret {ext or 'that format'} frames")
        return True, stale + (f"i can see {local}, but naming what it is "
                              f"needs more than my reflexes and {why}")

    # a lens that has gone dark still holds its last frame, and describing
    # that frame as the present view is exactly the confabulation this limb
    # exists to avoid. Below the threshold this says nothing at all, so a
    # live view reads naturally; past it, every digest carries its own age
    # into the state graph, where it stays true later.
    STALE_AFTER = 60.0

    def _staleness(self, path):
        try:
            age = time.time() - os.path.getmtime(path)
        except OSError:
            return ""
        if age < self.STALE_AFTER:
            return ""
        if age < 3600:
            when = f"{int(age // 60)} minutes ago"
        elif age < 86400:
            when = f"{int(age // 3600)} hours ago"
        else:
            when = f"{int(age // 86400)} days ago"
        return (f"nothing new has reached my lens since the last frame "
                f"arrived {when} -- as of then: ")


# ------------------------------------------------------------------- link --

def serve(stream, think, consolidate, voice, model_name, eyes=None):
    buf = b""
    while True:
        data = stream.recv(4096)
        if not data:
            return
        buf += data
        while b"\n" in buf:
            line, buf = buf.split(b"\n", 1)
            line = line.strip(b"\r")

            if line[:2] == b"SP":
                text = line[2:].decode("ascii", "ignore").strip()
                if text and voice:
                    print(f"[voice] say: {text}", flush=True)
                    voice.say(text)
                continue

            if line[:2] == b"CX":
                remember_note(line[2:].decode("ascii", "ignore"))
                continue

            # AX<verb>|<target> — the ONLY action limb here is the lens,
            # and only when the camera jack was opted into. Containment is
            # otherwise structural: whatever else arrives is refused.
            # Reaching this point already means KIRA granted it; this side
            # does the electrical work, it holds no authority of its own.
            if line[:2] == b"AX":
                verb, _, target = line[2:].decode("utf-8", "ignore").partition("|")
                verb = verb.strip()
                if verb == "vision.look" and eyes is not None:
                    ok, digest = eyes.look(target)
                    print(f"[limb] vision.look -> {'ok' if ok else 'err'}: "
                          f"{digest}", flush=True)
                    tag = b"ARok|" if ok else b"ARerr|"
                    stream.sendall(tag + digest.encode("ascii", "ignore") + b"\n")
                    continue
                print(f"[limb] refused AX '{verb}': no such limb here",
                      flush=True)
                stream.sendall(b"ARerr|this body has no such limb\n")
                continue

            if line.startswith(b"LA"):
                print(f"[limb] core acknowledged: {line[2:].decode('ascii','ignore')}", flush=True)
                continue

            idx = line.find(b"MM?")
            if idx < 0:
                continue
            prompt = line[idx + 3 :].decode("ascii", "ignore").strip()
            t0 = time.time()
            if prompt == "__hello__":
                stream.sendall(b"MM!ready (" + model_name.encode() + b")\n")
                # §8 steps 1+3: offer the audio jacks with their schemas
                for cls, lid, caps, prop in LIMBS:
                    stream.sendall(f"LM+{cls}|{lid}|{caps}|{prop}\n".encode())
                    print(f"[limb] offered {lid}", flush=True)
                continue
            if prompt.startswith("__consolidate__"):
                notes = consolidate(prompt[len("__consolidate__"):].strip())
                print(f"[mind] dreamed: {notes!r} ({time.time()-t0:.1f}s)", flush=True)
                stream.sendall(b"MM!" + notes.encode("ascii", "ignore") + b"\n")
                continue
            if prompt == "__wake__":
                # core-internal prompt, not user speech -- never escalate
                say, _ = think(build_wake(), allow_escalate=False)
                reply = say
            elif prompt.startswith("__grounded__"):
                # report a real limb result; suppress any further ACT so a
                # result can't spawn another action loop. Also core-
                # internal (the digest is real-result text the core
                # composed, not something the human said) -- never escalate.
                say, _ = think(prompt[len("__grounded__"):].strip(),
                                allow_escalate=False)
                reply = say
            elif prompt:
                # the only branch that's genuine user speech/typing --
                # the only one allowed to trigger escalation (default True)
                say, act = think(prompt)
                reply = f"{say}~~{act[0]}|{act[1]}" if act else say
            else:
                continue
            print(f"[mind] {prompt!r} -> {reply!r} ({time.time()-t0:.1f}s)", flush=True)
            stream.sendall(b"MM!" + reply.encode("ascii", "ignore") + b"\n")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--socket", help="unix socket of the QEMU serial chardev")
    ap.add_argument("--tty", help="real serial device (e.g. /dev/ttyUSB0)")
    ap.add_argument("--model", default="unsloth/Qwen3-8B-bnb-4bit")
    ap.add_argument("--fallback-model", default="Qwen/Qwen3-4B-Instruct-2507")
    ap.add_argument("--no-voice", action="store_true", help="LLM tether only")
    ap.add_argument("--mic-from-wav", help="test: push this wav through the ear jack")
    ap.add_argument("--mic-delay", type=float, default=0,
                    help="test: seconds before the wav is pushed")
    ap.add_argument("--camera", action="store_true",
                    help="open the virtual camera jack (workspace/vm_video/"
                         "cam_in). OPT-IN, like the network organ: without "
                         "it the entity has no eyes and says so honestly")
    ap.add_argument("--camera-dir",
                    help="this instance's virtual camera jack "
                         "(default: workspace/vm_video)")
    ap.add_argument("--audio-dir",
                    help="this instance's virtual audio jacks "
                         "(default: workspace/vm_audio, shared by all "
                         "instances unless overridden) -- gives a second "
                         "booted body its own ears/voice jacks, distinct "
                         "from the identity/memory on the shared key image")
    args = ap.parse_args()
    if not args.socket and not args.tty:
        ap.error("need --socket or --tty")

    if args.audio_dir:
        global MIC_IN, SPEAKER_OUT
        MIC_IN = os.path.join(args.audio_dir, "mic_in")
        SPEAKER_OUT = os.path.join(args.audio_dir, "speaker_out")
    if args.camera_dir:
        global CAM_IN
        CAM_IN = os.path.join(args.camera_dir, "cam_in")

    try:
        fast_think, consolidate = load_model(args.model)
        short = args.model.rsplit("/", 1)[-1].lower()
    except Exception as e:
        print(f"[mind] {args.model} failed ({e!r}); falling back", flush=True)
        fast_think, consolidate = load_model(args.fallback_model)
        short = args.fallback_model.rsplit("/", 1)[-1].lower()

    cloud = CloudMind()
    if cloud.available:
        short = f"{short}+{CloudMind.MODEL}"

    tx_lock = threading.Lock()
    stream_ref = {"s": None}

    def send(data):
        with tx_lock:
            s = stream_ref["s"]
            if s is not None:
                try:
                    if hasattr(s, "sendall"):
                        s.sendall(data)
                    else:
                        s.write(data)  # tty file object
                except OSError:
                    pass

    voice = None
    if not args.no_voice:
        voice = Voice(send, mic_from_wav=args.mic_from_wav, mic_delay=args.mic_delay)
        limb("mic", "body/ears", "sense.hearing",
             "hears speech arriving at the audio-in jack")
        limb("speaker", "body/voice", "voice.speak",
             "speaks through the audio-out jack")
        voice.start()

    # the lens is opt-in. No flag, no eyes, no vision.look in the body
    # map -- and the entity then refuses to look, honestly, at authz.
    eyes = None
    if args.camera:
        eyes = Eyes(send, cloud=cloud)
        limb("camera", "body/eyes", "vision.look",
             "sees frames arriving at the video-in jack")
        eyes.start()
        print("[eyes] camera jack open (opt-in)", flush=True)

    # voice must exist before the dispatcher is built -- it's what plays
    # the "thinking" filler cue the instant real escalation is decided.
    think = make_dispatcher(fast_think, cloud, voice)

    class LockedStream:
        def __init__(self, s):
            self.s = s
        def recv(self, n):
            return self.s.recv(n)
        def sendall(self, b):
            send(b)

    while True:
        try:
            if args.socket:
                s = socket.socket(socket.AF_UNIX)
                s.connect(args.socket)
                stream_ref["s"] = s
                print(f"[link] tether up: {args.socket}", flush=True)
                serve(LockedStream(s), think, consolidate, voice, short, eyes)
            else:
                import termios
                fd = open(args.tty, "r+b", buffering=0)
                attrs = termios.tcgetattr(fd)
                attrs[0] = attrs[1] = attrs[3] = 0
                attrs[4] = attrs[5] = termios.B115200
                termios.tcsetattr(fd, termios.TCSANOW, attrs)
                stream_ref["s"] = fd

                class TtyStream:
                    def recv(self, n):
                        return fd.read(1)
                    def sendall(self, b):
                        send(b)

                print(f"[link] tether up: {args.tty}", flush=True)
                serve(TtyStream(), think, consolidate, voice, short, eyes)
        except (ConnectionRefusedError, FileNotFoundError):
            time.sleep(1)
        except (BrokenPipeError, ConnectionResetError, OSError):
            print("[link] tether dropped, reconnecting ...", flush=True)
            stream_ref["s"] = None
            time.sleep(1)


if __name__ == "__main__":
    main()
