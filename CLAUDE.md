# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Methodology

**At the start of every session, invoke the `using-groundwork` skill (`/using-groundwork`).** This is the core development methodology for this project. It activates the full skill system (BDD, test-first, systematic-debugging, plan, begin/propose/land, etc.) as one connected methodology rather than isolated tools.
## Project

Runa is a cognitive runtime for AI agents, written in Rust. It enforces contracts between methodologies and the runtime through three primitives: artifact types (JSON Schema-validated work products), skill declarations (relationships to artifacts via requires/accepts/produces/may_produce edges), and trigger conditions (composable activation rules).

## Build Commands

```bash
cargo build                        # Debug build
cargo test --lib                   # Run all unit tests
cargo test --lib <test_name>       # Run a single test
cargo run --bin runa -- --version  # Run CLI
```

## Architecture

**Workspace crates:**
- `libagent` — Core library: data model, TOML manifest parsing, JSON Schema validation, dependency graph, artifact state tracking
- `runa-cli` — CLI binary (minimal, depends on libagent)

**libagent modules:**
- `model.rs` — Core types: `Manifest`, `ArtifactType`, `SkillDeclaration`, `TriggerCondition`
- `manifest.rs` — TOML parsing with validation (uniqueness checks at parse time)
- `validation.rs` — JSON Schema validation for artifact instances, collects all violations before returning
- `graph.rs` — Dependency graph from skill declarations: topological ordering, cycle detection, blocked-skill identification
- `store.rs` — Artifact state tracking: validation status, content hashing, JSON persistence in `.runa/artifacts/`

**Key design:**
- `TriggerCondition` uses tagged enum serialization (`#[serde(tag = "type")]`) with `all_of`/`any_of` composition
- Custom error types with `std::error::Error` source chains
- All tests are inline `#[cfg(test)]` modules within each source file

## Design Principles

Decisions trace to principles in `docs/PRINCIPLES.md` and ADRs in `docs/adr/`. Key constraints:

- **Sovereignty** — Clean ownership boundaries; runtime enforces contracts but never interprets domain semantics
- **Everything earns its place** — No speculative abstractions, backward-compat shims, or tech debt. Every element traces to a current need or gets removed
- **Verifiable completion** — Mechanically verifiable criteria only; no subjective "done"
- **Unconditional responsibility** — Problems are fixed or queued, never deferred silently

## Dependencies

Rust 2024 edition, resolver v3. Minimal dependency set: serde, serde_json, toml, jsonschema, sha2. No async/network dependencies.
