# Methodology Authoring Guide

This guide walks through building a methodology from scratch. It assumes you have read the [README](../README.md) and understand runa's three primitives — artifact types, protocol declarations, and trigger conditions — at the conceptual level. By the end, you will have a working methodology that `runa init` accepts and that demonstrates protocol chaining through declared dependencies.

## A Complete Example

The example is a two-protocol review pipeline. A `draft` protocol reads requirements and produces a design. A `review` protocol reads the requirements and the design and produces a review report. The chain emerges from their declarations: `draft` produces one of `review`'s required inputs (`design`).

### Directory Layout

```
code-review/
  manifest.toml
  schemas/
    requirements.schema.json
    design.schema.json
    review-report.schema.json
  protocols/
    draft/
      PROTOCOL.md
    review/
      PROTOCOL.md
```

Every artifact type has a schema at `schemas/{name}.schema.json`. Every protocol has an instruction file at `protocols/{name}/PROTOCOL.md`. These paths are derived from the names declared in the manifest — the manifest contains no explicit path fields.

### Manifest

`manifest.toml`:

```toml
name = "code-review"

[[artifact_types]]
name = "requirements"

[[artifact_types]]
name = "design"

[[artifact_types]]
name = "review-report"

[[protocols]]
name = "draft"
requires = ["requirements"]
produces = ["design"]
trigger = { type = "on_artifact", name = "requirements" }

[[protocols]]
name = "review"
requires = ["requirements", "design"]
produces = ["review-report"]
trigger = { type = "on_artifact", name = "design" }
```

### Schemas

Each schema is a standard JSON Schema that defines what a valid artifact instance contains.

`schemas/requirements.schema.json`:

```json
{
  "type": "object",
  "required": ["title"],
  "properties": {
    "title": { "type": "string" }
  }
}
```

`schemas/design.schema.json`:

```json
{
  "type": "object",
  "required": ["summary"],
  "properties": {
    "summary": { "type": "string" }
  }
}
```

`schemas/review-report.schema.json`:

```json
{
  "type": "object",
  "required": ["approved"],
  "properties": {
    "approved": { "type": "boolean" }
  }
}
```

### Protocol Instructions

Each `PROTOCOL.md` contains the instruction text delivered to the agent at execution time.

`protocols/draft/PROTOCOL.md`:

```markdown
Read the requirements. Produce a design document summarizing the approach.
```

`protocols/review/PROTOCOL.md`:

```markdown
Read the requirements and the design. Evaluate whether the design satisfies the requirements. Report approval status.
```

Because runa injects only declared `requires` and available `accepts` inputs, `review` declares both `requirements` and `design` explicitly.

### Initialize and Inspect

```bash
runa init --methodology code-review/manifest.toml
runa state
```

`runa state` shows both protocols as WAITING — no artifacts exist yet, so no trigger conditions are satisfied.

## How the Chain Works

When no artifacts exist, both protocols are WAITING. Their triggers monitor artifact state, and there is nothing to monitor yet.

An external source — a user, another tool, or a prior workflow — places a `requirements` artifact in the workspace (a JSON file under `<workspace>/requirements/`). Scan the workspace to reconcile the new file into runa's store:

```bash
runa scan
```

After reconciliation, the `draft` protocol's trigger (`on_artifact("requirements")`) is satisfied: a valid requirements instance exists. Its precondition (`requires = ["requirements"]`) is also met. `draft` becomes READY.

The `review` protocol remains WAITING. Its trigger needs a `design` artifact, which does not exist.

When `draft` executes and produces a valid `design` artifact, runa validates it against `design.schema.json`. Now `review`'s trigger (`on_artifact("design")`) is satisfied, and its `requirements` and `design` preconditions are both met. `review` becomes READY, executes, and produces a `review-report`. Both protocols have completed.

The manifest never declares this ordering. runa computes it from the dependency graph: `draft` produces `design`, `review` requires `design`, so `draft` must complete before `review` can execute. `review` also directly requires `requirements`, which remains available from the external input that activated `draft`.

## Manifest Reference

This section covers enough to modify the example. The [interface contract](interface-contract.md) is the authoritative specification.

### Artifact Types

Each `[[artifact_types]]` entry declares one artifact type. The only field is `name` — a unique identifier that must be a safe path component (no `/`, `\`, or `..`). The name determines the schema location: `schemas/{name}.schema.json`.

### Protocols

Each `[[protocols]]` entry declares one protocol. Two fields are required:

- **name** — unique identifier (same naming constraints as artifact types). Determines the instruction file location: `protocols/{name}/PROTOCOL.md`.
- **trigger** — the condition under which runa activates this protocol.

Four optional fields declare the protocol's relationship to artifacts:

- **requires** — artifact types that must exist and validate before execution. Hard dependency.
- **produces** — artifact types that must exist and validate after execution. runa enforces this.
- **accepts** — artifact types consumed if available. Soft dependency. The protocol operates with or without them.
- **may_produce** — artifact types that might be produced. Validated if present, not required.
- **scoped** — boolean, default `false`. `false` means the protocol is evaluated once in unscoped mode. `true` means the protocol is evaluated only when the caller supplies `--work-unit <ID>`, and only for that exact delegated work unit.

Unscoped protocols must not declare outputs whose schemas require `work_unit`. If an artifact schema named in `produces` or `may_produce` lists `work_unit` in its top-level `required` array, the protocol must declare `scoped = true`. Otherwise the manifest is invalid and `runa init` rejects it.

The example above uses `requires` and `produces`. See the [interface contract](interface-contract.md) for the full semantics of `accepts` and `may_produce`.

`scoped` is protocol metadata, not topology. Dependency edges still come only from artifact relationships. Use `scoped = true` when a protocol belongs to delegated work-unit execution and should never be discovered by scanning sibling artifact state.

### Trigger Conditions

The example uses `on_artifact`, which fires when a valid instance of the named artifact type exists. Two other primitive types are available:

- **on_change** — the named artifact is newer than the protocol's outputs for the same work unit.
- **on_invalid** — the named artifact exists but fails schema validation.

These compose through `all_of` (all conditions must hold) and `any_of` (at least one must hold), with arbitrary nesting. See the [interface contract](interface-contract.md) for the full trigger model.

## Where to Look Next

- [Interface contract](interface-contract.md) — the authoritative specification for all manifest fields, trigger conditions, layout conventions, and runtime behavior.
- [groundwork](https://github.com/pentaxis93/groundwork) — a real methodology built on runa. The canonical example of artifact types, protocol chaining, and trigger composition in practice.
- [README](../README.md) — core concepts, configuration, command reference, and MCP server documentation.
