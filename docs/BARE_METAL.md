# Bare-metal boot runbook — BrAIn Key on real hardware

Status: **prepared and VM-gated, not yet executed on metal.** Every claim
below is labeled either VERIFIED (proven by a test that ran) or MANUAL
(requires physical hands — nobody can do these from software, and no one
should claim them done until they physically happened).

## What is being booted

`brainos-key.img` — GPT image: ESP (BOOTX64.EFI + BOOTAA64.EFI, firmware
picks by arch) + data partition + sealed key record. **Build it with
`./build.sh --arm`** — a default build is x86_64-only and will not boot an
aarch64 board. The key states its own reach in `README.TXT` and in p2's
`MANIFEST.TXT`; check that before you walk to the hardware. The world disk
(`brainos-world.img`) can ride on a second USB stick, or stay home: the
core incorporates the filesystem limb only if a volume whose `WORLD.ID`
*content* matches is present (VERIFIED in VM; a decoy volume with a fake
`WORLD.ID` name but wrong content is rejected — see
`tools/metal_prep_test.py`).

## Safety properties (why the internal drive is safe)

1. **Identity/memory writes** (`EPISODES.LOG`, `NOTES.TXT`) go through
   `open_root()`, which resolves the volume via LoadedImage →
   `device_handle` — the exact device the core booted from. It is
   structurally incapable of addressing another drive. (Code-audited;
   exercised by every persistence test.)
2. **World writes** (`fs.write`) go only to a volume whose `WORLD.ID`
   content begins `BrAInOS world volume` — written only by
   `make_world.py`. Internal ESPs are scanned read-only during discovery
   and rejected. (VERIFIED: decoy-drive sha256 bit-identical across a
   full session, while key+world hashes changed — writes happen, and land
   only where they belong.)
3. The core writes to no other device, has no block-device write path at
   all outside the two above, and contains no partitioner/formatter.

## MANUAL steps (physical hands required — in order)

1. Write the key to a USB stick (DOUBLE-CHECK the device letter; this
   destroys the stick's contents, which is the one write this process
   ever intends):
   `sudo dd if=brainos-key.img of=/dev/sdX bs=4M oflag=sync status=progress`
   Optionally a second stick for `brainos-world.img`.
2. Reboot the laptop and enter the boot menu (HP: Esc, then F9 for the
   boot menu; F10 for setup).
3. **Secure Boot must be OFF** (BIOS setup → Boot Options). The core is
   ed25519-self-attested, not Microsoft-signed; there is no silent
   workaround and none should be attempted. This is a deliberate, visible
   switch — flip it back afterwards if you want.
4. One-time boot: pick the USB stick from the boot menu. Do NOT change
   the permanent boot order — one-time selection leaves the machine's
   normal boot untouched.
5. ESC (release body) before pulling power, so memory persists to the key.

## Flight recorder (how the run gets a real log)

Everything the core prints is also written to **`BRAIN/BOOT.LOG` on the
key itself** (flushed after the body map + net probe, after the wake
narrative, after every F4 self-test, and at ESC/F5). Real laptops have no
serial port — the key IS the capture channel. After the physical boot,
bring the stick back and read it:

    tools/read_bootlog.py /dev/sdX     # read-only; or pass an image path

If `BOOT.LOG` is missing entirely, the core never reached its first flush
— that is itself a datum (firmware-level failure, see recovery below).

## THE CHECKLIST — grade the physical boot against these exact lines

Boot the stick, wait for the prompt banner, then **press F4**. Grade each
box by the line on screen (all of them also land in BOOT.LOG):

