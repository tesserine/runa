# runa Session Surface Contract

This document defines the boundary between runa and the drivers or agents that
operate a session. It is the companion to
[`docs/interface-contract.md`](interface-contract.md): the interface contract
governs the runa-to-methodology boundary, while this contract governs the
runa-to-driver boundary.

The source invariant is commons
[ADR-0015: Mode Is a Property of the Session](https://github.com/tesserine/commons/blob/main/adr/0015-mode-is-a-property-of-the-session.md)
and
[Design Principle 21](https://github.com/tesserine/commons/blob/main/DESIGN-PRINCIPLES.md#21-mode-is-a-property-of-the-session-not-the-operation).
The operation and contract layer is mode-agnostic by construction. Autonomous
orchestrators and interactive drivers are clients of the same validated runa
surface.

## Scope

This contract defines session operation semantics, not their wire encoding.
An implementation may expose the surface through MCP, CLI adapters, or another
transport, but the transport must not change the meaning, validation, authority,
or artifact contract of any operation.

Mode changes who issues the outer session verb and at what checkpoint
granularity. Mode does not change what an operation means, what validates it, or
who holds transition authority.

## Invocation Contract

All session clients invoke the same runa surface. An interactive driver is a
client of that surface, not a reimplementation of readiness, context delivery,
lifecycle movement, or artifact recording.

### Two layers: the operator verb and the cascade it runs

runa derives the next action from artifact state. A session therefore advances
by one outer operation — *take the next step* — and that operation **cascades**
through a sequence of internal stages: reconcile artifact state, select the
ready protocol for the scope, build and inject that protocol's validated
context, let the agent perform the protocol and record its output through the
methodology's declared output contract, enforce postconditions, and commit the
transition. The autonomous orchestrator demonstrates the cascade running whole:
its run loop issues no separate readiness, context, or advance operation — one
loop tick runs the entire sequence internally, including context construction,
which is built as part of the agent invocation and is not a separately issued
step.

The surface accordingly has two layers, and keeping them distinct is what keeps
the vocabulary honest:

- **Outer layer — the operator verb.** The single operation a session driver
  issues to move the work: *advance the session by one step*. Autonomous mode
  issues it repeatedly to quiescence; interactive mode issues it one step at a
  time so a human can observe between ticks. This is the only layer an operator
  must address. Mode is which cadence issues this verb, not a different verb
  (ADR-0015).

- **Inner layer — the cascade stages.** Reconcile/select (readiness), context
  construction, output recording, and postcondition-gated commit are *stages of
  the outer operation*, not peer operations an operator wields. Output recording
  in particular crosses the runa↔methodology boundary: the artifact is recorded
  through the **methodology's own declared output tool**, validated by runa
  against the methodology schema (runa is blind to the artifact type by
  contract). It is the agent's interior write-back during the cascade, not an
  operator verb.

### Stage semantics

The inner stages, each performed during the outer operation:

| Stage | Semantics |
| --- | --- |
| reconcile / select (readiness) | Reconcile current artifact state and evaluate the methodology graph for the session scope, classifying protocols by the same ready, blocked, waiting, currentness, scan-gap, and scope rules runa applies everywhere else, and selecting the ready protocol to run. |
| context construction | Build the execution context for the selected protocol from runa's validated input set — protocol instructions, scoped work unit when present, valid required inputs, available accepted inputs, expected outputs — and inject it. The client never queries the store directly or synthesizes context. In the autonomous path this is built as part of the agent invocation. |
| output recording | Accept an output artifact only through the protocol's declared output contract. Runa validates the artifact against the methodology schema, applies scoped session metadata where the runtime owns it, rejects invalid output, and records only valid output as runtime state. Reached through the methodology's own output tool, not a runa verb. |
| postcondition-gated commit (advance) | Enforce the completed protocol's postconditions, then re-evaluate lifecycle progress from validated artifact state and commit the transition, following the methodology dependency graph, trigger rules, preconditions, postconditions, and required disposition artifacts. It is not a separate approval operation. |

Every stage that reconciles the workspace re-establishes the session's scoped
work-unit identity before evaluating readiness, serving context, or advancing
lifecycle state. Current-step readiness reconfirmation applies when the cascade
serves or completes that step; scoped identity revalidation is unconditional
after a rescan.

The stage semantics are identical in autonomous and interactive modes. Caller
identity, shell shape, launch path, or UI affordance must not create a second
meaning for any stage or for the outer verb.

### MCP exposure and the operator verb

runa is a state machine the operator advances one tick at a time, where each
tick performs the same operation: take the next step. The operator-facing
surface is therefore a **single outer verb** — `go` — and the operator addresses
nothing finer. Autonomous mode issues `go` repeatedly to quiescence; interactive
mode issues it one tick at a time. Mode is the cadence of `go`, not a different
or larger vocabulary.

The cascade's internal decomposition — how many functions implement reconcile,
select, context construction, output recording, and postcondition-gated commit,
and where their boundaries fall — is an **engineering concern, not an interface
concern**. Those boundaries are chosen for sound internal engineering:
long-term maintainability, and a substrate on which the recursive spiral can
operate effectively across radical scale. They are invisible to the operator and
carry no interface commitment; they may be refactored freely without touching
this contract.

The landed MCP session surface exposes three reserved tool names — `readiness`,
`next-protocol-context`, and `advance`. These are the mechanics of the cascade,
not operator verbs: a single `advance` already returns the full post-step
state (the completed step, the next step, and the complete readiness
classification), so no separate operator-issued `readiness` call carries
information `advance` has not already returned at the step boundary. A current
step must not declare an output artifact type named `readiness`,
`next-protocol-context`, or `advance`.

Observation does not enter through the control surface. A human observing a
single step closely — interactive mode's purpose — does so by looking *into* the
system through its observability vector (the durable artifact store and the
output reports built on it), not by inserting pauses or extra invocations into
the control flow. Control flow stays the uniform tick; observability is a
separate, sideways vector over the same durable state.

There are no contractual mid-step stopping points where the cascade pauses and
returns control to the human. Were such a pause ever introduced, the default
must remain full automation — the cascade runs to its step boundary unless a
pause is explicitly requested — so that automation is the path and any
human-in-the-cascade interruption is the deliberate exception. No such pause is
specified now, and none is required.

## Disposition-Authority Contract

Authority over a lifecycle transition is conformance, not per-operation human
approval. A transition is gated by typed disposition artifacts produced by
methodology protocols and validated by runa. For Groundwork review, the relevant
gate is the methodology's typed disposition, such as `change-approved`, emitted
by the `review` protocol and validated through the same artifact contract as any
other output.

No per-operation human approval gate exists in either mode. Interactive human
presence does not add a second approval path, and autonomous execution does not
remove a gate.

The operator holds intent authority. Operator intent enters once at the session
seed through the canonical commons
[request artifact](https://github.com/tesserine/commons/blob/main/REQUEST.md)
and the direction declared from it, such as Groundwork `take` session direction.
The operator may alter or withdraw that intent, and the session must regenerate
from the changed seed. The operator is not inserted as an approver of each
conforming transition.

If a conforming output is judged bad, the defect is in the substrate: the
protocol, schema, validator, or contract failed to encode what good required.
The repair belongs at that source, not in an ad hoc override of a conforming
transition.

## Lifecycle-Reachability Contract

The session lifecycle is expressed as methodology protocols and driven by
runa's dependency graph. The same graph-driven lifecycle must be reachable
through the same session surface in both autonomous and interactive modes.

The difference between modes is the cadence of the single outer verb: an
interactive client issues one tick at a time, while an autonomous orchestrator
continues issuing ticks until quiescence. That cadence difference does not
change the lifecycle. Both clients drive the same cascade — the same readiness
evaluation, the same context for the same ready protocol, the same
validation-gated output recording, and advancement only through the same graph
and typed dispositions.

Any driver path that hand-rolls readiness, context construction, artifact
recording, or lifecycle transition logic is outside this contract. The valid
interactive path is through runa's surface, not around it.

## Relationship to Methodology Contracts

Methodologies own artifact types, schemas, protocol declarations, trigger
conditions, protocol instructions, and disposition artifact meanings. Runa owns
validation, graph evaluation, scoped session operation, context injection, and
pre/postcondition enforcement.

This contract does not give runa permission to interpret methodology semantics.
It only requires every session client to reach those methodology-defined
semantics through the same validated runtime surface.
