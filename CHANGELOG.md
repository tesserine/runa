# Changelog

All notable changes to this project will be documented in this file.

The format is based on Keep a Changelog, and this project adheres to
Semantic Versioning.

## [Unreleased]

### Added

- The quickstart methodology README now walks the full
  scan → READY → produce loop without requiring an agent: entry artifact,
  cascade advance on artifact appearance, quiescence, and freshness-driven
  reopening — verified against the built binary.
- The `commons_exit_codes_match_specification` test now verifies the
  `ExitCode` enum against a vendored copy of the commons exit-code table
  (`runa-cli/tests/fixtures/commons-exit-codes.json`, provenance-pinned to
  commons `v0.3.0-rc.1`) instead of re-stated literals, making the
  commons-conformance claim in its name mechanically true. The enum carries
  a `canonical: commons/EXIT-CODES.md` back-reference.
- `RELEASING.md` Tooling Provenance section and a `scripts/release-check`
  provenance header: the script is runa-owned, the ceremony convention is
  canonical in commons, and no repo is the tooling upstream.

### Fixed

- Invariant-bearing modules now document their invariants at the code:
  `session.rs` opens with the session state-machine contract (state derives
  from artifacts, single current step, transactional `advance`,
  session-scoped exhaustion, promised-scope single step) referencing the
  session surface contract; `scoped_identity.rs` opens with the scope
  identity invariants referencing the interface contract; `selection.rs`
  documents the freshness/currentness machinery — input-set equality on
  the execution-record path, the timestamp fallback, and the
  `AnyRecorded`-wins mode merge.
- `ARCHITECTURE.md` documented the `step --json` envelope version as `4`;
  the code emits `5`. The doc now matches, and a workspace-contract test
  asserts code, `ARCHITECTURE.md`, and `docs/cli-reference.md` agree so the
  envelope version cannot silently drift again.

### Added

- Cold-start ticket entry: `runa run --ticket <REF>` and `runa go --ticket <REF>`
  open a scoped session from a forge ticket reference (a bare number, `#<N>`,
  `owner/repo#<N>`, a GitHub issue URL, or `sourcehut:<tracker_id>#<N>`) when no
  `work-unit` artifact exists yet. The runtime resolves the reference to a tracker
  identity — never reading ticket content — and serves the methodology's
  acquisition surface (the sole unscoped producer of the `work-unit` artifact),
  delivering the reference and `RUNA_ENTRY_TICKET` into the session. Once the
  methodology materializes the `work-unit`, the session binds and the cascade
  computes `take` next on it. A reference whose work-unit already exists degrades
  to a normal scoped session; downstream of acquisition the two are
  indistinguishable. `runa run --ticket --dry-run` projects the entry cascade and
  emits a version `3` JSON envelope with a top-level `entry` object.
  `runa-mcp --session --ticket <ref>` serves the same entry session surface.

### Changed

- Durable transcript capture settings and scoped forge identity can now live in
  `.runa/config.toml`, with `RUNA_TRANSCRIPT_*` and `RUNA_FORGE_*`
  environment variables retained as per-invocation overrides. Resolved forge
  identity is forwarded into launched agent and MCP environments.
- Scoped forge identity now uses runa-owned `RUNA_FORGE_*` environment names
  instead of Groundwork's private `GROUNDWORK_FORGE_*` namespace.
- Live agent launch is now agent-agnostic. A command named `claude` is no
  longer launched with injected `--mcp-config` / `--strict-mcp-config` flags;
  it receives the MCP session config through `RUNA_MCP_CONFIG` like every other
  runtime. Operators who relied on the old auto-injection should adapt by
  having their agent command or wrapper consume `RUNA_MCP_CONFIG`.
- Breaking migration for Claude Code adapter configs: the previously documented
  `./examples/agent-claude-code.sh` path is gone. Repoint `[agent].command` to
  `./adapters/agent-claude-code.sh`; the old `examples/` adapter path is not
  retained as a wrapper or symlink.

### Fixed

- The Codex adapter now registers runa's MCP session server under an
  invocation-scoped `mcp_servers` key, avoiding deep-merge collisions with an
  operator's existing `mcp_servers.runa` Codex config entry.

## [0.2.0-rc.1] — 2026-06-08

### Added

