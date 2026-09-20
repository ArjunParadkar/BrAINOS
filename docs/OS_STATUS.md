# `os/` — what was built, and what it shows

**Written 2026-09-20.** This document describes the `os/` workspace: the
substrate for Architecture v1.3 stages 3–8, built beside the existing
tree. It is a status report, not a plan — the plan is
[`EXECUTION_PLAN.md`](EXECUTION_PLAN.md) and the architecture is
`BrAInOS_Architecture_v1.3.pdf`.

Nothing in `mind/`, `core/`, `miniblur/`, `firmware/` or `body/` was
changed. Stages 1–2 and the Phase 0 work are exactly as they were and
still pass. `os/bodyd` *links* `miniblur/core`, so the hexapod's
geometry, tripod gait, tip-over check, servo table and rail budget in
the new stack are the ones the ESP32-S3 firmware already uses rather
than a second copy of them.

---

## Run it

```sh
cd os
cargo run --release --bin brainos -- demo     # Phase 0, in simulation
cargo run --release --bin brainos -- stack    # intent → gate → wire → body → gait
cargo run --release --bin brainos -- verify   # all three property domains
cargo run --release --bin brainos -- bench    # measured latencies and footprint
cargo run --release --bin brainos -- compound # does experience compound?
cargo run --release --bin brainos -- day      # can it be left running?
cargo test --workspace --release              # 618 tests
```

Transcripts of each are in [`artifacts/`](artifacts/).

---

## What exists

Eighteen crates, about 38,000 lines of Rust, 618 tests. Every crate
except `host` is `no_std`, forbids `unsafe`, and depends on nothing
outside the workspace but `ed25519-dalek` and `libm`.

| stage / domain | crate | what it is |
|---|---|---|
| Domain 1 · silicon | `crypto` | the BrAIn Key; SHA-256, HMAC, HKDF and a hash DRBG written from spec and checked against FIPS 180-4 and RFC 4231 vectors; a Merkle audit anchor; k-of-n quorum for the operations one party should not be able to do alone |
| Domain 2 · autonomic | `rt`, `bodyd` | pre-authorized envelopes, reflexes whose only escalation is stopping, rate-monotonic scheduling with exact response-time analysis, three kinds of watchdog, and the body-side runtime |
| Domain 3 · KIRA | `kira` | the eight-stage gate; capability attenuation with caveats; the audit trail as a Merkle log with signed checkpoints |
| Domain 4 · cognition | `cognition` | four tiers, brain placement (Stage 5), a forward model learned from the entity's own history, escalation under a leaky budget |
| Domain 5 · memory | `memory` | the state graph with provenance; retrieval with no model in it; the skill graph; consolidation; the segmented signed journal |
| §8 acquisition | `body`, `adapters` | the five-step protocol as a state machine, and ROS 2 / MHS bridges into steps 1–3 |
| §3.8 three worlds | `worlds` | self / human / internet postures, bounded sessions, ingress tainting, and the adversarial filter the entity runs on its human's behalf |
| Stage 4 | `entity` | one Key, one state graph, every body at once; arbitration; the life record |
| Stage 6 (part) | `proof` | two exhaustive property domains |
| Stage 7 | `fleet` | a registry that cannot command, a marketplace that cannot push, hosts that hold nothing |
| freedom principle | `export` | entity export v1.0 |
| — | `abi`, `transport`, `nostd`, `host` | vocabulary, the wire, the `no_std` canary, and the only crate with an operating system underneath it |

---

## Three guarantees that are structural rather than tested

**`Granted` has no public constructor.** It holds a private field of a
private type declared in `kira::capability`. No code outside
`brainos-kira` can write `Granted { .. }`, and exactly one expression
anywhere constructs one — the line in `gate::commit` that runs after all
six preceding stages returned `Pass`. A cognitive resource can produce
any bytes it likes and none of them are a `Granted`. This is not checked
at runtime because it cannot fail at runtime.

