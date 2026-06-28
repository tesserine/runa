# Architecture Decision Records

This directory holds runa's architecture decision records (ADRs): the durable
record of significant, hard-to-reverse decisions about runa's implementation
architecture, each carrying the reasoning that produced it and the alternatives
weighed and rejected.

runa documents its *implementation architecture* here. Cross-cutting ecosystem
*concepts and contracts* live in `tesserine/commons`, and the universal layer in
`pentaxis93/commons`. An ADR **cites** those higher-level sources as what it
realizes; it does not restate them (Single Home).

Records are numbered sequentially (`NNNN-kebab-title.md`) and never renumbered.
Status is one of **Proposed**, **Accepted**, or **Superseded** (naming the ADR
that supersedes it).

## Index

- [ADR-0001](0001-single-state-assess-and-route-operation.md) — The single
  state-assess-and-route operation (request as entry-state) — *Proposed*
