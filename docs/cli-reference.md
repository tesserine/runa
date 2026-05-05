# CLI Reference

## Configuration

### Config Resolution

Commands that load a methodology share a common config resolution chain. The first match wins; there is no per-field merging across sources:

1. `--config <PATH>` CLI flag
2. `RUNA_CONFIG` environment variable
3. `.runa/config.toml`
4. `$XDG_CONFIG_HOME/runa/config.toml`

For `runa init`, `--config` controls where the config file is written. For all other commands, it controls where the config file is read from.

### Logging

Runtime diagnostics use `tracing` on stderr. Command output stays on stdout.

Default behavior is quiet on successful runs: `warn` and above in human-readable text. `RUST_LOG` overrides the active filter for both `runa` and `runa-mcp`. When `RUST_LOG` is unset, the filter falls back to `config.logging.filter`, then to `warn`.

```toml
[logging]
format = "json"   # "text" (default) or "json"
filter = "info"   # any tracing env-filter directive
```

`format = "json"` switches stderr events to machine-readable JSON without changing stdout command output. `RUST_LOG` controls the filter only; output format is always determined by `config.logging.format`.

### Agent Execution

Live `runa step` (without `--dry-run`) requires an `[agent].command` entry in the config. Live `runa run` uses the same config entry by default, but a single invocation may override it with `--agent-command -- <argv tokens>`:

```toml
[agent]
command = ["./examples/agent-claude-code.sh"]
```

```bash
runa run --agent-command -- ./examples/agent-claude-code.sh --dangerously-skip-permissions
```

Runa executes that command in the project root with stdout and stderr attached to the terminal, renders a natural-language execution prompt from the planned protocol context, and writes the prompt on stdin. Before each invocation, runa exports `RUNA_MCP_CONFIG` — a JSON payload containing the resolved `runa-mcp` command, arguments, and environment — so the agent wrapper can launch the MCP server as its own child process. The payload is runtime-agnostic (`{command, args, env}`); agent wrappers adapt it to their runtime's schema. The config path above is resolved from the project root at execution time. To inspect the sample wrapper in this repository, see [`examples/agent-claude-code.sh`](../examples/agent-claude-code.sh), which converts the payload to Claude's `mcpServers` format.

When `RUNA_TRANSCRIPT_DIR` is set, live execution appends JSON Lines transcript
events to `$RUNA_TRANSCRIPT_DIR/events.jsonl`. Events include protocol prompts,
agent stdout/stderr chunks, agent exit status, and `runa-mcp` tool call/result
events when the agent runtime launches the MCP server from `RUNA_MCP_CONFIG`.
`RUNA_TRANSCRIPT_REDACT_ENV` may name comma-separated environment variables;
their current non-empty values are replaced with `[REDACTED:<name>]` before
events are written. Hidden model reasoning and runtime-private provider events
are outside runa's observable boundary.

## Commands

All commands accept a global `--config <PATH>` flag to override the config file location.

### `runa init`

```bash
runa init --methodology <PATH> [--config <PATH>]
```

Initializes a runa project. Parses the methodology manifest at `<PATH>`, validates its structure and layout convention (schemas and instruction files at their conventional paths), canonicalizes the methodology path, and creates the `.runa/` project directory containing:

- `config.toml` — methodology path, optional logging and agent settings
- `state.toml` — initialization timestamp, runa version
- `store/` — internal artifact state store
- the fixed artifact workspace directory `.runa/workspace/`

Reports the artifact type and protocol counts on success.

If pre-existing `.runa/` state or the selected config destination cannot be
written by the current user, `init` fails before writing project state and
reports the path, owner UID, current UID, likely causes, and remediation. This
usually means the path is managed by another tool, was left by a sudo-created
init, or the command is running in the wrong directory.

**Flags:**

- `--methodology <PATH>` — Path to the methodology manifest file. Required.

**Exit codes:** 0 on success. 6 on parse, validation, or I/O failure.