**Memory cannot live in a body or a model.** Every mutating method on
`StateGraph` takes `&MemoryHandle`. The handle is not `Clone`, not
`Copy`, not `Default`, and is issued once inside the entity. The failure
where a driver caches "its" state, or a model wrapper keeps a history
that is really the entity's memory living in the wrong place, does not
compile.

**A body that changes what it can do is a different body.** `BodyId` is
`H(attestation key ‖ schema digest)`. A device that reconnects
advertising a verb it did not have before does not match its old id and
must be acquired again through §8.

---

## Three property domains

`brainos verify` enumerates both completely and runs every case through
the real code.

**The gate — 8,640 cases.** Every combination of signature validity,
request freshness, body binding, argument shape, body fitness, ruleset,
argument danger and key availability. Seventeen properties asserted over
every resulting trace, including that each case refuses at exactly the
stage an independently-written oracle says it should.

**The two envelopes — 400 cases.** The property the body path rests on:
*a body never applies a command outside the intersection of what the
seat granted and what its own mechanism permits.* Six dimensions, run
against the real `BodyRuntime`, the real `Hexapod`, and the real
`miniblur-core` geometry.

**Hostile input — 36,000 inputs across nine decoders.** Random bytes and
mutated-valid bytes into every decoder in the workspace. Three
properties: no decoder panics; anything that decodes re-encodes to
exactly its own bytes; and a format carrying an integrity check catches
*every* mutation. It found a real canonicality bug — the float decoder
accepted `-0.0` and non-canonical NaNs, which the encoder flattens, so
decoding was not injective and a signature over the bytes was not a
signature over the value.

It is **not** a machine-checked proof, and the report says so as
prominently as it says the result. Outside the domains nothing is
established; that a body policy's `simulate()` is a sound forward model
of a physical system is a claim about physics that no enumeration
touches. Full Stage 6 remains 2028–29 work.

The harness has already earned its place three times. On its first run
it caught the gate and the oracle disagreeing about whether an
irreversible act on an unfit body refuses at *simulate* or at *policy* —
the gate was right. On its second it caught a domain dimension that
silently changed the request's shape and made half the other dimensions
inapplicable. The envelope domain's
`every_dimension_..._changes_something` test exists because a dimension
that never changes an outcome multiplies the domain for nothing.

---

## The property the body side exists for

`os/bodyd` runs on the machine with the motors and its job is a refusal:
**it checks the same envelope the seat checked, and refuses anyway.**

`a_subverted_seat_cannot_make_the_body_fall` runs a robot whose centre
of mass has moved aft with the seat's schema unchanged. The gate grants
the walk — the token is genuine, minted by the real commit stage; the
wire is authenticated. The body refuses with `would-break-invariant`
because its own convex-hull support polygon says the motion is a fall.
Nothing moves.

That is the realistic shape of the failure this defends against. Nobody
breaks ed25519; somebody ships a schema with a number in it that the
physical robot cannot survive.

---

## Two things the measurements found

`brainos bench` times the operations the architecture makes claims
about, in batches, reporting the minimum as the number to believe.

**The gate is dominated by ed25519.** Signing the proposal and verifying
it are most of an evaluation (~160 µs total on this laptop). On an
Xtensa core at 240 MHz the same evaluation is of order ten milliseconds,
so a body cannot gate at its own loop rate. That is not a performance
problem to optimise away — it is the reason Tier 0 runs inside a
pre-authorized envelope rather than asking per step. The measurement
agreeing with the architecture is the useful direction for it to agree
in.

**An entity at default bounds does not fit on the board.** A memory
encodes to about 120 bytes; 4,096 episodes is ~490 KiB, and an ESP32-S3
has 512 KiB of SRAM in total. `Bounds::EMBEDDED` is 256 episodes, about
30 KiB. The same entity remembers less in a small body and more in a
large one — the bound is a property of the body it is currently thinking
in, not of who it is.

---

## Can it be left running?

