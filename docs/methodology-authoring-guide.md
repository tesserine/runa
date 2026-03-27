# Methodology Authoring Guide

This guide is for someone who has finished the README and understands the
three primitives at a conceptual level: artifact types, protocol declarations,
and trigger conditions. Its goal is narrower than the
[Interface Contract](interface-contract.md): build one valid methodology that
`runa` can load, and use that example to show how protocol chaining emerges
from declarations.

## Start With A Complete Chain

Use one small pipeline, not isolated fragments. The smallest useful chain has:

- one input artifact type
- one protocol that turns that input into an intermediate artifact
- one protocol that requires the intermediate artifact and turns it into a
  final artifact

This guide uses `request -> outline -> summary`.

The full methodology layout looks like this:

```text
authoring-guide-example/
├── manifest.toml
├── protocols/
│   ├── plan-summary/
│   │   └── PROTOCOL.md
│   └── write-summary/
│       └── PROTOCOL.md
└── schemas/
    ├── outline.schema.json
    ├── request.schema.json
    └── summary.schema.json
```

## Write The Manifest

The manifest names artifact types and protocol declarations. It does not embed
schemas or instruction paths. `runa` derives those from the layout convention.

```toml
name = "authoring-guide-example"

[[artifact_types]]
name = "request"

[[artifact_types]]
name = "outline"

[[artifact_types]]
name = "summary"

[[protocols]]
name = "plan-summary"
requires = ["request"]
produces = ["outline"]
trigger = { type = "on_artifact", name = "request" }

[[protocols]]
name = "write-summary"
requires = ["outline"]
produces = ["summary"]
trigger = { type = "on_artifact", name = "outline" }
```

The trigger syntax matters: `TriggerCondition` is a tagged enum, so the TOML
must use the exact tagged shape `type = "..."` plus the fields that variant
expects. For `on_artifact`, that means `name`.

## Add The Schemas

Each artifact type needs a schema file at `schemas/{name}.schema.json`.

`schemas/request.schema.json`

```json
{
  "type": "object",
  "required": ["text"],
  "properties": {
    "text": {
      "type": "string"
    }
  }
}
```

`schemas/outline.schema.json`

```json
{
  "type": "object",
  "required": ["points"],
  "properties": {
    "points": {
      "type": "array",
      "items": {
        "type": "string"
      },
      "minItems": 1
    }
  }
}
```

`schemas/summary.schema.json`

```json
{
  "type": "object",
  "required": ["summary"],
  "properties": {
    "summary": {
      "type": "string"
    }
  }
}
```

## Add The Protocol Instructions

Each protocol needs an instruction file at `protocols/{name}/PROTOCOL.md`.

`protocols/plan-summary/PROTOCOL.md`

```md
Read the `request` artifact and produce an `outline` artifact with the key
points to cover.
```

`protocols/write-summary/PROTOCOL.md`

```md
Read the `outline` artifact and produce a `summary` artifact that turns those
points into a concise summary.
```

## How The Chain Works

`runa` does not ask you to declare a pipeline separately. The chain emerges
from the protocol declarations:

- `plan-summary` requires `request` and produces `outline`
- `write-summary` requires `outline` and produces `summary`

That is enough for `runa` to compute the dependency edge between the two
protocols. When a valid `request` artifact exists, `plan-summary` can become
ready. When a valid `outline` artifact exists, `write-summary` can become
ready.

This is the core pattern for methodology authoring: declare artifact
relationships honestly, and let the runtime derive topology from them.

## Check That `runa` Accepts It

Once the methodology directory exists, point a fresh project at its manifest:

```bash
mkdir authoring-guide-demo
cd authoring-guide-demo
runa init --methodology ../authoring-guide-example/manifest.toml
runa list
```

`runa init` should succeed without parse errors. `runa list` should show
`plan-summary` before `write-summary`, with `request` and `outline` as the
required artifacts that define the chain.

## Where To Go Next

- Read the [Interface Contract](interface-contract.md) for the authoritative
  definition of artifact types, protocol declarations, trigger conditions, and
  the layout convention.
- Read [groundwork](https://github.com/pentaxis93/groundwork) for a real
  methodology built on runa.
