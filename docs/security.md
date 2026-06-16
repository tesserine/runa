# Security and Safety Surface

Index of runa's security-relevant guarantees: what each guarantees, where
it is enforced, and where it is specified. This document adds no new
rules — each linked location is authoritative for its own guarantee.

| Guarantee | Enforced in | Specified in |
| --- | --- | --- |
| **Path safety by construction.** Artifact-type and protocol names must be single path components (no `/`, `\`, `..`); runa derives every schema/instruction/workspace path from validated names, so a methodology cannot point runa outside the project tree. | `libagent/src/manifest.rs` (name validation at parse time) | [interface-contract.md](interface-contract.md) (artifact-type and protocol naming rules) |
| **Validate before write.** Artifacts delivered through `runa-mcp` output tools are validated against the full schema before being written; invalid artifacts are rejected with details and never reach disk. | `runa-mcp` output tools → libagent validation path | [AGENTS.md](../AGENTS.md) § Artifact production contract |
| **Atomic, canonical persistence.** Store state is persisted with atomic writes; content hashing uses canonical JSON (sorted keys), so hashes are deterministic and partial writes are never observed. | `libagent/src/store.rs` (module docs) | [ARCHITECTURE.md](../ARCHITECTURE.md) § Key Design Patterns |
| **Ticket-content blindness.** A `--ticket` reference is resolved to a tracker *identity* only; runa performs no forge read, and a reference asserting a foreign deployment is rejected. Forge reads belong to the methodology's mechanics under its own credentials. | `libagent/src/entry.rs` | [interface-contract.md](interface-contract.md) § Entry References |
| **Connector credential hygiene.** Forge connector config may name a credential source, but resolved token values stay in process memory and are not serialized into config, MCP launch payloads, transcripts, or fixtures. | connector credential resolution in `runa-connector-github` and `runa-connector-sourcehut` | [cli-reference.md](cli-reference.md) § Configuration |
| **Scoped identity checks.** Tracker-backed work-unit roots get the runtime checks schema validation cannot express: id/handle agreement, duplicate-identity rejection, active-deployment agreement. | `libagent/src/scoped_identity.rs` (module docs) | [interface-contract.md](interface-contract.md) § scoped identity |
| **Transcript redaction and attribution.** Environment variables named by `[transcript].redact_env` (or `RUNA_TRANSCRIPT_REDACT_ENV`) have their current values redacted from emitted transcript events. Captured events are routed and labeled by deployment, work unit, and run before being written under the configured transcript root. | transcript emission in `libagent::transcript`, used by `runa-cli`/`runa-mcp` | [cli-reference.md](cli-reference.md) § configuration |
| **Driver verbs cannot bypass state.** Every session driver verb rescans and revalidates before reporting or advancing; transition authority stays in the session state machine, not the driver. | `libagent/src/session.rs` (module docs) | [session-surface-contract.md](session-surface-contract.md) |

Out of runa's scope by design: process isolation, credential injection, and
audit sealing belong to the runtime host —
[agentd](https://github.com/tesserine/agentd) (see its `ARCHITECTURE.md`
and `docs/audit-record.md`).