`brainos day` asks a different question from everything else here: not
whether something works, but whether an entity can be left alone with a
body for a working day. Twenty thousand ticks, rare faults, a human who
speaks occasionally, and an idle period at the end.

A representative day: both kinds of fault noticed on their **first**
occurrence; the human aware in **100%** of the windows a fault occurred
in; **90%** interrupt precision; **zero** interruptions on ticks where
nothing happened; full escalation budget at the end.

It found three attention bugs, none of them visible to a unit test:
repetition damping that never decayed, so a recurring fault went
permanently unmentioned; damping keyed on channel identity alone, so a
frequent mild anomaly masked a rare severe one on the same channel (54
leg stalls, none surfaced); and consequence being *additive*, so a
three-sigma blip on a tilt sensor interrupted a human because tilt
matters on a body that can fall. Consequence scales evidence now; it
does not create it.

And one finding that is not a bug: **consolidation never runs during a
busy day.** A workshop does not give an entity two hundred consecutive
quiet ticks, and rewriting memory while a gait loop runs is exactly what
the quiet check exists to prevent. The dreaming happens when the robot
is put away, which is where the architecture's word for it came from.

---

## Does experience compound?

`brainos compound` is the measurement apparatus for the curve §2.4 says
gets published — entity age against task success — run against a
simulated world.

The entity faces situations described by three observable conditions;
the world's hidden rule uses two and ignores the third, with ten percent
noise so nothing is perfectly learnable. What experience is *for* is not
doing the task better but deciding whether to attempt at all.

Over 2,000 episodes an entity that consults what it learned ends with
**90% judgement against a control's 29%**. It attempts about a quarter of
situations, which is exactly the fraction the world rewards. It learns
that the surface matters (94%) and the battery matters (100%), and does
not come to believe the irrelevant third condition matters — the failure
mode `Precondition`'s own documentation admits is possible.

Three things make it a result rather than a graph:

* **A null control.** The same experiment where success is a coin flip
  gives a three-point difference. An entity that "improved" there would
  mean the metric measures something other than learning.
* **The metric resists the obvious cheat.** An entity that declined
  everything would have a perfect success rate, so judgement counts
  correct declines and not incorrect ones, and a `Policy::Never` run is
  in the suite to prove the learner beats it.
* **Caution's cost is reported.** Some of what it declined would have
  worked; that percentage is printed rather than buried.

It does **not** show that a robot gets better. The world is simulated and
its structure is one I wrote. It shows that the mechanism finds structure
when there is some, and does not manufacture an improvement when there is
none — which is the part that can be established before there is real
data, and the apparatus is ready for when there is.

### And when the world changes?

The harder question, and the one that decides whether accumulated
experience is an asset or a **liability**.

Running it found a hole in the design rather than a bug in the code. The
entity learns that walking works on carpet, so it declines to walk on
tile. The floor is replaced with tile that walks perfectly well, and the
entity makes **zero** attempts there — never observes a success, never
revises, loses its competence permanently. Its own caution prevents its
correction, and the more experience it has the more confident the
mistake.

The obvious fix is a null. Occasional random exploration costs heavily in
the stable regime — attempting what you believe will fail is mostly
failing, and with noise the occasional lucky success gets encoded as a
precondition — and recovers little.

What closed it was changing what a precondition *is*. Necessity is a
claim about absence: what makes something required is that *without* it
the thing fails. Intersecting the context of successes measures
co-occurrence, which is a different claim, and holding a correlate as a
cause is the trap. So strength is measured from **tests** — attempts
made without the tag — an untested correlate never binds, and a belief
that has not been tested recently stops binding, which forces the
occasional cheap test that keeps it honest.

With that, the same entity recovers from 88% to 87% after the change,
making 382 attempts where it previously made none. And the epistemics
read properly: *carpet 87%*, *battery 100%*, and the irrelevant third
condition at **0%** — tested, and found not to matter.

