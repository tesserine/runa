#!/bin/sh
set -eu

# Supported adapter: runa always delivers the MCP session payload through
# RUNA_MCP_CONFIG. This wrapper translates that payload to Claude Code's
# current CLI config shape; runa does not do this translation implicitly.

if [ -z "${RUNA_MCP_CONFIG:-}" ]; then
    echo "agent-claude-code requires RUNA_MCP_CONFIG" >&2
    exit 1
fi

tmp_config="$(mktemp "${TMPDIR:-/tmp}/runa-mcp-config.XXXXXX.json")"
trap 'rm -f "$tmp_config"' EXIT HUP INT TERM

printf '{"mcpServers":{"runa":%s}}\n' "$RUNA_MCP_CONFIG" > "$tmp_config"

exec claude --mcp-config "$tmp_config" --strict-mcp-config "$@"
