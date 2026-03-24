# Architecture

Runa is a cognitive runtime for AI agents. This document describes the codebase as it exists today.

## Workspace Structure

Three crates, Rust 2024 edition, resolver v3:

- **`libagent`** — All domain logic: data model, TOML manifest parsing, JSON Schema validation, dependency graph, artifact state tracking, trigger condition evaluation, context injection construction, pre/post-execution enforcement, project loading, and protocol selection.
- **`runa-cli`** — Thin CLI binary. Clap-based argument parsing, delegates to libagent. No domain logic.
- **`runa-mcp`** — MCP server binary. Single-session stdio process that serves one (protocol, work_unit) pair per invocation. Loads the project, selects a ready candidate, exposes protocol outputs as MCP tools and context as an MCP prompt, then relies on the produced artifacts themselves as completion evidence.

## Data Flow

These are library capabilities exposed by libagent and consumed by both the CLI and the MCP server.

1. **TOML manifest → model types.** `manifest::parse` reads a methodology manifest file, deserializes TOML into `Manifest` (containing `ArtifactType` and `ProtocolDeclaration` vectors), validates artifact type names and protocol names are unique and safe as layout-derived path components, then resolves the methodology layout convention: loads schema content from `schemas/{name}.schema.json` and validates instruction file existence at `protocols/{name}/PROTOCOL.md`, both relative to the manifest directory.

