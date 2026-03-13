# AGENTS.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository. `CLAUDE.md` is a symlink to this file.

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
- `libagent` — Core library: data model, TOML manifest parsing, JSON Schema validation, dependency graph, artifact state tracking, trigger condition evaluation, pre/post-execution enforcement
- `runa-cli` — CLI binary (minimal, depends on libagent). Commands in `commands/`, shared project loading in `project.rs`

**libagent modules:**
- `model.rs` — Core types: `Manifest`, `ArtifactType`, `SkillDeclaration`, `TriggerCondition`
- `manifest.rs` — TOML parsing with validation (uniqueness checks at parse time)
- `validation.rs` — JSON Schema validation for artifact instances, collects all violations before returning
- `graph.rs` — Dependency graph from skill declarations: topological ordering, cycle detection, blocked-skill identification
- `store.rs` — Artifact state tracking: validation status, content hashing, JSON persistence in `.runa/store/`
- `scan.rs` — Workspace reconciliation: walk `artifacts_dir`, classify new/modified/removed instances, record invalid and malformed artifacts in store state
- `trigger.rs` — Trigger condition evaluation: recursive evaluator, six condition variants, pure function against TriggerContext
- `enforcement.rs` — Pre/post-execution enforcement: `enforce_preconditions` checks `requires`, `enforce_postconditions` checks `produces`/`may_produce`, three failure variants (Missing, Invalid, Stale)

**runa-cli modules:**
- `project.rs` — `Config` and `State` structs, config resolution chain (`--config` / `RUNA_CONFIG` / `.runa/config.toml` / XDG), `load()` function: resolves workspace and store paths, reads state, parses manifest, builds graph, opens store
- `commands/init.rs` — `runa init`: parse manifest, create `.runa/config.toml`, `.runa/state.toml`, `.runa/store/`, and the artifact workspace
- `commands/list.rs` — `runa list`: display skills in execution order with dependencies and blocked status
- `commands/doctor.rs` — `runa doctor`: check artifact health, skill readiness, cycle detection; exit 1 on problems
- `commands/scan.rs` — `runa scan`: reconcile the artifact workspace into the internal store and report findings

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

## Development Discipline

**Ground before designing.** Define the need before reading the code. What must this change enable, and for whom?

**BDD first.** Behavioral spec → test → implementation → verification. Tests describe what a system should do, not how it does it.

**Coherence on landing.** Every PR that ships must update affected documentation:
- CLI changes → README.md
- Module, data flow, or disk layout changes → ARCHITECTURE.md
- Module list, build commands, or pattern changes → CLAUDE.md Architecture section

**Conventions:**
- Conventional commits (e.g., `feat(trigger):`, `fix(store):`, `docs:`)
- Branch names: `issue-N/brief-description`
- One issue per PR
- `cargo fmt` and `cargo clippy` clean before merge
