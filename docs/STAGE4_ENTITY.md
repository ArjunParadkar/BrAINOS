# BrAInOS Stage 4 — "the Entity" (one mind, every body, no wait)

Direction from soapai, 2026-08-30 (verbatim intent): *finish BrAInOS to a
version that can control any device, act as a persistent entity, runs off an
array of models, has near-zero latency, and has the exact product description
as our V1. It has to be 200x better than anything Linux, Windows, Claude Code
or any harness offers. Think use cases.*

Stage 3 made the key the sole operator of ONE bootable machine. Stage 4 makes
the key an **entity**: one identity and one memory that is present in every
device it can reach at once, that gets faster the more it is used, and that
thinks with a pool of models rather than one.

This document is the spec every Stage 4 component is built against. Every
claim is either VERIFIED (a test that ran) or MANUAL (needs physical hands).

---

## 0. The product (V1 §15, restated as the thing you can hold)

**You buy a BrAIn Key. The key is the entity.** Plug it into a laptop and it
boots as the sole operator of that laptop. Leave the laptop running and open
the Reach app on your phone, Pi, or any other computer: the same entity — same
name, same memory, same key — is now in that body too. There is no "sync", no
account, no cloud profile: identity and memory live only on the key
(§13.2 memory-integrity law), the datacenter is a cognitive limb it rents
("a bridge, not a leash"), and every body is a set of hands it acquires.

What that buys the human, concretely:

| you say | what happens | tier |
|---|---|---|
| "open the terminal" | window appears | reflex, <5 ms |
| "on the pi, restart the print server" | KIRA-gated `shell.run` on the Pi via Reach | reflex/fast, <50 ms + RTT |
| "every morning at 8, pull the repo and tell me if CI is red" | a standing task; the *check* is compiled to a skill after the first deliberate run | deliberate once, then reflex |
| "set up this laptop for my mother: big fonts, Chrome, her mail" | multi-step deliberate plan, progress on the face, each step a KIRA-gated hand; **the plan is remembered as a skill**, so the next laptop takes seconds | deliberate → skill |
| "watch the build and tell me when it fails" | a watcher runs a predicate on an interval and speaks up | reflex (compiled) |
| "what did I ask you to do yesterday on the phone" | the ONE episodic memory, regardless of which body it happened in | fast |

The "200x" is not a single number, it is three compounding levers that no OS,
shell, or agent harness has together:

1. **Procedural compilation.** A harness (Claude Code, an agent loop) pays
   full model latency *every time*. A shell pays zero latency but knows
   nothing. BrAInOS pays once, then **compiles the successful plan to a
   skill** that replays in the reflex tier with no model in the path. The
   more it is used, the faster it gets — an 11.5 s deliberate turn through a
   remote body replayed in 1.4 ms (VERIFIED 2026-08-30, `entity_test.py` T3: 7928×).
2. **Every body at once.** Windows/Linux stop at the chassis. A harness stops
   at the process. BrAInOS treats every device it can reach as hands of the
   same entity, gated by the same KIRA, remembered in the same journal.
3. **Cheap-by-default cognition.** A pool of models, chosen per tier and per
   utterance; the expensive model is asked only when the cheap one escalates,
   and it is asked *speculatively in parallel* so its first token is already
   in flight when the cheap one gives up.

---

## 1. Use cases (these are what the tests drive)

- **UC1 Salvage laptop.** Plug the key into a junk Lenovo. It is yours in 18 s.
  No install, no account, no Windows password. (Stage 3, VERIFIED in QEMU; on
  metal MANUAL, `docs/BARE_METAL.md`.)
- **UC2 The house.** Laptop is the mind's seat; the Pi runs the printer; the
  phone is in your pocket. "print the PDF I just downloaded on the laptop" —
  `fs.read` on the laptop, `shell.run lp` on the Pi. One utterance, two bodies.
- **UC3 Muscle memory.** Anything you have asked for once and confirmed
  worked, you never wait for again. Say it the same way (or close to it) and
  it happens in the reflex tier. Say "forget that" and the skill is dropped.
