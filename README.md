# runa

Runa is an event-driven cognitive runtime for AI agents. It enforces contracts between methodologies and the runtime through three primitives: **artifact types** (JSON Schema-validated work products), **protocol declarations** (relationships to artifacts via requires/accepts/produces/may_produce edges), and **trigger conditions** (composable activation rules).

## Architecture

Runa is a runtime layer between an orchestrating daemon and methodology plugins. Methodologies register via TOML manifests declaring their artifact types, protocols, and triggers. Runa computes the dependency graph, validates artifacts against their schemas, tracks state, and evaluates trigger conditions.

See [ARCHITECTURE.md](ARCHITECTURE.md) for workspace structure, data flow, module descriptions, and disk layout.

## Usage

```bash
runa init --methodology path/to/manifest.toml [--artifacts-dir path/to/workspace]
```

Parses the methodology manifest, validates its structure, and creates a `.runa/` directory with `config.toml` (operator configuration: methodology path, optional artifact workspace directory, optional logging settings), `state.toml` (runtime state: initialization timestamp, runa version), `.runa/workspace/` (default artifact workspace), and `.runa/store/` (internal artifact state store). Reports the artifact type and protocol counts on success.

Commands that load a project manifest support `--config <PATH>` to override the config file location. The `RUNA_CONFIG` env var serves the same purpose. For `init`, `--config` controls where the config file is written; for manifest-loading commands, it controls where the config file is read from.

## Logging

Runtime diagnostics use `tracing` on stderr. Command output remains on stdout.

- Default logging is quiet on successful runs (`warn` and above, human-readable text)
- `RUST_LOG` overrides the active filter for both `runa` and `runa-mcp`
- `config.toml` can provide logging defaults when `RUST_LOG` is unset

Optional config snippet:

```toml
[logging]
format = "json"   # "text" (default) or "json"
filter = "info"   # any tracing env-filter directive
```

`format = "json"` switches stderr events to machine-readable JSON. This does not change stdout command output. `RUST_LOG` controls the filter only; output format is always determined by `config.logging.format` (defaulting to `text`).

```bash
runa list
```

Displays protocols in execution order with their artifact relationships, trigger conditions, and blocked status. Performs an implicit scan first so the output reflects the current workspace.
Blocked protocols report required-artifact failures using the same `missing`, `invalid`, and `stale` taxonomy used elsewhere in the CLI.

```bash
runa doctor
```

Checks project health: artifact validity, protocol readiness, and dependency cycles. Performs an implicit scan first so reported health matches the current workspace. Exits 0 if healthy, 1 if problems found.
Skill readiness reports required-artifact failures as `missing`, `invalid`, or `stale`.

```bash
runa scan
```

Scans the artifact workspace, reconciles it into `.runa/store/`, records valid, invalid, and malformed artifacts, and reports new/modified/revalidated/removed instances plus unreadable entries and unrecognized workspace directories. If the workspace is missing, scan succeeds only when the store is still empty; otherwise it fails to avoid wiping stored state. Per-file read failures are reported as findings and do not abort the scan. Exits 0 if reconciliation succeeds, even when findings are present.

```bash
runa status [--json]
```

Evaluates every protocol after an implicit scan and classifies it as `READY`, `BLOCKED`, or `WAITING`. Text output groups protocols in that order and shows the current inputs, precondition failures, or unsatisfied trigger conditions that explain each status. If the scan was only partial, status surfaces `Scan warnings` and blocks protocols whose required artifact types could not be fully reconciled with reason `scan_incomplete`; partially scanned accepted artifact types are omitted from reported inputs. `--json` emits `{ "version": 2, "methodology": "...", "scan_warnings": [...], "protocols": [...] }`, with a flat ordered `protocols` array containing `name`, `status`, `trigger`, and the status-specific fields `inputs`, `precondition_failures`, or `unsatisfied_conditions`. Exits 0 when status evaluation succeeds, even if some protocols are blocked or waiting.
Unreadable produced artifacts do not block protocols directly, but they do conservatively disable freshness suppression for every work unit of that output type until a clean scan restores trustworthy completion evidence.

```bash
runa step --dry-run [--json]
```

Builds an operator-facing execution plan after an implicit scan. The execution plan contains `READY` protocols that can be placed in a valid execution order, and each plan entry includes the protocol name, the human-readable trigger that activated it, and a serialized agent-facing context injection payload. The context payload contains the protocol name, all valid required and available accepted inputs with text paths, content hashes, and relationships, plus expected outputs split into `produces` and `may_produce`.

If the graph contains a hard dependency cycle, `step` reports the cycle as a warning and excludes the cyclic protocols from `execution_plan`; non-cyclic READY protocols still appear when they are orderable. Text output prints the execution plan and the same grouped protocol status view used by `runa status`, so operators can still see blocked and waiting reasons when nothing is runnable. `--json` emits `{ "version": 2, "methodology": "...", "scan_warnings": [...], "cycle": ["..."] | null, "execution_plan": [...], "protocols": [...] }`, where `protocols` reuses the same status entries as `runa status --json`. `runa step` without `--dry-run` is not implemented yet; it prints a placeholder message and exits with code 1.
Like `runa status`, unreadable produced artifacts conservatively keep all work units of that output type eligible for rerun rather than attempting to scope freshness loss to a single instance.

## MCP Server

`runa-mcp` is a single-session stdio MCP server that orchestrates one protocol execution per invocation. It is designed to be started by an outer orchestrator (e.g., an MCP client) for each protocol run.

```bash
runa-mcp
```

On startup, the server loads the project from the current directory (or `RUNA_WORKING_DIR`), scans the workspace, and selects the first ready (protocol, work_unit) candidate. It then serves an MCP session over stdio with:

- **Tools** — One tool per output artifact type (`produces` + `may_produce`). The tool input schema is the artifact's JSON Schema with `work_unit` removed. The server injects `work_unit` automatically.
- **Prompts** — A single `"context"` prompt that delivers the protocol's required and available inputs as prose, plus expected outputs.

When the session ends, the server re-scans the workspace and checks postconditions. Valid output artifacts in the workspace are the completion evidence; the next run derives freshness directly from their timestamps. The outer orchestrator can then restart `runa-mcp` for the next protocol.

Environment variables:
- `RUNA_WORKING_DIR` — Project directory (defaults to current directory)
- `RUNA_CONFIG` — Config file override (same as `--config` in the CLI)
- `RUST_LOG` — Tracing filter override for stderr diagnostics

## Build

Rust 2024 edition.

```bash
cargo build          # Debug build
cargo test --lib     # Run all unit tests
```

## Documentation

- [Commons](https://github.com/pentaxis93/commons) — Bedrock principles and architectural decision records (ADRs)
- [Interface Contract](docs/interface-contract.md) — Three primitives defining the methodology-runtime boundary
