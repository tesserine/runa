# ADR-0001 — The single state-assess-and-route operation (request as entry-state)

- **Status:** Proposed — decision 1 settled; decisions 2–6 under active reckoning ([#210](https://github.com/tesserine/runa/issues/210)).
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
<ID>` selects `Scoped`, its absence `Unscoped`, and `--ticket <REF>`
([#188](https://github.com/tesserine/runa/issues/188), landed) opens a third
promised-scope entry. These are parallel affordances the caller chooses among.
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

## Decision 2 — The entry spectrum and its routing *(open — to be reckoned)*

**Question:** how the operation resolves a `request` across everything it can
reference and dispatches accordingly. The spectrum: unstructured prose →
exploration that may terminate in work-unit-craft (`survey` → `decompose`); a
reference to an existing work-unit/ticket → resolve its maturity and route
(developed → execute; thin → refine); and the recursive case — a work-unit
already in hand → assess readiness → route to its state-based workflow.

**Known constraints:** routing intelligence lives in the operation, not in
`request` fields. The developed-referent route is the
[#188](https://github.com/tesserine/runa/issues/188) cold-start entry
(`entry::resolve_ticket_reference` → acquisition surface → bound scope); the
prose route is the planning cascade
([#174](https://github.com/tesserine/runa/issues/174)). To reckon: where the
referent's maturity is assessed, and how "developed vs thin" is decided without
prescribing implementation.

## Decision 3 — Command structure *(open — to be reckoned)*

**Question:** whether one command (`go`) suffices — the operation derives the
move from state — or a thin layer separates *seeding intent* (placing a
`request`) from *advancing*. And what the terse surface is (a reference is "the
same amount as a pathname").

**Known constraints:** "one command" must preserve the cascade-vs-one-tick
distinction that already exists — `run` loops to quiescence (autonomous),
`step` / `go` advance one bead (interactive). This is
[#167](https://github.com/tesserine/runa/issues/167)'s axis (who runs the loop,
at what granularity), not a second meaning. The shape to reckon: keep
loop-granularity, drop the *scope* flag (`--work-unit` / `--ticket`) in favor of
state-derivation. To be grounded against `docs/session-surface-contract.md`.

## Decision 4 — Mode-identity *(open — to be reckoned)*

**Question:** how the operation carries identical semantics in autonomous and
interactive sessions. [#167](https://github.com/tesserine/runa/issues/167): mode
is a property of the session, not the operation — it reduces to who issues the
verbs and at what checkpoint granularity, never an authorization-shaped second
difference.

**Known constraints:** the decision must be **backed by a concrete check of how
the configured agent runtimes (Codex, Claude Code adapters in `adapters/`)
already behave** — confirming the property holds, not assuming it
([#210](https://github.com/tesserine/runa/issues/210) AC). To be grounded
against `adapters/` and `docs/session-surface-contract.md`.

## Decision 5 — The request artifact shape *(open — to be reckoned)*

**Question:** the shape implied by decisions 2–3 — terse, a reference and/or
short prose, `additionalProperties` disciplined; whether the free-text `context`
catch-all is replaced by a typed `references` list.

**Known constraints:** the `request` is canonical in `tesserine/commons`
(`REQUEST.md` + `schemas/request/v1/`); the groundwork-vendored copy follows. The
discriminator is *the operation resolving the referent's state*, not request
fields — so the request stays thin. A previously-scoped standalone schema fix
(drop the `context` catch-all) folds in here. The commons major-version change
policy (ADR-0005) applies if a field is removed or an optional one made required.

## Decision 6 — Unification map *(open — to be reckoned)*

**Question:** how the model composes the existing pieces, naming what each
contributes and what changes:
[#188](https://github.com/tesserine/runa/issues/188) (landed cold-start ticket
entry) as the developed-referent route;
[#174](https://github.com/tesserine/runa/issues/174) (open `go`-unscoped) — does
it fold into the single operation or remain its unscoped-entry component; and
`survey` / `acquire` / `decompose` / the readiness-based scoped pipeline.

## Consequences

- Once decisions 2–6 settle, the implementation decomposes into coordinated
  work-units across runa (the operation/entry change), groundwork
  (`survey` / `acquire` alignment), and `tesserine/commons` (the request shape).
  The decomposition is follow-on work, not this ADR.
- Coordinates with: [#174](https://github.com/tesserine/runa/issues/174) (folds
  in or remains a component),
  [pentaxis93/commons#6](https://github.com/pentaxis93/commons/issues/6)
  (golden-rule fleshing),
  [pentaxis93/commons#9](https://github.com/pentaxis93/commons/issues/9)
  (inheritance that hardens the lineage citation).
- Builds on: [#188](https://github.com/tesserine/runa/issues/188) (landed).

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
