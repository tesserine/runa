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

Mode changes who issues session verbs and at what checkpoint granularity. Mode
does not change what a verb means, what validates it, or who holds transition
authority.

## Invocation Contract

All session clients invoke the same runa surface. An interactive driver is a
client of that surface, not a reimplementation of readiness, context delivery,
lifecycle movement, or artifact recording.

The required session verbs are:

| Verb | Semantics |
| --- | --- |
| `readiness` | Reconcile current artifact state and evaluate the methodology graph for the session scope. The result classifies protocols with the same ready, blocked, waiting, currentness, scan-gap, and scope rules runa applies everywhere else. |
| `next context` | Deliver the execution context for a ready protocol from runa's validated input set: protocol instructions, scoped work unit when present, valid required inputs, available accepted inputs, and expected outputs. The client does not query the store directly or synthesize context. |
| `record output` | Accept an output artifact only through the protocol's declared output contract. Runa validates the artifact against the methodology schema, applies scoped session metadata where the runtime owns it, rejects invalid output, and records only valid output as runtime state. |
| `advance` | Re-evaluate lifecycle progress from the validated artifact state. Advancement follows the methodology dependency graph, trigger rules, preconditions, postconditions, and required disposition artifacts. It is not a separate approval operation. |

In MCP session mode, the concrete driver tool names `readiness`,
`next-protocol-context`, and `advance` are reserved. A current step must not
declare an output artifact type with one of those names.

The semantics above are identical in autonomous and interactive modes. Caller
identity, shell shape, launch path, or UI affordance must not create a second
meaning for the same verb.

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

An interactive client may choose to stop after one checkpoint for observation,
while an autonomous orchestrator may continue issuing verbs until quiescence.
That loop ownership difference does not change the lifecycle. Both clients
observe the same readiness, receive the same context for the same ready
protocol, record outputs through the same validation gate, and advance only
through the same graph and typed dispositions.

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
