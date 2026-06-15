# CLI Reference

## Configuration

### Config Resolution

Commands that load a methodology share a common config resolution chain. The first match wins; there is no per-field merging across sources:

1. `--config <PATH>` CLI flag
2. `RUNA_CONFIG` environment variable
3. `.runa/config.toml`
4. `$XDG_CONFIG_HOME/runa/config.toml`

For `runa init`, `--config` controls where the config file is written. For all other commands, it controls where the config file is read from.

### Durable Project Settings

Durable project settings live in `.runa/config.toml`. Environment variables
with matching runtime meaning remain per-invocation overrides.

```toml
[runtime]
command = ["./adapters/agent-codex.sh", "--model", "gpt-5-codex"]

[transcript]
dir = "transcripts"
redact_env = ["SECRET_TOKEN", "API_KEY"]
```

Portable forge topology lives in `.runa/project.toml`:

```toml
[[forge.instances]]
name = "github"
type = "github"
host = "github.com"

[[forge.repositories]]
name = "runa"
instance = "github"
owner = "tesserine"
repository = "runa"

[[forge.trackers]]
name = "runa"
type = "github"
instance = "github"
repository = "runa"
```

`[transcript].dir` enables transcript capture and is resolved relative to the
project directory when it is not absolute. `RUNA_TRANSCRIPT_DIR` overrides it
for one invocation. `[transcript].redact_env` names environment variables whose
current values should be redacted from transcript events.
`RUNA_TRANSCRIPT_REDACT_ENV`, when set to a comma-separated list, overrides the
configured list.

`.runa/project.toml` supplies the active scoped work-unit deployment identity.
Runa validates the topology against the forge-address contract, uses the
derived identities for tracker-backed `work-unit` roots, and injects the
validated `RUNA_FORGE_ADDRESSES` JSON payload into launched agent and MCP
environments.

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

Live `runa step`, `runa go`, and `runa run` require a `[runtime].command` entry
in the config:

```toml
[runtime]
command = ["./adapters/agent-codex.sh", "--model", "gpt-5-codex"]
```

```toml
[runtime]
command = ["./adapters/agent-claude-code.sh", "-p", "--dangerously-skip-permissions"]
```

Runa executes the configured argv unmodified in the project root with stdout
and stderr attached to the terminal, renders a natural-language execution
prompt from the planned protocol context, and writes the prompt on stdin. Before
each invocation, runa exports `RUNA_MCP_CONFIG` — a runtime-agnostic JSON
payload containing the resolved `runa-mcp` command, arguments, and environment
— so the configured runtime or adapter can launch the MCP server as its own
child process. Runtime-specific translation, such as wrapping that payload in a
client-specific config file, belongs to the runtime or adapter, not to runa.

The supported runtime adapters live in `adapters/`: `agent-codex.sh` for Codex
and `agent-claude-code.sh` for Claude Code. Point `[runtime].command` at one of
those scripts and pass runtime-specific options after the script path. The
Codex adapter requires `jq` because Codex accepts external MCP servers through
`-c mcp_servers.<name>.*` TOML overrides rather than a JSON config file. It
registers each invocation under a process-scoped server name so an existing
operator-defined `mcp_servers.runa` entry is left untouched.
Migration note: older documentation used
`./examples/agent-claude-code.sh` for Claude Code. That path no longer exists;
repoint existing `[runtime].command` values to
`./adapters/agent-claude-code.sh`.

When transcript capture is enabled through `[transcript].dir` or
`RUNA_TRANSCRIPT_DIR`, the configured path is the transcript root. Live
execution appends JSON Lines transcript events beneath that root, separated by
deployment, work unit, and run:

```text
<root>/deployments/<deployment>/work-units/<work-unit-or-_unscoped>/runs/<run-id>/events.jsonl
```

The deployment is derived from runa's resolved forge identity when configured
and otherwise from the project path. Events also carry `deployment`, `run_id`,
and, when scoped, `work_unit` fields so a copied event file remains
attributable without its parent directories. Events include protocol prompts,
agent stdout/stderr chunks, agent exit status, and `runa-mcp` tool call/result
events when the agent runtime launches the configured MCP server. Configured or
environment-supplied redaction names cause their current non-empty values to be
replaced with `[REDACTED:<name>]` before events are written. Hidden model
reasoning and runtime-private provider events are outside runa's observable
boundary.

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

