# Changelog

All notable changes to this project will be documented in this file.

The format is based on Keep a Changelog, and this project adheres to
Semantic Versioning.

## [Unreleased]

### Changed

- Adopted the shared commons cargo-release discipline for workspace releases,
  including a root release configuration and repeatable release-adoption
  verification.
- `runa-cli` now uses the shared commons exit code convention across
  `init`, `scan`, `list`, `state`, `doctor`, `step`, and `run`.
- Breaking change: `.runa/config.toml` no longer accepts `artifacts_dir`.
  Artifact files now always live under `.runa/workspace/`, and configs that
  still declare `artifacts_dir` fail to load with an actionable error.
- Breaking change for callers parsing runa exit codes:
  code `2` no longer means `QuiescentFailures`.
  Code `2` now means `usage_error`, and the old `QuiescentFailures`
  outcome now exits as code `5` (`work_failed`).
- Migration for callers such as agentd:
  update any interpretation that treated exit code `2` as attempted-work
  failure. The new mapping is `2 => usage_error`, `5 => work_failed`.
- `runa step` now differentiates quiescent outcomes instead of collapsing
  them into success: `3` for blocked/waiting/cyclic no-ready states, `4` for
  no-actionable-work states where outputs are already current, `5` for
  attempted-work failures, and `6` for infrastructure failures.
- `runa step` now treats its final no-ready re-scan as authoritative: if new
  artifacts make a protocol READY during that refresh, the same invocation
  executes that protocol instead of incorrectly reporting `No READY protocols.`
  with exit `3` or `4`.
- `runa run` now accepts `--agent-command -- <argv tokens>` as a per-invocation
  override for `[agent].command`. When the override flag is present but no
  usable argv follows `--`, `run` keeps the existing
  `AgentCommandNotConfigured` failure instead of falling back to config.
- `runa run` now validates its effective agent command before reporting a
  quiescent `nothing_ready` outcome, so malformed or missing live command
  input is no longer masked when no protocols are READY.
- `runa init` now reports an actionable diagnostic when pre-existing `.runa/`
  state or the selected config destination is not writable, including likely
  causes and remediation instead of a raw permission error.
- Breaking change: `runa-cli` no longer carries Unix/non-Unix alternate code
  paths. Live `runa step` and `runa run` no longer provide the previous
  contract-defined non-Linux `UnsupportedPlatform` runtime rejection; non-Linux
  build and runtime outcomes are unsupported and unspecified.

## [0.1.0] — 2026-04-03

First release. Runa is a runtime that makes multi-step AI agent workflows
reliable by validating every work product against declared schemas, computing
which steps are ready to run, and delivering only validated inputs to each
agent invocation.

### Methodology model

- Methodology plugins register via TOML manifest declaring artifact types,
  protocols, and trigger conditions.
- Artifact types carry JSON Schemas; every instance is validated on scan.
- Protocols declare artifact relationships through four edges: requires,
  accepts, produces, may_produce. Execution order emerges from the dependency
  graph — it is not declared.
- Three trigger primitives — on_artifact, on_change, on_invalid — compose
  through all_of and any_of with arbitrary nesting.
- Protocol scoping: unscoped (default) or scoped with caller-supplied work
  unit via `--work-unit`.
- Directory layout convention derives paths from declared names: schemas at
  `schemas/{name}.schema.json`, instructions at
  `protocols/{name}/PROTOCOL.md`.

### CLI

- `runa init` — initialize a project from a methodology manifest.
- `runa scan` — reconcile the artifact workspace into the store; classifies
  artifacts as valid, invalid, malformed, or removed.
- `runa list` — display protocols in topological execution order with
  dependency and trigger details.
- `runa state` — evaluate and classify protocol readiness as READY, BLOCKED,
  or WAITING. Supports `--json` and `--work-unit`.
- `runa step` — execute or preview (`--dry-run`) the next ready protocol.
  Delivers a natural-language context prompt on stdin and exports
  `RUNA_MCP_CONFIG` for agent wrappers.
- `runa run` — walk the ready frontier to quiescence with tolerant
  continuation after per-protocol failures, outcome-specific exit codes
  (0, 2, 3, 130), and boundary-scoped Ctrl-C interrupt handling.
- `runa doctor` — check project health: artifact validity, protocol
  readiness, cycle detection.
- Configuration resolution chain: `--config` flag, `RUNA_CONFIG` env var,
  `.runa/config.toml`, XDG config. First match wins.

### MCP server

- `runa-mcp` — single-session stdio MCP server serving one protocol
  invocation per process.
- Each output artifact type (produces and may_produce) becomes one MCP tool
  with a schema-derived input schema.
- Validates artifact instances against the full schema before writing to the
  workspace. Invalid artifacts are rejected with validation error details.
- Scope validation: rejects scoped protocols without `--work-unit` and
  unscoped protocols with one.

### Execution model

- Agent execution via configurable `[agent].command` with a rendered
  natural-language prompt on stdin.
- `RUNA_MCP_CONFIG` exports the resolved runa-mcp command, arguments, and
  environment so agent wrappers can launch the MCP server as a child process.
- Precondition enforcement before execution; postcondition enforcement after.
  Requires-edge inputs must have at least one valid instance. Produces-edge
  outputs must all exist and validate.
- Freshness suppression skips work whose valid outputs are current, using
  execution-record input-set comparison with timestamp fallback.
- Graph-based dry-run projection for `runa run --dry-run` derives the
  optimistic cascade from manifest topology without synthesizing artifacts.
- Scope-driven evaluation: unscoped and scoped commands see only their
  respective protocols. No readiness path enumerates sibling work units from
  artifact state.

### Runtime

- Artifact store under `.runa/store/` with content hashing (canonical JSON,
  SHA-256), modification timestamps, and schema-hash tracking.
- Execution records persisted per (protocol, work_unit) pair for freshness
  decisions across invocations.
- Scan-gap awareness: unreadable files become metadata rather than command
  failures; partial scans conservatively block affected protocols.
- Rust 2024 edition. Three-crate workspace: libagent (domain logic),
  runa-cli, runa-mcp.
- Linux target for live execution. Non-Linux platforms are rejected
  explicitly before agent launch.
- Quickstart example: a two-protocol review pipeline with manifest, schemas,
  and protocol instructions.
