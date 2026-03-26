# runa

Runa is an event-driven cognitive runtime for AI agents. It enforces contracts between methodologies and the runtime through three primitives: **artifact types** (JSON Schema-validated work products), **protocol declarations** (relationships to artifacts via requires/accepts/produces/may_produce edges), and **trigger conditions** (composable activation rules).

Runa targets Linux. Read-only commands and dry-run planning remain available as documented, but live `runa step` and live `runa run` fail explicitly on non-Linux platforms instead of degrading execution contracts silently.

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
command = ["./examples/agent-claude-code.sh"]
```

`runa step` and `runa run` without `--dry-run` require `[agent].command`. Runa executes that argv command in the project root, renders a natural-language execution prompt from the planned protocol context, writes that prompt on stdin, and leaves stdout/stderr attached to the child process. Before each invocation it also exports `RUNA_MCP_CONFIG`, a JSON description of how the wrapper should spawn `runa-mcp` for the selected protocol run. The wrapper adapts that config to the specific agent runtime. `--json` is dry-run only.

The exported `RUNA_MCP_CONFIG` payload stays runtime-agnostic: `{command,args,env}`. Agent wrappers are responsible for adapting that generic server description to their runtime's schema. The Claude example wrapper wraps it as `{"mcpServers":{"runa":...}}` before invoking `claude`. The exported command and env paths are absolute whenever `runa` resolves them from the local filesystem, so wrappers do not depend on their child process cwd to launch `runa-mcp`.

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
runa state [--json]
```

Evaluates every protocol after an implicit scan and classifies it as `READY`, `BLOCKED`, or `WAITING`. Evaluation is work-unit scoped: protocols are checked once per discovered work unit, with unscoped protocols still evaluated once overall. Text output groups protocols in that order and shows the current inputs, precondition failures, or unsatisfied trigger conditions that explain each status. If the scan was only partial, state surfaces `Scan warnings` and blocks protocols whose required artifact types could not be fully reconciled with reason `scan_incomplete`; partially scanned accepted artifact types are omitted from reported inputs. `--json` emits `{ "version": 2, "methodology": "...", "scan_warnings": [...], "protocols": [...] }`, with a flat ordered `protocols` array containing `name`, optional `work_unit`, `status`, `trigger`, and the status-specific fields `inputs`, `precondition_failures`, or `unsatisfied_conditions`. Exits 0 when state evaluation succeeds, even if some protocols are blocked or waiting.
Unreadable produced artifacts do not block protocols directly, but they do conservatively disable freshness suppression for every work unit of that output type until a clean scan restores trustworthy completion evidence.

```bash
runa step [--dry-run] [--json]
```

Builds an operator-facing preview of the next execution after an implicit scan. `step` evaluates protocols per discovered work unit and suppresses activated work when valid outputs are newer than the relevant inputs for that work unit. `step --dry-run` therefore reports at most one concrete `(protocol, work_unit)` candidate: the next non-cyclic READY execution in graph order. That entry includes the protocol name, optional `work_unit`, the human-readable trigger that activated it, an MCP server config for the selected candidate, and a serialized agent-facing context payload. The context payload contains the protocol name, optional `work_unit`, the preloaded `PROTOCOL.md` instruction content, all valid required and available accepted inputs with display-only `display_path` strings, content hashes, and relationships, plus expected outputs split into `produces` and `may_produce`. Live prompt rendering still reads the exact filesystem path internally, so valid Unix filenames with non-UTF8 bytes are preserved end-to-end.
`--dry-run` is planning-only: it previews the next concrete MCP launch config but does not require a discoverable `runa-mcp` binary. Live execution resolves `runa-mcp` only when a READY candidate will actually run, preferring a sibling binary next to the running `runa` executable and falling back to `PATH`.

If the graph contains a hard dependency cycle, `step` reports the cycle as a warning and excludes the cyclic protocols from `execution_plan`; non-cyclic READY protocols still appear when they are orderable. `--dry-run` prints the next execution and the same grouped protocol status view used by `runa state`, so operators can still see blocked and waiting reasons when nothing is runnable. `--json` emits `{ "version": 4, "methodology": "...", "scan_warnings": [...], "cycle": ["..."] | null, "execution_plan": [...], "protocols": [...] }`, where `execution_plan` now contains at most one entry and `protocols` reuses the same status entries as `runa state --json`.