### `runa scan`

```bash
runa scan [--config <PATH>]
```

Reconciles the artifact workspace into `.runa/store/`. Reads `*.json` files under `<workspace>/<type_name>/`, validates each against its artifact type schema, and classifies results as new, modified (content hash changed), revalidated (content unchanged but schema hash changed), or removed. Invalid and malformed artifacts are recorded in store state rather than discarded. Unreadable files become scan-gap metadata. Unrecognized top-level workspace directories are reported separately.

A missing workspace directory is an error unless the store is still empty.

**Exit codes:** 0 on successful reconciliation, even when invalid, malformed, or unreadable findings are present. 6 on project-load, store, or I/O failure.

### `runa list`

```bash
runa list [--config <PATH>]
```

Displays protocols in topological (execution) order after an implicit scan. For each protocol, shows non-empty relationship fields (requires, accepts, produces, may_produce), the trigger condition, and a `BLOCKED` indicator when required artifacts have no valid instance or when scan trust is incomplete. Invalid, malformed, or stale siblings still appear in health reporting, but they do not block a protocol that already has a valid required instance. On cycle detection, falls back to manifest order with a warning.

**Exit codes:** 0 on success. 6 on failure.

### `runa state`

```bash
runa state [--json] [--work-unit <ID>] [--config <PATH>]
```

Evaluates protocols after an implicit scan and classifies each as `READY`, `BLOCKED`, or `WAITING`. Classification is ordered and mutually exclusive: WAITING when execution cannot proceed yet, BLOCKED when the trigger is satisfied but preconditions fail, READY otherwise. `READY` means plannable under the active evaluation scope. In-scope hard-cycle participants are reported as `WAITING` with an explicit cycle condition so `state` and `step` cannot disagree.

Without `--work-unit`, `state` evaluates only unscoped protocols (`scoped = false`) and each protocol appears at most once with no `work_unit`. With `--work-unit <ID>`, `state` evaluates only scoped protocols (`scoped = true`) for that exact delegated work unit. It does not enumerate sibling work units from artifact state.

Text output groups protocols in READY, BLOCKED, then WAITING order, preserving scope-filtered topological protocol order within each group. READY entries list valid required and accepted artifact instances. BLOCKED entries list required-artifact failures (missing, invalid, stale, scan_incomplete). WAITING entries list the trigger condition and the specific reason execution cannot proceed, including explicit cycle conditions for in-scope hard-cycle participants. For `on_artifact`, those reasons are phrased in terms of the absence of valid instances rather than the presence of unhealthy siblings.

When scan reconciliation is partial, `state` surfaces scan warnings and blocks protocols whose required artifact types could not be fully reconciled. Partially scanned accepted types are omitted from reported inputs.

**Flags:**

- `--json` — Emits a versioned JSON envelope: `{ "version": 2, "methodology": "...", "scan_warnings": [...], "protocols": [...] }`. The `protocols` array is flat and ordered, with each entry containing `name`, optional `work_unit`, `status`, `trigger`, and the status-specific field `inputs`, `precondition_failures`, or `unsatisfied_conditions`.
- `--work-unit <ID>` — Evaluate only scoped protocols for the delegated work unit `<ID>`. Unscoped protocols are excluded in this mode.

**Exit codes:** 0 when evaluation succeeds, regardless of whether protocols are ready, blocked, or waiting. 6 on project-load, scan, or serialization failure.

### `runa doctor`

```bash
runa doctor [--config <PATH>]
```

Checks project health after an implicit scan. Three checks:

1. **Artifact health** — enumerates instances per artifact type and reports invalid, malformed, or stale instances with details.
2. **Protocol readiness** — checks each protocol's required artifact types and reports missing, invalid, or stale failures when no valid required instance exists.
3. **Cycle detection** — reports hard dependency cycles in the protocol graph.

