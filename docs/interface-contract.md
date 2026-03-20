# runa Interface Contract

This document defines the boundary between runa (the runtime) and methodology plugins. Everything inside this boundary, runa sees and enforces. Everything outside, methodologies own entirely.

## Three Primitives

The interface consists of three primitive concepts. All runtime behavior derives from these.

### 1. Artifact Types

An artifact type is a named category of work product with a machine-checkable contract. Methodologies define their artifact types. runa validates instances against their contracts.

An artifact type declaration:

- **name** — unique identifier within the methodology (e.g., `constraints`, `behavior-contract`, `test-evidence`)
- **schema** — JSON Schema defining what a valid instance contains. This schema is the artifact's contract. There is no separate contract mechanism.

runa ships no artifact types. Every artifact type is methodology-owned.

### 2. Protocol Declarations

A protocol declares its relationship to artifacts through four kinds of edges:

- **requires** — the named artifact type must exist and validate before the protocol can execute. Hard dependency.
- **accepts** — the named artifact type may be consumed if available. The protocol operates with or without it. Soft dependency.
- **produces** — the named artifact type will exist and validate after the protocol executes. runa fails the protocol if a declared output is missing or invalid.
- **may_produce** — the named artifact type might be produced. runa validates any instance that appears but does not fail the protocol for its absence.

A protocol declaration:

- **name** — unique identifier
- **requires** — zero or more artifact type names
- **accepts** — zero or more artifact type names
- **produces** — zero or more artifact type names
- **may_produce** — zero or more artifact type names. Absent optional outputs do not fail postconditions, but they also do not create completion evidence. If output should always be produced, the artifact type belongs in `produces`.
- Completion is derived from output artifact timestamps. Protocols with no `produces` types are never suppressed by freshness — runa cannot derive completion from artifacts that don't exist. If a protocol needs completion tracking, it must declare at least one `produces` artifact type.
- **trigger** — one trigger condition (see below)

Topology is not declared. It emerges from the graph of requires/produces/may_produce relationships across protocols. A pipeline emerges when protocols chain linearly. A graph emerges when protocols fan in or fan out. A cycle emerges when a protocol produces an artifact type that another protocol's trigger monitors for change. The methodology does not tell runa what shape it is. runa computes the shape from declarations.

### 3. Trigger Conditions

A trigger condition defines when runa should activate a protocol. Triggers are composable from four primitive types:

- **on_artifact(name)** — the named artifact exists and satisfies its schema
- **on_change(name)** — the named artifact is newer than this protocol's current output artifacts for the same work unit. runa derives freshness from artifact timestamps in the store rather than persisting separate completion records.
- **on_invalid(name)** — an instance of the named artifact type exists but fails validation against its declared schema
- **on_signal(name)** — an external event (operator action, webhook, scheduler)

These compose through two operators:

- **all_of(conditions...)** — all conditions must be satisfied
- **any_of(conditions...)** — at least one condition must be satisfied

Nesting is permitted. `all_of(on_artifact("constraints"), any_of(on_signal("approved"), on_artifact("auto-approve")))` means: constraints must exist, and either operator approval or an auto-approve artifact must be present.

## What runa Does

runa is an event-driven runtime. The CLI commands (init, scan, list, status, step, doctor, signal) are windows into its state. The runtime itself is the monitoring loop.

Given the declarations above, runa provides five runtime capabilities:

**Monitoring.** runa watches artifact state and evaluates trigger conditions on relevant state changes. When a protocol's trigger condition becomes satisfied, runa activates the protocol.

**Validation.** When an artifact is produced, runa validates it against its declared schema. A protocol's execution is not complete until its `produces` artifacts exist and validate. `may_produce` artifacts are validated if present but not required.

**Graph computation.** runa computes the dependency graph from protocol declarations. This enables: freshness analysis (which artifacts are stale), execution ordering (what can run now), cycle detection (where the methodology creates loops), and blocked-protocol identification (what's waiting on what).

**Enforcement.** A protocol cannot execute if its `requires` artifacts are missing or invalid. A protocol's execution is incomplete if its `produces` artifacts are missing or invalid. These are hard constraints the runtime enforces regardless of what the methodology intends.

**Context injection.** When a protocol is ready to execute, runa resolves which artifact instances the protocol needs — all valid `requires` instances and all available valid `accepts` instances — and delivers them as the protocol's input context alongside the expected output artifact types. The protocol receives its inputs without querying the store directly.

## What runa Does Not Do

runa does not define artifact types. Methodologies do.

runa does not define protocol content. Methodologies do.

runa does not prescribe topology. Topologies emerge from declarations.

runa does not interpret methodology semantics. If a methodology calls a stage "grounding" or "verification," runa does not know or care what those words mean. It sees declarations and artifacts.

## Methodology Registration

A methodology registers with runa through a manifest file declaring:

- The methodology's artifact types and their schemas
- The methodology's protocols and their declarations
- No other configuration

The manifest format is the methodology's only interface with the runtime. runa reads it, builds the graph, begins monitoring.
