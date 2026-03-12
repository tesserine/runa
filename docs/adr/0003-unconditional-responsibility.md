# ADR-0003: Unconditional Responsibility

**Status:** Accepted
**Traces to:** Principle 4 (Obligation to Dissent), Principle 3 (Grounding)

## Decision

Every participant — human or agent — is responsible for the whole system's health and trajectory, not just their assignment. If you see a problem, you own it: fix it or queue it. No scoping away, no deferral because something is "preexisting" or "out of scope."

## Context

runa is built and operated by autonomous agents working with minimal supervision. In that environment, problems that no one owns compound silently. A defect noticed but deferred becomes two defects — the original and the process failure that let it pass. An architectural concern flagged but scoped away becomes a structural weakness that every subsequent decision must navigate around.

The integrity of an unsupervised agentic system depends on every participant refusing to let problems pass.

## Consequences

**No scoping away.** If you encounter a defect, a design problem, or an inconsistency while working on something else, you own it. Fix it if the fix is small. File an issue if it requires separate work. Do not note it and move on. Do not rationalize it as someone else's responsibility.

**Ground against mission, not against task list.** The task is the immediate assignment. The mission is the system's purpose and trajectory. If satisfying the task while the system's trajectory drifts, the task has become a ceiling. Every piece of work should ask: does this serve the mission, or just clear an obligation?

**Scope upward, not just outward.** If you are building for one use case when the architecture could serve many, that is a scoping failure. Unconditional responsibility means seeing the larger context and building for it — not gold-plating, but refusing comfortable local optima when the system needs more.

**What this means for the builder agent:** When you see something wrong — a naming inconsistency, a missing edge case, a design that doesn't scale, a document that contradicts the code — act on it. If the fix is within your current scope, fix it. If it requires separate work, file an issue with enough context that someone else can act on it. The worst outcome is a known problem that everyone has seen and no one has addressed.