**Exit codes:** 0 if no problems found. 1 if any check reports a problem. 6 on project-load, scan, or serialization failure.

### `runa step`

```bash
runa step [--dry-run] [--json] [--work-unit <ID>] [--config <PATH>]
```

Selects at most one `(protocol, work_unit)` candidate after an implicit scan: the next READY execution in scope-filtered topological order. Activated work is suppressed when valid outputs are newer than relevant inputs for that work unit. Unreadable output instances conservatively disable freshness suppression for every work unit of the affected output type.

Without `--work-unit`, `step` considers only unscoped protocols. With `--work-unit <ID>`, it considers only scoped protocols for that exact delegated work unit.

With `--dry-run`, text output prints the next execution plus the grouped READY/BLOCKED/WAITING status view. The execution entry includes the protocol name, optional work unit, the trigger that activated it, an MCP server config, and the serialized agent-facing context payload (protocol name, optional work unit, instruction content, valid required and available accepted inputs with display paths and content hashes, and expected outputs split into `produces` and `may_produce`). Dry-run does not require a discoverable `runa-mcp` binary. If the in-scope graph contains a hard dependency cycle, `step` reports it as a warning, exposes the scope-filtered cycle in JSON, and keeps those participants out of `READY`.

Without `--dry-run`, `step` requires `[agent].command` in the config and a Linux host. If the initial scan finds no READY work, `step` performs one final re-scan and re-evaluates readiness before returning a no-work outcome. If that refreshed state exposes a READY candidate, the same invocation executes that one protocol. Only when the refreshed state still has no actionable work does it print `No READY protocols.` and exit without requiring `runa-mcp`: exit `3` when work remains blocked, waiting, or trapped in a cycle, and exit `4` when no actionable work remains because outputs are already current. Otherwise, it resolves `runa-mcp` (preferring a sibling binary next to the running `runa` executable, falling back to `PATH`), executes the candidate, re-scans the workspace, enforces postconditions, and prints the refreshed status view.

**Flags:**

- `--dry-run` — Preview only. Does not execute the agent.
- `--json` — Dry-run only. Emits a versioned JSON envelope: `{ "version": 4, "methodology": "...", "scan_warnings": [...], "cycle": [...] | null, "execution_plan": [...], "protocols": [...] }`. The `execution_plan` array contains at most one entry. The `protocols` array reuses the same status entries as `runa state --json`.
- `--work-unit <ID>` — Plan or execute only scoped protocols for the delegated work unit `<ID>`.

**Exit codes** (the same `3` / `4` distinction applies to `--dry-run` when no execution plan is available):

| Code | Meaning |
|------|---------|
| 0 | `step` found a READY candidate and either previewed it or executed it successfully. |
| 2 | Usage error, currently `--json` without `--dry-run`. |
| 3 | No READY candidate and work remains blocked, waiting on prerequisites, or trapped in a cycle. |
| 4 | No READY candidate and no actionable work remains because the in-scope outputs are already current. |
| 5 | Work was attempted, but the agent failed or postconditions were not satisfied. |
| 6 | Infrastructure failure: project/config/load/scan/serialization/bootstrap/MCP lookup/runtime I/O failure prevented completion from being established. |

### `runa run`

```bash
runa run [--dry-run] [--json] [--work-unit <ID>] [--agent-command -- <argv tokens>] [--config <PATH>]
```

The cascade command. Walks the READY frontier repeatedly until quiescence instead of stopping after one execution.

Without `--work-unit`, `run` considers only unscoped protocols. With `--work-unit <ID>`, it considers only scoped protocols for that exact delegated work unit.

With `--dry-run`, projects the full optimistic cascade from the same scope-filtered execution order used by evaluation and planning, plus declared `produces` outputs, dependency edges, and the caller-supplied evaluation scope. `may_produce` outputs do not advance the projection unless they already exist on disk. Initially ready entries include MCP config and full context on first emission; downstream projected entries carry only protocol name, optional work unit, trigger, and projection kind. The projection never synthesizes artifact values, forks the store, bypasses schema validation, or discovers sibling work units from artifact state.

