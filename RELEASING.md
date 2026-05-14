# Releasing runa

Audience: the release operator cutting a runa repository release or release
candidate. This document assumes access to the repository, GitHub, Rust,
`jq`, and `cargo-release`.

## Release Identity

runa uses one repository tag for the Rust workspace. The tag is `vX.Y.Z` for
stable releases and `vX.Y.Z-rc.N` for deployment release candidates, following
commons ADR-0006, ADR-0011, and ADR-0012.

Artifacts built from the tag must report that identity:

- `Cargo.toml` `[workspace.package].version` is `X.Y.Z` or `X.Y.Z-rc.N`.
- `runa --version` reports `runa X.Y.Z` or `runa X.Y.Z-rc.N`.
- `runa-mcp --version` reports `runa-mcp X.Y.Z` or `runa-mcp X.Y.Z-rc.N`.

runa's own release check does not validate base images. The
`org.tesserine.runa.ref` label on base images is owned by base and by the
ecosystem-level release manifest verification in commons.

## Pre-Release Gate

A releasable commit is on `main`, up to date with `origin/main`, and has a
clean working tree. `--allow-dirty` is not part of the release path.

Before tagging:

```sh
git checkout main
git pull --ff-only
git status --short
./scripts/release-check metadata
```

For a final tag-time check against a version already rolled into the source:

```sh
cargo build --release --locked --bin runa --bin runa-mcp
./scripts/release-check release "vX.Y.Z" \
  --runa-bin target/release/runa \
  --runa-mcp-bin target/release/runa-mcp
```

## Atomic Release Operation

Stable cargo-workspace releases use the configured `cargo-release` path:

```sh
cargo release patch --execute
```

Use `minor` or `major` instead of `patch` when the release semantics require
that version level. The command bumps the workspace version, applies the
configured changelog roll, commits, creates an annotated tag named `vX.Y.Z`,
and pushes the commit plus tag.

Deployment release candidates use the same tool path:

```sh
cargo release rc --execute
```

Release candidates are immutable refs for deployment testing. A bad or
superseded candidate is corrected by cutting the next `rc.N`, not by rewriting
the existing tag.

## Post-Release Gate

The tag push runs `.github/workflows/release.yml`. That workflow restores the
tag ref, verifies that its commit still matches the event commit, and verifies
the annotated tag and main-branch ancestry with git-only checks before running
repository release code. It then builds both release binaries, verifies
workspace and binary identity, extracts release notes from `CHANGELOG.md`, and
publishes the GitHub Release.
Only `vX.Y.Z-rc.N` tags are published as GitHub prereleases.

Manual GitHub Release creation, when needed after a workflow failure, uses the
same notes source:

```sh
./scripts/release-check notes "vX.Y.Z" > /tmp/runa-release-notes.md
gh release create "vX.Y.Z" \
  --title "runa vX.Y.Z" \
  --notes-file /tmp/runa-release-notes.md \
  --verify-tag
```

For an RC tag, include `--prerelease`:

```sh
./scripts/release-check notes "vX.Y.Z-rc.N" > /tmp/runa-release-notes.md
gh release create "vX.Y.Z-rc.N" \
  --title "runa vX.Y.Z-rc.N" \
  --notes-file /tmp/runa-release-notes.md \
  --verify-tag \
  --prerelease
```

## Failure Modes

If a published tag points at source that violates the release identity checks,
the tag is invalid. If it has no external consumers, delete it locally and
remotely and re-run the release operation. If it has external consumers, leave
the bad tag in the public record and cut the next version.

If the GitHub Release workflow fails after the tag is valid, repair the
workflow or environment and create the GitHub Release from
`scripts/release-check notes`. Do not edit release notes by hand unless the
changelog section is also corrected in source.
