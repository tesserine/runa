# AGENTS.md

**At the start of every session, invoke the `orient` skill (`/orient`).**

Study the following before working in this project:

Orientation: `README.md`
Architecture: `ARCHITECTURE.md`
Contribution conventions: `CONTRIBUTING.md`
Bedrock principles: [commons](https://github.com/pentaxis93/commons) — read both `PRINCIPLES.md` and every ADR in `adr/`

This project does not vendor agent skills in-repo. Resolve project skills from
your global installs under `~/.claude/skills` and `~/.codex/skills`.

**CLAUDE.md and AGENTS.md are the same file** — CLAUDE.md is a symlink to AGENTS.md. Edit AGENTS.md. Never break the symlink.
