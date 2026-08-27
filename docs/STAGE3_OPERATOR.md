# BrAInOS Stage 3 — "the Operator" (production body)

Direction from soapai, 2026-08-26 (verbatim intent): *finish the production
BrAInOS — declare an instance, load it on a USB, and BrAInOS is the sole
operator of the device. Control anything, orchestrate tasks, teach the user,
use any app, build any UI element, be incredibly fast (slower than a normal
OS = useless), main model on the Fable API, smaller models loaded too, still
ONE entity. Enable people to do 1000x more.*

This document is the spec every Stage 3 component is built against.

## 1. Shape of the product

One USB image, `brainos.img` (GPT):

| part | label            | fs      | contents |
|------|------------------|---------|----------|
| p1   | `BRAINOS-BOOT`   | FAT32   | `/EFI/BOOT/BOOTX64.EFI` = the BrAIn core (seed, unchanged identity/KIRA/attestation); `/EFI/BRAIN/BODY.EFI` = the body (Linux UKI: kernel+initramfs+cmdline); `/BRAIN/*` = key, journal (EPI_A/B, NOTE_A/B), BOOT.LOG |
| p2   | `BRAINOS-BODY`   | ext4    | the body rootfs (Arch-based): kernel modules, firmware, systemd, NetworkManager, PipeWire, Hyprland (+sway fallback), chromium, the operator, the face, local models |
| p3   | `BRAINOS-SECURE` | raw 4M  | brain key record (unchanged) + `ANTHROPIC.KEY` record (see §6) |

Boot arc: firmware → core (identity, attestation, KIRA self-test, brand screen —
**fast path, ≤1.5 s total when BODY.EFI is present**) → KIRA-gated
`Action::Incorporate` → `LoadImage`/`StartImage` of `\EFI\BRAIN\BODY.EFI` → Linux
body → autologin session → Hyprland → `brainos-operator` + `brainos-face`.
If BODY.EFI is absent or fails to start, the core falls back to the Phase-0
experience loop exactly as today (nothing regresses).

The core and the body are ONE entity: both read/write the same journal on p1
(`/brain` inside Linux). The body's operator rehydrates the seed's episodic
memory at start and writes back on shutdown/periodically.

## 2. Speed contract (non-negotiable)

- The OS underneath is a normal, fast Linux. Apps run native. The entity is an
  operator layered on top, never in the path of ordinary input.
- Three cognition tiers, dispatched by the operator:
  1. **reflex** — no model. A compiled intent table (open app, volume, brightness,
     wifi, screenshot, window ops, type/click, search, lock, power…). Target
     < 5 ms from utterance to action.
  2. **fast** — local model (`llama-server`, Qwen2.5-1.5B-Instruct Q4_K_M, grammar-
     constrained JSON) → `{intent, verb, args}` or `escalate`. Target < 400 ms on CPU.
     Also: short chit-chat, disambiguation, narration of actions.
  3. **deliberate** — Fable API (`claude-fable-5`, `fallbacks: "default"`), streaming,
     full tool loop over all hands. Plans, multi-step orchestration, teaching,
     building UI, code. First token is shown/spoken immediately.
- Every tier is async; the face never blocks; a running deliberate task can be
  cancelled (`cancel`) at any time.
- Local model server starts at boot, warm, pinned in RAM. Whisper + piper likewise.

## 3. Hands (the operator's verbs, all KIRA-gated `Action::UseLimb`)

| verb | backing |
|------|---------|
| `shell.run <cmd>` | bash, timeout, bounded digest |
| `app.open <name>` / `app.list` / `app.close` | desktop entries (`/usr/share/applications`), `gtk-launch`/exec, wm close |
| `window.list/focus/move/resize/workspace/close/fullscreen/tile` | `hyprctl` (or `swaymsg`) behind a `wm` abstraction |
| `input.type <text>` / `input.key <combo>` / `input.click <x,y>` / `input.scroll` | `wtype` (text/keys), `ydotool` (pointer) |
| `screen.look [region]` | `grim` → Fable vision (deliberate) or local text via `hyprctl clients` + window titles |
| `web.open <url>` / `web.read` / `web.find <text>` / `web.click <sel>` / `web.type <sel> <text>` / `web.search <q>` | Chromium with `--remote-debugging-port=9222` (CDP over websocket) |
| `fs.list/read/write/mkdir/delete/move/stat/search` | real files under `/home/human` |
| `sys.volume/brightness/wifi/bluetooth/battery/power/lock/notify/clipboard` | `wpctl`, `brightnessctl`, `nmcli`, `bluetoothctl`, `upower`, `systemctl`, `hyprlock`, `notify-send`, `wl-copy` |
| `ui.build <name> <html>` / `ui.update` / `ui.close` | a face **surface** (WebKitGTK panel) the entity fills with HTML/JS — "build any UI element" |
| `ui.teach <x,y,w,h> <caption>` / `ui.point` / `ui.clear` | face highlight overlay — "teach the user" |
| `ui.say <text>` | face transcript + TTS |
| `notes.write/read` | the key journal (`/brain/NOTE_*`) |
| `task.plan <goal>` / `task.status` / `task.cancel` | orchestration: a deliberate plan executed step-by-step with progress on the face |
| `code.run <lang> <src>` | python3 / bash / node in a scratch dir — real language runtimes now |
| `voice.listen` / `voice.speak` | whisper.cpp / piper inside the body |

