# runa

Runa is an event-driven cognitive runtime for AI agents. It enforces contracts between methodologies and the runtime through three primitives: **artifact types** (JSON Schema-validated work products), **skill declarations** (relationships to artifacts via requires/accepts/produces/may_produce edges), and **trigger conditions** (composable activation rules).

## Architecture

Runa is a runtime layer between an orchestrating daemon and methodology plugins. Methodologies register via TOML manifests declaring their artifact types, skills, and triggers. Runa computes the dependency graph, validates artifacts against their schemas, tracks state, and evaluates trigger conditions.

See [ARCHITECTURE.md](ARCHITECTURE.md) for workspace structure, data flow, module descriptions, and disk layout.

## Usage

```bash
runa init --methodology path/to/manifest.toml [--artifacts-dir path/to/workspace]
```

Parses the methodology manifest, validates its structure, and creates a `.runa/` directory with `config.toml` (operator configuration: methodology path, optional artifact workspace directory), `state.toml` (runtime state: initialization timestamp, runa version), `.runa/workspace/` (default artifact workspace), and `.runa/store/` (internal artifact state store). Reports the artifact type and skill counts on success.

Commands that load a project manifest support `--config <PATH>` to override the config file location. The `RUNA_CONFIG` env var serves the same purpose. For `init`, `--config` controls where the config file is written; for manifest-loading commands, it controls where the config file is read from.

```bash
runa list
```

Displays skills in execution order with their artifact relationships, trigger conditions, and blocked status. Performs an implicit scan first so the output reflects the current workspace.

```bash
runa doctor
```

Checks project health: artifact validity, skill readiness, and dependency cycles. Performs an implicit scan first so reported health matches the current workspace. Exits 0 if healthy, 1 if problems found.

```bash
runa scan
```

Scans the artifact workspace, reconciles it into `.runa/store/`, records valid, invalid, and malformed artifacts, and reports new/modified/revalidated/removed instances plus unreadable entries and unrecognized workspace directories. If the workspace is missing, scan succeeds only when the store is still empty; otherwise it fails to avoid wiping stored state. Per-file read failures are reported as findings and do not abort the scan. Exits 0 if reconciliation succeeds, even when findings are present.

```bash
runa signal begin <name>
runa signal clear <name>
runa signal list
```

Manages persisted operator signals in `.runa/signals.json`, an optional runtime-state file with shape `{ "active": ["name1", "name2"] }`. Signal names must match `[a-z0-9][a-z0-9_-]*`. `begin` and `clear` are idempotent state setters: redundant requests still succeed and report the resulting state. `list` prints the active signals in lexicographic order or an explicit empty-state message when none are active.

```bash
runa status [--json]
```

Evaluates every skill after an implicit scan and classifies it as `READY`, `BLOCKED`, or `WAITING`. Text output groups skills in that order and shows the current inputs, precondition failures, or unsatisfied trigger conditions that explain each status. `on_signal` triggers read the persisted active signal set from `.runa/signals.json`; if the file is absent, status treats the signal set as empty. If the scan was only partial, status surfaces `Scan warnings` and blocks skills whose required artifact types could not be fully reconciled with reason `scan_incomplete`; partially scanned accepted artifact types are omitted from reported inputs. `--json` emits `{ "version": 1, "methodology": "...", "scan_warnings": [...], "skills": [...] }`, with a flat ordered `skills` array containing `name`, `status`, `trigger`, and the status-specific fields `inputs`, `precondition_failures`, or `unsatisfied_conditions`. Exits 0 when status evaluation succeeds, even if some skills are blocked or waiting. Commands that do not evaluate triggers do not read `signals.json`.

```bash
runa step --dry-run [--json]
```

Builds an operator-facing execution plan after an implicit scan. The execution plan contains `READY` skills that can be placed in a valid execution order, and each plan entry includes the skill name, the human-readable trigger that activated it, and a serialized agent-facing context injection payload. The context payload contains the skill name, all valid required and available accepted inputs with text paths, content hashes, and relationships, plus expected outputs split into `produces` and `may_produce`.

If the graph contains a hard dependency cycle, `step` reports the cycle as a warning and excludes the cyclic skills from `execution_plan`; non-cyclic READY skills still appear when they are orderable. `on_signal` triggers use the same persisted active signal set as `runa status`. Text output prints the execution plan and the same grouped skill status view used by `runa status`, so operators can still see blocked and waiting reasons when nothing is runnable. `--json` emits `{ "version": 1, "methodology": "...", "scan_warnings": [...], "cycle": ["..."] | null, "execution_plan": [...], "skills": [...] }`, where `skills` reuses the same status entries as `runa status --json`. `runa step` without `--dry-run` is not implemented yet; it prints a placeholder message and exits with code 1.

## Build

Rust 2024 edition.

```bash
cargo build          # Debug build
cargo test --lib     # Run all unit tests
```

## Documentation

- [PRINCIPLES.md](docs/PRINCIPLES.md) — Seven bedrock principles governing runtime and boundary decisions
- [Interface Contract](docs/interface-contract.md) — Three primitives defining the methodology-runtime boundary
- [ADRs](docs/adr/) — Architectural decision records
