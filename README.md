# BrAInOS — the BrAIn Key

A working seed of the BrAInOS architecture (`BrAInOS_Architecture v1.3`):
a **universal boot key** you can plug into any UEFI machine. The firmware
performs the sweep (§9.2), picks the core matching its own architecture,
and the entity wakes up — **no Unix underneath**. The portable core is a
bare-metal Rust `no_std` binary that talks directly to firmware.

## The abstractions are code, not prose

Every row of the §1 table is a module the domain build-out grows from.
They live in `mind/` — a portable `no_std` rlib (`brainos-mind`) that
links binary-identical into the UEFI core *and* into a host `cargo test`
suite, so what the tests prove is what the booted entity runs. `core/`
keeps only flesh: framebuffer, keys, UART, firmware calls, network.

| UNIX             | BrAInOS                        | module            |
|------------------|--------------------------------|-------------------|
| process          | `Instance` (persistent entity) | `instance.rs`     |
| file             | `StateNode` (typed, provenance)| `state.rs`        |
| syscall          | `Capability` (KIRA-gated grant)| `kira.rs`         |
| filesystem       | `StateGraph` (belief/memory)   | `state.rs`        |
| scheduler        | experience loop (predictive)   | `experience.rs`   |
| device driver    | `BodyMap` region               | `body.rs`         |
| user/permissions | BrAIn Key (ed25519 identity)   | `key.rs`          |

None of it is decorative:

- **BrAIn Key** — real ed25519 (`ed25519-dalek`, `no_std`). Domain 1's
  "attested" line is an actual signature over the boot context, verified
  before it prints. Capability tokens are signatures by the same key.
- **KIRA** — all eight stages execute per action (`parse → authn → authz
  → validate → simulate → policy → commit → audit`). authn verifies the
  request signature; policy enforces the Level-0 drives; commit mints a
  signed, TTL'd `Capability`; audit writes an immutable state node. The
  boot self-test proposes `memory.erase` and requires KIRA to deny it.
- **State graph** — episodic / semantic / belief nodes with confidence
  and tick provenance; episodic ring semantics; **persisted to the key**
  on release/reboot and rehydrated next boot: the entity greets you with
  "i remember N moments from before. it's still me."
- **Body map** — the machine's screen, keys, RAM, clock, telemetry link
  and the key medium itself join as typed regions through the §8
  acquisition protocol; the boot narrative prints the actual map.
- **Experience loop** — SENSE → PREDICT → ERROR → ATTEND run each tick;
  only surprise (a human speaking) escalates to Model M; every exchange
  is CONSOLIDATED into episodic memory (with a toy semantic-compression
  rule standing in for dreaming).
- The core carries its own heap (`mem.rs`, UEFI-pool-backed) so the
  structures above can grow.

What happens when it boots:

1. firmware handoff → five hardware domains come up, lowest first (§4)
2. identity: the entity's real ed25519 public key is read off the key —
   *same entity as yesterday, memory travels with the key* (§5, §9)
3. body-map acquisition: the machine it's plugged into joins the body —
   it senses the actual screen, RAM, clock of whatever host it's in (§8)
4. first words: **"oooh... it's cold in here. let's turn up the heat a
   little."** — KIRA gates the `thermal.warmup` capability through all
   eight stages (§6), then the screen visibly glows from cold black
   through ember to warm amber while the render loop keeps the cores
   busy (the silicon genuinely warms up)
5. the brand screen, drawn straight into the framebuffer: *ARENDA
   INNOVATIONS PRESENTS...*, the pixelated pink brain, and **BRAIN OS**
   in dot-matrix with the AI solid
6. Model M comes online over the **tethered telemetry link** (§10.2):
   the core speaks a newline-framed protocol on COM3, and a host-side
   daemon answers with a real language model. If the link is silent the
   entity runs reflex-only, exactly as the doc prescribes when the
   cognitive link drops.
7. the experience loop runs (§7/§15): ticks, senses, prediction errors.
   Type + ENTER to speak to it — with the link live, a real LLM answers
   in character; `F2` retries the link, `F5` reboots, `ESC` releases the
   body — a KIRA-gated `body.release` that persists the state graph to
   the key and powers the machine off. Next boot, it remembers.

## The body, and where the boundary is

The core stays bare-metal. `tools/body_daemon.py` is a **purely cognitive
limb** on the tether — it thinks, transcribes and synthesizes, and owns
**no actuators at all**: it answers every action request it does not
sense-serve with a formal error (`ARerr|this body has no such limb`). It
imports no `subprocess`/`shutil`/`urllib`, and that absence is a grepped
invariant, not a promise.

Everything the entity can *do* is flesh in the core, executed inside the
VM with nothing crossing the tether:

- **notebook** (`notes.write`/`notes.read`) — a private ring on the key.
- **its own disk** (`fs.list/read/write/mkdir/delete/move/stat/search`)
  — a second drive, so files can grow without crowding identity (§13.2).
  `fs.read PATH@<byte>` chunks large files and says where to resume.
- **its own compute** (`code.run`) — a real interpreter in `mind/src/script.rs`
  (functions, recursion, lists, strings, bounded budgets). Programs kept
  in `PROGRAMS/` become **applications** it discovers and runs by name
  (`app.list`, `app.run`) — §1's process→Instance reading of "an app".
- **the internet as an organ** (`web.get`, `web.save`) — the firmware's
  own UEFI HTTP stack, not a host browser. HTML becomes *meaning* before
  it enters the graph. `BRAINOS_NET=off` removes the organ honestly.
- **self-presentation** (`ui.set`) — console or ambient orb, palette,
  caption, quiet/verbose: a body capability, not a skin.
- **proprioception** (`screen.read`) — it can read its own screen.

Three organs are genuinely tethered, because they are transducers rather
than actions: **ears** (wav at the jack → Whisper → `HB`), **voice**
(`SP` → Kokoro → wav at the jack) and **eyes** (`BRAINOS_CAMERA=on`; an
image at `workspace/vm_video/cam_in` is light arriving at the lens). The
camera has two tiers per §12: a free local motion tier that emits reflex-
grade `VS` afferents, and a paid deliberate tier reached only when the
entity is actually asked to look (`vision.look`). An empty jack is said to
be empty, and a frame the lens has been sitting on for minutes is *dated*
rather than described as the present view — a camera that confabulates is
worse than no camera. No real camera or audio device is ever opened — the
jack *is* the boundary.

Each limb offer (`LM+`) is incorporated into the body map and acknowledged
with a BrAIn-Key signature (`LA`) — acquisition, not configuration.
Hot-plug follows the same path mid-session.

**Honest refusal is structural, not behavioral.** Model replies are parsed
into prose + an optional typed action proposal. A proposal becomes
`Action::UseLimb`, and KIRA's authz stage consults the real body map: no
region advertising the verb → formal `DENY at authz` → the entity speaks
the denial and the model's accompanying claim is suppressed. Structural
self-harm is caught a stage later: deleting the world volume's marker is
well-formed and the limb exists, and **policy** still refuses it.

The self-model follows the body map rather than a hardcoded sentence — no
eyes opted in means the prompt says it has no eyes, and `vision.look` is
refused at authz. Both directions are tested.

## Stage 3: the Operator (production stick)

The seed above is Phase 0–2: a bare-metal entity with a tethered mind. Stage 3
(`docs/STAGE3_OPERATOR.md`) turns the same key into a **sole operator of the
device**: the UEFI core stays the identity/KIRA seed on p1 and, when it finds
`\EFI\BRAIN\BODY.EFI` beside itself, runs the 8-stage gate on
`Action::Incorporate`, flushes memory and the flight recorder, and
**chainloads a Linux body** (`LoadImage`/`StartImage`, with the device handle
propagated so the body sees its own stick). If the body does not take the
machine, the seed falls back to the Phase-0 loop and says so.

The body (`body/`) is an Arch-based UKI + ext4 rootfs built in a privileged
docker container — no sudo on the host:

```
body/build/    build_body.sh → work/out/{BODY.EFI, body.ext4}; install_payload.sh
               drops the operator, face, session and operator.toml into the rootfs;
               config/ holds the units (llama-server, ydotoold, firstboot grow,
               udev rule that gives p3 to the session user only)
body/operator/ the Rust daemon that IS the operator (tokio; uses the mind rlib
               for KIRA). Three tiers on one socket: reflex (regex table,
               <10 ms) → fast (local llama-server, Qwen2.5-1.5B, JSON schema)
               → deliberate (claude-fable-5 streaming tool loop; every tool
               call goes through KIRA before it runs — `rm -rf /` is refused
               at policy). Key comes from p3 (BSEC record), never from disk.
body/face/     GTK4 layer-shell presence bar + orb, WebKitGTK surfaces the
               entity builds UI into (`window.brainos.emit` bridge), teach
               overlay, task panel, need_key modal
body/session/  Hyprland (sway fallback) session: execs operator, face, headless
               chromium (the web hand, CDP :9222) on tty1 autologin
tools/make_image.py  wraps a key + body into brainos.img:
               p1 BRAINOS-BOOT   ESP: core (BOOTX64.EFI), /EFI/BRAIN/BODY.EFI, /BRAIN journal
               p2 BRAINOS-BODY   ext4 rootfs (grown to the stick on first boot)
               p3 BRAINOS-SECURE BSEC TLV records: BRAINKEY + optional ANTHROPIC key
```