Displays protocols in topological (execution) order after an implicit scan. For each protocol, shows non-empty relationship fields (requires, accepts, produces, may_produce, required_output_choice), the trigger condition, and a `BLOCKED` indicator when required artifacts have no valid instance or when scan trust is incomplete. Invalid, malformed, or stale siblings still appear in health reporting, but they do not block a protocol that already has a valid required instance. On cycle detection, falls back to manifest order with a warning.

**Exit codes:** 0 on success. 6 on failure.

### `runa state`

```bash
runa state [--json] [--work-unit <ID>] [--config <PATH>]
```

Evaluates protocols after an implicit scan and classifies each as `READY`, `BLOCKED`, or `WAITING`. Classification is ordered and mutually exclusive: WAITING when execution cannot proceed yet, BLOCKED when the trigger is satisfied but preconditions fail, READY otherwise. `READY` means plannable under the active evaluation scope. In-scope hard-cycle participants are reported as `WAITING` with an explicit cycle condition so `state` and `step` cannot disagree.

Without `--work-unit`, `state` evaluates only unscoped protocols (`scoped = false`) and each protocol appears at most once with no `work_unit`. With `--work-unit <ID>`, `state` evaluates only scoped protocols (`scoped = true`) for that exact delegated work unit. It does not enumerate sibling work units from artifact state.

If any `work-unit` artifacts are recorded, `<ID>` must exactly equal one recorded
`work-unit` instance id. Non-exact values fail before readiness evaluation and
the error names the supplied value plus the available canonical ids. Invalid or
malformed recorded `work-unit` artifacts still establish canonical ids, but
tracker-handle consistency checks run only for valid, parseable roots. A
methodology with no recorded `work-unit` artifacts has no canonical ids to
enforce, so scoped behavior remains unchanged.

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

When recorded `work-unit` artifacts exist, `<ID>` must be the exact canonical
`work-unit` instance id, with the same rejection behavior described for
`runa state`.

With `--dry-run`, text output prints the next execution plus the grouped READY/BLOCKED/WAITING status view. The execution entry includes the protocol name, optional work unit, the trigger that activated it, an MCP server config, and the serialized agent-facing context payload (protocol name, optional work unit, instruction content, valid required and available accepted inputs with display paths and content hashes, and expected outputs split into `produces`, `may_produce`, and `required_output_choices`). Dry-run does not require a discoverable `runa-mcp` binary. If the in-scope graph contains a hard dependency cycle, `step` reports it as a warning, exposes the scope-filtered cycle in JSON, and keeps those participants out of `READY`.

Without `--dry-run`, `step` requires `[runtime].command` in the config and a Linux host. If the initial scan finds no READY work, `step` performs one final re-scan and re-evaluates readiness before returning a no-work outcome. If that refreshed state exposes a READY candidate, the same invocation executes that one protocol. Only when the refreshed state still has no actionable work does it print `No READY protocols.` and exit without requiring `runa-mcp`: exit `3` when work remains blocked, waiting, or trapped in a cycle, and exit `4` when no actionable work remains because outputs are already current. Otherwise, it resolves `runa-mcp` (preferring a sibling binary next to the running `runa` executable, falling back to `PATH`), exports the candidate MCP launch config through `RUNA_MCP_CONFIG`, launches the configured agent argv unmodified, re-scans the workspace, enforces postconditions, and prints the refreshed status view.

**Flags:**

