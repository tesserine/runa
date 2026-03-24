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

### Fixed

- Make the MCP context prompt and output tool descriptions explicitly
  instructional so agents are told to deliver required outputs by calling the
  matching tool, while tool text accurately describes validation and workspace
  writes.
- Preserve `status` and `step` readiness reporting when scans encounter
  unreadable produced artifacts by disabling freshness suppression for the
  affected output type instead of blocking protocols outright.