## 4. IPC contract: operator ⇄ face

Unix socket `$XDG_RUNTIME_DIR/brainos/operator.sock`, newline-delimited JSON.

operator → face:
- `{"t":"state","mode":"idle|listening|thinking|acting","tier":"reflex|fast|deliberate"}`
- `{"t":"say","id":..,"text":"...","delta":true|false,"done":true|false}`   (streaming)
- `{"t":"act","verb":"app.open","target":"firefox","status":"grant|deny|ok|err","digest":"..."}`
- `{"t":"surface","name":"x","html":"..."}` / `{"t":"surface","name":"x","close":true}`
- `{"t":"teach","x":..,"y":..,"w":..,"h":..,"caption":"..."}` / `{"t":"teach","clear":true}`
- `{"t":"task","id":..,"title":"...","steps":[{"title","status"}],"done":bool}`
- `{"t":"heard","text":"..."}`   (what the ears made of the last push-to-talk — the human's own line)
- `{"t":"need_key"}`   (no API key on p3 — face asks the human)

face → operator:
- `{"t":"utter","text":"..."}`  `{"t":"ptt","on":true|false}`  `{"t":"cancel"}`
  - `ptt on` starts `pw-record` on the default source; `ptt off` stops it,
    whisper transcribes, the text runs the same tiers as a typed utterance, and
    the reply (gathered off the `say` stream) is spoken with piper → `pw-play`.
    Silence is reported as idle, not as an utterance; `cancel` kills capture and
    playback. Spoken text is normalised before the reflex table ("Open the
    terminal." → `app.open terminal`).
- `{"t":"surface_event","name":"x","event":"click","data":{...}}`
- `{"t":"set_key","key":"sk-ant-..."}`

## 5. Body filesystem layout

```
/opt/brainos/bin/brainos-operator      Rust, uses ../mind rlib (KIRA/state/journal)
/opt/brainos/face/                     Python GTK4 layer-shell face
/opt/brainos/models/qwen2.5-1.5b-instruct-q4_k_m.gguf
/opt/brainos/models/ggml-base.en.bin   whisper
/opt/brainos/voices/*.onnx(+json)      piper
/etc/brainos/operator.toml             ports, model paths, tier thresholds
/brain                                 p1 mounted (journal, key pub)
/home/human                            the human's files; session user `human` (uid 1000)
```

Session: agetty autologin `human` on tty1 → `~/.bash_profile` → `brainos-session`
(starts Hyprland with software-render fallbacks; if it dies in 5 s, sway) →
Hyprland `exec-once`: `brainos-operator`, `brainos-face`, chromium (CDP, hidden).

## 6. The API key

Never in git, never logged. Lives on p3 as record `ANTHROPIC.KEY` (written at
mint time from `--api-key-file`, default `~/.brainos_api_key`, or entered on
first boot through the face and stored there). Operator reads it once at start.
Without a key the entity still runs reflex + fast tiers and says so honestly.

## 7. Verification (must be green before "production")

- `./build.sh` → `brainos.img`; `tools/run_body.sh` boots it in QEMU (KVM, virtio-gpu)
- core: seed self-test still passes; BODY.EFI incorporate line prints; fallback loop when absent
- body: `tools/body_test.py` drives the operator socket headlessly inside the VM
  (via a serial console + ssh-less harness): reflex < 5 ms, fast tier JSON, a
  deliberate tool loop with `ANTHROPIC.KEY`, `ui.build` reaches the face,
  `app.open foot` yields a window, `web.search` returns text.
- voice: `stick_test.sh` pushes piper-synthesised speech through a PipeWire
  null-sink "microphone", holds push-to-talk through the face's control socket,
  and requires `heard` + a spoken reply that reaches the HDA card (host-side wav
  has real samples). The VM has a real `intel-hda` codec (`run_body.sh`).
- surface: a deliberate turn's `ui.build` must show up as a `brainos-surface`
  Hyprland layer rendered by WebKitGTK 6.0 (screenshot pulled to the host).
- mind 77/77 still green.
