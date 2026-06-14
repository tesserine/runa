# runa

**Contracts for cognition: the runtime that makes agent work verifiable
instead of hoped-for.**

Runa makes multi-step AI agent workflows reliable. When each step in a workflow declares what it needs and what it produces, runa validates every work product against its declared schema, computes which steps are ready to run, and delivers the right inputs to each agent invocation. The result: an orchestrator can compose agent steps into pipelines and trust that if a step completes, its output actually satisfies the contract — and that downstream steps receive only validated inputs.

This means agent workflows become composable. Teams can define reusable schemas and step definitions, swap implementations behind stable contracts, and build multi-stage pipelines where each handoff is enforced. Runa handles the enforcement so the agents and the orchestrator don't have to.

Three properties make this different from orchestration glue:

- **Order is emergent, not scripted.** No step list exists anywhere; execution
  order falls out of the dependency graph of what protocols require and
  produce. Change the artifacts and the topology reorganizes itself.
- **Invalid work cannot flow.** Artifacts delivered through the agent
  interface are validated *before* they are written
  ([the production contract](AGENTS.md#artifact-production-contract)) — a
  failing output is rejected with details and never reaches disk. Anything
  else that appears in the workspace is caught at scan: it never satisfies a
  dependency and is never handed to an agent as validated input. Where each
  guarantee is enforced: [docs/security.md](docs/security.md).
- **You can feel the loop in two minutes, no agent required.** The
  [quickstart](examples/quickstart-methodology/) walks the full
  scan → READY → produce cascade by hand — every transition derived from
  artifact state alone.

Runa is not an agent framework and does not include an AI model. It is the
runtime layer between an orchestrator and the agents it directs — the
**Enforce** tier of the [Tesserine](https://github.com/tesserine) stack, and
the engine both shipped methodologies
([groundwork](https://github.com/tesserine/groundwork),
[gazette](https://github.com/tesserine/gazette)) run on, unchanged. Any
disciplined process you can declare as artifacts and protocols inherits the
same guarantees.

## Core Concepts

Runa's interface rests on four concepts. Each builds on the ones before it.

An **artifact type** is a named category of work product — for example, `constraints`, `design-doc`, or `test-evidence` — with a JSON Schema that defines what a valid instance contains. The schema is the artifact's contract. Runa validates every instance against it.

A **protocol** (in runa, this term means a declared unit of work — not a network protocol or communication standard) specifies its relationship to artifacts through **requires** (at least one valid instance must exist before execution), **accepts** (consumed if available), **produces** (must exist and validate after execution), **may_produce** (validated if present, not required), and **required_output_choices** (named output groups where exactly one member artifact type must be produced and validate). Invalid, malformed, or stale sibling instances remain health findings, but they do not block `requires` when a valid instance exists. Each protocol also carries an activation rule and optional scope metadata: protocols default to unscoped, while `scoped = true` means the protocol runs only for an explicit work unit supplied by the caller. Execution order is not declared — it emerges from the dependency graph of requires/produces/may_produce relationships across protocols.

A **trigger condition** is the activation rule that defines when runa activates a protocol. Three primitive types — `on_artifact` (at least one valid instance of the named artifact exists), `on_change` (a named artifact is newer than the protocol's outputs for the same work unit), and `on_invalid` (a named artifact exists but fails validation) — compose through `all_of` and `any_of` operators with arbitrary nesting depth.

A **methodology** is a plugin configuration that registers with runa through a TOML manifest file. The manifest declares the methodology's artifact types, protocols, and trigger conditions. A directory layout convention places JSON Schemas at `schemas/{name}.schema.json` and protocol instruction files at `protocols/{name}/PROTOCOL.md`, both relative to the manifest. Runa derives these paths from declared names; the manifest contains no explicit path fields.

Scope is methodology-owned metadata, not something runa infers from schemas or artifact contents. Unscoped commands evaluate only unscoped protocols. `runa state --work-unit <ID>`, `runa step --work-unit <ID>`, `runa run --work-unit <ID>`, and `runa go --work-unit <ID>` evaluate only `scoped = true` protocols for that delegated work unit.

When a methodology records `work-unit` artifacts, scoped entry points require
`<ID>` to exactly match one recorded `work-unit` instance id. Tracker-looking
aliases such as ticket numbers or partial ids are rejected and the diagnostic
lists the available canonical ids. If no `work-unit` artifacts are recorded,
scoped evaluation remains inert and accepts the caller-supplied id as before.

To start scoped work from nothing but a tracker ticket, `runa run --ticket <REF>`
and `runa go --ticket <REF>` open a cold-start session from a forge ticket
reference. A bare number or `#<N>` is accepted when the project has exactly one
configured tracker; multi-tracker projects use `<tracker-selector>#<N>`. The
runtime resolves the reference through the configured forge-address payload and
serves the methodology's acquisition surface; the methodology reads the ticket
and materializes the `work-unit`, after which the session is indistinguishable
from one opened on a recorded work-unit. The runtime performs no forge read of
its own.

## Quick Start

A methodology needs a manifest file, a JSON Schema for each artifact type, and an instruction file for each protocol. Here is a minimal example with one protocol that consumes a `spec` artifact and produces a `report`.

Directory layout:

```
my-methodology/
  manifest.toml
  schemas/
    spec.schema.json
    report.schema.json
  protocols/
    analyze/
      PROTOCOL.md
```

`manifest.toml`:

```toml
name = "example"

[[artifact_types]]
name = "spec"

[[artifact_types]]
name = "report"

[[protocols]]
name = "analyze"
requires = ["spec"]
produces = ["report"]
trigger = { type = "on_artifact", name = "spec" }
```

Each schema file is a standard JSON Schema. Here, a spec must have a `title` and a report must have a `summary`:

`schemas/spec.schema.json`:

```json
{
  "type": "object",
  "required": ["title"],
  "properties": {
    "title": { "type": "string" }
  }
}
```

`schemas/report.schema.json`:

```json
{
  "type": "object",
  "required": ["summary"],
  "properties": {
    "summary": { "type": "string" }
  }
}
```

`protocols/analyze/PROTOCOL.md` contains the instruction text delivered to the agent at execution time. Any content is valid.

Initialize the project:

```bash
runa init --methodology my-methodology/manifest.toml
runa state
```

`runa state` shows `analyze` as WAITING — no `spec` artifact exists yet. Create one that violates the schema (missing the required `title` field) and scan:

```bash
mkdir -p .runa/workspace/spec
echo '{"score": 1}' > .runa/workspace/spec/first.json
runa scan
```

Runa finds the artifact but reports it as invalid:

```
Invalid:
  spec/first (.runa/workspace/spec/first.json)
    - /required: "title" is a required property
```

The `analyze` protocol remains WAITING — runa will not activate it with invalid inputs. Fix the artifact and scan again:

```bash
echo '{"title": "Widget API"}' > .runa/workspace/spec/first.json
runa scan
runa state
```

Now `runa state` shows `analyze` as READY with the validated spec listed as input:

```
READY:
  analyze
    - spec/first (requires)
```

`runa step --dry-run` previews the execution: the protocol to run, the input artifacts it will receive, and the MCP server configuration for the agent runtime.

## Ecosystem

Runa is the enforcement layer in a larger agent toolchain:

- [**agentd**](https://github.com/tesserine/agentd) — a process runtime that orchestrates autonomous agents through runa-managed workflows.
- [**groundwork**](https://github.com/tesserine/groundwork) — a methodology plugin that provides protocols and artifact types for runa.

Both projects are in early development.

## Documentation

- [CLI Reference](docs/cli-reference.md) — Commands, configuration, and MCP server
- [Methodology Authoring Guide](docs/methodology-authoring-guide.md) — Building a first methodology from scratch
- [Interface Contract](docs/interface-contract.md) — The three primitives defining the methodology-runtime boundary
- [Session Surface Contract](docs/session-surface-contract.md) — The mode-agnostic driver/agent boundary for session invocation and lifecycle movement
- [Security and Safety Surface](docs/security.md) — Index of guarantees: path safety, validate-before-write, ticket-content blindness, redaction, and where each is enforced
- [Architecture](ARCHITECTURE.md) — Workspace structure, data flow, module descriptions, disk layout
- [Contributing](CONTRIBUTING.md) — Conventions for landing PRs
- [Releasing](RELEASING.md) — Repository release operation and verification
- [Quickstart Example](examples/quickstart-methodology/) — A two-protocol review pipeline you can browse and run
- [Commons](https://github.com/tesserine/commons) — The ecosystem's convention and ADR authority: cross-component contracts, release conventions, and the [source-of-truth map](https://github.com/tesserine/commons/blob/main/SOURCE-OF-TRUTH.md). The principles themselves live at their canonical home, [pentaxis93/principles](https://github.com/pentaxis93/principles). All development on runa follows both as active guidelines, not optional reading

## Commands

| Command | Purpose |
|---------|---------|
| `runa init` | Initialize a project from a methodology manifest |
| `runa scan` | Reconcile the artifact workspace into the store |
| `runa list` | Display protocols in topological order |
| `runa state` | Evaluate and classify protocol readiness |
| `runa doctor` | Check project health |
| `runa step` | Execute the next ready protocol |
| `runa run` | Walk the ready frontier to quiescence |
| `runa go` | Advance one scoped interactive session tick through the session MCP surface |
| `runa-mcp` | MCP server for artifact production and session driver verbs |

See [CLI Reference](docs/cli-reference.md) for flags, exit codes, configuration, and behavioral details.

## Configuration

`runa init` creates project configuration under `.runa/`. Portable forge
addresses live in `.runa/config.toml` as instances, repositories, and trackers;
machine-local launch and path settings live in `.runa/local.toml`. Runa delivers
the resolved forge-address set to agents and MCP servers through the
`RUNA_PROJECT_FORGE_ADDRESSES` payload.

Runa ships supported agent adapters for Codex and Claude Code in `adapters/`.
Set `[launch].command` to `./adapters/agent-codex.sh` or
`./adapters/agent-claude-code.sh`; the adapter translates `RUNA_MCP_CONFIG` for
the selected runtime.

## Build

Rust 2024 edition. Runa targets Linux.

```bash
cargo build
cargo test --workspace
```
