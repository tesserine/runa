# ADR-0002 — Operator intent seeding: `wish` authors, `go` advances

**Status:** Withdrawn — 2026-06-30. Decision A (`runa wish`) is unsound and never reached Accepted; see the withdrawal note below.
**Supersedes:** nothing. The supersession this ADR claimed over ADR-0001 Decisions 3 and 5 is void — Decision 3 stands in full; Decision 5's shape stands and its rename is carried elsewhere (below).

## Withdrawal note (2026-06-30)

This ADR is withdrawn. Its organizing decision — **Decision A**, that the operator
`wish` gesture is a **`runa` verb** — is unsound against the substrate, and the ADR
has no remaining reason to exist as a runa decision once that falls.

**Why Decision A is void.** `runa`'s entire command surface (`Init`, `List`,
`Doctor`, `Scan`, `State`, `Step`, `Run`, `Go`) operates only on artifacts that
*already exist*: it scans, validates, computes readiness, and advances. It carries
no model and authors no content; even its cold-start path (`go --ticket`) performs
no forge read of its own and hands materialization to the methodology. A verb that
greets an operator, elicits prose intent, and authors a `{description, source}`
seed is the one capability runa's architecture defines it as *not* having. This
ADR's own Context conceded runa "validates artifacts, it doesn't author intent" —
and then had it author intent. That is the contradiction.

