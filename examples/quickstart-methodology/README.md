# Quickstart Methodology: Code Review Pipeline

A two-protocol review pipeline that demonstrates dependency-driven protocol chaining.

A `draft` protocol reads requirements and produces a design. A `review` protocol reads the requirements and the design and produces a review report. Because runa only injects declared `requires` and available `accepts` inputs, the manifest declares both review inputs explicitly. Runa computes the execution order from the dependency graph.

## Contents

- `manifest.toml` — methodology manifest declaring three artifact types and two protocols
- `schemas/` — JSON Schema definitions for each artifact type
- `protocols/` — instruction files delivered to the agent at execution time

## Usage

```bash
runa init --methodology examples/quickstart-methodology/manifest.toml
runa state
```

## Further Reading

- [Methodology Authoring Guide](../../docs/methodology-authoring-guide.md) — full walkthrough of this example with explanations
- [Interface Contract](../../docs/interface-contract.md) — authoritative specification for manifest fields, triggers, and runtime behavior