- Methodology manifests can declare `required_output_choices`: named output
  groups where each protocol execution must produce exactly one valid member
  artifact type. Choice members are exposed as MCP tools, included in agent
  context, enforced after live execution, and reported by `runa list`.
- Scoped `state`, `step`, `run`, and `runa-mcp` invocations now enforce
  canonical `work-unit` identity when recorded work-unit roots exist. The
  supplied `--work-unit` must exactly match a recorded `work-unit` instance id,
  and valid tracker-backed roots are checked for handle-number agreement,
  duplicate tracker identity, and active `GROUNDWORK_*` deployment agreement
  against Groundwork's released handle contract.
- Added the session surface contract documenting the mode-agnostic driver and
  agent boundary for readiness, context delivery, output recording, lifecycle
  advancement, and disposition authority.
- `runa-mcp --session --work-unit <ID>` now serves a unified scoped session
  surface with driver tools for readiness, context retrieval, and advance
  alongside the current step's output artifact tools.
- `runa go --work-unit <ID>` now advances one interactive session tick by
  launching the configured agent against the unified session MCP surface.

### Fixed

- `runa go` now uses the session surface's successful `advance` result as its
  advancement authority instead of comparing execution-record snapshots, so
  regenerating a deleted output with unchanged inputs no longer reports a false
  no-advance failure.
- Required output choice freshness and dry-run projection now stay conservative
  when choice-member scans are incomplete, while still projecting downstream
  cascades through an already-present exactly-one choice member.
- Session-mode driver verbs now emit the advertised MCP
  `notifications/tools/list_changed` notification when they move to a different
  current step, so caching clients can rediscover the new step's output tools.
- Session-mode execution records now preserve the input provenance delivered by
  `next-protocol-context` even when inputs change before `advance`.
- Session-mode `advance` now reopens exhausted work when relevant inputs change
  and does not persist a completed step's execution record until the next step
  has been selected and validated.
- Session-mode `readiness` now selects a current step when blocked scoped work
  later becomes ready, refuses unservable selected steps, and `advance` now
  rejects completion if the current step's trigger or required inputs are no
  longer ready.
- Session-mode `next-protocol-context` now revalidates the current step's
  readiness before serving context, refusing stale current steps whose trigger
  or required inputs are no longer ready.
- Session-mode rescans now revalidate the scoped work-unit identity before
  readiness, context, or advance can evaluate or act on newly discovered
  artifacts.
- Session-mode MCP driver calls now append transcript tool events, including
  failed driver results, when `RUNA_TRANSCRIPT_DIR` is set.
- Fixed-protocol `runa-mcp --protocol` servers now keep output tools whose
  artifact type names match session driver verbs such as `advance`; those names
  are reserved only on the session surface.
- Choice-only protocols with unsupported optional outputs now start `runa-mcp`
  correctly instead of being rejected as optional-output-only sessions.

## [0.1.2] — 2026-05-17

### Changed

- Added release ceremony tooling for runa, including `scripts/release-check`,
  stable and RC release verification, GitHub Release workflows, and
  operator-facing release documentation.
- Live `runa step`, `runa run`, and `runa-mcp` can now emit structured
  transcript events when `RUNA_TRANSCRIPT_DIR` is set, capturing protocol
  prompts, agent stdout/stderr, agent exit status, and MCP tool calls/results
  for orchestrators such as agentd to persist.
- Adopted the shared commons cargo-release discipline for workspace releases,
  including a root release configuration and repeatable release-adoption
  verification.
- Hardened the cargo-release configuration so the no-op pre-release hook is
  PATH-resolved and protected against hostile user-level cargo-release config.
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

### Fixed

- Direct Claude Code live agent commands now receive runa's generated
  per-protocol MCP configuration automatically, so `runa-mcp` artifact tools
  are available without requiring an operator-supplied wrapper.
- Live `runa step` and `runa run` preserve inherited agent stdout/stderr when
  `RUNA_TRANSCRIPT_DIR` is unset, keeping transcript capture opt-in and
  retaining the default attached-terminal behavior.
- Release publication now delegates `v*` tag filtering to `release-check`,
  restores annotated tag refs after checkout, and verifies the restored tag
  still matches the triggering event commit before tag-trust checks. Refs
  tesserine/commons#34.

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
