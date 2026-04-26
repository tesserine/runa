# Contributing

## Development setup

Runa requires the Rust 2024 edition toolchain. No external services or
databases are needed.

```bash
cargo build
cargo test --workspace
cargo fmt --check
cargo clippy
```

Use a Linux environment for development and verification. Runa targets Linux,
and live `runa step` and live `runa run` are Linux-only.

## Architecture constraint

The workspace has three crates:

- **libagent** — all domain logic: manifest parsing, validation, dependency
  graph, artifact state, trigger evaluation, context injection, enforcement,
  protocol selection.
- **runa-cli** — thin CLI binary. Argument parsing and output formatting only.
  Delegates to libagent.
- **runa-mcp** — thin MCP server binary. Serves one protocol invocation per
  process. Delegates to libagent.

New domain logic goes in libagent. The CLI and MCP server must not contain
domain logic — this keeps behavior testable and reasoned about in one place.
See [ARCHITECTURE.md](ARCHITECTURE.md) for module detail.

## Conventions

- Conventional commits (e.g., `feat(trigger):`, `fix(store):`, `docs:`)
- Branch names: `issue-N/brief-description`
- One issue per PR
- `cargo fmt` and `cargo clippy` clean before merge
- Agent skills are a user-level prerequisite, not a repo-local asset. Install
  and maintain them under `~/.claude/skills` and `~/.codex/skills`.

## Pull request checklist

- [ ] Tests pass: `cargo test --workspace`
- [ ] Formatted: `cargo fmt --check`
- [ ] Lint-clean: `cargo clippy`
- [ ] One issue per PR
- [ ] Documentation updated for any user-visible change (see below)

## Documentation coherence

Documentation updates ship in the same PR as the changes that caused them.

- CLI changes, build commands, configuration changes → [README.md](README.md)
- Module, data flow, disk layout, or design pattern changes → [ARCHITECTURE.md](ARCHITECTURE.md)
- Significant architectural decisions → new ADR
- User-visible behavior changes → [CHANGELOG.md](CHANGELOG.md)
