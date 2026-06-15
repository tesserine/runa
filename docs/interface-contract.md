# runa Interface Contract

This document defines the boundary between runa (the runtime) and methodology
plugins. Everything inside this boundary, runa sees and enforces. Everything
outside, methodologies own entirely.

For the companion boundary between runa and the drivers or agents that operate
a session, see the [Session Surface Contract](session-surface-contract.md).

## Three Primitives

The methodology interface consists of three primitive concepts. Runtime-owned
session and configuration surfaces are declared separately below.

### 1. Artifact Types

An artifact type is a named category of work product with a machine-checkable contract. Methodologies define their artifact types. runa validates instances against their contracts.

An artifact type declaration:

- **name** — unique identifier within the methodology (e.g., `constraints`, `behavior-contract`, `test-evidence`)
- Artifact type names must be safe single path components because runa derives schema paths from them. Names must not contain `/`, `\`, or `..`.
- **schema** — JSON Schema defining what a valid instance contains. This schema is the artifact's contract. There is no separate contract mechanism. The schema is not declared in the manifest — runa derives its location from the methodology layout convention (see below).

runa ships no artifact types. Every artifact type is methodology-owned.

### 2. Protocol Declarations

A protocol declares its relationship to artifacts through input edges and output
contracts:

- **requires** — the named artifact type must have at least one valid instance before the protocol can execute. Hard dependency. Invalid, malformed, or stale siblings remain health findings but do not block execution when a valid instance exists.
- **accepts** — the named artifact type may be consumed if available. The protocol operates with or without it. Soft dependency.
- **produces** — the named artifact type will exist and validate after the protocol executes. runa fails the protocol if a declared output is missing or invalid.
- **may_produce** — the named artifact type might be produced. runa validates any instance that appears but does not fail the protocol for its absence.
- **required_output_choices** — a named group of artifact types where exactly one member type must exist and validate after execution. This models mutually exclusive required outcomes such as `approved` versus `needs-revision`.

A protocol declaration:

- **name** — unique identifier
- Protocol names must be safe single path components because runa derives instruction paths from them. Names must not contain `/`, `\`, or `..`.
- **requires** — zero or more artifact type names
- **accepts** — zero or more artifact type names
- **produces** — zero or more artifact type names
- **may_produce** — zero or more artifact type names. Absent optional outputs do not fail postconditions, but they also do not create completion evidence. If output should always be produced, the artifact type belongs in `produces`.
- **required_output_choices** — zero or more tables with `name` and `members`. Each group name must be unique within the protocol. Each group must list at least two registered artifact type names. Members must not repeat and must not overlap with the protocol's `produces`, `may_produce`, or another required output choice member.
- **scoped** — optional boolean, default `false`. When `false`, the protocol participates only in unscoped evaluation. When `true`, the protocol participates only in caller-scoped evaluation for an explicit work unit supplied by the orchestrator.
- Output schema consistency is part of manifest validity for unscoped protocols. Unscoped protocols (`scoped = false` or omitted) must not declare output schemas in `produces`, `may_produce`, or `required_output_choices` members whose top-level `required` array includes `work_unit`.
- Freshness suppression uses successful execution records when available. After a protocol finishes with passing postconditions, runa records the freshness-relevant input set it processed for that `(protocol, work_unit)` pair. Those execution-record snapshots are mode-aware: `on_change` and `on_invalid` preserve any recorded matching instance, while `on_artifact` and `requires` compare only valid instances. Later evaluations suppress reruns only when the current mode-appropriate input set matches that execution record. If no execution record exists, runa falls back to output artifact timestamps, which still compare relevant inputs by latest recorded modification time. `produces` and the exactly one selected member of each `required_output_choices` group create completion evidence; `may_produce` does not.
- **trigger** — one trigger condition (see below)

Example required output choice syntax:

```toml
[[protocols.required_output_choices]]
name = "disposition"
members = ["approved", "needs-revision"]
```

After that protocol executes, zero produced members fails postconditions, one
valid member satisfies the choice, and multiple produced members fail as a
conflict.

Topology is not declared. It emerges from the graph of requires/produces/may_produce relationships across protocols. Required output choice members are branch-dependent outputs; they can order downstream consumers softly, but dry-run projection does not invent an unknown branch. A pipeline emerges when protocols chain linearly. A graph emerges when protocols fan in or fan out. A cycle emerges when a protocol produces an artifact type that another protocol's trigger monitors for change. The methodology does not tell runa what shape it is. runa computes the shape from declarations.

Scope is not topology. Dependency edges remain type-level. `scoped = true` does not change graph structure, and runa does not infer scope from artifact schemas, `work_unit` fields, or artifact filenames. But unscoped protocols still cannot declare outputs whose schemas require `work_unit`: if a protocol's outputs require `work_unit`, the protocol itself must be declared `scoped = true`.

### 3. Trigger Conditions

A trigger condition defines when runa should activate a protocol. Triggers are composable from three primitive types:

- **on_artifact(name)** — at least one valid instance of the named artifact exists
- **on_change(name)** — the named artifact is newer than this protocol's current output artifacts for the same work unit. Trigger satisfaction remains timestamp-based even though freshness suppression may later use a recorded input set from the last successful execution.
- **on_invalid(name)** — an instance of the named artifact type exists but fails validation against its declared schema

For `on_change` protocols, timestamps still decide whether the trigger fires. A successful execution then records the freshness-relevant input set for later suppression checks. Because `on_change` uses any recorded matching instance in execution records, an invalid or malformed sibling changing content reopens the protocol instead of being hidden by a prior successful run; `on_artifact` and `requires` keep comparing only valid instances once execution-record freshness is available, so unrelated invalid siblings do not reopen purely-valid work forever. When no execution record exists yet, the timestamp fallback still applies.

These compose through two operators:

- **all_of(conditions...)** — all conditions must be satisfied
- **any_of(conditions...)** — at least one condition must be satisfied

Nesting is permitted. `all_of(on_artifact("constraints"), any_of(on_change("review"), on_artifact("auto-approve")))` means: constraints must exist, and either a review change or an auto-approve artifact must be present.

## Runtime-Owned Scoped Identity

Scoped evaluation is a runtime capability. When the caller supplies
`--work-unit <ID>`, runa validates that `<ID>` exactly matches a recorded
`work-unit` artifact instance when any such instances exist. Tracker-looking
aliases such as bare issue numbers are not accepted as scope identifiers.

For tracker-backed delegated work, runa also owns the forge-address contract
used by scoped work-unit validation. The durable operator surface is the
portable `.runa/project.toml` forge table: an instance declares its type and
service hosts, and repositories or trackers declare resources hosted by that
instance.

```toml
[[forges.instances]]
id = "github-com"
type = "github"
host = "github.com"

