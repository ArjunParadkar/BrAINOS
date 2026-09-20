# BrAInOS — the substrate

> The instance is to the device as we are to our body.

This workspace is Architecture v1.3 stages 3–8, built beside the existing
tree. Stages 1–2 and Phase 0 live in `../mind`, `../core`, `../miniblur`,
`../firmware` and `../body`; they are untouched and still pass.

```
                                      what it answers
  ┌───────────────────────────────────────────────────────────────────┐
  │  Domain 5 · memory      what does it remember?                    │  memory
  │  Domain 4 · cognition   escalating resources; proposes, never acts│  cognition
  │  Domain 3 · KIRA        what may it do — provably?                │  kira
  │  Domain 2 · autonomic   reflexes inside a pre-authorized envelope │  rt
  │  Domain 1 · silicon     who is this?                              │  crypto
  └───────────────────────────────────────────────────────────────────┘
        trust flows down · constraint flows up
```

## Run it

```sh
cargo run --release --bin brainos -- demo     # Phase 0, in simulation
cargo run --release --bin brainos -- stack    # the whole path, intent to servo
cargo run --release --bin brainos -- verify   # the Stage 6 property report
cargo run --release --bin brainos -- bench    # measured latencies and footprint
cargo run --release --bin brainos -- wire     # the transport under a bad link
cargo run --release --bin brainos -- compound # does experience compound?
cargo run --release --bin brainos -- stat     # what this build contains

cargo test --workspace --release              # 529 tests
tools/check-no-std.sh                         # the bare-metal build, when a target exists
```

`--release` matters for `verify` and for the full suite: the property
harness is about 26,000 ed25519 operations and a debug build takes
minutes.

## The crates

| crate | what it is | stage |
|---|---|---|
| `abi` | the shared vocabulary: canonical codec, typed refusals, verbs, allocation-free text | — |
| `crypto` | the BrAIn Key, SHA-256/HMAC/HKDF from spec, the Merkle audit anchor, k-of-n quorum, a hash DRBG | Domain 1 |
| `rt` | pre-authorized envelopes, reflexes that can only stop, rate-monotonic scheduling with response-time analysis, three kinds of watchdog | Domain 2 |
| `memory` | the state graph with provenance, retrieval with no model in it, the skill graph, consolidation, the segmented signed journal | Domain 5, Stage 3 |
| `body` | §8 as a state machine; capability schemas; per-body policy | §8 |
| `kira` | the eight-stage gate, capability attenuation, the audit trail | Domain 3 |
| `cognition` | four tiers, brain placement, the ambient loop, escalation under a budget | Domain 4, Stage 5 |
| `worlds` | self / human / internet, ingress tainting, advocacy | §3.8 |
| `entity` | Key + state graph + every body at once; arbitration; the life record | Stage 4 |
| `transport` | framing, attested sessions, replay protection, link health | — |
| `adapters` | ROS 2 node graphs and MHS device schemas become capability schemas | Phase 1 |
| `proof` | the properties KIRA must have, checked exhaustively over a bounded domain | Stage 6 (part) |
| `fleet` | a directory that cannot command, a marketplace that cannot push, hosts that hold nothing | Stage 7 |
| `export` | entity export v1.0 — complete, documented, signed, round-tripped | freedom principle |
| `bodyd` | what runs on the machine with the motors; links `miniblur-core` | Domain 2 |
| `nostd` | a canary that depends on everything and is unconditionally `#![no_std]` | — |
| `host` | the only crate allowed an operating system underneath it | — |

## Discipline

Inherited from Part 0 and §3.9, and not negotiable here.

- **`no_std` first.** Every crate below `host` compiles with nothing
  beneath it. The platforms that eventually matter will not have Linux.
- **Near-zero dependencies.** `ed25519-dalek` for the Key, because
  hand-rolling a signature scheme is how you get one with a bug in it.
  Everything else — hashes, codecs, schedulers, indexes, the JSON reader,
  the proof harness — is ours and is understood at author level.
- **`#![forbid(unsafe_code)]`** in every crate, with no exceptions in
  this workspace.
- **A stage is not complete until its suite is green.**

## The properties that are structural rather than checked

Some guarantees are not tested because they cannot fail at runtime.

**`Granted` has no public constructor.** It holds a private field of a
private type declared in `kira::capability`. No code outside
`brainos-kira` can write `Granted { .. }`, and exactly one expression
anywhere constructs one — the line in `gate::commit` that runs after all
six preceding stages returned `Pass`. A cognitive resource, however
capable or however manipulated, can produce any bytes it likes and none
of them are a `Granted`.

**Memory cannot live in a body or a model.** Every mutating method on
`StateGraph` takes `&MemoryHandle`. The handle is not `Clone`, not
`Copy`, not `Default`, and is issued once inside the entity. The failure
mode where a driver caches "its" state, or a model wrapper keeps a
history that is really the entity's memory living in the wrong place,
does not compile.

**A body that changes what it can do is a different body.** `BodyId` is
`H(attestation key ‖ schema digest)`. A device that reconnects
advertising a verb it did not have before does not match its old id and
must be acquired again through §8.

## What is checked, and what is not

`brainos proof` enumerates 8,640 cases — every combination of signature
validity, request freshness, body binding, argument shape, body fitness,
ruleset, argument danger and key availability — runs each through the
real gate, and asserts 17 properties over every resulting trace. Within
that domain the properties hold for *all* inputs.

