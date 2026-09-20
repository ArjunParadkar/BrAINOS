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
cargo run --release --bin brainos -- fleet    # does it transfer between entities?
cargo run --release --bin brainos -- day      # can it be left running?
cargo run --release --bin brainos -- incident # reconstruct it, and prove it
cargo run --release --bin brainos -- degrade  # what happens when the cloud goes
cargo run --release --bin brainos -- leave    # the freedom principle, exercised
cargo run --release --bin brainos -- export FILE   # write an entity out
cargo run -p read-export --release -- FILE         # and read it with none of our code
cargo run --release --bin brainos -- stat     # what this build contains

cargo test --workspace --release              # 683 tests
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
| `proof` | five domains: the gate and the two envelopes enumerated exhaustively, every decoder fuzzed, twenty-nine attacks that try to get away with it, and every instant at which power can be lost during a journal write | Stage 6 (part) |
| `fleet` | a directory that cannot command, a marketplace that cannot push, hosts that hold nothing | Stage 7 |
| `export` | entity export v3 — complete, documented, signed, round-tripped, and readable by `tools/read-export`, which depends on no crate of ours | freedom principle |
| `bodyd` | what runs on the machine with the motors; a hexapod that links `miniblur-core` and an arm with a torque budget | Domain 2 |
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

A third harness feeds every decoder in the workspace random bytes and
mutated-valid bytes — about 36,000 inputs — and asserts three things: no
decoder panics; anything that decodes re-encodes to exactly its own
bytes; and for the formats that carry an integrity check, no mutation
gets through at all. That harness found the float canonicality bug above.

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

### And when the world changes?

The harder question, and the one that decides whether accumulated
experience is an asset or a liability. The entity learns that walking
works on carpet, so it declines to walk on tile. If the floor is later
replaced with tile that walks perfectly well, it never attempts there,
never observes a success, and never revises — *its own caution prevents
it from learning that its caution is wrong*, and the more experience it
has the more confident the mistake.

Running it confirmed the hole exactly: **zero** attempts in the new
regime, competence lost permanently.

The obvious fix — occasional random exploration — is a null. Attempting
what you believe will fail is mostly failing, it costs a great deal in
the stable regime, and it recovers little.

What closed it was changing what a precondition *is*. Necessity is a
claim about absence: what makes something required is that *without* it
the thing fails. Intersecting the context of successes measures
co-occurrence, which is a different claim, and holding a correlate as a
cause is the trap above. So strength is now measured from **tests** —
attempts made without the tag — and a belief that has not been tested
recently stops binding, which forces the occasional test that keeps it
honest. Attempting with the tag a thousand more times says nothing about
whether the tag is needed.

With that, the same entity recovers from 88% to 87% after the world
changes under it, making 382 attempts where it previously made none. And
the epistemics read properly: it ends up holding *carpet 87%* and
*battery 100%* while the irrelevant third condition sits at **0%** —
tested, and found not to matter.

## Can it be left running?

Every other test asks whether something works. `brainos day` asks
whether an entity can be left alone with a body for a working day:
twenty thousand ticks of telemetry, rare faults, a human who speaks now
and then, and an idle period at the end when the robot is put away.

The balance it measures is easy to get wrong in both directions. An
entity that surfaces everything is a firehose nobody keeps; one that
surfaces nothing is a machine that quietly fails. So the metric is
deliberately **not** per-occurrence recall — a leg stalling fifty times
should be known about once, promptly, and then counted, and scoring
per-occurrence recall would rank the firehose highest.

A representative day: both kinds of fault noticed on their **first**
occurrence; the human aware of a fault in **100%** of the windows one
occurred in; 42 interruptions, **90%** of which deserved one; **zero**
interruptions on ticks where nothing happened; and the full escalation
budget still available at the end.

Three bugs came out of it, all in the attention model and none visible
to a unit test:

1. **Repetition damping never decayed.** An entity that surfaced a
   recurring fault six times went permanently silent about it. A leg
   that keeps stalling should keep being mentioned; the tenth occurrence
   is worse news than the first.
2. **Damping keyed on the channel alone.** A leg drawing 1.9 A under
   heavy motion — often, harmlessly — saturated the counter for
   `leg.current`, so a leg drawing 2.8 A because it had *stalled* was
   never mentioned at all. Fifty-four stalls, none surfaced. Damping is
   keyed on the channel and the severity band now.
