# `os/` — what was built, and what it shows

**Written 2026-09-20.** This document describes the `os/` workspace: the
substrate for Architecture v1.3 stages 3–8, built beside the existing
tree. It is a status report, not a plan — the plan is
[`EXECUTION_PLAN.md`](EXECUTION_PLAN.md) and the architecture is
`BrAInOS_Architecture_v1.3.pdf`.

Nothing in `mind/`, `core/`, `miniblur/`, `firmware/` or `body/` was
changed.

---

## If you read one section, read this one

Six things came out of running the system rather than reading it, and
each one changes a decision rather than a line of code.

**1. An entity does not fit on the ESP32-S3, and shrinking its memory is
not the fix.** The old footprint number counted the encoded state graph
and called that the entity, leaving out the retrieval index entirely and
the audit trail's record window. Measured properly: default bounds need
4,530 KiB, `Bounds::EMBEDDED` alone still needs 285 KiB, and the board
has about 192 KiB. It fits at 153 KiB only with the audit window shrunk
too. *Phase 0 hardware decision.*

**2. What is left over on the board is a duration, not a size.** The
audit log keeps 32 bytes per gated decision forever, so the remaining
39 KiB is roughly two minutes at ten decisions a second. Leaves have to
go to flash, and now safely can: the root comes from the frontier, so
forgetting leaves does not move it and every checkpoint stays correct.
*Firmware decision.*

**3. A skill marketplace without provenance is net-negative.** Measured,
not argued: a skill adopted from a deployment unlike your own left the
entity *permanently worse* than one that learned alone — 80% against
87%, still short after twelve hundred episodes. Selling skills blind
degrades a fleet. The fix (deliver the claim, not the seller's numbers,
and probe it) recovers it to 87%, and both halves are now in the types.
*§6 product decision.*

**4. A `Delegation` could not be used by anyone but the entity itself.**
Its own documentation says "standing authority handed to something that
is not the entity", and the gate authenticated the request against the
*issuer's* key, which a third-party holder by definition does not have.
Every test in the suite delegated to the entity's own id, so it never
came up. Stage 7's fleet story rested on a feature that did not work.

**5. A skill's learned judgement did not survive a reboot.** Sixty
attempts establishing that a gait needs carpet, nineteen of them genuine
tests, and the entity woke up knowing its success rate and nothing about
why. That is §2.4's moat evaporating at every power cycle and at every
export — including in the vendor migration, whose tests passed because
they counted skills rather than asking what the skills knew.

**6. An entity's age was whatever number the host passed to `wake()`.**
§2.4 makes age the x-axis of the curve the moat becomes visible on, and
it was settable by the one party with an interest in setting it.