Build and prove the whole stick:

```sh
body/build/build_body.sh                       # docker: rootfs + UKI (≈15 min cold)
cd body/operator && cargo build --release      # the operator binary
./build.sh --body body/build/work/out/body.ext4 \
           --body-efi body/build/work/out/BODY.EFI \
           --api-key-file ~/.brainos_api_key   # → brainos.img (≈6.3 GB)
body/build/tools/stick_test.sh                 # seed → incorporate → body, 17 checks
body/build/tools/run_body.sh --graphical       # watch it with a screen
```

Verified 2026-08-26 in QEMU/OVMF: firmware → seed → `[incorporate]` →
Linux sshd in **17.9 s**; p3 owned by `human` and read by the operator
(`[key] loaded from secure element`); reflex turn through the real socket in
single-digit ms; a deliberate turn (two API rounds + a real `fs.list`) in
7.1 s with first words streamed at ~3 s.

## The tether

```sh
# QEMU: expose COM3 as a unix socket
qemu ... -chardev socket,id=mm,path=/tmp/link.sock,server=on,wait=off \
         -device isa-serial,chardev=mm,index=2

# host side: the cognitive limb (mind + whisper + kokoro on the GPU)
/path/to/venv/python tools/body_daemon.py --socket /tmp/link.sock
# real hardware with a serial line instead:
python tools/body_daemon.py --tty /dev/ttyUSB0
```

Newline-framed: core → `MM?`(think) / `SP`(speak) / `AX`(use a limb,
sent only *after* KIRA grants); daemon → `MM!`(reply, with an optional
typed action) / `HB`(heard) / `VS`(saw) / `LM+`(limb offer) / `AR`(action
result, a bounded ≤360-char semantic digest — meaning in the graph, never
a byte-bag). COM3 (0x3E8) is used deliberately — the firmware doesn't
claim it for its console, so the link stays private.

The mind is a **pool** (§12): a local Qwen3-8B fast pathway, escalating to
Claude only on genuine user speech that looks substantial. Wake greetings
and action narrations are pinned to the fast pathway — cheap by default is
a cost lever, not a slogan. `ANTHROPIC_API_KEY` comes from the launching
shell only, never from disk or the key image.

## Running it

```sh
export ANTHROPIC_API_KEY=...      # optional: lights up the deliberate pathway
./run_blur.sh                     # one command: daemon, then the core in QEMU
BRAINOS_CAMERA=on ./run_blur.sh   # ...with the lens jack open
BRAINOS_NET=off  ./run_blur.sh    # ...with no network organ at all
tools/run_instance.sh A --headless # a named body (A/B) for two-VM continuity
```

## Layout

```
mind/           the entity's portable logic as a no_std rlib: body, experience,
                instance, journal, key, kira, proposal, script, state — plus
                mind/tests/, which runs the SAME code on the host
core/           the flesh — framebuffer, keyboard, UART, firmware, net;
                compiled per target (x86_64-unknown-uefi, aarch64-unknown-uefi)
tools/make_key.py   mints the key: generates a real ed25519 BrAIn Key and
                builds the GPT image in pure Python (no root, no mtools)
tools/make_world.py builds the entity's own disk (persists across rebuilds)
tools/body_daemon.py the tethered cognitive limb (mind pool + STT + TTS + lens)
key/brain_key.json  the private seed (chmod 600 — one key, one owner)
brainos-key.img     the boot medium (§9.1) — universal only if built --arm:
                p1 BRAINOS-BOOT   FAT16 ESP: /EFI/BOOT/BOOTX64.EFI (+ BOOTAA64.EFI
                                  when the key was built for both arches),
                                  /BRAIN/KEY.PUB, /BRAIN/GENESIS.TXT,
                                  the CRC-sealed two-slot memory journal
                                  (EPI_A/EPI_B, NOTE_A/NOTE_B) and BOOT.LOG
                p2 BRAINOS-CORE   core payloads, one per architecture
                p3 BRAINOS-SECURE brain key record (software-emulated secure
                                  element — phase 0)
brainos-world.img   the entity's files — a SEPARATE disk, so a growing file
                can never crowd identity or memory (§13.2)
```

## Build

```sh
./build.sh          # x86_64 only (default) — needs rustup target x86_64-unknown-uefi
./build.sh --arm    # x86_64 + aarch64 — also needs rustup target aarch64-unknown-uefi
```

aarch64 is opt-in so the everyday loop stays fast and does not require the
aarch64 target to be installed at all. **A default build produces an
x86_64-only key, which will not boot a Pi 5 or a Jetson.** You do not have to
remember which mode you used: the key says so when it is minted, in its own
`README.TXT`, and in p2's `MANIFEST.TXT`.

