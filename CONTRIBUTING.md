# Contributing

## Coherence on landing

Every PR that ships must update affected documentation:
- CLI changes, build commands → README.md
- Module, data flow, disk layout, or design pattern changes → ARCHITECTURE.md

## Conventions

- Conventional commits (e.g., `feat(trigger):`, `fix(store):`, `docs:`)
- Branch names: `issue-N/brief-description`
- One issue per PR
- `cargo fmt` and `cargo clippy` clean before merge
