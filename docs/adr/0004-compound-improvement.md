# ADR-0004: Compound Improvement

**Status:** Accepted
**Traces to:** Principle 5 (Recursive Improvement)

## Decision

Every operation refines both the object and the process that created it. Single-level work — improving only the object — accumulates friction in the process. The system gets better at getting better, or it decays. There is no steady state.

## Context

runa's development process is itself a system that either improves or degrades with each iteration. A bug fix that addresses the symptom but not the condition that allowed the bug leaves the process unchanged — the same class of bug will recur. A skill that works but whose creation process was painful means the next skill will be equally painful. Object-only work is entropy with extra steps.

The recursive spiral — refinement plus propulsion — is the topology that prevents stagnation. Each pass through the system should leave both the output and the machinery in better shape.

## Consequences

**When you fix a bug, also fix what let the bug happen.** The bug is the object. The gap in testing, validation, or design that admitted the bug is the process. Address both. A bug fix without a process improvement is half the work.

**When you add a capability, also improve how capabilities get added.** If adding a new trigger type is painful, the pain is signal. The trigger type is the object; the extension mechanism is the process. Improve both.

**When a correction is needed, produce both the correction and the structural change that prevents recurrence.** A correction that will need to be made again is not a correction — it is a patch. The structural change is what makes it a correction.

**Refactoring is not extra work.** It is the process-level half of every task. If the code you touched is cleaner than when you found it, you did both levels. If it is not, you did only one.

**What this means for the builder agent:** After completing any unit of work, ask: did I address both the immediate thing and the process that produced it? If a test was hard to write, improve the test infrastructure. If a module was hard to understand, improve its documentation or its structure. If an error message was misleading, improve the error reporting pattern, not just the one message. The compounding effect of dual-level work is the difference between a system that improves and one that merely accumulates features.
