# runa Interface Contract

This document defines the boundary between runa (the runtime) and methodology plugins. Everything inside this boundary, runa sees and enforces. Everything outside, methodologies own entirely.

## Three Primitives

The interface consists of three primitive concepts. All runtime behavior derives from these.

### 1. Artifact Types

An artifact type is a named category of work product with a machine-checkable contract. Methodologies define their artifact types. runa validates instances against their contracts.

An artifact type declaration:

- **name** — unique identifier within the methodology (e.g., `constraints`, `behavior-contract`, `test-evidence`)
- Artifact type names must be safe single path components because runa derives schema paths from them. Names must not contain `/`, `\`, or `..`.
- **schema** — JSON Schema defining what a valid instance contains. This schema is the artifact's contract. There is no separate contract mechanism. The schema is not declared in the manifest — runa derives its location from the methodology layout convention (see below).

runa ships no artifact types. Every artifact type is methodology-owned.

### 2. Protocol Declarations

A protocol declares its relationship to artifacts through four kinds of edges:

- **requires** — the named artifact type must have at least one valid instance before the protocol can execute. Hard dependency. Invalid, malformed, or stale siblings remain health findings but do not block execution when a valid instance exists.
- **accepts** — the named artifact type may be consumed if available. The protocol operates with or without it. Soft dependency.
- **produces** — the named artifact type will exist and validate after the protocol executes. runa fails the protocol if a declared output is missing or invalid.
- **may_produce** — the named artifact type might be produced. runa validates any instance that appears but does not fail the protocol for its absence.

A protocol declaration:

- **name** — unique identifier
- Protocol names must be safe single path components because runa derives instruction paths from them. Names must not contain `/`, `\`, or `..`.
- **requires** — zero or more artifact type names
- **accepts** — zero or more artifact type names
- **produces** — zero or more artifact type names
- **may_produce** — zero or more artifact type names. Absent optional outputs do not fail postconditions, but they also do not create completion evidence. If output should always be produced, the artifact type belongs in `produces`.
- **scoped** — optional boolean, default `false`. When `false`, the protocol participates only in unscoped evaluation. When `true`, the protocol participates only in caller-scoped evaluation for an explicit work unit supplied by the orchestrator.
- Output schema consistency is part of manifest validity. When a protocol is unscoped (`scoped = false` or omitted), any artifact type named in `produces` or `may_produce` must be servable without a delegated work unit. In practice, an unscoped protocol must not declare output schemas whose top-level `required` array includes `work_unit`.
- Completion is derived from output artifact timestamps. Protocols with no `produces` types are never suppressed by freshness — runa cannot derive completion from artifacts that don't exist. If a protocol needs completion tracking, it must declare at least one `produces` artifact type.
- **trigger** — one trigger condition (see below)

Topology is not declared. It emerges from the graph of requires/produces/may_produce relationships across protocols. A pipeline emerges when protocols chain linearly. A graph emerges when protocols fan in or fan out. A cycle emerges when a protocol produces an artifact type that another protocol's trigger monitors for change. The methodology does not tell runa what shape it is. runa computes the shape from declarations.

Scope is not topology. Dependency edges remain type-level. `scoped = true` does not change graph structure, and runa does not infer scope from artifact schemas, `work_unit` fields, or artifact filenames. But scope and output schemas still have to agree: if a protocol's outputs require `work_unit`, the protocol itself must be declared `scoped = true`.

### 3. Trigger Conditions

A trigger condition defines when runa should activate a protocol. Triggers are composable from three primitive types:

- **on_artifact(name)** — at least one valid instance of the named artifact exists
- **on_change(name)** — the named artifact is newer than this protocol's current output artifacts for the same work unit. runa derives freshness from artifact timestamps in the store rather than persisting separate completion records.
- **on_invalid(name)** — an instance of the named artifact type exists but fails validation against its declared schema

Completion is derived from output artifact timestamps. For `on_change` protocols, the output must change content to evidence that the changed input was processed. If the correct response to changed input is "the existing output is still valid," the protocol's capstone should still reflect the verification — for example, by including a review timestamp or updated rationale that changes the content hash. Identical rewrites are invisible to the artifact store.

These compose through two operators:

- **all_of(conditions...)** — all conditions must be satisfied
- **any_of(conditions...)** — at least one condition must be satisfied

Nesting is permitted. `all_of(on_artifact("constraints"), any_of(on_change("review"), on_artifact("auto-approve")))` means: constraints must exist, and either a review change or an auto-approve artifact must be present.

## What runa Does

runa is an event-driven runtime. The CLI commands (init, scan, list, state, step, doctor) are windows into its state. The runtime itself is the monitoring loop.

Given the declarations above, runa provides five runtime capabilities:

**Monitoring.** runa watches artifact state and evaluates trigger conditions on relevant state changes within the caller's evaluation scope. When a protocol's trigger condition becomes satisfied, runa activates the protocol.

**Validation.** When an artifact is produced, runa validates it against its declared schema. A protocol's execution is not complete until its `produces` artifacts exist and validate. `may_produce` artifacts are validated if present but not required.

**Graph computation.** runa computes the dependency graph from protocol declarations. This enables: freshness analysis (which artifacts are stale), execution ordering (what can run now), cycle detection (where the methodology creates loops), and blocked-protocol identification (what's waiting on what).

**Enforcement.** A protocol cannot execute if any `requires` artifact type lacks a valid instance. A protocol's execution is incomplete if its `produces` artifacts are missing or invalid. These are hard constraints the runtime enforces regardless of what the methodology intends.

**Context injection.** When a protocol is ready to execute, runa resolves which artifact instances the protocol needs — all valid `requires` instances and all available valid `accepts` instances within the active scope — and delivers them as the protocol's input context alongside the protocol's instruction content and expected output artifact types. The protocol receives its inputs without querying the store directly.

## What runa Does Not Do

runa does not define artifact types. Methodologies do.

runa does not define protocol content. Methodologies do.

runa does not prescribe topology. Topologies emerge from declarations.

runa does not interpret methodology semantics. If a methodology calls a stage "grounding" or "verification," runa does not know or care what those words mean. It sees declarations and artifacts.

## Methodology Layout Standard

The interface contract defines conventional locations for methodology content relative to the manifest file:

- **Schemas:** `schemas/{artifact_type_name}.schema.json`
- **Protocol instructions:** `protocols/{protocol_name}/PROTOCOL.md`

These conventions are part of the interface contract — the same layer that defines manifest format, field names, and trigger condition types. A valid methodology conforms to this layout. runa derives paths from names it already has; the manifest does not include explicit path fields.

Both schema files and instruction files must exist at their conventional locations when the manifest is parsed. Missing files are parse errors, caught before any runtime operation. Schema files are read and parsed at parse time. Instruction files are also read at parse time and stored on the resolved protocol declarations. Resolved manifests also enforce schema/scope consistency for declared outputs, rejecting unscoped protocols whose `produces` or `may_produce` schemas require `work_unit`.
Unsafe artifact type or protocol names are also parse errors, rejected before runa attempts any layout-derived filesystem lookup.

## Methodology Registration

A methodology registers with runa through a manifest file and the layout convention. The manifest declares:

- The methodology's artifact types (names only — schemas are at conventional paths)
- The methodology's protocols and their declarations (instruction content is at conventional paths)
- No other configuration

The manifest and its accompanying layout are the methodology's only interface with the runtime. runa reads the manifest, resolves schemas and instruction content from the layout convention, builds the graph, and begins monitoring.
