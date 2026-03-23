# Changelog

All notable changes to this project will be documented in this file.

The format is based on Keep a Changelog, and this project adheres to
Semantic Versioning.

## [Unreleased]

### Fixed

- Preserve `status` and `step` readiness reporting when scans encounter
  unreadable produced artifacts by disabling freshness suppression for the
  affected output type instead of blocking protocols outright.
