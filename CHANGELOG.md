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
- Simplify `runa-mcp` into a pure tool server with required `--protocol` and
  optional `--work-unit` arguments, removing workspace scanning, candidate
  selection, and shutdown postcondition checks from the MCP process.

### Fixed

- Move prompt rendering into libagent so live `runa step` writes the agent's
  prose prompt on stdin while `runa-mcp` advertises tool capabilities only.
- Preserve `status` and `step` readiness reporting when scans encounter
  unreadable produced artifacts by disabling freshness suppression for the
  affected output type instead of blocking protocols outright.