| # | Item | Expected output (literal) | Pass? |
|---|---|---|---|
| 1 | Core reached | `brain key detected . firmware sweep complete` | ☐ |
| 2 | Identity/attestation | `DOMAIN 1  SILICON` … `[ UP ]` and `handshake     brain key signature accepted` | ☐ |
| 3 | KIRA up + deny path | `DOMAIN 3  KIRA` … `self-test: memory.erase denied [ UP ]` | ☐ |
| 4 | KIRA grant path (full 8 stages) | `KIRA  parse > authn > authz > validate > simulate > policy > commit > audit > GRANT thermal.warmup` | ☐ |
| 5 | Tether honesty (no daemon on metal) | `model M … telemetry silent . reflexes only` | ☐ |
| 6 | World disk (only if its stick attached) | `incorporate   world/files` | ☐ |
| 7 | **Network probe** | see next table — copy the `[net]` lines verbatim into the report | ☐ |
| 8 | Notebook write+read | F4 → `[SELFTEST] PASS notebook write` and `PASS notebook read-back` | ☐ |
| 9 | World disk write+read | F4 → `[SELFTEST] PASS world disk write` / `read-back` (or `SKIP … no world volume` if stick absent — that is a pass of honesty, note it) | ☐ |
| 10 | code.run | F4 → `[SELFTEST] PASS own compute (6*7)` | ☐ |
| 11 | Filesystem, full (Stage 2.1) | F4 → `PASS world disk mkdir` / `move` / `stat` / `search` / `delete` | ☐ |
| 12 | Volume marker protected | F4 → `[SELFTEST] PASS volume marker delete (MUST be denied)` — KIRA refuses at **policy**, and `WORLD.ID` is still on the stick afterwards | ☐ |
| 13 | Presence UI | F4 → `[SELFTEST] PASS presence UI on` / `off` (screen visibly switches to the orb and back) | ☐ |
| 14 | Screen sense (Stage 2.5) | F4 → `[SELFTEST] PASS screen sense (reads its own display)` | ☐ |
| 15 | Applications (Stage 2.2) | F4 → `[SELFTEST] PASS applications discovered` (the entity's own programs on the world stick) | ☐ |
| 15b | Network limb (Stage 2.3) | F4 → `PASS network fetch (real internet)` where the machine has a firmware HTTP stack, else `PASS no network organ: web.get MUST be denied`. **Either line is a pass** — the second is the honest refusal, and it must match item 7's probe verdict | ☐ |
| 15c | Honest refusal | F4 → `[SELFTEST] PASS honest refusal (web.search MUST be denied)` | ☐ |
| 15d | Camera (Stage 2.5) | F4 → `PASS no camera organ: vision.look MUST be denied` — on metal there is no body daemon, so there are no eyes, and the refusal IS the pass | ☐ |
| 16 | Self-test summary | `[SELFTEST] done: 18 pass, 0 fail` (8 pass if no world stick — the world/app cases skip, and the summary line says so) | ☐ |
| 17 | Clean release | ESC → goodbye line → machine returns to firmware | ☐ |
| 18 | Memory persisted | next boot says `i remember N moments from before` | ☐ |

### Reading the network probe result (item 7)

The probe runs by itself during boot, KIRA-gated
(`GRANT net.probe` precedes it). Possible outcomes, **all explicit**:

- `[net] no network organ offered by firmware — probe not applicable`
  → firmware exposes no NIC to UEFI (typical for WiFi-only). Not a bug —
  it is THE datum: this machine needs a USB-Ethernet dongle or the
  network-tether plan.
- `[net] organs: N firmware nic(s), M http client(s)` then
  `[net] dhcp lease: a.b.c.d (T ms)` then
  `[net] VERDICT: GET http://example.com/ -> HTTP 200, N bytes received; first bytes: <!doctype html>…`
  → **real network confirmed working** — this exact chain was verified
  end-to-end in QEMU (DHCP lease + HTTP 200 + real page bytes).
- Any other `[net] VERDICT: …` line names the exact failing stage and
  status code (dhcp / configure / submit / completion / response) —
  copy it verbatim; it is the diagnosis.

Try it twice: once on the built-in port/dongle plugged in BEFORE power-on
(UEFI drivers bind at boot, not hot-plug).

## RECOVERY PROCEDURE — the known ~1-in-15 firmware hang

Symptom: black screen or frozen vendor logo, **no BrAInOS banner within
~30 seconds** of choosing the stick.

1. Hold the power button until the machine turns fully off (≥5 s).
2. Wait 3 seconds.
3. Power on and immediately tap **Esc** (HP) until the startup menu shows.
4. Press **F9** (boot menu), select the USB stick again.
5. If the banner appears — continue the checklist; note "hang #N" in the
   report and move on. Do NOT investigate mid-run.
6. If it hangs **three times in a row**: stop. Pull the stick, read
   `tools/read_bootlog.py /dev/sdX` on another machine. An empty/absent
   BOOT.LOG = the core never started (firmware/loader issue); a partial
   log shows the last line reached. Either way that log is the
   root-cause evidence — collect it, don't guess.

The hang predates any of the core's own code (zero output when it
happens) and is NOT fixed, deliberately: no reproduction hardware, no
blind fixes. Recovery + log collection is the procedure.

