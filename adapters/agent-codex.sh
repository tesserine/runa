#!/bin/sh
set -eu

# Supported adapter for Codex CLI. Depends on jq for JSON parsing.
# Runa delivers {command,args,env} through RUNA_MCP_CONFIG; Codex consumes
# stdio MCP servers through TOML config overrides on `codex exec`.

if [ -z "${RUNA_MCP_CONFIG:-}" ]; then
    echo "agent-codex requires RUNA_MCP_CONFIG" >&2
    exit 1
fi

if ! command -v jq >/dev/null 2>&1; then
    echo "agent-codex requires jq to parse RUNA_MCP_CONFIG" >&2
    exit 1
fi

command_toml="$(
    printf '%s' "$RUNA_MCP_CONFIG" |
        jq -er '.command | if type == "string" then @json else error("RUNA_MCP_CONFIG.command must be a string") end'
)"
args_toml="$(
    printf '%s' "$RUNA_MCP_CONFIG" |
        jq -cer '.args | if type == "array" and all(.[]; type == "string") then . else error("RUNA_MCP_CONFIG.args must be a string array") end'
)"
env_toml="$(
    printf '%s' "$RUNA_MCP_CONFIG" |
        jq -er '(.env // {}) | if type == "object" and all(.[]; type == "string") then to_entries | map((.key | @json) + " = " + (.value | @json)) | "{ " + join(", ") + " }" else error("RUNA_MCP_CONFIG.env must be a string object") end'
)"

server_name="runa_session_$$"

exec codex exec \
    -c "mcp_servers.$server_name.command=$command_toml" \
    -c "mcp_servers.$server_name.args=$args_toml" \
    -c "mcp_servers.$server_name.env=$env_toml" \
    "$@"
