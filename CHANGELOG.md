# Changelog

All notable changes to this project will be documented in this file.

The format is based on Keep a Changelog, and this project adheres to
Semantic Versioning.

## [Unreleased]

### Fixed

- Make the MCP context prompt and output tool descriptions explicitly
  instructional so agents are told to deliver required outputs by calling the
  matching tool, while tool text accurately describes validation and workspace
  writes.
- Preserve `status` and `step` readiness reporting when scans encounter
  unreadable produced artifacts by disabling freshness suppression for the
  affected output type instead of blocking protocols outright.