Everything below is the detail. The critical-path item is unchanged and
is not software: **Mini-Blur has never been powered on.** Stages 1–2 and the Phase 0 work are exactly as they were and
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
cargo run --release --bin brainos -- verify   # all five property domains
cargo run --release --bin brainos -- bench    # measured latencies and footprint
cargo run --release --bin brainos -- compound # does experience compound?
cargo run --release --bin brainos -- fleet    # does it transfer between entities?
cargo run --release --bin brainos -- day      # can it be left running?
cargo run --release --bin brainos -- incident # reconstruct it, and prove it
cargo run --release --bin brainos -- degrade  # what happens when the cloud goes
cargo run --release --bin brainos -- limits   # where this breaks
cargo run --release --bin brainos -- leave    # the freedom principle, exercised
cargo run --release --bin brainos -- export FILE   # write an entity out
cargo run -p read-export --release -- FILE         # read it back with none of our code
cargo test --workspace --release              # 712 tests
```

Transcripts of each are in [`artifacts/`](artifacts/).

---

## What exists

Eighteen crates plus a standalone tool, about 41,000 lines of Rust, 662
tests. Every crate
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
| Stage 6 (part) | `proof` | five property domains: three exhaustive, one fuzzed, one adversarial |
| Stage 7 | `fleet` | a registry that cannot command, a marketplace that cannot push, hosts that hold nothing |
| freedom principle | `export`, `tools/read-export` | entity export v3, and a reader that depends on no BrAInOS crate |
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

## Five property domains

`brainos verify` enumerates the first two completely and runs every case
through the real code.

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

**The adversary — 29 attacks.** The first three domains ask whether the
system does the right thing; this one asks whether it can be made to do
the wrong thing on purpose, which is a different question and the one
that matters for a word like *unforgeable*. Forge a proposal; keep a
genuine signature and swap the request under it; pair a valid token with
a different action, body, time or entity; widen a delegation, drop its
caveats, use it after revocation; reach a drive directly, through a
delegation scoped to everything, and with the owner's full cooperation;
launder an untrusted claim into an admissible one through consolidation;
remove an audit entry; reach a quorum by signing twice; replay a frame;
command a body before it has proven what it is. Every attack runs
against the real types — a forged signature is a real ed25519 signature
by the wrong key.

None succeeded. One failed in the *wrong place*, which is what the
domain was worth building for: a delegation scoped to everything died at
*authenticate* rather than at the drive, because the gate authenticated
the request against the **issuer's** key and a third-party holder by
definition does not have it. `Delegation` is documented as "standing
authority handed to something that is not the entity" and it could not
be; every delegation test in the suite delegated to the entity's own id,
so the case never came up. The gate now separates the signer from the
authority.

**Power loss — 546 instants.** Robots lose power; not as an edge case,
as a normal Tuesday. §3.7 puts memory in the state graph and nowhere
else, which makes the journal the single point at which an entity's
identity can be destroyed. For a write of *n* bytes there are *n+1*
moments it can stop, and every one is enumerated, on three media models:
an unwritten tail that reads zero, one that still holds the slot's
previous contents, and one that is garbage. The second is the nastiest —
valid-looking data sitting immediately after a valid header — and the
records are deliberately of differing lengths so that a short write over
a longer one leaves that tail behind.

The property is not "the newest write survives". It cannot be, and a
system claiming it would be lying. It is weaker and more useful:
**whatever comes back was actually committed, in full, and is never
older than the last write that completed.** An entity may lose the last
thing it was doing. It may not come back as a mixture of two entities,
and it may not come back as an earlier version of itself. Across every
depth of history from an empty medium to six generations, 546 instants:
four kept the newest write, 542 fell back cleanly, none lost anything
that had been committed, and none produced a state the entity had never
been in. Separately, every single-bit flip in a complete record is
caught by the CRC or the signature; 418 sampled, none slipped.

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

**An entity does not fit on the board, and shrinking its memory is not
enough.** This number was wrong in the optimistic direction until the
limits run forced a proper measurement. It counted the *encoded* state
graph — what goes in the journal — and called that the entity. The
entity is larger than its journal: strings and vectors carry capacities
and headers, the retrieval index is not in the journal at all, and the
audit trail keeps a working window of records with eight stage notes
each. Populated to the bound, with every action actually gated:

| configuration | graph | index | audit | total | 192 KiB available |
|---|---|---|---|---|---|
| default bounds | 1,304 KiB | 688 KiB | 2,536 KiB | 4,530 KiB | no |
| embedded bounds | 82 KiB | 43 KiB | 158 KiB | 285 KiB | **still no** |
| embedded bounds + embedded audit window | 82 KiB | 43 KiB | 27 KiB | **153 KiB** | yes |

So `Bounds::EMBEDDED` on its own is not sufficient, which the old
measurement said it was. The audit trail's *record window* dominates,
because a record keeps its eight stage notes as eight separate strings,
and the records' real home is the machine of record rather than the
board. `Entity::for_embedded_body` sets both numbers.

**And the remaining headroom on a board is a duration, not a size.** The
Merkle log keeps 32 bytes per gated decision forever, so the 39 KiB left
over is about 1,236 more decisions — roughly two minutes at ten a
second. Keeping every leaf in SRAM was a workstation decision being made
silently on the board. `MerkleLog::forget_leaves_before` is the answer
and it is safe for a specific reason: the root comes from the frontier,
not from the leaves, so forgetting them does not move it and every
checkpoint the entity ever signed stays correct. What is lost is the
ability to *produce* a proof for a forgotten decision — and a proof
issued before it was forgotten still verifies, because a proof is
checked against a root rather than against a log. The board keeps
minutes of leaves and hands the rest to flash.

---

## Where this breaks

Every claim in the architecture has an implied scale, and a claim whose
scale has not been measured is a claim with a hole in it. Part 0 says no
artifact ships with a known correctness defect; `brainos limits` is the
other half of that — knowing where the claims stop being true, before
somebody finds out in a warehouse.

Each limit is labelled by kind, because the three call for completely
different reactions. **hard** is a constant in the code, there on
purpose. **bound** is a configured cap; raising it costs memory and
nothing else. **soft** means nothing stops you, it simply gets worse —
and those are the point of the exercise, because there is no error to
notice.

| limit | kind | value |
|---|---|---|
| bodies one entity holds at once | soft | 1,000+ accepted; verb lookup is a linear scan over all of them |
| episodes an entity remembers | bound | 4,096 default, 256 embedded |
| candidate preconditions per skill | hard | 32 |
| a node's structured form | bound | 4,096 bytes; refused, never truncated |
| delegation chain depth | soft | 4,096+ verified; bounded by the expiry being narrowed |
| transport frame payload | hard | 448 bytes |
| envelope channels | hard | 8 |
| the retrieval index's own footprint | soft | ~210 bytes per memory |
| producing one audit proof | soft | linear in the whole history |
| term length for retrieval | hard | 8 bytes, truncating |

**Retrieval holds.** One memory that answers the question, planted among
distractors of the same shape and vocabulary — a haystack of unrelated
text would flatter BM25. It comes back at rank 1 at 100, 1,000, 4,096
and 16,385 memories, and a query takes 1–7 µs.

**But the index is the embedded constraint, not the memories.** At the
default bound the index is ~870 KiB — larger than the memories it
indexes, and on its own more than an ESP32-S3's entire 512 KiB. The
footprint number in `bench` counts the memories and not the index; that
understatement is the kind that survives into a datasheet, so there is
now a test asserting the index is the bigger number.

**And a defect the run found.** `root()` rebuilt every level of the
Merkle tree — 0.7 s at a million decisions, which a robot making ten
gated decisions a second reaches in about a day. The root is what every
checkpoint needs, and checkpoints are what make an audit log evidence
rather than a diary, so an expensive root means taking fewer of them. It
is now a frontier of perfect-subtree roots, and the promote-the-odd-node
tree turns out to be exactly the fold of the binary decomposition of *n*
from the right, so the two constructions coincide:

| decisions | proof path | root before | root after | prove |
|---|---|---|---|---|
| 1,000 | 10 | 898 µs | **4 µs** | 879 µs |
| 100,000 | 17 | 67 ms | **3 µs** | 72 ms |
| 1,000,000 | 20 | 732 ms | **4 µs** | 709 ms |

Producing a *proof* is still linear and is left alone on purpose: proving
happens after an incident, not in the loop. A clever root that disagreed
with the obvious one would silently invalidate every proof ever issued,
so `root_the_slow_way` is kept and the two are checked against each other
on every prefix up to 1,500 leaves.

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

### And on the board Phase 0 actually runs on

§5 says the bound is a property of the body rather than of who the
entity is. That is easy to write and worth checking: an entity that
notices what matters with four thousand episodes of room and misses it
with two hundred and fifty-six is an entity whose attention was paid for
by its memory, and Phase 0 runs on the small one.

One day does not test it — the entity ends with 201 memories and the
embedded bound is 256, so the bound never bites, and there is a test
asserting that so nobody shortens the comparison and reads a vacuous
pass as a result. Ten days do:

| | workstation | ESP32-S3 |
|---|---|---|
| memories at the end | 2,431 | **260** |
| …of those, about faults | 448 (18%) | 256 (**98%**) |
| …routine work | 1,980 | 2 |
| faults noticed on the first | 1 | 1 |
| human aware of what mattered | 98% | 98% |
| interrupt precision | 98% | 98% |
| interruptions per 1k ticks | 2 | 2 |
| could be left running | yes | yes |

The board keeps a tenth of what the workstation keeps and loses
**nothing** in noticing. Attention turns out to be a property of the
ambient loop — the damping, the severity bands, consequence scaling
evidence rather than adding to it — and not of how much is remembered.

And look at *what* it keeps. Forced to forget, the small body ends up
98% faults against the workstation's 18%: the routine walks go and the
things that went wrong stay. A constrained entity's memory is not merely
smaller, it is denser in the things worth having kept.

**Getting to that number required fixing something.** Asking "does it
forget in the right order" exposed that there was no order to speak of,
because the salient things were never memories in the first place: over
ten simulated days the entity surfaced dozens of rail dips and leg
stalls to its human and wrote *none* of them down. A robot that stalls a
leg fifty times and remembers none of them cannot learn from them and
cannot tell anyone next week that this has happened before. What the
ambient loop surfaces is now recorded, carrying the salience it already
computed, so attention and pruning are one judgement rather than two
that can disagree. And salience had only ever been set once at 80 for
everything and then decayed, which made the forgetting order
approximately "oldest first".

A surfaced reading also used to be the bare channel name — `leg.current`
— which is a useless thing to tell a human and a worse thing to write
into a memory somebody may read a week later. It now carries what was
read, what was expected and how far off that is.

### And one finding that is not a bug

**Consolidation never runs during a busy day.** A workshop does not give an entity two hundred consecutive
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

## Can the customer actually leave?

§1 says the customer owns the entity permanently and that leaving means
abandoning a life rather than losing access to one. Every company says
something like that; the question is whether the architecture makes it
true. `brainos leave` does it.

An entity lives its whole life on one vendor's hexapod — learns that the
workshop floor is carpet, learns that its own left middle femur binds,
practises walking and finds out by trying that the gait works on carpet
and fails on the tiled section. The customer exports it. Then that
vendor is **destroyed entirely**: entity, bodies, gate, audit log,
journal. One file crosses the gap, plus a Key the vendor never held. A
competitor's six-axis arm, described in the manufacturer's own format,
adopts it.

What arrives: the same entity, its memories, its skills, its age, and
*what it worked out by trying* — `carpet 100% over 19 tests`, intact.
That last one is the moat, and it is not the file, it is the nineteen
attempts behind it.

What correctly does not: the hexapod's calibration is still remembered —
memory is not edited for convenience — but it does not govern the arm,
which is calibrated from scratch. `legs.walk` is gone from the
repertoire and asking for it is refused at *authorize*. **A memory of a
skill is not a licence to act with a body you do not have.** The arm
then moves on the first try.

And the file is readable without us. `os/tools/read-export` is a
standalone program whose entire dependency list is `ed25519-dalek`, with
SHA-256 implemented in eighty lines from FIPS 180-4 and the layout taken
from the specification that ships *inside every export*. It verifies the
signature, recomputes the Merkle root and prints what the entity
remembers. A test asserts the dependency list, because the list is the
claim: the first sign of "the customer owns their entity" rotting would
be a `brainos-*` line appearing there. A committed fixture pins the
format; 326 single-byte mutations are all caught, truncations read as
broken rather than as a smaller entity, and a byte appended after the
signature is refused.

---

## When the cloud goes away

Stage 5 says where the mind runs is a deployment decision, not an
identity decision: reflexes on the body because ten milliseconds is too
slow to catch a fall, the frontier in a datacenter because it does not
fit in 512 KiB of SRAM. That claim has a bill. If part of the mind is
somewhere else, sometimes part of the mind is **not there**.

The question is not whether the robot keeps working — the tier structure
makes that easy. It is the one that hurts in the field: *does anyone
find out?* A robot that silently drops from frontier reasoning to
reflexes and carries on looking busy is worse than one that stops,
because the results degrade in a way nobody attributes to the network
for three weeks.

`brainos degrade` takes a distributed deployment through losing the
datacenter, then the edge node, then getting both back:

| phase | ceiling | served at the tier warranted | short on reach | memories |
|---|---|---|---|---|
| everything reachable | frontier | 100% | 0 | 6 |
| datacenter unreachable | deliberate | 0% | 15 | 6 |
| edge unreachable as well | fast | 0% | 15 | 6 |
| reach restored | frontier | 100% | 0 | 6 |

The body kept moving throughout, the reflex tier never went away, and
memory kept working in every phase — §3.7 puts memory on the entity, so
an outage cannot cost memories and the record of the outage survives the
outage. Two outages are written into the entity's own state graph at the
moment the reach dropped, which is what an owner asks for after a week of
mediocre results.

**And the defect this run was written to find.** Every degraded decision
was already marked as degraded — but with the wrong cause. The field was
one boolean called `budget_limited`, set whichever of budget or reach
bit, and `wanted = reason.warrants().min(ceiling)` meant a dropped link
set it. So a network outage was reported as an affordability problem. A
human tops up the budget, nothing changes, and the robot goes on being
quietly less capable than it was yesterday. It is now
`limited_by: Option<Limit>` with `Budget` and `Reach` as separate
answers, because they are different problems with different fixes:

```
→ deliberate (novel) [limited: out of reach]
```

---

## After something goes wrong

A robot works a shift in a workshop it shares with people. At the end of
the day somebody says: *your machine damaged my equipment around two
o'clock.* The owner wants to know what happened, the insurer wants
evidence that is not simply the vendor's word for it, and the vendor
would very much like the answer not to depend on being believed.

§4 makes KIRA's eighth stage audit and says it always runs — a refusal
is recorded as carefully as a grant. On its own that is worth very
little: **an audit log held by the party it might incriminate is a
diary.** What makes it evidence is that somebody who assumes we are
lying can check it.

`brainos incident` works the shift, names the window, seals a bundle and
then has it checked by a party holding nothing but the bundle and
thirty-two bytes of public key — no log, no entity, no us. A
representative run: 180 gated decisions, 160 granted and 20 refused; the
complaint's window covers 41 of them; the bundle is about 30 kB.

**The refusals are the evidence.** What exonerates a machine is not a
record of it behaving well. It is a record of the gate having refused,
at a named stage, the specific thing it is accused of. Four of the 41
decisions in that window were requests to move faster than the body's
schema allows, each refused at *validate* with `out-of-bounds`, written
down at the time, before anyone had complained about anything.

Four ways of lying with the bundle, and what happens:

| the attempt | the result |
|---|---|
| edit what a decision says it was for | refused — the proof is over the record's exact bytes |
| edit it *and* re-encode so the two agree | refused — the new bytes are not in the log |
| pair a genuine proof with a different decision | refused — `verify_record` checks both |
| drop the refusals and present only the grants | **verifies** |

The fourth is the honest limit and it is worth stating plainly: a bundle
cannot be forged, but it *can* be curated, and nothing in the
cryptography stops its producer choosing what to include. What the
design can do is make the holes visible. The log is append-only and its
indices are consecutive, so a bundle running #70 to #110 with #76, #85,
#94 and #103 absent has four holes in it, and the insurer does not need
to know what #76 says in order to ask for it. An honest bundle over a
contiguous window has none, and a test asserts that — because if the
honest bundle also had holes, a hole would prove nothing.

And the bundle states what it is not. An audit log is a record of
*decisions*, not of physics. It can say the gate granted a walk at
0.08 m/s having checked eight things; it cannot say whether the floor
was wet or where the equipment was standing. A document that did not say
so would be read as an account of events.

---

## Does experience transfer between entities?

`compound` answers the first half of §2.4. This is the second half, and
it is the half §6's skill marketplace rests on: is what one entity
worked out worth anything to another?

It has an obvious failure mode, worth stating before the numbers. What
an entity learns is not "walking needs carpet". It is "walking needs
carpet *here*" — and nothing in the claim says where here is. A robot in
a carpeted workshop and one in a tiled kitchen learn opposite rules and
both are right.

Thirty-six entities across three deployments whose hidden rules differ
and are all equally true. Four arms: learn alone, adopt a veteran's
experience from the same kind of deployment, adopt from a random one, or
be handed the true rule as a ceiling. Pre-registered before the run:
*matched > alone > blind, and blind recovers eventually.*

The ordering held. **The recovery did not.** Under the first adoption
rule — copy the seller's evidence wholesale — a badly-sourced skill left
the entity permanently worse than one that started from nothing: 80%
against alone's 87%, still short after twelve hundred episodes. The cost
showed up as *refusal* rather than failure: blind entities declined
something that would have worked 19% of the time against alone's 12%,
and that number did not shrink. The mechanism is the one `skill.rs`
already warned about in its own comment.

So the rule changed. What transfers is the **claim**, not the seller's
numbers; the buyer holds it on trust, acts on it — which is what makes a
bought skill worth buying — and deliberately violates it one blocked
opportunity in twelve until it has five tests of its own.

|                     | first slice | final | missed opportunity |
|---|---|---|---|
| old rule · blind    | 73% | 80% | 19% → 19% |
| **new rule · blind**| 77% | **87%** | 18% → **12%** |
| old rule · matched  | 89% | 90% | |
| **new rule · matched** | 85% | **89%** | |
| learning alone      | 80% | 87% | 15% → 12% |
| ceiling (true rule) | 89% | 90% | |

A badly-sourced skill now stops costing anything after about 600
episodes and reaches a fresh entity's final level by 1,000; under the
old rule it reached neither, ever. A well-sourced one is still worth
hundreds of episodes of experience delivered on the first day and lands
within a point of the ceiling. The price is about one point off
well-sourced adoption and four deliberate violations per entity — the
cost of checking a claim that happened to be true. Under the old rule
the same mechanism made a claim that happened to be false permanent.

**The product consequence is concrete:** a skill marketplace that ships
without recording what kind of place a skill was learned in, *and*
without the buyer re-testing it, does not merely fail to help. It
degrades the fleet, permanently.

Both halves are now in the types rather than in this paragraph. A
`Manifest` carries `learned_in`, a package that will not say where it
was learned cannot be published at all, and the label is covered by the
publisher's signature so a reseller cannot relabel it. Whether somewhere
is *like here* stays the buyer's call — there is no authority that knows
whether two workshops are alike, and one that pretended to would be
confidently wrong in exactly the cases that matter.

And the last link is closed. `fleet` could decide whether a package may
be installed — signed, countersigned, within the authority the owner
granted — and nothing carried what it teaches into an entity, so the
experiment had to reach past it. `SkillContents` is what a package
delivers, bound into the publisher's signature so the contents cannot be
swapped after an owner countersigns the manifest; `Entity::take_delivery_of`
adopts them as claims held on trust. A test buys a skill learned on
shipboard ladders, installs it in a building with carpeted stairs, and
watches the entity probe its way out of the condition that does not
apply here.

The three deployments are three rules I wrote. This says what the
mechanism does when worlds differ; it does not say how much real
deployments differ, and quoting the number without that sentence
misquotes it.

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
14. **An entity's age was whatever number the host passed to `wake()`.**
    §2.4 makes age the x-axis of the curve the moat becomes visible on,
    and it was settable by the one party with an interest in setting it.
    A 25-tick-old entity printed as 50,000 ticks old because that was
    the number I happened to pick. The counter is the entity's now: a
    returning entity resumes from where it actually stopped, recovered
    from its own record. Found by running the vendor migration.
15. The wake episode was stamped with the caller's tick rather than the
    entity's. Those used to be the same number.
16. **A skill's learned judgement did not survive a reboot.** A skill
    node's content is prose, and prose was the only durable form a skill
    had — so an entity that spent sixty attempts establishing that its
    gait needs carpet, nineteen of them genuine tests, woke up knowing
    its success rate and nothing about why. Measured: `[("carpet", 100,
    19)]` before, `[]` after. That is the moat evaporating at every power
    cycle and at every export. Nodes now carry their exact structured
    form alongside their readable one.
17. A delegation could not be used by anyone but the entity itself,
    because the gate authenticated the request against the issuer's key.
    Found by the adversary domain, and only because an attack failed in
    the wrong *place* rather than failing to fail.
18. A skill's candidate preconditions were unbounded — fine when they
    lived in memory, not fine once a skill is serialized into a node.
    Capped at 32, evicting the weakest *untested* candidate first:
    forget a hunch before a finding.
19. **A dropped network link was reported as a budget problem.** The
    escalation carried one boolean, `budget_limited`, set whichever of
    budget or reach was the constraint — and since the wanted tier is
    the warranted tier capped by the ceiling, an unreachable datacenter
    set it. A human reads "budget-limited", tops up the allowance, and
    nothing changes. Now `limited_by: Option<Limit>` distinguishes
    `Budget` from `Reach`.
20. **The Merkle root was rebuilt from every level on each call**, which
    is O(n) and invisible in a demo. It is what every checkpoint needs,
    and for the thing this product is *for* — an entity that accumulates
    for years — it reached 0.7 s at a million decisions, about a day of
    a robot making ten gated decisions a second. An expensive checkpoint
    means taking fewer, and checkpoints are what make an audit log
    evidence. Now a frontier of perfect subtrees: 4 µs, flat.
21. **The board-footprint number counted the wrong thing.** It measured
    the encoded state graph and called that the entity, which left out
    the retrieval index entirely and the audit trail's record window.
    Measured properly, `Bounds::EMBEDDED` alone is *not* enough — 285
    KiB against 192 KiB available — and the audit window has to shrink
    too. A board budget wrong in the optimistic direction is the worst
    direction for a number that decides whether a robot works.
22. **The entity never remembered the things it thought were worth
    interrupting a human about.** Ten simulated days, dozens of rail
    dips and leg stalls surfaced, none written down. And salience was
    set once at 80 for every memory and then only decayed, so the
    pruning order was approximately "oldest first" rather than the
    architecture's "least salient first". Both fixed; the measured
    consequence is in the day section above.
23. In the fleet experiment itself, veterans were selling every tag they
    had ever tracked rather than the ones they could back up, which made
    the matched and blind arms come out identical to the digit. A seller
    sells findings, not hunches. There is now a test that the two arms
    differ at all.

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