3. **Consequence was additive.** A channel that matters carried a
   constant into the score whatever it read, so a three-sigma blip on a
   tilt sensor — six tenths of a degree, well inside its own noise —
   interrupted a human. Consequence answers *if this is real, how bad is
   it*; surprise answers *how much reason is there to think it
   happened*. They multiply now. A refusal is treated as evidence in its
   own right, because the gate refusing something *is* the event.

And one finding that is not a bug: **consolidation never runs during a
busy day.** A workshop does not give an entity two hundred consecutive
quiet ticks, and rewriting memory while a gait loop is running is
exactly what the quiet check exists to prevent. The dreaming happens
when the robot is put away, which is where the architecture's word for
it came from.

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
8. The tripod gait has a slowest usable cycle; below it the body shuffles
   and the rail current rises rather than falls. It now refuses rather
   than quietly walking faster than it was asked to.
9. `default_envelope` took the *intersection* of what a region's verbs
   declared for a channel, which makes a region with a fast verb and a
   slow one unable to perform the fast one. An arm whose `home` was
   limited to 60 °/s capped `move_joint` at 60 too, though the
   manufacturer declared 180. It takes the union now, and the narrow
   verb is still held to its own bound at validate — three checks, each
   doing one job. Found by running the whole stack end to end, which is
   the kind of bug unit tests do not have the shape to see.
10. The codec's float decoder accepted `-0.0` and non-canonical NaNs,
    which the encoder flattens. Decoding was therefore not injective:
    two different byte strings produced the same value, so a signature
    over the bytes was not a signature over the value — the exact
    property the audit Merkle log, the export signature and the
    body-identity digest all rest on. Found by the hostile-input
    harness, which mutates a valid encoding and checks that anything
    which decodes re-encodes to its own bytes. Three of four hundred
    mutated commands did not.
11. The hostile-input harness's own mutation generator could pick the
    same bit twice and cancel, emitting its own seed unmodified — which
    reads in the report exactly like an integrity check having failed.
    One frame in six thousand, about what the arithmetic predicts.
12. An entity's age was whatever number the host passed to `wake()`.
    §2.4 makes age the x-axis of the curve the moat becomes visible on,
    and it was settable by the one party with an interest in setting it:
    a 25-tick-old entity printed as 50,000 ticks old because that was
    the number I happened to pick. `Life`'s own comment says that a
    metric which is easy to move without getting better will be moved
    without getting better. The counter is the entity's now, recovered
    from its own record, and the caller's number is ignored. Found by
    running the vendor migration and reading the output rather than the
    assertion.
13. A skill's learned judgement did not survive a reboot. A skill node's
    content is prose, and prose was the only durable form a skill had,
    so `rehydrate` recovered the verb, the name and the counts and
    reconstructed everything else as empty. An entity that spent sixty
    attempts establishing that its gait needs carpet — nineteen of them
    genuine tests of the claim — woke up knowing its success rate and
    nothing about why:

        BEFORE  preconditions=[("carpet", 100, 19)]  binding=1  per-body=1
        AFTER   preconditions=[]                     binding=0  per-body=0

    Nodes now carry their exact structured form alongside their readable
    one. Found by going to build the fleet-transfer experiment and
    asking where one entity's preconditions are actually kept.
14. A `Delegation` could not be used by anyone except the entity itself,
    although its own documentation says it is "standing authority handed
    to something that is not the entity". The gate authenticated the
    request against the **issuer's** key, which a third-party holder by
    definition does not have. Every delegation test in the suite
    delegated to the entity's own id, so the case never arose. Found by
    the adversary domain — and only because an attack failed in the
    wrong *place* rather than failing to fail, which is why that domain
    distinguishes `Repelled` from `RepelledWrongly`.
15. A skill's candidate preconditions were unbounded. Harmless while
    they lived only in memory; not harmless once a skill is serialized
    into a node, because an unbounded skill is then an unbounded node.
    Capped at 32, evicting the weakest *untested* candidate first — an
    entity should forget a hunch before it forgets something it found
    out.
16. In the fleet experiment itself, veterans were selling every tag they
    had ever tracked rather than the ones they could back up, so the
    matched and blind arms adopted near-identical lists and came out to
    the digit. A seller sells findings, not hunches. There is now a test
    that the two arms differ at all, because two arms agreeing perfectly
    is what a broken experiment looks like when it is lucky enough to be
    obvious.
