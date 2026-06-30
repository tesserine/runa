# Architecture Decision Records

This directory holds runa's architecture decision records (ADRs): the durable
record of significant, hard-to-reverse decisions about runa's implementation
architecture, each carrying the reasoning that produced it and the alternatives
weighed and rejected.

runa documents its *implementation architecture* here. The higher-level sources
an ADR cites sit above it: cross-cutting ecosystem *concepts and contracts* live
in `tesserine/commons`; the *domain-specific golden rules* and cross-cutting
conventions of the pentaxis93 base live in `pentaxis93/commons`; and the
*domain-neutral universal principles* those rules are rooted in live at their
canonical home, `pentaxis93/principles`. An ADR **cites** those sources as what
it realizes; it does not restate them (Single Home).

Records are numbered sequentially (`NNNN-kebab-title.md`) and never renumbered.
Status is one of **Proposed**, **Accepted**, **Superseded** (naming the ADR
that supersedes it), or **Withdrawn** (a proposal found unsound before it was
accepted — nothing supersedes it; the withdrawal note states why).

## Index

- [ADR-0001](0001-single-state-assess-and-route-operation.md) — The single
  state-assess-and-route operation (request as entry-state) — *Accepted*
  (Decision 3 stands in full; Decision 5's shape stands, its artifact name changing
  under the `request` → `intent` rename re-homed to `tesserine/commons#94`)
- [ADR-0002](0002-operator-intent-seeding-wish.md) — Operator intent seeding:
  `wish` authors, `go` advances — *Withdrawn* (Decision A unsound: `wish` is an
  `agentd` verb, not runa — `tesserine/agentd#152`; the `request` → `intent` rename
  it identified is re-homed to `tesserine/commons#94`)