Without `--dry-run`, requires a Linux host plus an effective agent command. `run` resolves that command in this order: `--agent-command -- <argv tokens>` when supplied, otherwise `[agent].command` from config, otherwise the existing `AgentCommandNotConfigured` error. If `--agent-command` is present but no usable argv tokens follow the `--`, `run` fails with `AgentCommandNotConfigured` and does not fall back to config. If no READY protocol is ever dispatched, `run` exits `4` (`nothing_ready`) instead of treating that invocation as `success`. Failed candidates are skipped for the rest of the invocation; any artifacts emitted before failure are still reconciled into workspace state for downstream readiness. Previously exhausted work reopens when a later reconciliation changes relevant inputs.

**Interrupt behavior.** The first `Ctrl-C` is boundary-scoped: the current protocol run completes its scan and postcondition reconciliation. After that reconciliation, `run` exits `130` only if the interrupt prevented the next READY candidate from starting. If no further READY work remains, the quiescent topology outcome takes precedence. A second `Ctrl-C` forces immediate exit with status `130`; the isolated child process may continue running after `runa` terminates.

**Flags:**

- `--dry-run` — Preview the projected cascade. Does not execute agents.
- `--json` — Dry-run only. Same envelope structure as `runa step --json`, but `execution_plan` may contain multiple entries including projected downstream work.
- `--work-unit <ID>` — Plan or execute only scoped protocols for the delegated work unit `<ID>`.
- `--agent-command -- <argv tokens>` — Override `[agent].command` for this live `run` invocation. Pass the agent argv after `--` so hyphen-prefixed tokens are forwarded unchanged.

**Exit codes** (`4` is live-only; dry-run still reflects current topology state, not the projection):

| Code | Meaning |
|------|---------|
| 0 | Topology fully satisfied after executing at least one protocol, or dry-run sees fully satisfied topology. |
| 2 | Usage error, currently `--json` without `--dry-run`. |
| 3 | Quiescent but work remains blocked, waiting, or trapped in a cycle. |
| 4 | Nothing ready — live `run` did not dispatch any protocol because none were READY. |
| 5 | Work was attempted, but one or more protocols failed or violated postconditions during the invocation. |
| 6 | Infrastructure failure: project/config/load/scan/serialization/bootstrap/runtime failure prevented completion from being established. |
| 130 | Interrupted — `Ctrl-C` prevented the next candidate from starting. |

## MCP Server

`runa-mcp` is a single-session stdio MCP server that serves one named protocol invocation per process.

```bash
runa-mcp --protocol <name> [--work-unit <name>]
```

On startup, the server loads the project, resolves the named protocol from the manifest, validates that its declared scope matches the presence or absence of `--work-unit`, validates that its output types can be served as MCP tools, and serves an MCP session over stdio. Each output artifact type (`produces` and `may_produce`) becomes one MCP tool. The tool input schema is the artifact type's JSON Schema with the `work_unit` field removed — the server injects `work_unit` automatically from the `--work-unit` argument.

`runa step` does not spawn `runa-mcp` directly. It passes the agent wrapper a `RUNA_MCP_CONFIG` JSON payload containing the resolved `runa-mcp` command, arguments, and environment so the agent runtime can launch the server as its own child process. The exported command and environment paths are absolute whenever runa resolves them from the local filesystem, so wrappers do not depend on child process cwd to launch `runa-mcp`. Transcript environment variables are forwarded into that payload when transcript capture is enabled, which lets the MCP server append tool events to the same transcript stream as the CLI execution events.

**Environment variables:**

- `RUNA_WORKING_DIR` — Project directory. Defaults to the current directory.
- `RUNA_CONFIG` — Config file override (same as `--config` in the CLI).
- `RUST_LOG` — Tracing filter override for stderr diagnostics.