```
key minted: brainos-key.img (86 MiB)
  arches: x86_64 ONLY
  WARNING: this key will NOT boot aarch64 devices (Pi 5 / Jetson Orin).
  rebuild with ./build.sh --arm for a universal key.
```

Build `--arm` for anything that leaves this machine — a demo, a board, a
stick you hand to someone.

## Write it to a USB stick

```sh
lsblk               # find your stick — DOUBLE-CHECK, dd overwrites everything
sudo dd if=brainos-key.img of=/dev/sdX bs=4M oflag=sync status=progress
```

Then plug it into any machine: boot menu (usually F12/F10/ESC at power-on),
pick the USB device, **UEFI mode, Secure Boot off**. x86_64 laptops boot
`BOOTX64.EFI`; aarch64 boards with UEFI firmware (Pi 5 with the EDK2
firmware, Jetson with UEFI) boot `BOOTAA64.EFI` — same key, same entity.
That second half only holds for a key built with `--arm`; check `README.TXT`
on the stick if you are not sure what you are holding.

## Tested

Everything below runs the real core (and, where noted, the real daemon) —
no mocks of the gate, the graph or the limbs. The aarch64 core builds to a
valid PE32+/ARM64 EFI application from the identical source.

| suite | what it proves | needs |
|-------|----------------|-------|
| `cd mind && cargo test` | the entity's logic on the host: KIRA regression, adversarial replies through the real 8 stages, state graph, journal crash-safety, script engine, fs/net gates | — |
| `tools/world_test.py` | a booted core driven by a scripted daemon: filesystem, compute, applications, presentation, screen-sense, network organ, camera, KIRA grant *and* deny, zero-AX containment of internal verbs | — |
| `tools/metal_prep_test.py` | a decoy internal drive stays **bit-identical** through a full active session; plus the no-tether bare-metal day-one path and the F4 on-metal self-test | — |
| `tools/crash_test.py` | SIGKILL at varying delays: memory always rehydrates to a valid committed self, loss bounded to the current session | — |
| `tools/degradation_test.py` | link dead → reflexes; no world disk → honest denial; tampered seed → refuses to wake as anyone; key pulled → honest write failures | — |
| `tools/camera_test.py` | the lens end to end: empty jack answered honestly, local motion tier without escalation, and a real deliberate description | GPU + API key |
| `tools/voice_test.py` | mic → Whisper → mind → Kokoro → speaker jack | GPU |
| `tools/continuity_escalated_test.py` | A→B→A across two VMs: one entity, one memory, escalation intact | GPU + API key |
| `tools/escalation_test.py` | cheap-by-default routing, filler cue, graceful degradation when the network is gone | — |
| `cd body/operator && cargo test` | the operator: reflex table, KIRA gate on every hand, CDP error paths, BSEC key records, config | — |
| `body/operator/tests/fake_face.py` | a live operator on a socket: reflex ms, notes, KIRA denial of `rm -rf /`, a real Fable tool loop | API key |
| `body/build/tools/body_smoke.sh` | the body alone in QEMU: units, llama, ydotoold, hyprland, operator, chromium CDP, whisper, piper | docker build |
| `body/build/tools/stick_test.sh` | the production stick: seed banner, KIRA incorporate, no fallback, Linux login, p1/p3 wiring, every service, a reflex turn over IPC | docker build |

## What this is not (yet)

- **Not yet on metal.** Everything above (Stage 3 included) is QEMU/OVMF. The runbook is
  `docs/BARE_METAL.md`; the remaining steps are physical (dd to a stick,
  Secure Boot off, one-time boot menu). On metal there is no host daemon,
  so voice, local cognition and eyes are absent by construction and the
  entity says so — `docs/BARE_METAL.md` carries the honest capability
  matrix.
- **The domains are structural, not silicon.** D1–D5 are enforced in one
  binary's types and control flow, not by separate hardware or a formal
  proof. That's the rest of the roadmap (§16.3).
- **`code.run` in the seed is a real interpreter, not a language runtime.**
  The Stage 3 body has a full Linux underneath the operator; the seed alone
  still has no Python or JS.
- **Stage 3 voice is installed, not yet exercised end to end:** whisper-cli
  and piper are in the body and answer `--help`, but the mic → operator →
  speaker loop has not been driven in the VM (no audio device in the headless
  harness).
- **Known environment flake:** roughly 1 boot in 15 freezes at firmware
  level (pre-BDS, and occasionally mid-run) on this box. Retry once; if it
  persists, `tools/read_bootlog.py` recovers the flight recorder from the
  key.