Without `--dry-run`, `step` requires `[agent].command`. If no READY work exists, it prints `No READY protocols.` and exits without requiring `runa-mcp`. Otherwise it resolves `runa-mcp`, executes exactly one READY candidate, re-scans the workspace, enforces postconditions for that `(protocol, work_unit)`, then prints the refreshed READY/BLOCKED/WAITING view so the operator can see what became ready next. A non-zero exit still stops execution immediately and skips the post-execution reconciliation cycle.

Live `runa step` targets Linux. On non-Linux platforms it fails explicitly before resolving agent or MCP execution.

```bash
runa run [--dry-run] [--json]
```

`run` is the cascade command. Live execution walks the same non-cyclic READY frontier as `step`, but continues until quiescence instead of stopping after one protocol. Previously exhausted work reopens only when a later successful execution, postcondition-failing reconciliation, or agent-failing reconciliation changes inputs that are relevant to that candidate. Agent failures and postcondition failures are tolerated for the remainder of the invocation: the failed candidate is skipped, any artifacts emitted before failure are reconciled into the workspace state for downstream readiness, other READY work continues, and the command exits `2` after quiescence if any protocol failed. If no READY work exists and some work remains blocked, waiting on external input, or trapped in a hard dependency cycle, `run` exits `3`. A fully satisfied topology exits `0`. The first `Ctrl-C` is boundary-scoped: the current protocol run is allowed to finish its reconciliation cycle. After that reconciliation, `run` exits `130` with outcome `interrupted` only if an interrupt prevented the next READY candidate from starting. If the same reconciliation leaves no further READY work, the quiescent topology outcome takes precedence (`0`, `2`, or `3`) because the interrupt did not prevent any work from executing. A second `Ctrl-C` forces immediate exit with status `130`.

`run --dry-run` projects the full optimistic cascade from manifest topology only: declared `produces` outputs, `requires`/`accepts` edges, trigger declarations, and the current evaluated work-unit state. `may_produce` outputs remain optional and do not advance the projection unless they already exist on disk. Initially ready entries include the same MCP config and context shape used by `step --dry-run` only on their first concrete emission; downstream projected entries carry only protocol name, optional work unit, trigger, and projection kind. Projected work-unit scoping comes from manifest relationships plus the current artifact state, not from synthesizing artifact payloads. The dry-run projection never synthesizes values, never forks the artifact store, and never bypasses schema validation. Its exit status still reflects the current evaluated topology after the initial scan rather than forcing success because a projection was printed.

Live `runa run` targets Linux. On non-Linux platforms it fails explicitly before resolving agent or MCP execution.

## MCP Server

`runa-mcp` is a single-session stdio MCP server that serves one named protocol invocation per process. It is designed to be started by an outer orchestrator (e.g., an MCP client) for each protocol run.

```bash
runa-mcp --protocol <name> [--work-unit <name>]
```

On startup, the server loads the project from the current directory (or `RUNA_WORKING_DIR`), resolves the named protocol from the manifest, validates that its output types can be served as MCP tools, and then serves an MCP session over stdio with:

- **Tools** — One tool per output artifact type (`produces` + `may_produce`). The tool input schema is the artifact's JSON Schema with `work_unit` removed. The server injects `work_unit` automatically.

Environment variables:
- `RUNA_WORKING_DIR` — Project directory (defaults to current directory)
- `RUNA_CONFIG` — Config file override (same as `--config` in the CLI)
- `RUST_LOG` — Tracing filter override for stderr diagnostics

`runa step` does not spawn `runa-mcp` directly. It passes the agent wrapper a `RUNA_MCP_CONFIG` JSON payload containing the resolved `runa-mcp` command location, candidate-specific arguments, and required environment so the agent runtime can launch the MCP server as its own child process.

## Build

Rust 2024 edition.

```bash
cargo build          # Debug build
cargo test --workspace
```

## Documentation

- [Commons](https://github.com/pentaxis93/commons) — Bedrock principles and architectural decision records (ADRs)
- [Interface Contract](docs/interface-contract.md) — Three primitives defining the methodology-runtime boundary
