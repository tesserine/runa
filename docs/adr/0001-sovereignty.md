# ADR-0001: Sovereignty

**Status:** Accepted
**Traces to:** Principle 1 (Sovereignty)

## Decision

Every interface between two entities in the runa system has a clean ownership boundary. Each side owns its domain fully. Neither crosses.

## Context

runa sits between a daemon (agentd) and methodology plugins (groundwork, others). It also mediates between operators and execution agents, between skills and their consumers, and between producers and validators. Each of these is a boundary where ownership can blur.

When boundaries hold, maximum output emerges from minimum input. When they blur, corruption is symmetric — domination by either side breaks the system.

The principle is fractal. It applies at every interface, at every scale.

## Consequences

**Runtime/methodology boundary.** runa enforces structure. It never prescribes methodology content, topology, or domain semantics. Methodologies own all artifact types, skills, schemas, and domain decisions. The interface contract defines this boundary — three primitives, nothing more. If runa begins interpreting what a methodology's artifact types mean, or suggesting what topology a methodology should use, the boundary has been violated.

**Operator/agent boundary.** The operator declares WHAT through methodology configuration, manifest content, and trigger signals. The agent owns HOW — execution strategy, implementation decisions, craft. runa's enforcement mechanisms support this split: operators define schemas and declarations; agents produce artifacts that satisfy them.

**Skill/consumer boundary.** A skill declares what it produces. A consumer declares what it requires. runa validates the contract between them. Neither skill nor consumer reaches into the other's domain. The typed artifact at the boundary is the interface — not shared state, not runtime internals, not implicit conventions.

**What this means for the builder agent:** When designing any interface — between runa and a methodology, between runa and the daemon, between internal modules — ask: who owns what on each side? Make that ownership explicit. If a component needs to know something about another component's internals to function, the boundary is wrong.
