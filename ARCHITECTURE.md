# Architecture

Runa is a cognitive runtime for AI agents. This document describes the codebase as it exists today.

## Workspace Structure

Two crates, Rust 2024 edition, resolver v3:

- **`libagent`** — All domain logic: data model, TOML manifest parsing, JSON Schema validation, dependency graph, artifact state tracking, trigger condition evaluation, context injection construction, pre/post-execution enforcement.
- **`runa-cli`** — Thin CLI binary. Clap-based argument parsing, delegates to libagent. No domain logic.

## Data Flow

These are library capabilities exposed by libagent. No runtime loop exists yet.

1. **TOML manifest → model types.** `manifest::parse` reads a methodology manifest file, deserializes TOML into `Manifest` (containing `ArtifactType` and `SkillDeclaration` vectors), and validates name uniqueness at parse time.

2. **Skill declarations → dependency graph.** `DependencyGraph::build` takes `&[SkillDeclaration]` and computes edges from requires/accepts → produces/may_produce relationships. Provides topological ordering (Kahn's algorithm), cycle detection (falls back to hard-edges-only on combined-graph cycle), and blocked-skill identification.

3. **Artifact workspace → validated state.** `scan::scan` walks the artifact workspace, parses `*.json` files under `<workspace>/<type_name>/`, validates them against the type schemas, and reconciles the results into `.runa/store/`. Valid, invalid, and malformed artifacts are all stored — invalid and malformed state are meaningful for trigger evaluation. Per-file read failures are collected as unreadable findings rather than aborting reconciliation.

4. **Trigger evaluation.** `trigger::evaluate` recursively evaluates a `TriggerCondition` tree against a `TriggerContext` (artifact store, per-skill activation timestamps, active signals) and returns a `TriggerResult`. Pure function, no side effects.

5. **Context injection construction.** `context::build_context` converts a ready `SkillDeclaration` plus the current artifact store into the stable agent-facing payload used by `runa step`: skill name, available required/accepted artifact refs, and expected outputs.

## Modules

### `model.rs`

Core types: `Manifest`, `ArtifactType`, `SkillDeclaration`, `TriggerCondition`. `TriggerCondition` is a tagged enum (`#[serde(tag = "type", rename_all = "snake_case")]`) with six variants: `OnArtifact`, `OnChange`, `OnInvalid`, `OnSignal`, `AllOf`, `AnyOf`. `AllOf`/`AnyOf` hold `Vec<TriggerCondition>` for arbitrary nesting depth.

### `manifest.rs`

TOML parsing and structural validation. `parse` reads from a file path; `from_str` parses a TOML string. Both validate that artifact type names and skill names are unique within the manifest, returning `ManifestError` on duplicates, I/O failures, or TOML parse errors.

### `validation.rs`

JSON Schema validation for artifact instances using the `jsonschema` crate. `validate_artifact` compiles the schema, runs validation, and collects all violations into a `Vec<Violation>` before returning. Each `Violation` carries the artifact type name, a description, and both schema and instance JSON Pointer paths.

### `graph.rs`

Dependency graph built from skill declarations. Edges derive from artifact relationships: `requires` → `produces` creates hard edges, `requires` → `may_produce` and `accepts` → any producer create soft edges. `topological_order` runs Kahn's algorithm on combined (hard+soft) edges first; on cycle, retries hard-edges-only. Hard-edge cycles return `CycleError`. `blocked_skills` identifies skills whose `requires` artifacts are not in a provided available set.

### `context.rs`

Stable agent-facing context injection contract. `build_context` gathers all valid required artifacts and all valid available accepted artifacts for a skill into ordered `ArtifactRef` entries carrying `artifact_type`, `instance_id`, lossy text `path`, `content_hash`, and `relationship` (`requires` or `accepts`). `ExpectedOutputs` exposes `produces` and `may_produce` artifact type names without embedding trigger/operator details.

### `store.rs`

Artifact state tracking keyed by `(type_name, instance_id)`. Each `ArtifactState` records the filesystem path, `ValidationStatus` (Valid, Invalid with violations, Malformed with a parse error, or Stale), millisecond-precision modification timestamp, a `sha256:<hex>` content hash, and a `schema_hash` for the artifact type schema used during validation. Parsed JSON uses canonical JSON hashing (recursively sorted keys); malformed files hash raw bytes. Persists as JSON files under `.runa/store/{type_name}/{instance_id}.json`. Uses atomic write (tmp + rename).

### `scan.rs`

Filesystem reconciliation from the artifact workspace into the store. `scan` treats `<workspace>/<type_name>/<instance_id>.json` as the artifact convention, ignores non-JSON files, reports unrecognized top-level directories, classifies new/modified/revalidated/removed instances, records invalid or malformed artifacts in store state, and collects unreadable file entries with their path and error message. Modified means the content hash changed and `last_modified_ms` was updated to the scan timestamp. Revalidated means the artifact content was unchanged but the schema hash changed, so validation was rerun without updating `last_modified_ms`. If the workspace directory is missing, scan returns an error unless the store is still empty.

### `trigger.rs`

Recursive trigger condition evaluator. `evaluate` is a pure function that takes a `TriggerCondition`, a `TriggerContext` (read-only references to the artifact store, activation timestamps, and active signals), and a skill name. Six condition variants: `OnArtifact` distinguishes between missing valid instances and visible invalid/stale instances, `OnChange` compares latest modification against the skill's activation timestamp, `OnInvalid` checks `store.has_any_invalid`, `OnSignal` checks set membership, `AllOf` short-circuits on first failure, `AnyOf` short-circuits on first success.

### `enforcement.rs`

Pre/post-execution enforcement of skill contracts. Two pure functions that check a `SkillDeclaration` against an `ArtifactStore`:
- `enforce_preconditions` — verifies all `requires` artifacts exist with all instances valid. `accepts` is explicitly not checked.
- `enforce_postconditions` — verifies all `produces` artifacts exist with all instances valid; validates `may_produce` artifacts if present (absent is ok). `accepts` is not checked.

Returns `EnforcementError` on failure, containing the skill name, enforcement phase, and a list of `ArtifactFailure` entries. Three failure variants distinguish corrective actions: `Missing` (no instances), `Invalid` (schema violations), `Stale` (needs revalidation).

## `.runa/` Directory Layout

```
.runa/
  config.toml                   # Created by `runa init`: methodology_path, optional artifacts_dir
  state.toml                    # Created by `runa init`: initialized_at, runa_version
  workspace/                    # Default artifact workspace (configurable via artifacts_dir)
    {type_name}/
      {instance_id}.json        # Agent-produced artifact file
  store/                        # Internal artifact state store (not configurable)
    {type_name}/
      {instance_id}.json        # ArtifactState: path, status, last_modified_ms, content_hash, schema_hash
```

## CLI Commands

All commands that operate on an initialized project share `project::load`, which resolves the config file, reads the methodology path from it, parses the manifest, builds the dependency graph, and opens the artifact store.

Config resolution is whole-file (first found wins, no per-field merging): `--config` CLI flag → `RUNA_CONFIG` env var → `.runa/config.toml` → `$XDG_CONFIG_HOME/runa/config.toml` → error.

### `runa init --methodology <PATH> [--artifacts-dir <DIR>] [--config <PATH>]`

Parses the manifest at `<PATH>` via `libagent::manifest::parse`, canonicalizes the path, creates `.runa/config.toml` (or writes to the `--config` path) containing the canonical methodology path and optional artifact workspace directory. Creates `.runa/state.toml`, `.runa/store/`, and the resolved artifact workspace directory. Reports the artifact type and skill counts on success.

### `runa list`

Runs an implicit workspace scan, then displays skills in topological (execution) order with their artifact relationships and trigger conditions. For each skill, shows non-empty relationship fields (requires, accepts, produces, may_produce), the trigger condition, and a `BLOCKED` indicator if required artifact types have no valid instances. On cycle detection, falls back to manifest order with a warning.

### `runa doctor`

Runs an implicit workspace scan, then reports on project health. Three checks:
1. **Artifact health** — enumerates instances per artifact type via `store.instances_of()`, reports invalid, malformed, or stale instances with details.
2. **Skill readiness** — for each skill, checks whether all `requires` artifact types have valid instances. Reports missing or invalid artifact types.
3. **Cycle detection** — runs `graph.topological_order()`, reports any cycle.

Exits 0 if no problems found, 1 otherwise.

### `runa scan`

Runs the workspace reconciliation pass. Reads artifact files from the resolved workspace directory, updates `.runa/store/`, reports new/modified/revalidated/removed artifacts, reports invalid, malformed, and unreadable entries separately, and lists unrecognized top-level workspace directories. A missing workspace is treated as an error unless the store is still empty. Per-file read failures are findings, not command failures. Exits 0 on successful reconciliation and non-zero only for load/store/I/O failures.

### `runa status`

Runs an implicit workspace scan, then evaluates every skill against current runtime state. Classification is ordered and mutually exclusive: `WAITING` when the trigger is not satisfied, `BLOCKED` when the trigger is satisfied but `enforce_preconditions` fails, and `READY` otherwise. Uses an empty `TriggerContext` for activation timestamps and active signals because no runtime state loop exists yet.

Text output groups skills as `READY`, `BLOCKED`, then `WAITING`, preserving the graph-derived skill order within each group. `READY` entries list valid required and accepted artifact instances, `BLOCKED` entries list required artifact failures (`missing`, `invalid`, `stale`, `scan_incomplete`), and `WAITING` entries list detailed unsatisfied trigger conditions including the trigger condition and the specific `TriggerResult::NotSatisfied` reason. When scan reconciliation is partial, status prints scan warnings before the skill groups and treats any skill whose `requires` includes an affected artifact type as blocked because readiness cannot be verified; affected `accepts` types remain non-blocking and are omitted from the reported inputs.

`--json` emits a versioned envelope:
- `version` — integer envelope version, currently `1`
- `methodology` — manifest name
- `scan_warnings` — array of human-readable warnings for partial scan findings, empty when scan reconciliation is complete
- `skills` — flat ordered array of skill objects with `name`, `status`, `trigger`, plus the status-specific field `inputs`, `precondition_failures`, or `unsatisfied_conditions`

Exits 0 for successful status evaluation regardless of whether skills are ready, blocked, or waiting. Non-zero exit remains reserved for project-load, scan, or serialization failures.

### `runa step --dry-run [--json]`

Runs the same implicit scan and shared skill evaluation used by `runa status`, then builds an execution plan from the `READY` skills that can be placed in a valid execution order. Plan entries preserve graph order for the non-cyclic frontier and include the skill name, the trigger condition string, and the JSON-serialized `libagent::context::ContextInjection` payload. If a hard dependency cycle exists, `step` reports it as a warning, excludes the cycle participants from the plan, and still includes any unrelated orderable READY skills. Text output prints the execution plan followed by the grouped READY/BLOCKED/WAITING view. JSON output adds an `execution_plan` array plus an optional `cycle` path while reusing the same `skills` status entries and `scan_warnings` envelope fields as `runa status`.

`runa step` without `--dry-run` is a deliberate stub: it prints `Agent execution is not yet implemented. Use --dry-run to see the execution plan.` and exits with code 1.

## Key Design Patterns

- **Custom error types with source chains.** Each module defines its own error enum implementing `std::fmt::Display` and `std::error::Error` with `source()` for chaining.
- **Inline test modules.** All tests are `#[cfg(test)] mod tests` within their source file.
- **Tagged enum serialization.** `TriggerCondition` uses `#[serde(tag = "type", rename_all = "snake_case")]` so conditions serialize as `{"type": "on_artifact", "name": "..."}`.
- **Canonical JSON for content hashing.** Object keys are recursively sorted before SHA-256 hashing, ensuring deterministic hashes regardless of insertion order.