[[forges.repositories]]
id = "runa"
instance = "github-com"
owner = "tesserine"
name = "runa"
```

The launched-runtime environment surface is the runa-owned
`RUNA_FORGE_ADDRESSES` JSON payload. Callers do not override individual forge
coordinates; they select configured resources by non-secret selector where a
forge operation needs one.

Runa computes deployment and tracker identities from that contract. A GitHub
repository identity is shaped as `github@github.com/repo/<owner>/<name>`, and
its tracker identity is `github@github.com/tracker/<owner>/<name>`. SourceHut
trackers include the declared git and tracker hosts plus tracker coordinates.

When a valid recorded `work-unit` root contains a forge-tagged tracker handle,
runa enforces the runtime checks that JSON Schema cannot express: the canonical
instance id's tracker number agrees with the handle number, duplicate tracker
roots are rejected, and the handle deployment identity agrees with the active
runtime deployment identity. The methodology still owns the `work-unit` schema
and the semantics of the artifact content; runa owns only the scope identity
checks described here.

### Entry References and the Acquisition Surface

A session may be opened from a forge ticket reference instead of a recorded
`work-unit` instance id, via `runa run --ticket <REF>` or
`runa go --ticket <REF>`. The accepted reference forms are a bare ticket number,
`#<N>`, `owner/repo#<N>`, a GitHub issue URL, or `sourcehut:<tracker_id>#<N>`.
runa parses the reference and normalizes it to a tracker identity
(`github:<owner>/<name>:<N>` or `sourcehut:<tracker_id>:<N>`); a reference that
asserts a deployment other than the active one is rejected, and a bare reference
inherits the active deployment identity. **runa never reads ticket content** —
the reference carries identity only, and the methodology performs all forge
reads through its own mechanics. During entry, runa exports `RUNA_ENTRY_TICKET`
(the ticket number) alongside the `RUNA_FORGE_*` atoms so those mechanics can
resolve the ticket.