**Where the work actually lives.** The operator `wish` gesture belongs to
**`agentd`** — the session launcher, which already lowers operator intent into a
conforming seed (`agentd-runner` `resolve_invocation_input`, arm
`InvocationInput::RequestText`: takes `description`, stamps `source: operator`,
validates against the methodology's request schema, seeds the session). It is
tracked by **`tesserine/agentd#152`** (`agentd wish`).

**The one sound thing this ADR identified — the rename — is carried forward, not
lost.** Decision B (rename the entry artifact `request` → `intent`) is a real and
necessary change, but it is an artifact decision **owned by `tesserine/commons`**
(the artifact's home), not a runa ADR concern. It is re-homed to **`tesserine/commons#94`**
and sequenced *after* the `babbie-ops#67` integration test (it does not gate it).

**Effect on ADR-0001.** Nothing is superseded. Decision 3 (single outer verb
`go`; seeding is data) stands in full — the matured-surface framing that would have
superseded it is void. Decision 5's v2 *shape* stands; only the artifact *name*
changes, under the commons rename above.

The body below is retained as the historical record of the withdrawn proposal. It
does not govern.

---

**Original status (superseded by this withdrawal):** Proposed
**Originally claimed to supersede (in part):** [ADR-0001](0001-single-state-assess-and-route-operation.md) — Decision 3 (command structure) and Decision 5 (request artifact shape). *Both claims void; see withdrawal note above.*

## Lineage

This ADR realizes the operator-surface invariant whose canonical home is
`tesserine/commons` (`session-surface-contract.md`, source invariant commons
ADR-0015): *operator intent enters once at the session seed, and the operator
addresses the lifecycle through a minimal outer surface.* ADR-0001 adopted that
contract's then-current statement — a **single** outer verb, `go`, with seeding
held as data rather than a verb. This ADR records the matured surface that the
commons contract revision establishes; it cites that contract as what it
realizes and does not restate it (Single Home).

## Context

ADR-0001 settled, in Decision 3, that the operator surface is one verb (`go`)
and that *"seeding is data; `go` is the verb. They are not two operator
operations."* Seed-supply affordances were named as "a `request` artifact in the
workspace, or a reference (carried by `--ticket`)."

That position is architecturally clean and operationally incomplete. "A
`request` artifact in the workspace" is not a supply *affordance* — in practice
it is the **absence** of one: the operator must hand-author a schema-valid JSON
file at `.runa/workspace/request/<instance_id>.json`, choosing the instance-id,
matching the vendored schema, and placing it under the correct per-type
subdirectory. There is no command for this. `runa init`, `scan`, `state`,
`step`, `run`, and `go` are the whole CLI surface; none authors a seed.

This gap is not theoretical. The `babbie-ops#67` interactive-acceptance work
reached this exact point and **halted on it three times** across sessions: each
time, driving real work through the deployment stopped at "the operator must now
hand-write the entry artifact," and each time that was judged — correctly — as
an operator surface too rudimentary to accept. The dogfood that validated
ADR-0001's routing decisions is the same dogfood that outgrew its seeding
decision.

`--ticket` already demonstrates the resolution. The contract classifies it as
*seed delivery — "not a new verb and not a third mode"* — a sanctioned affordance
for supplying a **reference** seed. What is missing is the symmetric affordance
for the **prose** seed: a sanctioned way to author the `description`/`source`
seed that the prose entry route consumes. Supplying that affordance does not
reintroduce operator-named scope (ADR-0001 Decision 1 stands — scope is read from
the seed, never named); it makes seed-supply ergonomic where today it is a
filesystem chore.

## Decision A — The operator surface is seed-authoring plus advance

The operator-facing surface is **two verbs with one clean division**:

- **`wish`** — author the seed. `runa wish` materializes the canonical entry
  artifact (correctly shaped, correctly placed, instance-id assigned by the
  command, validated on creation), from operator-supplied intent. It is the
  prose-seed delivery affordance, symmetric to `--ticket`'s reference-seed
  delivery — *seed delivery, not a control verb and not a mode.*
- **`go`** — advance. Unchanged from ADR-0001 Decision 3: `go` reads the seed,
  derives scope and route (Decisions 1, 2), and advances the operation at the
  operator's chosen cadence.

This **supersedes ADR-0001 Decision 3's** clause *"seeding is data; `go` is the
verb. They are not two operator operations."* The matured statement: seeding is
still **data** (an artifact, not a control action over the lifecycle), and `go`
is still the single **control/advance** verb — but **authoring that data is a
first-class command (`wish`)**, not a hand-edit. The operator still addresses
nothing finer than "convey intent" (`wish`) and "advance" (`go`); no
per-operation approval gate and no operator-named scope are introduced. The
distinction ADR-0001 drew — that the operator does not perform a "seed" control
operation separate from advancing — is preserved: `wish` produces the seed
*data*; it is not a second step in the advance loop.

`wish` is observation-free and control-free with respect to the running session:
it creates an entry artifact and exits. It does not advance, evaluate, or report
session state (those remain `go` and the read-only `state` vector,
respectively).

## Decision B — The entry artifact is renamed `request` → `intent`

The entry artifact ADR-0001 Decision 5 reshaped (v2: `description`, `source`,
`references`; `context` removed) is renamed from `request` to **`intent`**. The
artifact *is* the operator's intent crystallized — the contract's own ontology
(*"operator intent enters once at the seed"*) names it. `wish` produces an
`intent`; the prose route reads its `description`; the reference routes read its
`references`. The name `request` — thin and transactional — is replaced by the
word for what the artifact holds.

**This supersedes ADR-0001 Decision 5's** artifact name (not its shape — the v2
field set stands). The rename is a breaking change to the canonical artifact,
which is owned by `tesserine/commons` (`REQUEST.md` → `INTENT.md`;
`schemas/request/v2/` → `schemas/intent/v1/` or the commons-chosen path), under
the commons change policy (ADR-0005), and re-vendored by groundwork (the
`survey` trigger `on_artifact(request)` → `on_artifact(intent)`).

**Sequencing — deferred.** The rename rides the already-staged schema-v2
migration (ADR-0001 Decision 5) and **lands after `babbie-ops#67`'s integration
acceptance**, so the first proving run of the operator surface is not coupled to
a cross-repo rename. Until the rename lands, `wish` authors the artifact under
its current canonical name; the verb is stable across the rename (the operator
types `wish` regardless of the artifact's internal type name).

## Decision C — Authoring affordance, not authoring policy

`wish` decides *that* the seed is authored by a command and *where/how* it is
placed and validated. It does **not** decide the seed's content discipline —
that the seed carries stimulus and provenance only, never downstream thinking —
which is governed where it already lives: the `intent`/request artifact contract
(ADR-0001 Decision 5's `context`-removal closed the leak field) and the
methodology's survey discipline. `wish` materializes whatever intent the
operator conveys; it is an affordance, not a gate on intent quality.

## Consequences

- **Decomposes into coordinated work-units** (filed under the operator-entry
  epic, not here): the `wish` CLI verb and entry-artifact materialization in
  **runa**; the `session-surface-contract.md` revision (single-verb → author +
  advance) in **tesserine/commons**; the `request`→`intent` rename across
  **commons** (canonical schema + prose) and **groundwork** (re-vendor + trigger),
  sequenced post-#67. Coupled with the ADR-0001 entry-routing implementation and
  with **#174** (unscoped `go` entry — the route `wish`'s seed feeds).
- **The commons contract is revised, not merely cited** for the surface change:
  the operator surface statement matures from "a single outer verb" to
  "seed-authoring (`wish`) and advance (`go`)." This ADR realizes that revision in
  runa; the contract is its source.
- **`babbie-ops#67` depends on this** — its integration runbook's entry step
  becomes `runa wish "<intent>"`, replacing the hand-authored workspace JSON. The
  integration acceptance proves the matured surface.
- **ADR-0001 stands except where superseded here** — Decisions 1, 2, 4, 6 are
  untouched; Decision 3's single-verb framing and Decision 5's artifact name are
  superseded by Decisions A and B above.

## References

- [ADR-0001](0001-single-state-assess-and-route-operation.md) — the superseded-in-part predecessor.
- `tesserine/commons` `session-surface-contract.md` (source invariant commons ADR-0015) — the operator-surface contract this realizes.
- `tesserine/commons` ADR-0005 — the change policy under which the artifact rename/schema is breaking.
- runa #167 (dual-mode epic), #174 (unscoped `go` planning entry).
- `babbie-ops#67` — the interactive-acceptance dogfood that surfaced the gap.
