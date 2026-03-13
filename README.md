# runa

Runa is an event-driven cognitive runtime for AI agents. It enforces contracts between methodologies and the runtime through three primitives: **artifact types** (JSON Schema-validated work products), **skill declarations** (relationships to artifacts via requires/accepts/produces/may_produce edges), and **trigger conditions** (composable activation rules).

## Architecture

Runa is a runtime layer between an orchestrating daemon and methodology plugins. Methodologies register via TOML manifests declaring their artifact types, skills, and triggers. Runa computes the dependency graph, validates artifacts against their schemas, tracks state, and evaluates trigger conditions.

See [ARCHITECTURE.md](ARCHITECTURE.md) for workspace structure, data flow, module descriptions, and disk layout.

## Usage

```bash
runa init --methodology path/to/manifest.toml
```

Parses the methodology manifest, validates its structure, and creates a `.runa/` directory with `state.toml` recording the canonical methodology path and name. Reports the artifact type and skill counts on success.

```bash
runa list
```

Displays skills in execution order with their artifact relationships, trigger conditions, and blocked status.

```bash
runa doctor
```

Checks project health: artifact validity, skill readiness, and dependency cycles. Exits 0 if healthy, 1 if problems found.

## Build

Rust 2024 edition.

```bash
cargo build          # Debug build
cargo test --lib     # Run all unit tests
```

## Documentation

- [PRINCIPLES.md](docs/PRINCIPLES.md) — Seven bedrock principles governing runtime and boundary decisions
- [Interface Contract](docs/interface-contract.md) — Three primitives defining the methodology-runtime boundary
- [ADRs](docs/adr/) — Architectural decision records