The acquisition surface runa serves is the single unscoped protocol whose
declared outputs (`produces`, `may_produce`, or required output choice members)
include the `work-unit` artifact type. A manifest with zero or more than one
such protocol does not support ticket entry; runa names the offending
declarations.

Entry substitutes only the acquisition protocol's trigger: the operator's
reference is that step's activation condition. The protocol's preconditions,
output validation, postconditions, execution recording, and scoped-identity
checks all apply unchanged. The promised scope resolves to the unique valid
`work-unit` instance whose tracker handle identity equals the reference
identity. If that instance already exists when the session opens, no acquisition
step runs; a session opened from a reference and a session opened from the
materialized work-unit id are indistinguishable downstream of acquisition.

## What runa Does

runa is an event-driven runtime. The CLI commands (init, scan, list, state, step, doctor) are windows into its state. The runtime itself is the monitoring loop.

Given the declarations above, runa provides five runtime capabilities:

**Monitoring.** runa watches artifact state and evaluates trigger conditions on relevant state changes within the caller's evaluation scope. When a protocol's trigger condition becomes satisfied, runa activates the protocol.

**Validation.** When an artifact is produced, runa validates it against its declared schema. A protocol's execution is not complete until its `produces` artifacts exist and validate and each `required_output_choices` group has exactly one valid produced member. `may_produce` artifacts are validated if present but not required.

**Graph computation.** runa computes the dependency graph from protocol declarations. This enables: freshness analysis (which artifacts are stale), execution ordering (what can run now), cycle detection (where the methodology creates loops), and blocked-protocol identification (what's waiting on what).

**Enforcement.** A protocol cannot execute if any `requires` artifact type lacks a valid instance. A protocol's execution is incomplete if its `produces` artifacts are missing or invalid, or if a required output choice has zero or multiple produced member types. These are hard constraints the runtime enforces regardless of what the methodology intends.

**Context injection.** When a protocol is ready to execute, runa resolves which artifact instances the protocol needs — all valid `requires` instances and all available valid `accepts` instances within the active scope — and delivers them as the protocol's input context alongside the protocol's instruction content and expected outputs, including any required output choices. The protocol receives its inputs without querying the store directly.

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

Both schema files and instruction files must exist at their conventional locations when the manifest is parsed. Missing files are parse errors, caught before any runtime operation. Schema files are read and parsed at parse time. Instruction files are also read at parse time and stored on the resolved protocol declarations. Resolved manifests also enforce the unscoped-output rule for declared outputs, rejecting unscoped protocols whose `produces`, `may_produce`, or required output choice member schemas require `work_unit`.
Unsafe artifact type or protocol names are also parse errors, rejected before runa attempts any layout-derived filesystem lookup.

## Methodology Registration

A methodology registers with runa through a manifest file and the layout convention. The manifest declares:

- The methodology's artifact types (names only — schemas are at conventional paths)
- The methodology's protocols and their declarations (instruction content is at conventional paths)
- No other configuration

The manifest and its accompanying layout are the methodology's only interface with the runtime. runa reads the manifest, resolves schemas and instruction content from the layout convention, builds the graph, and begins monitoring.
