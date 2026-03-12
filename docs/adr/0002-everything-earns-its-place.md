# ADR-0002: Everything Earns Its Place

**Status:** Accepted
**Traces to:** Principle 3 (Grounding), Principle 2 (Sequence)

## Decision

Every element in the runa codebase — every line of code, every test, every document, every abstraction — traces to a current need or gets removed. New work grounds in what it must enable before building. Existing work that no longer serves today's exigency is debt regardless of how well it functions.

## Context

runa is being built from scratch after a failed migration attempt that carried historical artifacts, naming conventions, and structural assumptions from a predecessor system. That experience demonstrated the cost of preserving elements that no longer earn their place: debugging time spent untangling inherited assumptions exceeded the time a clean build would have taken.

Two disciplines combine here. Grounding asks: what must this enable, and for whom? Zero tech debt asks: does this element still serve a current need? They are the same discipline applied at different moments — grounding prevents unearned elements from entering; zero tech debt removes them when they lose their justification.

Sequence matters: grounding fires before generation, not after. Orient to the need first, then build. Retrofitting justification onto existing code is not grounding.

## Consequences

**No backward compatibility layers.** When a design changes, the old design is replaced. No shims, no adapters, no "support both during transition." The system reflects current requirements only.

**No migration narratives in artifacts.** Documents, comments, and commit messages describe the present system. They do not explain what used to exist or why it changed. History lives in git, not in the codebase.

**No speculative abstractions.** Every abstraction serves a current, concrete use case. "We might need this later" is not justification. If a future need arises, the abstraction is built then — grounded in that need, not in today's guess about it.

**No tests that assert absence.** Tests verify what the system does, not what it doesn't do. A test that asserts "this function does not call X" or "this module does not contain Y" is testing against a ghost. If X and Y were never supposed to be there, there is nothing to test.

**What this means for the builder agent:** Before writing any code, answer: what does this enable? If you cannot trace a component to a requirement in the interface contract or the ADRs, it does not belong. When modifying existing code, apply the same test — if a component no longer traces to a current need, remove it rather than working around it.
