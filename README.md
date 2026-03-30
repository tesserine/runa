# runa

Runa makes multi-step AI agent workflows reliable. When each step in a workflow declares what it needs and what it produces, runa validates every work product against its declared schema, computes which steps are ready to run, and delivers the right inputs to each agent invocation. The result: an orchestrator can compose agent steps into pipelines and trust that if a step completes, its output actually satisfies the contract — and that downstream steps receive only validated inputs.

This means agent workflows become composable. Teams can define reusable schemas and step definitions, swap implementations behind stable contracts, and build multi-stage pipelines where each handoff is enforced. Runa handles the enforcement so the agents and the orchestrator don't have to.

Runa is not an agent framework and does not include an AI model. It is the runtime layer between an orchestrator and the agents it directs.

## Core Concepts

Runa's interface rests on four concepts. Each builds on the ones before it.

An **artifact type** is a named category of work product — for example, `constraints`, `design-doc`, or `test-evidence` — with a JSON Schema that defines what a valid instance contains. The schema is the artifact's contract. Runa validates every instance against it.

A **protocol** (in runa, this term means a declared unit of work — not a network protocol or communication standard) specifies its relationship to artifacts through four edges: **requires** (must exist and validate before execution), **accepts** (consumed if available), **produces** (must exist and validate after execution), and **may_produce** (validated if present, not required). Each protocol also carries an activation rule. Execution order is not declared — it emerges from the dependency graph of requires/produces relationships across protocols.

A **trigger condition** is the activation rule that defines when runa activates a protocol. Three primitive types — `on_artifact` (a named artifact exists and validates), `on_change` (a named artifact is newer than the protocol's outputs for the same work unit), and `on_invalid` (a named artifact exists but fails validation) — compose through `all_of` and `any_of` operators with arbitrary nesting depth.

A **methodology** is a plugin configuration that registers with runa through a TOML manifest file. The manifest declares the methodology's artifact types, protocols, and trigger conditions. A directory layout convention places JSON Schemas at `schemas/{name}.schema.json` and protocol instruction files at `protocols/{name}/PROTOCOL.md`, both relative to the manifest. Runa derives these paths from declared names; the manifest contains no explicit path fields.

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

- [**agentd**](https://github.com/pentaxis93/agentd) — a process runtime that orchestrates autonomous agents through runa-managed workflows.
- [**groundwork**](https://github.com/pentaxis93/groundwork) — a methodology plugin that provides protocols and artifact types for runa.

Both projects are in early development.

## Documentation

- [CLI Reference](docs/cli-reference.md) — Commands, configuration, and MCP server
- [Methodology Authoring Guide](docs/methodology-authoring-guide.md) — Building a first methodology from scratch
- [Interface Contract](docs/interface-contract.md) — The three primitives defining the methodology-runtime boundary
- [Architecture](ARCHITECTURE.md) — Workspace structure, data flow, module descriptions, disk layout
- [Contributing](CONTRIBUTING.md) — Conventions for landing PRs
- [Quickstart Example](examples/quickstart-methodology/) — A two-protocol review pipeline you can browse and run
- [Commons](https://github.com/pentaxis93/commons) — Shared governance for the ecosystem

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
| `runa-mcp` | MCP server for artifact production |

See [CLI Reference](docs/cli-reference.md) for flags, exit codes, configuration, and behavioral details.

## Build

Rust 2024 edition. Runa targets Linux.

```bash
cargo build
cargo test --workspace
```
