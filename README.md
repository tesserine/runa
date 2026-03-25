# runa

Runa is an event-driven cognitive runtime for AI agents. It enforces contracts between methodologies and the runtime through three primitives: **artifact types** (JSON Schema-validated work products), **protocol declarations** (relationships to artifacts via requires/accepts/produces/may_produce edges), and **trigger conditions** (composable activation rules).

## Architecture

Runa is a runtime layer between an orchestrating daemon and methodology plugins. Methodologies register via TOML manifests declaring their artifact types, protocols, and triggers. Schema content and protocol instruction files live at conventional locations relative to the manifest (`schemas/{name}.schema.json` and `protocols/{name}/PROTOCOL.md`). Runa derives these paths from the manifest, computes the dependency graph, validates artifacts against their schemas, tracks state, and evaluates trigger conditions.

See [ARCHITECTURE.md](ARCHITECTURE.md) for workspace structure, data flow, module descriptions, and disk layout.

## Usage

```bash
runa init --methodology path/to/manifest.toml [--artifacts-dir path/to/workspace]
```

Parses the methodology manifest, validates its structure, and creates a `.runa/` directory with `config.toml` (operator configuration: methodology path, optional artifact workspace directory, optional logging settings, optional agent command), `state.toml` (runtime state: initialization timestamp, runa version), `.runa/workspace/` (default artifact workspace), and `.runa/store/` (internal artifact state store). Reports the artifact type and protocol counts on success.

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

Optional agent execution config:

```toml
[agent]
command = ["/path/to/agent-runtime", "--flag"]
```

`runa step` without `--dry-run` requires `[agent].command`. Runa executes that argv command in the project root, sends one pretty-printed JSON execution payload on stdin for each planned protocol, and leaves stdout/stderr attached to the child process. `--json` is dry-run only.

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

Evaluates every protocol after an implicit scan and classifies it as `READY`, `BLOCKED`, or `WAITING`. Evaluation is work-unit scoped: protocols are checked once per discovered work unit, with unscoped protocols still evaluated once overall. Text output groups protocols in that order and shows the current inputs, precondition failures, or unsatisfied trigger conditions that explain each status. If the scan was only partial, status surfaces `Scan warnings` and blocks protocols whose required artifact types could not be fully reconciled with reason `scan_incomplete`; partially scanned accepted artifact types are omitted from reported inputs. `--json` emits `{ "version": 2, "methodology": "...", "scan_warnings": [...], "protocols": [...] }`, with a flat ordered `protocols` array containing `name`, optional `work_unit`, `status`, `trigger`, and the status-specific fields `inputs`, `precondition_failures`, or `unsatisfied_conditions`. Exits 0 when status evaluation succeeds, even if some protocols are blocked or waiting.
Unreadable produced artifacts do not block protocols directly, but they do conservatively disable freshness suppression for every work unit of that output type until a clean scan restores trustworthy completion evidence.

```bash
runa step [--dry-run] [--json]
```

Builds an operator-facing execution plan after an implicit scan. `step` evaluates protocols per discovered work unit and suppresses activated work when valid outputs are newer than the relevant inputs for that work unit. The execution plan therefore contains `READY` `(protocol, work_unit)` pairs that can be placed in a valid execution order. Each plan entry includes the protocol name, optional `work_unit`, the human-readable trigger that activated it, and a serialized agent-facing context payload. The context payload contains the protocol name, the preloaded `PROTOCOL.md` instruction content, all valid required and available accepted inputs with text paths, content hashes, and relationships, plus expected outputs split into `produces` and `may_produce`.

If the graph contains a hard dependency cycle, `step` reports the cycle as a warning and excludes the cyclic protocols from `execution_plan`; non-cyclic READY protocols still appear when they are orderable. `--dry-run` prints the execution plan and the same grouped protocol status view used by `runa status`, so operators can still see blocked and waiting reasons when nothing is runnable. `--json` emits `{ "version": 2, "methodology": "...", "scan_warnings": [...], "cycle": ["..."] | null, "execution_plan": [...], "protocols": [...] }`, where `execution_plan` entries and `protocols` status entries may include an optional `work_unit`, and `protocols` reuses the same status entries as `runa status --json`.

Without `--dry-run`, `step` requires `[agent].command`, then invokes that command once per execution-plan entry in order. For each invocation, runa writes the exact plan entry JSON to the child process on stdin and waits for exit status `0` before continuing to the next entry. A non-zero exit stops execution immediately and skips post-execution validation; scan/reconciliation and cascading readiness remain future work.
Like `runa status`, unreadable produced artifacts conservatively keep all work units of that output type eligible for rerun rather than attempting to scope freshness loss to a single instance.

## MCP Server

`runa-mcp` is a single-session stdio MCP server that serves one named protocol invocation per process. It is designed to be started by an outer orchestrator (e.g., an MCP client) for each protocol run.

```bash
runa-mcp --protocol <name> [--work-unit <name>]
```

On startup, the server loads the project from the current directory (or `RUNA_WORKING_DIR`), resolves the named protocol from the manifest, validates that its output types can be served as MCP tools, and then serves an MCP session over stdio with:

- **Tools** — One tool per output artifact type (`produces` + `may_produce`). The tool input schema is the artifact's JSON Schema with `work_unit` removed. The server injects `work_unit` automatically.
- **Prompts** — A single `"context"` prompt that delivers the protocol name, preloaded instructions, required and available inputs as prose, and expected outputs.

Environment variables:
- `RUNA_WORKING_DIR` — Project directory (defaults to current directory)
- `RUNA_CONFIG` — Config file override (same as `--config` in the CLI)
- `RUST_LOG` — Tracing filter override for stderr diagnostics

## Build

Rust 2024 edition.

```bash
cargo build          # Debug build
cargo test --workspace
```

## Documentation

- [Commons](https://github.com/pentaxis93/commons) — Bedrock principles and architectural decision records (ADRs)
- [Interface Contract](docs/interface-contract.md) — Three primitives defining the methodology-runtime boundary