This is the most substantive change to the architecture in the work
below. It is worth Arjun's attention specifically, because it says
something about the moat: accumulated experience compounds only if the
entity keeps checking that the world still matches it, and the checking
has to be built in rather than hoped for.

---

## What this does not show

**Nothing here has touched a motor.** Mini-Blur is simulated throughout.
§7 of the Technical Execution Report is blunt that this is exactly what
Phase 0 changes, and it still is. What is now true is narrower and
worth saying precisely: *everything between the human and the servo is
built, tested and demonstrable on a laptop today.* The servo has not
moved.

**The `no_std` claim is checked at source level, not by a bare-metal
build.** `cargo test -p brainos-nostd-check` verifies that no crate
below `host` mentions `std`, that every one declares `no_std` and
forbids `unsafe`, and that the external dependency list is still two
crates long. The stronger check — building the canary for a target with
no operating system — needs `rustup target add` and no bare target is
installed on this machine. `os/tools/check-no-std.sh` runs it when one
is, and exits 2 rather than 0 when none is, so a check that did not
happen cannot report success.

**The ROS 2 adapter is brittle on purpose.** A ROS topic declares a
message type and nothing about what values are safe, so a robot that
publishes no limits comes back visible and unmovable. Risk T4 in the
register says the adapter is brittle across robots; it is brittle in the
direction of refusing to move, and there is deliberately no default
speed.

---

## Bugs the suites found

Recorded because a test suite that has never found anything has not been
tried. Each passed a hand-written test and failed a property or an
end-to-end assertion.

1. The retrieval index went stale as memories were added, so an entity
   could confidently not remember something it had just done.
2. Consolidation wrote the observation count into a semantic note's
   text, which changed the note's identity every pass — so learning the
   same lesson twice made the entity wordier rather than surer.
3. Semantic memory decayed on the episodic schedule, so a long quiet
   period left the entity knowing less than when it started.
4. Goal parents were indices into a vector goals were removed from, so
   retiring one goal silently re-pointed another's parent.
5. A channel with zero observed variance made a healthy 7.4 V rail read
   as ten sigmas. The tolerance floor is now one percent of the reading.
6. A new observation channel was reported as novel three times running
   while it built a baseline, damping its own salience before the
   occasion that mattered.
7. `Calibration::is_complete` treated a body with no joints — a speaker,
   a camera, a lab instrument — as permanently incomplete.
8. The tripod gait has a slowest usable cycle: below about a third of
   nominal the body shuffles, the rail current rises rather than falls,
   and the support polygon spends longer near its worst margin. The body
   now refuses rather than quietly walking faster than it was asked to.
9. `default_envelope` took the *intersection* of what a region's verbs
   declared for a channel, which makes a region with a fast verb and a
   slow one unable to perform the fast one. Found by running the whole
   stack end to end.
10. The codec's float decoder accepted `-0.0` and non-canonical NaNs,
    so decoding was not injective and a signature over the bytes was not
    a signature over the value. Found by the hostile-input harness.
11. Three attention bugs found by a day of ambient operation: damping
    that never decayed, damping keyed on channel rather than channel and
    severity, and consequence adding to salience rather than scaling it.
12. A decline was being recorded as an attempt — and, once beliefs could
    expire, as a *test* of them, so an entity that declined because it
    did not want to find out was recorded as having found out.
13. Preconditions were learned by intersecting the context of successes,
    which measures co-occurrence and not necessity. See above; this one
    is a design change rather than a fix.

---

## Where this leaves Phase 0

The eight-week plan's entity-side column — the §8 native adapter, the
hexapod capability schema, the KIRA policy, calibration into the state
graph, two-bodies-one-memory — has a second, independent implementation
in `os/` with the whole path from proposal to servo command tested end
to end. It does not replace the Phase 0 work on this branch; it is the
substrate that work grows into, and `os/bodyd` sharing `miniblur-core`
means the two agree about the body by construction rather than by
review.

The critical-path item is unchanged and is not software: **Mini-Blur has
never been powered on.**