It is **not** a machine-checked proof, and the report says so in the same
type size as the result. Outside the domain, nothing is established. That
a body policy's `simulate()` is a sound forward model of a physical
system is a claim about physics that no amount of enumeration touches.
The full Stage 6 work is dated 2028–29 in the roadmap and this does not
pretend to be it.

The harness has already earned its place twice: on its first run it
caught the gate and an independently-written oracle disagreeing about
whether an irreversible act on an unfit body refuses at *simulate* or at
*policy* (the gate was right), and on its second it caught a domain
dimension that silently changed the request's shape and made half the
other dimensions inapplicable.

## Measured

`brainos bench` times the operations the architecture makes claims
about, in batches, and reports the minimum as the number to believe —
the noise on a timing is one-sided, so the minimum over many batches is
the closest thing to the uncontended cost. Representative figures from
this machine:

| operation | uncontended | budget |
|---|---|---|
| one reflex pass (Domain 2) | ~60 ns | 10 ms (Tier 0) |
| the tip-over check on one pose | ~220 ns | 10 ms |
| sealing one wire frame | ~1.8 µs | — |
| a whole gait cycle planned and verified | ~40 µs | 1 s (Tier 2) |
| one full eight-stage gate evaluation | ~160 µs | 100 ms (Tier 1) |
| recall over a graph at its episodic bound | ~1.3 ms | 100 ms |
| sealing the state graph into a journal record | ~2.1 ms | — |

Two findings fell out of measuring rather than being designed in, and
both are reported by `bench` as derived sentences rather than as a
paragraph that can go stale:

**The gate is dominated by ed25519.** Signing the proposal and verifying
it are most of an evaluation. On an Xtensa core at 240 MHz the same
evaluation is of order ten milliseconds, so a body cannot gate at its own
loop rate. That is not a performance problem to optimise away — it is the
reason Tier 0 runs inside a pre-authorized envelope instead of asking per
step, and the measurement agreeing with the architecture is the useful
direction for it to agree in.

**An entity at default bounds does not fit on the board.** A memory
encodes to about 120 bytes; 4,096 episodes is roughly 490 KiB, and an
ESP32-S3 has 512 KiB of SRAM in total. `Bounds::EMBEDDED` is 256
episodes — about 30 KiB — and that is why it exists. The same entity
remembers less in a small body and more in a large one; the bound is a
property of the body it is currently thinking in, not of who it is.

## Does experience compound?

The company's core claim is that an entity with experience is measurably
better than a new one, and §2.4 says the curve gets published.
`brainos compound` is the measurement apparatus for that curve, run
against a simulated world.

The entity faces situations described by three observable conditions.
The world's hidden rule uses two of them and ignores the third, with ten
percent noise so nothing is perfectly learnable. The thing experience is
*for* is not doing the task better — there is no motor skill to improve —
it is deciding whether to attempt at all.

Over two thousand episodes, an entity that consults what it learned ends
with 90% judgement against a control's 29%. It attempts about a quarter
of situations, which is exactly the fraction the world rewards. It learns
that the surface matters (94%) and the battery matters (100%), and does
*not* come to believe the irrelevant third condition matters — which is
the failure mode `Precondition`'s own documentation admits is possible.

Three things make that number mean something rather than being a graph:

* **A null control.** The same experiment in a world where success is a
  coin flip gives a three-point difference — noise. An entity that
  "improved" where there is no structure would mean the metric measures
  something other than learning, and the result would be worthless.
* **The metric cannot be gamed.** An entity that declined everything
  would have a perfect success rate, so the metric counts correct
  declines and not incorrect ones, and a `Policy::Never` run is in the
  suite to prove the learner beats it.
* **Caution's cost is reported.** Some of what the entity declined would
  have worked; that percentage is printed rather than hidden inside a
  success rate.

What it does not show is that a robot gets better. The world is simulated
and its structure is one I wrote. What it shows is that the mechanism
finds structure when there is some and does not manufacture an
improvement when there is none.

## What none of this shows

Nothing in this workspace has touched a motor. Mini-Blur is simulated
throughout. §7 of the Technical Execution Report is blunt about that
being exactly what Phase 0 changes, and it still is: everything between
the human and the servo is done, tested, and demonstrable on a laptop
today — and the servo has not moved.

## Bugs the suites found

Recorded because a test suite that has never found anything has not been
tried. Each of these passed a hand-written test and failed a property or
an end-to-end assertion.

1. The retrieval index went stale as memories were added, so an entity
   could confidently not remember something it had just done. Fixed by
   making the read path responsible for currency.
2. Consolidation wrote the observation count into a semantic note's text,
   which made the note's identity change every pass — so learning the
   same lesson twice made the entity wordier instead of surer.
3. Semantic memory decayed on the episodic schedule, so a long quiet
   period left the entity knowing less than when it started: the note
   faded, and the episodes it was protecting lost their protector.
4. Goal parents were indices into a vector that goals were removed from,
   so retiring one goal silently re-pointed another's parent.
5. A channel with zero observed variance made a healthy 7.4 V power rail
   read as ten sigmas. The tolerance floor is now relative — one percent
   of the reading, which is the honest claim about any sensor on a robot.
6. A new observation channel was reported as novel three times running
   while it built a baseline, damping its own salience before the
   occasion that mattered.
7. `Calibration::is_complete` treated a body with no joints — a speaker,
   a camera, a lab instrument — as permanently incomplete.
