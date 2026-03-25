# Changelog

All notable changes to this project will be documented in this file.

The format is based on Keep a Changelog, and this project adheres to
Semantic Versioning.

## [Unreleased]

### Changed

- Define methodology layout standard in the interface contract. Schema content
  is derived from `schemas/{artifact_type_name}.schema.json` and protocol
  instruction file existence is validated at `protocols/{protocol_name}/PROTOCOL.md`,
  both relative to the manifest directory. The manifest TOML no longer includes
  explicit `schema` fields on artifact type declarations.
- Preload protocol instruction content during manifest parsing and include it in
  the shared context payload used by `runa step --dry-run`, so agents receive
  the exact self-contained instructions that real execution uses.
- Allow `runa step` to execute a configured `[agent].command` by sending each
  planned protocol as a rendered natural-language prompt on stdin, while
  keeping `--dry-run` as the exact execution preview surface.
- Pass candidate-specific `runa-mcp` launch configuration through
  `RUNA_MCP_CONFIG` for both dry-run inspection and live `step` execution, so
  agent wrappers can attach the advertised MCP tools to the same prompt-driven
  workflow.
- Simplify `runa-mcp` into a pure tool server with required `--protocol` and
  optional `--work-unit` arguments, removing workspace scanning, candidate
  selection, and shutdown postcondition checks from the MCP process.

### Fixed

- Preserve valid Unix artifact filenames containing non-UTF8 bytes across
  `.runa/store` persistence and live `runa step` prompt rendering by keeping
  exact paths internally, storing a byte-preserving encoded path on disk, and
  exposing only display-only `display_path` strings in the dry-run context
  payload.
- Wrap the documented Claude example wrapper's `--mcp-config` file in Claude's
  required `mcpServers.runa` schema and export absolute resolved MCP command
  and config paths from `runa step`, so live agent execution no longer depends
  on the wrapper's working directory.
- Move prompt rendering into libagent so live `runa step` writes the agent's
  prose prompt on stdin while `runa-mcp` advertises tool capabilities only.
- Preserve `status` and `step` readiness reporting when scans encounter
  unreadable produced artifacts by disabling freshness suppression for the
  affected output type instead of blocking protocols outright.
- Restore `runa step` MCP discovery so `--dry-run` remains a planning-only
  preview when `runa-mcp` is absent, while live execution prefers a sibling
  `runa-mcp` binary and falls back to `PATH` for split-install layouts.