2. **Skill declarations → dependency graph.** `DependencyGraph::build` takes `&[ProtocolDeclaration]` and computes edges from requires/accepts → produces/may_produce relationships. Provides topological ordering (Kahn's algorithm), cycle detection (falls back to hard-edges-only on combined-graph cycle), and blocked-protocol identification.

3. **Artifact workspace → validated state.** `scan::scan` walks the artifact workspace, parses `*.json` files under `<workspace>/<type_name>/`, validates them against the type schemas, and reconciles them into `.runa/store/`. Valid, invalid, and malformed artifacts are all stored — invalid and malformed state are meaningful for trigger evaluation. Per-file read failures are collected as unreadable findings rather than aborting reconciliation. Scan also records in-memory scan-gap metadata on the store: type-level gaps when scan cannot identify which instances were missed, and instance-level gaps when a specific artifact file is unreadable.

4. **Trigger evaluation.** `trigger::evaluate` recursively evaluates a `TriggerCondition` tree against a `TriggerContext` (artifact store, scan metadata) plus the protocol declaration and returns a `TriggerResult`. `OnChange` derives temporal state from output artifact timestamps for the same work unit and consults the store's scan-gap metadata so any unreadable output instance conservatively invalidates completion evidence for the whole output type. Pure function, no side effects.

5. **Context injection construction.** `context::build_context` converts a ready `ProtocolDeclaration` plus the current artifact store into the stable agent-facing payload used by `runa step` and `runa-mcp`: protocol name, available required/accepted artifact refs, and expected outputs.

6. **Protocol selection.** `selection::discover_ready_candidates` evaluates protocols in topological order, discovers candidate work_units from artifact instances, and returns (protocol, work_unit) pairs where trigger, preconditions, and scan trust are all satisfied. Current work whose valid outputs are newer than all relevant inputs is suppressed directly from artifact freshness, with unreadable output instances conservatively disabling suppression for every work unit of the affected output type.

7. **Tracing bootstrap.** Both binaries bootstrap tracing with env/default settings before any config lookup, then reconfigure the shared subscriber from `config.toml` when logging settings are available. Tracing events always go to stderr; operator-facing command output stays on stdout.

8. **MCP runtime loop.** `runa-mcp` loads the project, scans, selects the first ready candidate, serves an MCP session via stdio, then re-scans and checks postconditions. Successful postconditions leave completion evidence in the workspace via the output artifacts themselves.

## Modules

### `model.rs`

Core types: `Manifest`, `ArtifactType`, `ProtocolDeclaration`, `TriggerCondition`. `ProtocolDeclaration` includes an `instructions: Option<String>` field populated by `manifest::parse` with the protocol's `PROTOCOL.md` content (`None` from `from_str`). `TriggerCondition` is a tagged enum (`#[serde(tag = "type", rename_all = "snake_case")]`) with five variants: `OnArtifact`, `OnChange`, `OnInvalid`, `AllOf`, `AnyOf`. `AllOf`/`AnyOf` hold `Vec<TriggerCondition>` for arbitrary nesting depth.

### `manifest.rs`

TOML parsing, structural validation, and methodology layout resolution. `from_str` deserializes a TOML string into raw types and converts to model types with unresolved schemas (`Value::Null`) and no instruction content. `parse` reads from a file path, calls `from_str`, then resolves the methodology layout convention — loading schema JSON from `schemas/{artifact_type_name}.schema.json` and loading instruction text from `protocols/{protocol_name}/PROTOCOL.md`. Both validate that artifact type names and protocol names are unique within the manifest and safe as single path components, rejecting names containing `/`, `\`, or `..` before any filesystem lookup. The TOML format uses `deny_unknown_fields` on artifact type declarations, rejecting old-format manifests that include explicit `schema` fields.

### `validation.rs`

JSON Schema validation for artifact instances using the `jsonschema` crate. `validate_artifact` compiles the schema, runs validation, and collects all violations into a `Vec<Violation>` before returning. Each `Violation` carries the artifact type name, a description, and both schema and instance JSON Pointer paths.

### `logging.rs`

Shared tracing bootstrap and reconfiguration. `configure_tracing` installs one global subscriber backed by a reloadable `EnvFilter` and a runtime-selectable text/JSON stderr formatter. `resolve_logging_config` applies the fixed precedence `RUST_LOG` → `config.logging.filter` → default `warn`, while `config.logging.format` chooses text or JSON output.

### `graph.rs`

Dependency graph built from protocol declarations. Edges derive from artifact relationships: `requires` → `produces` creates hard edges, `requires` → `may_produce` and `accepts` → any producer create soft edges. `topological_order` runs Kahn's algorithm on combined (hard+soft) edges first; on cycle, retries hard-edges-only. Hard-edge cycles return `CycleError`. `blocked_protocols` identifies protocols whose `requires` artifacts are not in a provided available set.

### `context.rs`

Stable agent-facing context injection contract. `build_context` gathers all valid required artifacts and all valid available accepted artifacts for a protocol into ordered `ArtifactRef` entries carrying `artifact_type`, `instance_id`, lossy text `path`, `content_hash`, and `relationship` (`requires` or `accepts`). It also copies the preloaded protocol instruction content into the payload. `ExpectedOutputs` exposes `produces` and `may_produce` artifact type names without embedding trigger/operator details.

### `store.rs`

Artifact state tracking keyed by `(type_name, instance_id)`. Each `ArtifactState` records the filesystem path, `ValidationStatus` (Valid, Invalid with violations, Malformed with a parse error, or Stale), millisecond-precision modification timestamp, a `sha256:<hex>` content hash, a `schema_hash` for the artifact type schema used during validation, and an optional `work_unit` string extracted from artifact JSON at record time. Parsed JSON uses canonical JSON hashing (recursively sorted keys); malformed files hash raw bytes. Persists as JSON files under `.runa/store/{type_name}/{instance_id}.json`. Uses atomic write (tmp + rename). Separately, the store keeps non-persisted scan-gap metadata for the current process so completion/freshness checks can distinguish whole-type scan failures from unreadable specific instances.

Query methods (`is_valid`, `has_any_invalid`, `instances_of`, `latest_modification_ms`) accept an `Option<&str>` work unit filter. `None` returns all instances (unscoped). `Some(wu)` returns instances matching that work unit plus unpartitioned instances (those with no `work_unit` field). This scoping threads through trigger evaluation, enforcement, and context injection.

### `scan.rs`

Filesystem reconciliation from the artifact workspace into the store. `scan` treats `<workspace>/<type_name>/<instance_id>.json` as the artifact convention, ignores non-JSON files, reports unrecognized top-level directories, classifies new/modified/revalidated/removed instances, records invalid or malformed artifacts in store state, and collects unreadable file entries with their path and error message. Modified means the content hash changed and `last_modified_ms` was updated to the scan timestamp. Revalidated means the artifact content was unchanged but the schema hash changed, so validation was rerun without updating `last_modified_ms`. If the workspace directory is missing, scan returns an error unless the store is still empty. Known unreadable artifact files become instance-level scan gaps in the store; unreadable type directories and other unidentified failures become type-level scan gaps.

### `trigger.rs`

Recursive trigger condition evaluator. `evaluate` is a pure function that takes a `TriggerCondition`, the enclosing `ProtocolDeclaration`, and a `TriggerContext` (read-only references to the artifact store and scan metadata). Five condition variants: `OnArtifact` distinguishes between missing valid instances and visible invalid/stale instances, `OnChange` compares the named input's latest timestamp against the protocol's derived output timestamp, `OnInvalid` checks `store.has_any_invalid`, `AllOf` short-circuits on first failure, `AnyOf` short-circuits on first success. Completion derivation for `OnChange` uses the store's scan-gap metadata so unreadable outputs conservatively invalidate freshness for the whole output type.

### `util.rs`

Shared internal utilities (`pub(crate)`). Contains `current_time_ms`, which returns the current wall-clock time as milliseconds since the Unix epoch. Used by `store.rs` and `scan.rs` for artifact modification timestamps. Not part of the public API.

### `enforcement.rs`

Pre/post-execution enforcement of protocol contracts. Two pure functions that check a `ProtocolDeclaration` against an `ArtifactStore`:
- `enforce_preconditions` — verifies all `requires` artifacts exist with all instances valid. `accepts` is explicitly not checked.
- `enforce_postconditions` — verifies all `produces` artifacts exist with all instances valid; validates `may_produce` artifacts if present (absent is ok). `accepts` is not checked.

Returns `EnforcementError` on failure, containing the protocol name, enforcement phase, and a list of `ArtifactFailure` entries. Three failure variants distinguish corrective actions: `Missing` (no instances), `Invalid` (schema violations), `Stale` (needs revalidation).

### `project.rs`

Shared project loading logic used by both `runa-cli` and `runa-mcp`. Config resolution chain (explicit override → `.runa/config.toml` → XDG config → error), manifest parsing, dependency graph construction, and artifact store initialization.

### `selection.rs`

Work-unit discovery and protocol selection. `discover_ready_candidates` evaluates protocols in topological order, collecting candidate work_units from artifact instances referenced by the protocol's edges and trigger tree. For each candidate: checks scan trust (skips if any `requires` type is partially scanned), evaluates the trigger condition, checks preconditions, and suppresses work whose outputs are already fresh relative to its inputs. Output scan gaps do not block candidates directly; they only make freshness/completion untrustworthy for the whole output type. Returns candidates in topological protocol order with deterministic lexicographic work_unit ordering within each protocol.

## runa-mcp Modules

### `main.rs`

Runtime loop: loads the project, scans the workspace, discovers ready candidates, selects the first, builds the MCP handler, serves via stdio transport, then re-scans and checks postconditions. Successful outputs become the completion evidence for the next selection pass.

### `handler.rs`

`ServerHandler` implementation. `RunaHandler` derives one MCP tool per output artifact type (`produces` + `may_produce`), with tool input schemas derived from artifact type JSON Schemas (with `work_unit` stripped). `call_tool` validates artifacts before writing, then writes to the workspace and records in the store. A single `"context"` prompt delivers the protocol context.

### `context.rs`

Natural language context prompt renderer. Transforms `ContextInjection` into a `GetPromptResult` with the protocol heading, a protocol-instructions section, prose-rendered artifact content, and output guidance. JSON objects become labeled key-value sections with humanized keys, arrays become numbered lists, and nested structures are indented. Artifact read/parse errors are rendered inline rather than failing the prompt.

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
      {instance_id}.json        # ArtifactState: path, status, last_modified_ms, content_hash, schema_hash, work_unit
```

## CLI Commands

Commands that operate on a loaded methodology share `project::load`, which resolves the config file, reads the methodology path from it, parses the manifest, builds the dependency graph, and opens the artifact store.

Config resolution is whole-file (first found wins, no per-field merging): `--config` CLI flag → `RUNA_CONFIG` env var → `.runa/config.toml` → `$XDG_CONFIG_HOME/runa/config.toml` → error.

### `runa init --methodology <PATH> [--artifacts-dir <DIR>] [--config <PATH>]`

Parses the manifest at `<PATH>` via `libagent::manifest::parse`, canonicalizes the path, creates `.runa/config.toml` (or writes to the `--config` path) containing the canonical methodology path, optional artifact workspace directory, and optional logging settings. Creates `.runa/state.toml`, `.runa/store/`, and the resolved artifact workspace directory. Reports the artifact type and protocol counts on success.

### `runa list`

Runs an implicit workspace scan, then displays protocols in topological (execution) order with their artifact relationships and trigger conditions. For each protocol, shows non-empty relationship fields (requires, accepts, produces, may_produce), the trigger condition, and a `BLOCKED` indicator when `enforce_preconditions` reports required artifact failures. `BLOCKED` reasons are rendered with the shared `missing` / `invalid` / `stale` taxonomy. On cycle detection, falls back to manifest order with a warning.

### `runa doctor`

Runs an implicit workspace scan, then reports on project health. Three checks:
1. **Artifact health** — enumerates instances per artifact type via `store.instances_of()`, reports invalid, malformed, or stale instances with details.
2. **Skill readiness** — for each protocol, uses `enforce_preconditions` to check `requires` artifact types and reports `missing`, `invalid`, or `stale` failures.
3. **Cycle detection** — runs `graph.topological_order()`, reports any cycle.

Exits 0 if no problems found, 1 otherwise.

### `runa scan`

Runs the workspace reconciliation pass. Reads artifact files from the resolved workspace directory, updates `.runa/store/`, reports new/modified/revalidated/removed artifacts, reports invalid, malformed, and unreadable entries separately, and lists unrecognized top-level workspace directories. A missing workspace is treated as an error unless the store is still empty. Per-file read failures are findings, not command failures. Exits 0 on successful reconciliation and non-zero only for load/store/I/O failures.

### `runa status`

Runs an implicit workspace scan, then evaluates every protocol against current runtime state. Classification is ordered and mutually exclusive: `WAITING` when the trigger is not satisfied, `BLOCKED` when the trigger is satisfied but `enforce_preconditions` fails, and `READY` otherwise. `on_change` freshness is derived directly from artifact timestamps in the store.

Text output groups protocols as `READY`, `BLOCKED`, then `WAITING`, preserving the graph-derived protocol order within each group. `READY` entries list valid required and accepted artifact instances, `BLOCKED` entries list required artifact failures (`missing`, `invalid`, `stale`, `scan_incomplete`), and `WAITING` entries list detailed unsatisfied trigger conditions including the trigger condition and the specific `TriggerResult::NotSatisfied` reason. When scan reconciliation is partial, status prints scan warnings before the protocol groups and treats any protocol whose `requires` includes an affected artifact type as blocked because readiness cannot be verified; affected `accepts` types remain non-blocking and are omitted from the reported inputs.

`--json` emits a versioned envelope:
- `version` — integer envelope version, currently `2`
- `methodology` — manifest name
- `scan_warnings` — array of human-readable warnings for partial scan findings, empty when none apply
- `protocols` — flat ordered array of protocol objects with `name`, optional `work_unit`, `status`, `trigger`, plus the status-specific field `inputs`, `precondition_failures`, or `unsatisfied_conditions`

Exits 0 for successful status evaluation regardless of whether protocols are ready, blocked, or waiting. Non-zero exit remains reserved for project-load, scan, or serialization failures.

### `runa step --dry-run [--json]`

Runs the same implicit scan and shared candidate classification used by `runa status`, then builds an execution plan from the `READY` `(protocol, work_unit)` pairs that can be placed in a valid execution order. Candidate discovery, trigger evaluation, and freshness suppression use the same work-unit-scoped selection logic as `runa-mcp`, so `step --dry-run` previews the same ready work that MCP would attempt to serve. Plan entries preserve graph order for the non-cyclic frontier and include the protocol name, optional `work_unit`, the trigger condition string, and the JSON-serialized `libagent::context::ContextInjection` payload, including preloaded protocol instructions. If a hard dependency cycle exists, `step` reports the cycle as a warning, excludes the cycle participants from the plan, and still includes any unrelated orderable READY protocols. Text output prints the execution plan followed by the grouped READY/BLOCKED/WAITING view. JSON output adds an `execution_plan` array plus an optional `cycle` path while reusing the same `protocols` status entries and `scan_warnings` envelope fields as `runa status`. `runa step` without `--dry-run` is a deliberate stub: it prints `Agent execution is not yet implemented. Use --dry-run to see the execution plan.` and exits with code 1.

## Key Design Patterns

- **Custom error types with source chains.** Each module defines its own error enum implementing `std::fmt::Display` and `std::error::Error` with `source()` for chaining.
- **Inline test modules.** All tests are `#[cfg(test)] mod tests` within their source file.
- **Tagged enum serialization.** `TriggerCondition` uses `#[serde(tag = "type", rename_all = "snake_case")]` so conditions serialize as `{"type": "on_artifact", "name": "..."}`.
- **Canonical JSON for content hashing.** Object keys are recursively sorted before SHA-256 hashing, ensuring deterministic hashes regardless of insertion order.
