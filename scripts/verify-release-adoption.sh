#!/usr/bin/env bash
set -euo pipefail

workspace_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
real_home="${HOME:?}"
cargo_home="${CARGO_HOME:-$real_home/.cargo}"
rustup_home="${RUSTUP_HOME:-$real_home/.rustup}"

if ! cargo release --version >/dev/null 2>&1; then
    echo "cargo-release is required: install the cargo-release cargo subcommand" >&2
    exit 1
fi

scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT

source_repo="$scratch/source"
remote_repo="$scratch/origin.git"
fresh_checkout="$scratch/fresh-checkout"
release_log="$scratch/cargo-release.log"
dirty_release_log="$scratch/dirty-release.log"
hostile_home="$scratch/hostile-home"
resolved_config_log="$scratch/cargo-release-config.log"

mkdir "$source_repo"
tar \
    --exclude=.git \
    --exclude=target \
    --exclude=.idea \
    --exclude=.vscode \
    -C "$workspace_root" \
    -cf - . \
    | tar -C "$source_repo" -xf -

git -C "$source_repo" init -q
git -C "$source_repo" config user.name "runa release verification"
git -C "$source_repo" config user.email "runa-release-verification@example.invalid"
git -C "$source_repo" checkout -q -b main
git -C "$source_repo" -c core.excludesFile=/dev/null add .
git -C "$source_repo" commit -q -m "test: seed release verification"

git init --bare -q "$remote_repo"
git -C "$source_repo" remote add origin "$remote_repo"
git -C "$source_repo" push -q -u origin main

workspace_version() {
    sed -n '/^\[workspace.package\]/,/^\[/{s/^version = "\(.*\)"/\1/p}' "$1/Cargo.toml"
}

old_version="$(workspace_version "$source_repo")"

printf '\nrelease verification dirty probe\n' >> "$source_repo/README.md"
if (
    cd "$source_repo"
    cargo release patch --execute --no-confirm >"$dirty_release_log" 2>&1
); then
    echo "cargo-release did not refuse a dirty working tree" >&2
    exit 1
fi

if ! grep -Fq "uncommitted changes detected" "$dirty_release_log"; then
    echo "dirty-tree release refusal did not report uncommitted changes" >&2
    exit 1
fi

git -C "$source_repo" checkout -q -- README.md

mkdir -p "$hostile_home/.config/cargo-release"
cat >"$hostile_home/.config/cargo-release/release.toml" <<'EOF'
release = false
tag = false
verify = false
pre-release-hook = ["/bin/false"]
push-options = ["--repo", "unexpected"]
owners = ["unexpected"]
enable-features = ["unexpected"]
enable-all-features = true
metadata = "required"
certs-source = "native"
EOF

(
    cd "$source_repo"
    HOME="$hostile_home" \
        XDG_CONFIG_HOME="$hostile_home/.config" \
        CARGO_HOME="$cargo_home" \
        RUSTUP_HOME="$rustup_home" \
        cargo release config >"$resolved_config_log"
)

assert_resolved_config() {
    local expected="$1"

    if ! grep -Fq "$expected" "$resolved_config_log"; then
        echo "cargo-release config did not preserve workspace pin: $expected" >&2
        exit 1
    fi
}

assert_resolved_config 'release = true'
assert_resolved_config 'tag = true'
assert_resolved_config 'verify = true'
assert_resolved_config 'pre-release-hook = ["true"]'
assert_resolved_config 'push-options = []'
assert_resolved_config 'owners = []'
assert_resolved_config 'enable-features = []'
assert_resolved_config 'enable-all-features = false'
assert_resolved_config 'metadata = "optional"'
assert_resolved_config 'certs-source = "webpki"'

(
    cd "$source_repo"
    cargo release patch --execute --no-confirm 2>&1 | tee "$release_log"
)

new_version="$(workspace_version "$source_repo")"
tag_name="v$new_version"
release_commit="$(git -C "$source_repo" rev-parse HEAD)"
tag_commit="$(git -C "$source_repo" rev-list -n 1 "$tag_name")"

if [[ "$new_version" == "$old_version" ]]; then
    echo "workspace version did not change from $old_version" >&2
    exit 1
fi

if ! grep -Fq "## [$new_version] — $(date +%F)" "$source_repo/CHANGELOG.md"; then
    echo "CHANGELOG.md was not rolled to [$new_version] with today's date" >&2
    exit 1
fi

if [[ "$(git -C "$source_repo" cat-file -t "$tag_name")" != "tag" ]]; then
    echo "$tag_name is not an annotated tag" >&2
    exit 1
fi

if [[ "$tag_commit" != "$release_commit" ]]; then
    echo "$tag_name does not point at the release commit" >&2
    exit 1
fi

if ! git --git-dir="$remote_repo" rev-parse --verify --quiet "refs/heads/main" >/dev/null; then
    echo "release branch was not pushed to the remote" >&2
    exit 1
fi

if ! git --git-dir="$remote_repo" rev-parse --verify --quiet "refs/tags/$tag_name" >/dev/null; then
    echo "$tag_name was not pushed to the remote" >&2
    exit 1
fi

if grep -Fq "Uploading" "$release_log"; then
    echo "release log indicates registry publishing occurred" >&2
    exit 1
fi

git clone -q "$remote_repo" "$fresh_checkout"
git -C "$fresh_checkout" checkout -q "$tag_name"
cargo build --release --workspace --manifest-path "$fresh_checkout/Cargo.toml"

runa_version_output="$("$fresh_checkout/target/release/runa" --version)"
if [[ "$runa_version_output" != "runa $new_version" ]]; then
    echo "runa --version reported '$runa_version_output', expected 'runa $new_version'" >&2
    exit 1
fi

runa_mcp_version_output="$("$fresh_checkout/target/release/runa-mcp" --version)"
if [[ "$runa_mcp_version_output" != "runa-mcp $new_version" ]]; then
    echo "runa-mcp --version reported '$runa_mcp_version_output', expected 'runa-mcp $new_version'" >&2
    exit 1
fi

echo "verified release adoption for $tag_name"
