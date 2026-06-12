# Quickstart Methodology: Code Review Pipeline

A two-protocol review pipeline that demonstrates dependency-driven protocol chaining.

A `draft` protocol reads requirements and produces a design. A `review` protocol reads the requirements and the design and produces a review report. Because runa only injects declared `requires` and available `accepts` inputs, the manifest declares both review inputs explicitly. Runa computes the execution order from the dependency graph.

## Contents

- `manifest.toml` — methodology manifest declaring three artifact types and two protocols
- `schemas/` — JSON Schema definitions for each artifact type
- `protocols/` — instruction files delivered to the agent at execution time

## Usage

From `examples/quickstart-methodology/`:

```bash
runa init --methodology manifest.toml
runa state
```

Both protocols are WAITING — no `requirements` artifact exists yet.

## The scan → READY → produce loop

The full loop can be walked without an agent by producing the artifacts by
hand; runa derives every transition from the workspace state.

1. **Deliver the entry artifact and scan.** `draft` becomes READY:

   ```bash
   mkdir -p .runa/workspace/requirements
   echo '{"title": "Widget API"}' > .runa/workspace/requirements/widget.json
   runa scan && runa state
   ```

   ```
   READY:
     draft
       - requirements/widget (requires)
   ```

2. **Produce `draft`'s output.** In a live session the agent delivers this
   through the `design` MCP tool; producing it by hand shows the same
   state movement:

   ```bash
   mkdir -p .runa/workspace/design
   echo '{"summary": "Three endpoints, one store"}' > .runa/workspace/design/widget.json
   runa scan && runa state
   ```

   `draft` is now WAITING — its output is current — and `review` is READY
   with both inputs listed. The cascade advanced one stage because an
   artifact appeared, not because anything was commanded.

3. **Produce `review`'s output** the same way
   (`echo '{"approved": true}' > .runa/workspace/review-report/widget.json`,
   then `runa scan && runa state`): every protocol reports WAITING with
   outputs current. The topology is quiescent — `runa run` would exit `4`
   (nothing ready).

4. **Reopen the loop.** Touch the requirements artifact with changed
   content and scan: downstream outputs are no longer current, and the
   affected protocols come back READY. Freshness is derived from the
   artifact store, not from any run history you must manage.

For live execution the same loop is driven by an agent:
`runa step` executes the single next READY protocol through your
configured agent command, and `runa run` repeats that to quiescence — see
the [CLI reference](../../docs/cli-reference.md).

## Further Reading

- [Methodology Authoring Guide](../../docs/methodology-authoring-guide.md) — full walkthrough of this example with explanations
- [Interface Contract](../../docs/interface-contract.md) — authoritative specification for manifest fields, triggers, and runtime behavior