- **UC4 Standing orders.** "every 20 minutes check if the deploy is green"
  / "when the download finishes, open it". Watchers run in the body, speak
  through the face, survive the session (they are skills + a schedule in the
  journal), and are visible: "what are you watching?"
- **UC5 Teaching.** "show me how to set up SSH keys" — the deliberate tier
  drives the real apps while `ui.teach` highlights what it is doing. (Stage 3.)
- **UC6 Degradation is honest.** No network → local models only, and it says
  so. No local model → reflex + skills only, and it says so. A body goes away →
  its verbs vanish from the body map and KIRA refuses them at authz.

---

## 2. Reach — control any device

A device that cannot boot the key (a phone, a Pi already running something, a
Mac, a Windows box, a friend's laptop) runs **`reach/brainos-reach.py`**: one
file, Python 3 standard library only, no pip, runs anywhere Python runs. It
*offers* the device's hands to the entity and does nothing else.

### 2.1 Trust: the key is the only authority

- Pairing = the device learns the entity's **public key** (`KEY.PUB` hex) once
  (`brainos-reach.py pair <hex>` or first-connect TOFU with a printed
  fingerprint). Nothing secret is stored on the device.
- Handshake: device sends `{"t":"hello","name":..,"nonce":<32 hex>,"hands":[...]}`;
  the operator replies `{"t":"attest","sig":ed25519(nonce||name)}`; the device
  verifies against the pinned public key (pure-Python ed25519, RFC 8032, ~150
  lines, no dependency) and only then accepts commands. A wrong key is refused
  and logged as a **tamper event** on the device (UC6).
- After the handshake the TCP session is the channel. Transport is plain TCP on
  the LAN (or a WireGuard/SSH tunnel the human sets up); TLS is deliberately
  NOT bolted on here — the signature is the trust, the tunnel is the human's.
  This is stated honestly in `reach/README.md`.

### 2.2 The device never remembers (§13.2)

`brainos-reach.py` keeps no history, no transcript, no result cache. It writes
only `~/.brainos-reach/entity.pub` (the pairing) and a tamper log. Every
result goes back to the operator as a **bounded digest** (≤4 KiB) and becomes
memory *there*, in the journal on the key.

### 2.3 Hands a Reach body offers (probed on the device, only what exists)

| group | verbs | backing |
|---|---|---|
| `shell` | `shell.run`, `code.run` | subprocess, timeout, bounded |
| `fs` | `fs.list/read/write/mkdir/delete/move/stat/search` | pathlib under the device home |
| `sys` | `sys.notify`, `sys.time`, `sys.battery`, `sys.volume`, `sys.brightness`, `sys.lock`, `sys.power` | notify-send/osascript/termux-*/powershell — whichever exists |
| `app` | `app.open`, `app.list` | xdg-open / open / start / termux-open |
| `screen` | `screen.look` | grim/scrot/screencapture/termux — returns a PNG path digest |
| `input` | `input.type`, `input.key` | wtype/xdotool/osascript when present |

### 2.4 Operator side

- `reach.rs`: a TCP listener (`[reach] listen = "0.0.0.0:7777"`, off by
  default on the dev host, on in the body) and a UDP beacon responder so a
  device can find the seat by name. Each connected body becomes a **Region**
  `reach/<name>` in the body map with exactly the verbs it offered — so KIRA
  authz sees them and the deliberate tier's tool list advertises them.
- Every hand's tool schema gains an optional `body` argument. Omitted = the
  seat (local). The reflex table understands the spoken form
  `on <body>[,:] <utterance>` and `<utterance> on <body>` → same intent, routed.
- `body.list` — proprioception: which bodies the entity is in right now,
  with RTT. `body.forget <name>` — drop a body (its region leaves the map).
- A body that disconnects is removed from the map immediately; a verb aimed at
  it is refused by KIRA at authz — the entity says it no longer has that hand.

---

## 3. Skills — procedural compilation (near-zero latency, the learning lever)

### 3.1 What is compiled

When a **deliberate** turn ends and (a) every tool call in it succeeded
(`is_error == false`), (b) at least one tool call happened, and (c) no call
was in the `NEVER_COMPILE` set (`sys.power`, `fs.delete`, `voice.listen`,
`task.*`, `ui.build` with dynamic HTML), the operator records a **Skill**:

```
skill|<tick>|<confidence>|<normalized utterance> :: <json steps>
steps = [{"verb":"fs.list","input":{"path":"/tmp/x"},"body":"seat"}, ...]
```

Fast-tier `act` turns are compiled the same way (one step).

### 3.2 Persistence (signed, separate from the episodic journal)

The mind crate deliberately does NOT rehydrate `skill` nodes from the
episodic journal (`mind/tests/adversarial.rs`: a tampered journal must not be
able to plant a `kira.bypass` pathway). That stays. Compiled skills live in
their own two-slot CRC journal on the key, `SKL_A` / `SKL_B`, and **every
record is signed by the BrAIn Key** over its canonical JSON. At load, a record
whose signature does not verify against the entity's own key — tampered, or
written by another entity — is dropped and counted. A planted skill is not the
entity's and never replays (VERIFIED, `skills::tests`). Every replayed step is
KIRA-gated again regardless.

### 3.3 Matching (reflex tier, before the regex table)

Utterance normalized (`reflex::normalize_utterance`, lowercased). Match order:
1. exact normalized string;
2. token-set Jaccard ≥ 0.80 **and** identical *slot* tokens (numbers, paths,
   URLs, quoted strings must match exactly — "open file A" never replays
   "open file B").
No embedding model, no fuzziness that could fire the wrong plan. Ambiguity
falls through to the tiers as before.

### 3.4 Replay

Each step is KIRA-gated **again** at replay (the gate is stateless; policy
may have changed; a body may be gone). A denied or failed step stops the
replay, the entity says which step failed and why, and the skill's confidence
is reduced by 20; at ≤20 it is dropped ("that stopped working, I'll think it
through next time"). Success reinforces (+5, cap 100).

### 3.5 Human control

- "forget that" / "forget how to <utterance>" → `skill.forget` (last replayed
  skill, or by utterance).
- "what do you know how to do" → `skill.list`.
- A replay is announced tersely on the face as `act` messages with
  `tier: "skill"` so the human always knows it was muscle memory, not thought.

---

## 4. Speculative cognition (latency without guessing)

Today: reflex → fast (≤600 ms) → deliberate — sequential, so an escalation
pays the fast timeout *plus* the API's first-token latency.

Stage 4: for any utterance that misses reflex and skills and for which an API
key exists, the deliberate request is **started immediately in parallel** with
the fast tier, in a *held* state: its text deltas are buffered and its tool
calls are not executed. Then:

- fast answers `chat`/`act` → deliberate is cancelled (HTTP dropped); nothing
  it said is shown. Cost: one aborted streaming request.
- fast answers `escalate`, fails, or times out → the held stream is **released**:
  buffered deltas flush to the face in order, tool calls proceed. The first
  token was already in flight, so perceived latency ≈ max(fast, API) instead
  of fast + API.

Budget control: `[tiers] speculate = true|false` and
`speculate_only_if = "keywords|always"`. Default in the body: `always` — the
API cost of a cancelled request is bounded by the fast tier's 600 ms budget.

---

## 5. The model pool (an array of models thinking as one mind, §12)

`pool.rs` + `[[pool.models]]` in `operator.toml`:

```toml
[[pool.models]]
role = "fast";       kind = "llama";     url = "http://127.0.0.1:8080"
[[pool.models]]
role = "fast";       kind = "ollama";    url = "http://127.0.0.1:11434"; model = "qwen2.5:7b"
[[pool.models]]
role = "deliberate"; kind = "anthropic"; model = "claude-fable-5"
[[pool.models]]
role = "deliberate"; kind = "anthropic"; model = "claude-opus-5"        # failover
[[pool.models]]
role = "vision";     kind = "anthropic"; model = "claude-fable-5"
```

- Health is probed at boot and every 60 s; the face gets
  `{"t":"pool","models":[{role,model,ok,ms}]}`; `pool.status` is a hand
  ("which brains do you have right now?").
- Tiers pick the first healthy model for their role, in order; on a request
  error they fail over to the next and remember it for that session.
- Models are stateless muscles: the ONE history and ONE state graph are the
  operator's; a swapped model sees the same context.

---

## 6. Standing tasks — watchers and schedules (UC4)

New hands (all KIRA-gated, all visible):

| verb | meaning |
|---|---|
| `task.every {seconds, utterance}` | run the utterance through the tiers every N seconds (min 10) |
| `task.when {cmd, contains, utterance}` | run `cmd` every 15 s; when its output contains the string, run the utterance once and stop |
| `task.list` / `task.stop {id}` | proprioception and control |

The utterance a standing task runs is exactly what the human would say, so it
benefits from skills: the first run may be deliberate, every later run is a
reflex replay. Tasks live in the journal as `semantic` nodes tagged
`standing-task` and are re-armed at boot.

---

## 7. Verification (must be green before "Stage 4 done")

| suite | proves |
|---|---|
| `cd mind && cargo test` | skills survive serialize/load; old journals still load |
| `cd body/operator && cargo test` | skill matching (exact, Jaccard, slot guard, never-compile set), reach protocol framing + signature check, pool failover order, reflex `on <body>` routing |
| `body/operator/tests/entity_test.py` | a live operator on the host + a live `brainos-reach.py` body on localhost: T1 reach handshake accepted with the real key and **refused with a wrong key**; T2 `on lab: run echo hi` executes on the reach body via KIRA; T3 a deliberate turn compiles a skill and the identical utterance replays in the reflex tier (timing of both printed; replay must be < 20 ms); T4 `forget that` drops it; T5 a standing `task.when` fires; T6 body disconnect → verb refused at authz; T7 speculative path: first deliberate token arrives no later than the fast timeout | needs `~/.brainos_api_key` for T3/T7 |
| `body/build/tools/stick_test.sh` | the 27 Stage 3 checks still pass with the new operator in the body, plus 4 Reach checks: a host-side `brainos-reach.py` body joins the entity booted in QEMU (KEY.PUB from p1), a routed reflex runs on it, and the body kept no memory |

Numbers that must be printed, not claimed: reflex ms, skill-replay ms,
deliberate ms (first token and total), reach RTT.

---

## 8. What Stage 4 is NOT (honest limits)

- **Reach is trust-by-signature over the human's network.** No TLS. On a
  hostile LAN, commands could be observed (not forged — every session is
  bound to a challenge only the key can sign). Use a tunnel.
- **Skills are exact-plan replays**, not generalizations. "open file A" never
  becomes "open file B". That is deliberate: a wrong replay is worse than a
  slow think.
- **Speculation costs money** (cancelled API requests). It is a config switch.
- **Standing tasks run only while a seat is awake.** There is no datacenter
  scheduler in this stage; the entity is where its key is.
- **Reflex latency on metal is still MANUAL** — QEMU numbers only until
  soapai boots the Lenovo.
- Domains remain structural (types + control flow), not silicon (§16.3).

---

## 9. Status (2026-08-30)

VERIFIED on the dev host, `body/operator/tests/entity_test.py` 24/24 with the
real Fable API and a real Reach body on localhost:

| number | measured |
|---|---|
| reflex on the seat | 7.6 ms first call, 1.0 ms after |
| Reach handshake (pure-Python ed25519 verify) | 170 ms |
| reflex routed to a Reach body | 1.0 ms + RTT |
| deliberate turn using a remote hand | 11.5 s (first token 11.4 s — the model went straight to the tool) |
| the same utterance as a compiled skill | **1.4 ms** (7928×) |
| after operator restart (signed store) | 1.1 ms |
| speculative deliberate, fast tier absent | first token 7.4 s (API latency; nothing added by the tiers) |

Unit suites: operator 53 + 13 (skills, reach protocol, pool failover, tasks,
reflex entity rules), mind unchanged + `BodyMap::release`. Production stick:
see `stick_test.sh` result in the README's Tested table.

MANUAL (needs soapai): boot on the Lenovo; pair a real phone/Pi and say
"on the pi, …"; both are the same code paths the harness drives.
