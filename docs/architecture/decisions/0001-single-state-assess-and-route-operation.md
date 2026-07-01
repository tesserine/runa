# ADR-0001 — The single state-assess-and-route operation (request as entry-state)

- **Status:** Accepted — 2026-06-30; all six decisions settled. Implementation decomposes from this ADR. (ADR-0002 was withdrawn 2026-06-30; it claimed to supersede Decisions 3 and 5, but that supersession is void. **Decision 3 stands in full.** Decision 5's v2 *shape* stands; only the artifact *name* changes, under the `request` → `intent` rename re-homed to `tesserine/commons#94` and sequenced after `babbie-ops#67`.)
- **Date:** 2026-06-28
- **Deciders:** Robbie and the governance station.
- **Spike:** [#210](https://github.com/tesserine/runa/issues/210) · **Epic:** [#167](https://github.com/tesserine/runa/issues/167) (dual-mode).

## Lineage

This ADR realizes, at runa's implementation level, the `pentaxis93/commons`
golden rules **"One idempotent operation that infers its work from state"** and
**"State is the interface"** (both seeds, homing under
[pentaxis93/commons#6](https://github.com/pentaxis93/commons/issues/6)), and
operationalizes the `tesserine/commons` concept **The Cognitive State Machine** —
artifacts as cognitive state, protocols as morphisms — from which it derives as
that document anticipates ("specific commitments derive from it as ADRs"). The
universal principles beneath the rules — Traceability, Verifiable Completion,
Single Home, Source Repair, Sovereignty — are reached *through* the rules, not
restated here.

The formal inheritance path runa → universal runs through `tesserine/commons`
inheriting `pentaxis93/commons`, which is in-flight
([pentaxis93/commons#9](https://github.com/pentaxis93/commons/issues/9)). This
ADR cites the golden rules directly; #9 hardens that citation when it lands.

*"Realizes," not "projects":* the corpus reserves **projection** for a
universal's own domain face authored inside `pentaxis93/principles`. This ADR is
neither a projection nor a new golden rule — it is runa's implementation-level
realization of an existing rule, citing it as what runa implements.

## Context

runa already performs one operation at every bead: scan the artifact workspace
into current state, then evaluate protocol readiness in dependency-topological
order and select the next ready step. This is
`selection::discover_ready_candidates`, shared across `runa state`, `step`,
`run`, `go`, and the `runa-mcp` session surface (`readiness` /
`next-protocol-context` / `advance`) through one readiness path
(`status::evaluate_protocols`). The operation is parameterized by
`EvaluationScope`: `Unscoped` evaluates planning protocols (survey, decompose)
with no work-unit; `Scoped(id)` evaluates execution protocols (take…land) for
one work-unit.

The operation is therefore already state-assessed, idempotent (execution-record
suppression makes re-runs converge rather than accumulate), and recursive within
a scope (each bead re-scans and re-selects). What is not yet intrinsic is the
**selection of the scope itself**: today the caller supplies it — `--work-unit
<ID>` selects `Scoped`, its absence `Unscoped`, and at the time of this ADR
the former ticket delivery flag
([#188](https://github.com/tesserine/runa/issues/188), landed) opened a third
promised-scope entry. These were parallel affordances the caller chose among.

**2026-07-01 amendment:** the ticket delivery flag is retired. Reference-seed
delivery now comes from assessed seed state (`intent.target`), so promised scope
entry is derived at the entry boundary instead of being selected by an operator
flag.
At the entry boundary specifically, a `request` artifact routes only to `survey`,
and only because `survey`'s trigger is `on_artifact(request)`; nothing resolves
what the request *refers to* and routes on it.

This ADR reckons the model in which the route is derived from assessed state at
the entry boundary too — one operation, applied to the one boundary where it is
not yet applied.

## Decision 1 — The single operation *(settled)*

The operation runa performs at every bead is: scan the workspace into current
artifact state, then evaluate protocol readiness in dependency-topological order
and select the next ready step. This is already the one shared path — `runa
state`, `step`, `run`, `go`, and the `runa-mcp` session surface all route through
the same readiness evaluation (`status::evaluate_protocols`). It is already
recursive in the sense that matters: each bead re-scans and re-selects, so a
work-unit in hand advances to its next step by the same evaluation that selected
the first.

What is not yet intrinsic is the selection of the scope itself.
`discover_ready_candidates` is invoked under a scope the caller has already
chosen. At the entry boundary, a `request` routes only to `survey` by trigger,
with nothing resolving its referent.

**Decision:** complete the single operation by making the route a **derivation of
assessed state at the entry boundary**, identical to how it is already derived
mid-pipeline. On a tick with a `request` present and no scoped work in motion,
the operation resolves the request's referent and selects the route —
acquire-and-execute a developed referent, refine a thin one, survey prose — by
the same assess-then-select discipline that already advances an in-flight
work-unit. Scope ceases to be a caller-supplied flag and becomes what the
operation reads from state. The recursion the model names is already true
mid-pipeline; this extends it to the one boundary where it is not.

This is the load-bearing decision: the work is not "add a router," it is "scope
becomes a derivation, not an input." `discover_ready_candidates` already proves
the engine can derive the next move from state; decisions 2–6 work out applying
that one step earlier, at entry.

## Decision 2 — The entry spectrum and its routing *(settled)*

**Question:** how the operation resolves a `request` across everything it can
reference and dispatches accordingly — prose, a reference to a developed
work-unit/ticket, a reference to a thin one, or a work-unit already in hand.

**Decision.** At the entry boundary the operation derives the route by
**resolving any reference the request carries and attempting to bring it to a
valid `work-unit`**, then dispatching on the outcome. The maturity of the
referent is not classified separately; it is read from whether the referent
*materializes into a schema-valid `work-unit`*. Three routes result, each an
existing mechanism:

1. **Reference → already a valid work-unit, or materializes cleanly → execute.**
   `entry::resolve_ticket_reference` resolves the reference to a tracker identity
   (identity only, no forge read); a reference that already resolves to a
   recorded valid `work-unit` binds scope directly, and one that does not is
   brought through the methodology's acquisition surface, which materializes the
   `work-unit` from the ticket. Once bound, execution is the ordinary scoped
   operation. *(This is the [#188](https://github.com/tesserine/runa/issues/188)
   cold-start entry.)*

2. **Reference → fails materialization (thin) → refine, then re-resolve.** When
   the referent does not map onto the `work-unit` schema — no extractable
   acceptance criteria, an empty body, a non-open ticket — materialization fails
   with a named work-unit-quality defect. That defect routes to `decompose`'s
   `refine-work-unit` discipline, which improves the ticket *at its planning
   home*; the operation then re-resolves. Materialization **never fabricates the
   missing content** — a thin referent is improved at its source, not hand-filled
   into an execution snapshot the planning home never authorized. *(This is
   `acquire`'s existing gap-routing.)*

3. **No resolvable reference (prose) → survey.** A request carrying prose and no
   resolvable referent enters `survey`, which assesses the exigence and may
   terminate in `decompose` (work-units), or in "no work needed." *(This is the
   existing `request` → `survey` path, drivable unscoped per
   [#174](https://github.com/tesserine/runa/issues/174).)*

The **maturity criterion is therefore the `work-unit` schema itself**: a referent
is "developed" exactly when it materializes into a schema-valid `work-unit`, and
"thin" exactly when it does not. No separate maturity classifier is introduced,
and the discriminator lives in the operation (resolve-and-materialize), not in
`request` fields — which is decision 1's "scope becomes a derivation" applied to
the entry spectrum. The recursive case the model names — a work-unit already in
hand → assess readiness → next step — is precisely the scoped
`discover_ready_candidates` the operation already runs; entry resolution simply
produces the bound work-unit that the recursion then carries.

**Boundaries (routed to their own decisions, not resolved here):**

- *How autonomously the operation traverses these routes* — proceed straight
  through (e.g. thin → refine → re-resolve → execute) versus halt at a fork and
  report for operator judgment — is a property of who runs the loop and at what
  granularity, and is decided in **Decision 4 (mode-identity)**.
- *A request carrying both a reference and prose* — whether the reference sets
  the route with prose as accompanying intent, or prose can redirect a developed
  referent into survey/reckon — is a property of the request's shape and is
  decided in **Decision 5**.
- *A `survey` terminal that files an unplanned, undecomposed issue for later*
  (the "explore, then park" outcome) is a `survey`/`decompose` behavior addressed
  in the **Decision 6** unification, not a new entry route.

## Decision 3 — Command structure *(settled — by deference to the session-surface contract)*

The command structure is already settled by the committed
[`session-surface-contract.md`](../../session-surface-contract.md) (source
invariant: commons ADR-0015): *"The operator-facing surface is therefore a
single outer verb — `go` — and the operator addresses nothing finer... Mode is
the cadence of `go`, not a different or larger vocabulary."* This ADR adopts that
surface; it does not re-decide it. The "one command" the model reaches for is the
landed contract, not a new proposal.

What this ADR records is the **engine delta** that brings the running CLI to the
contract under decision 1:

- `step`, `run`, and `go` are the single verb `go` at different **cadences** —
  one tick versus issued to quiescence — not separate verbs. `state` is the
  separate **observability vector** (read-only over durable state), which the
  contract holds distinct from the control surface ("Observation does not enter
  through the control surface").
- The direct scope flag `--work-unit <ID>`, which *names* a scope and bypasses
  seed-derivation, is retired from the operator surface in favor of decision 1:
  scope is **read from the seed**, not named by the operator. Seed-supply
  affordances remain — a `request` artifact in the workspace, or a reference
  carried by seed data, which the contract classifies as seed delivery, "not a
  new verb and not a third mode".
- There is no thin seed-vs-advance layer at the operator surface. Intent enters
  once at the seed (contract: "operator intent enters once at the session seed
  through the canonical commons request artifact"), and `go` advances. Seeding is
  *data*; `go` is the *verb*. They are not two operator operations.

The cascade's internal decomposition (how reconcile/select, context, recording,
and commit are factored) remains, per the contract, an engineering concern
carrying no interface commitment.

## Decision 4 — Mode-identity *(settled — by deference to ADR-0015, confirmed against the adapters)*

Mode-identity is fixed by commons
[ADR-0015](https://github.com/tesserine/commons/blob/main/adr/0015-mode-is-a-property-of-the-session.md)
and stated operationally by the session-surface contract: the stage semantics are
identical in both modes, and *"caller identity, shell shape, launch path, or UI
affordance must not create a second meaning"*; there is no per-operation human
approval gate in either mode; authority over a transition is **conformance** (a
typed disposition artifact runa validates), not approval. This ADR adopts the
invariant; it does not re-decide it.

The spike requires this be **confirmed against the configured runtimes, not
assumed** ([#210](https://github.com/tesserine/runa/issues/210) AC). Confirmed:
`adapters/agent-codex.sh` and `adapters/agent-claude-code.sh` are pure
MCP-registration shims. Each requires `RUNA_MCP_CONFIG`, translates it to its
runtime's MCP-config shape (Codex: TOML `mcp_servers` overrides on `codex exec`;
Claude Code: a temp `--mcp-config` JSON on `claude`), and `exec`s the runtime
with passthrough arguments. **Neither carries any notion of mode, cadence, scope,
work-unit, ticket, or operator.** They are mode-agnostic by construction: runa
builds the same `RUNA_MCP_CONFIG` whether an autonomous `run` loop or an
interactive `go` tick launched the agent, and the adapter has no input that
distinguishes the caller. Mode lives entirely in runa's loop cadence — who issues
`go`, and how many times — never in the runtime invocation. The property holds.

The entry-routing operation therefore inherits mode-identity at no cost: deriving
the route from state at the entry boundary is the same assess-then-select stage
in both modes. This **resolves decision 2's deferred autonomy boundary**: how
autonomously the operation traverses the entry routes (e.g. thin → refine →
re-resolve → execute) is the *cadence of `go`*, not a routing variant —
autonomous traverses to quiescence, interactive one tick at a time, with full
automation the default and observation a sideways vector over durable state,
never a mid-cascade pause (session-surface contract).

## Decision 5 — The request artifact shape *(settled)*

**Decision.** The `request` is

`{ description (required), source (required), references (optional) }`,
`additionalProperties: false`,

and the free-text `context` catch-all is **removed**. `description` carries the
ask and the direction; `source` carries provenance; `references` is a typed list
of pointers the operation resolves.

The shape follows from decision 2: `references` is the structural input the
operation reads to derive the entry route — typed references present and
resolvable → the developed/thin routes (execute / refine); none → the prose route
(`description` → survey). This also fixes the *reference + prose together*
question deferred from decision 2: when both are present, the references set the
route and `description` is the intent/direction carried into it — matching the
session-surface contract's "operator intent enters once at the session seed... and
the direction declared from it." The request never declares its own route through
a `kind` field; the operation derives the route from whether the references
resolve. Removing `context` is what closes the leak it invited: an open "anything
else" field admits the downstream thinking (`survey`'s exigence, `decompose`'s
plan) that the request must not carry.

**Consequence — schema major version.** The `request` is canonical in
`tesserine/commons` (`REQUEST.md` + `schemas/request/v1/`). Removing a field is a
breaking change under the commons change policy (ADR-0005), so this lands as
**request schema v2** (`schemas/request/v2/`), with v1 retained for existing
consumers during migration and the groundwork-vendored copy moving to v2. The
exact `references` entry schema (pointer kinds, format) is implementation detail
for the commons schema work, not fixed here.

## Decision 6 — Unification map *(settled)*

The model composes existing pieces; little is built new, and what changes is
named here.

| Piece | Role in the single operation | Change |
| --- | --- | --- |
| [#188](https://github.com/tesserine/runa/issues/188) (landed) | The developed-referent route (decision 2 routes 1–2): reference → resolve → bind, or acquire-and-bind. The promised-scope entry. | None — used as-is. |
| [#174](https://github.com/tesserine/runa/issues/174) (open) | The unscoped-planning **component** the operation requires for the prose route (decision 2 route 3): `go` driving survey/decompose with no work-unit. | Prerequisite, not absorbed — lands as its own work; the operation depends on it. |
| `acquire` | The maturity criterion (decision 2): materialize, or route to refine. | None — its existing two-outcome behavior *is* the discriminator. |
| `survey` | The prose route's protocol: exigence assessment. | None for the core model. A future "explore, then park as an unplanned issue" terminal (the collaborative-reckon outcome) is a separate `survey`/`decompose` behavior, not required here. |
| `decompose` (`refine-work-unit`) | The thin-referent route: improve the ticket at its planning home, then re-resolve. | None — used as-is. |
| Scoped pipeline (`take`…`land`) | What routes 1–2 execute, via the recursive scoped `discover_ready_candidates`. | None. |

**What changes in runa:** the operator surface collapses to the single verb `go`
(decision 3, per the session-surface contract); the engine derives scope/route
from the seed at the entry boundary rather than from `--work-unit` (decision 1).

**What this implies for a committed contract.** The session-surface contract's
scope model is *bound | promised* — both leading to a work-unit (routes 1–2). The
single operation adds the **unscoped/prose entry** (route 3), which that model
does not yet include. Completing this therefore implies a **coordinated update to
[`session-surface-contract.md`](../../session-surface-contract.md)** extending its
scope model to the unscoped entry. That contract change is follow-on work, named
here, not performed in this ADR.

## Consequences

- **Implementation decomposes from this ADR** into coordinated work-units across
  runa (the entry-derivation engine change plus the `go` / scope-flag CLI move),
  `tesserine/commons` (request schema v2), and the groundwork methodology (the
  re-vendored request schema; `survey` / `acquire` / `decompose` used as-is). The
  decomposition is follow-on work, not this ADR.
- **Request schema goes to v2** (decision 5): removing `context` is breaking under
  the commons change policy (ADR-0005); v1 is retained during migration.
- **A coordinated `session-surface-contract.md` update** (decision 6) extends its
  *bound | promised* scope model to the unscoped/prose entry route.
- **Builds on** [#188](https://github.com/tesserine/runa/issues/188) (landed);
  **requires** [#174](https://github.com/tesserine/runa/issues/174) as the
  unscoped-planning component.
- **Coordinates with**
  [pentaxis93/commons#6](https://github.com/pentaxis93/commons/issues/6)
  (golden-rule fleshing) and
  [pentaxis93/commons#9](https://github.com/pentaxis93/commons/issues/9)
  (inheritance that hardens the lineage citation).

## References

- Golden rules: `pentaxis93/commons/golden-rules/README.md` — "One idempotent
  operation that infers its work from state", "State is the interface".
- Concept: `tesserine/commons/concepts/_drafts/cognitive-state-machine.md` — The
  Cognitive State Machine.
- Principles: `pentaxis93/principles` — Traceability, Verifiable Completion,
  Single Home, Source Repair, Sovereignty.
- Spike: [#210](https://github.com/tesserine/runa/issues/210) · Epic:
  [#167](https://github.com/tesserine/runa/issues/167) · Entry:
  [#188](https://github.com/tesserine/runa/issues/188) · Unscoped `go`:
  [#174](https://github.com/tesserine/runa/issues/174).