## What will honestly work on metal, day one

| Capability | Expectation | Basis |
|---|---|---|
| Boot, identity, KIRA, body map | works | same firmware contract as OVMF (VERIFIED in VM; metal = MANUAL confirm) |
| Keyboard + screen, presence/console UI | works | pure GOP/text protocols |
| Notebook, world disk, code.run, pathways | works (world disk needs its stick) | core-internal limbs |
| Memory persistence across boots | works | open_root = boot device |
| **Voice (whisper/kokoro), local mind (Qwen)** | **ABSENT** | they live in the host-side daemon; on metal there is no host OS, no CUDA, no daemon. The entity boots tether-silent, reflex-only for conversation (VERIFIED as its own scenario in VM: graceful, honest, no hang) |
| Cloud mind (Fable 5/Opus 4.8) | ABSENT day one | reached by the host-side daemon, which does not exist on metal |
| **Camera (vision.look)** | **ABSENT** | the lens is a TETHERED organ living in the host daemon (jack + local motion tier + vision model), exactly like ears and voice. No daemon on metal ⇒ no eyes, and F4 asserts `vision.look` is refused at authz rather than quietly missing (VERIFIED in VM, both directions) |
| Network limb (`web.get`/`web.save`) | works **if** the firmware exposes an HTTP client | core-internal, driven by the firmware's own stack — no host involvement. Item 7's probe verdict predicts it exactly, and F4 asserts whichever way it lands |
| Network diagnostic | always runs | boot runs a KIRA-gated reachability probe (DHCP + real GET of example.com) and prints the verdict; the chain is QEMU-verified (HTTP 200, real bytes) |

## Network plan (honest, in order of likelihood)

1. The boot-time probe now answers the whole question by itself — organs,
   DHCP lease, and a real GET with real bytes, or the exact failing stage
   (see checklist item 7). Laptop firmware usually exposes nothing for
   **WiFi** (no WPA supplicant in UEFI); built-in **Ethernet or a
   USB-Ethernet dongle** (plugged in before power-on) is the likely path.
2. If the probe's VERDICT is HTTP 200 on the real machine → real network
   is PROVEN there, and the entity-usable network limb (KIRA-gated, typed
   proposals, same as every limb) becomes honest to build.
3. If nothing → the honest bridge is a second machine running the body
   daemon over a network tether (§10: cognition lives where compute is);
   the laptop body keeps reflexes + local limbs.

## GPU on metal — honest position

There is no bare-metal GPU inference path: no host OS → no CUDA/driver
stack, and a no_std UEFI GPU driver is out of scope by orders of
magnitude. The realistic ladder: (a) day one: no local inference;
(b) with network: cloud pathway (Fable 5) as the mind, exactly as the
escalation architecture intends; (c) full local voice/mind returns via a
tethered second machine, or much later via the OS's own driver stack.