- `--dry-run` — Preview only. Does not execute the agent.
- `--json` — Dry-run only. Emits a versioned JSON envelope: `{ "version": 5, "methodology": "...", "scan_warnings": [...], "cycle": [...] | null, "execution_plan": [...], "protocols": [...] }`. The `execution_plan` array contains at most one entry. The `protocols` array reuses the same status entries as `runa state --json`.
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
runa run [--dry-run] [--json] [--work-unit <ID> | --ticket <REF>] [--config <PATH>]
```

The cascade command. Walks the READY frontier repeatedly until quiescence instead of stopping after one execution.

Without `--work-unit`, `run` considers only unscoped protocols. With `--work-unit <ID>`, it considers only scoped protocols for that exact delegated work unit.

With `--ticket <REF>` (mutually exclusive with `--work-unit`), `run` opens a
cold-start session from a forge ticket reference. Accepted forms are a bare
number, `#<N>`, `owner/repo#<N>`, a GitHub issue URL, or
`sourcehut:<tracker_id>#<N>`; the asserted deployment must match the active
project topology, and a bare reference inherits it. When the referenced
work-unit already exists, `run` behaves as `--work-unit <id>` on it. Otherwise
the methodology's acquisition surface (the sole unscoped producer of the
`work-unit` artifact) runs first — under `--dry-run` it is projected as the
`current, entry` step with the acquired work-unit's `take` projected after it;
live, it executes (the runtime delivers the reference and `RUNA_ENTRY_TICKET`,
never the ticket's content), and the cascade then continues scoped on the
materialized work-unit. The cold-start dry-run JSON envelope is version `3` and
adds a top-level `entry` object (`reference`, `ticket_number`,
`acquisition_protocol`, `resolved_work_unit`).

When recorded `work-unit` artifacts exist, `<ID>` must be the exact canonical
`work-unit` instance id, with the same rejection behavior described for
`runa state`.

With `--dry-run`, projects the full optimistic cascade from the same scope-filtered execution order used by evaluation and planning, plus declared `produces` outputs, dependency edges, and the caller-supplied evaluation scope. `may_produce` outputs do not advance the projection unless they already exist on disk. Required output choice members are branch-dependent, so projection uses an already-present single member when one exists and otherwise does not synthesize a choice branch. Initially ready entries include MCP config and full context on first emission; downstream projected entries carry only protocol name, optional work unit, trigger, and projection kind. The projection never synthesizes artifact values, forks the store, bypasses schema validation, or discovers sibling work units from artifact state.

Without `--dry-run`, requires a Linux host plus `[runtime].command` in config. If no READY protocol is ever dispatched, `run` exits `4` (`nothing_ready`) instead of treating that invocation as `success`. Failed candidates are skipped for the rest of the invocation; any artifacts emitted before failure are still reconciled into workspace state for downstream readiness. Previously exhausted work reopens when a later reconciliation changes relevant inputs.

**Interrupt behavior.** The first `Ctrl-C` is boundary-scoped: the current protocol run completes its scan and postcondition reconciliation. After that reconciliation, `run` exits `130` only if the interrupt prevented the next READY candidate from starting. If no further READY work remains, the quiescent topology outcome takes precedence. A second `Ctrl-C` forces immediate exit with status `130`; the isolated child process may continue running after `runa` terminates.

**Flags:**

- `--dry-run` — Preview the projected cascade. Does not execute agents.
- `--json` — Dry-run only. Emits version `2` and otherwise uses the same envelope structure as `runa step --json`, but `execution_plan` may contain multiple entries including projected downstream work.
- `--work-unit <ID>` — Plan or execute only scoped protocols for the delegated work unit `<ID>`.
- `--ticket <REF>` — Open a cold-start session from a forge ticket reference. Mutually exclusive with `--work-unit`. An unparseable reference or one that disagrees with the active deployment exits `2`; a manifest with no single unscoped `work-unit` producer exits `6`; acquisition that produces no matching work-unit exits `5`.

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

### `runa go`

```bash
runa go (--work-unit <ID> | --ticket <REF>) [--config <PATH>]
```

Advances one scoped interactive session tick. `go` evaluates the delegated work
unit, launches the configured agent with a `runa-mcp --session --work-unit
<ID>` MCP config, and sends only a generic one-tick instruction on stdin. The
agent retrieves the current protocol context through `next-protocol-context`,
records outputs through the current output tools, calls `advance`, and stops.

With `--ticket <REF>` (mutually exclusive with `--work-unit`; same reference
grammar as `runa run --ticket`), `go` opens a cold-start session. When the
referenced work-unit already exists, the tick proceeds as a normal bound session
on it. Otherwise this tick is the acquisition step: `go` launches a
`runa-mcp --session --ticket <REF>` config, the agent materializes the
work-unit, the session binds, and `go` reports the acquired work-unit with `take`
ready. Mode changes only who issues the verb at what cadence, never its meaning.

`go` does not expose readiness, context, record, or approval as operator
commands. Those remain the session surface mechanics inside `runa-mcp`; the
operator-facing command is the single tick.

If no scoped protocol is READY, `go` prints `No READY protocols.` and exits
with the same no-work distinction as `step`: `3` when work remains blocked or
waiting, `4` when no actionable work remains because outputs are current. If the
agent exits successfully but does not advance the selected session step, `go`
fails with exit `5`.

**Flags:**

- `--work-unit <ID>` — Advance only scoped protocols for the delegated work unit
  `<ID>`. Required unless `--ticket` is given.
- `--ticket <REF>` — Open the tick from a forge ticket reference (cold-start
  entry). Mutually exclusive with `--work-unit`.

**Exit codes:**

| Code | Meaning |
|------|---------|
| 0 | The configured agent advanced one session step. |
| 2 | Usage error, or a ticket reference that is unparseable or disagrees with the active deployment. |
| 3 | No READY candidate and work remains blocked, waiting on prerequisites, or trapped in a cycle. |
| 4 | No READY candidate and no actionable work remains because the in-scope outputs are already current. |
| 5 | Work was attempted, but the agent failed or did not advance the selected session step. |
| 6 | Infrastructure failure: project/config/load/scan/serialization/bootstrap/MCP lookup/runtime I/O failure prevented completion from being established. |

## MCP Server

`runa-mcp` is a stdio MCP server with two modes.

```bash
runa-mcp --protocol <name> [--work-unit <name>]
runa-mcp --session (--work-unit <name> | --ticket <ref>)
```

In fixed-protocol mode, the server loads the project, scans the workspace, resolves the named protocol from the manifest, validates that its declared scope matches the presence or absence of `--work-unit`, validates canonical `work-unit` identity for scoped sessions, validates that its required output types can be served as MCP tools, and serves that protocol's output tools over stdio. Each output artifact type (`produces`, required output choice members, and viable `may_produce`) becomes one MCP tool. The tool input schema is the artifact type's JSON Schema with the `work_unit` field removed — the server injects `work_unit` automatically from the `--work-unit` argument.

In session mode, the server serves one scoped work-unit session in a single MCP connection, opened either with `--work-unit <name>` (a recorded work-unit) or `--ticket <ref>` (a forge ticket reference for cold-start entry). With `--ticket`, the session begins in a promised scope serving the methodology's acquisition surface; once the agent materializes the `work-unit`, `advance` binds the session to it and the tool list flips to the bound step's tools. The runtime resolves the reference to an identity only and performs no forge read. The tool list always includes the driver tools `readiness`, `next-protocol-context`, and `advance`, plus the output tools for the current ready step. A current step is refused if any declared output type for that step would collide with one of those reserved driver tool names. Every driver verb rescans and revalidates the scoped work-unit identity before reporting, serving, or advancing. `readiness` reports the same status classification as `runa state` for the session scope, and selects the first non-exhausted ready step when the session has no current step. `next-protocol-context` verifies that the current step still satisfies readiness authority for its trigger and preconditions, and returns both the structured context and rendered prompt for the current step without advancing it. `advance` verifies that the current step still satisfies readiness authority for its trigger and preconditions, enforces postconditions for the current step, uses staged execution metadata to select and validate the next ready step, and only then persists that metadata and advances the session. Any driver verb that changes the current step emits `notifications/tools/list_changed` so caching MCP clients can rediscover the current step's output tools. Output tools validate and write artifacts exactly as in fixed-protocol mode; recording an output does not advance the session.

`runa step` currently continues to use fixed-protocol mode. `runa go` uses
session mode. Neither command spawns `runa-mcp` directly. Before launching the
configured agent, runa exports the resolved `runa-mcp` command, arguments, and
environment as a `RUNA_MCP_CONFIG` JSON payload so the runtime or adapter can
launch the server as its own child process. Runa does not inspect the agent
binary name, inject runtime-specific flags, or write runtime-specific config
files. The exported command and environment paths are absolute whenever runa
resolves them from the local filesystem, so adapters do not depend on child
process cwd to launch `runa-mcp`. Transcript environment variables are forwarded
into the MCP config when transcript capture is enabled, which lets the MCP
server append tool events under the same deployment/work-unit/run transcript
path as the CLI execution events. Configured forge identity is forwarded the same way through
`RUNA_FORGE_*` entries so tooling inside the agent session can
use the project-local identity without user-global shell state.

**Environment variables:**

- `RUNA_WORKING_DIR` — Project directory. Defaults to the current directory.
- `RUNA_CONFIG` — Config file override (same as `--config` in the CLI).
- `RUNA_TRANSCRIPT_DIR` — Transcript directory override for one invocation.
- `RUNA_TRANSCRIPT_REDACT_ENV` — Comma-separated transcript redaction-name
  override for one invocation.
- `RUNA_TRANSCRIPT_DEPLOYMENT`, `RUNA_TRANSCRIPT_RUN_ID` — Internal transcript
  attribution values propagated by runa into child MCP servers.
- `RUNA_FORGE_ADDRESSES` — Validated forge-address payload injected by runa
  when `.runa/project.toml` is present.
- `RUST_LOG` — Tracing filter override for stderr diagnostics.
