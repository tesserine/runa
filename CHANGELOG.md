# Changelog

All notable changes to this project will be documented in this file.

The format is based on Keep a Changelog, and this project adheres to
Semantic Versioning.

## [Unreleased]

### Changed

- `runa run` now exits `4` (`nothing_ready`) for live invocations that
  dispatch no protocols because none are READY. Exit `0` remains reserved for
  invocations that execute at least one protocol and then finish fully
  complete. `runa run --dry-run` is unchanged.

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
