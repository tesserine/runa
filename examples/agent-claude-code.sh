#!/bin/sh
set -eu

if [ -z "${RUNA_MCP_CONFIG:-}" ]; then
    echo "agent wrapper requires RUNA_MCP_CONFIG" >&2
    exit 1
fi

tmp_config="$(mktemp "${TMPDIR:-/tmp}/runa-mcp-config.XXXXXX.json")"
trap 'rm -f "$tmp_config"' EXIT HUP INT TERM

printf '%s\n' "$RUNA_MCP_CONFIG" > "$tmp_config"

exec claude --mcp-config "$tmp_config" --strict-mcp-config "$@"
