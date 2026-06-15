mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn runa_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_runa"))
}

fn runa_mcp_bin_path() -> PathBuf {
    Path::new(env!("CARGO_BIN_EXE_runa"))
        .parent()
        .unwrap()
        .join(format!("runa-mcp{}", std::env::consts::EXE_SUFFIX))
}

fn init_project(project_dir: &Path, manifest_path: &Path) {
    let output = runa_bin()
        .arg("init")
        .arg("--methodology")
        .arg(manifest_path)
        .current_dir(project_dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn append_agent_command_config(project_dir: &Path, command: &[&Path]) {
    let config_path = project_dir.join(".runa/config.toml");
    let mut config = fs::read_to_string(&config_path).unwrap();
    config.push_str("\n[runtime]\ncommand = [");
    for (index, part) in command.iter().enumerate() {
        if index > 0 {
            config.push_str(", ");
        }
        config.push_str(&format!("{:?}", part.display().to_string()));
    }
    config.push_str("]\n");
    fs::write(config_path, config).unwrap();
}

fn write_executable(path: &Path, content: &str) {
    use std::os::unix::fs::PermissionsExt;

    fs::write(path, content).unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

fn drive_session_mcp_once(project_dir: &Path, runa_mcp_path: &Path, log_path: &Path) {
    let output = Command::new("sh")
        .arg("-c")
        .arg(
            r#"
set -eu
{
    printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"go-parity-test","version":"1.0.0"}}}'
    printf '%s\n' '{"jsonrpc":"2.0","method":"notifications/initialized"}'
    printf '%s\n' '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"next-protocol-context","arguments":{}}}'
    printf '%s\n' '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"claim","arguments":{"instance_id":"claim-1","scope":"claim this work"}}}'
    printf '%s\n' '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"advance","arguments":{}}}'
    sleep 1
} | "$1" --session --work-unit work-unit-168 > "$2"
if grep -q '"error"' "$2"; then
    cat "$2" >&2
    exit 23
fi
"#,
        )
        .arg("drive-session")
        .arg(runa_mcp_path)
        .arg(log_path)
        .env_remove("RUNA_FORGE_TYPE")
        .env_remove("RUNA_FORGE_TRACKER_ID")
        .env("RUNA_FORGE_OWNER", "tesserine")
        .env("RUNA_FORGE_NAME", "runa")
        .current_dir(project_dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "direct session MCP tick failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn write_go_session_agent(agent_path: &Path) {
    write_executable(
        agent_path,
        r#"#!/bin/sh
set -eu
cat > "$1"
{
    printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"go-parity-test","version":"1.0.0"}}}'
    printf '%s\n' '{"jsonrpc":"2.0","method":"notifications/initialized"}'
    printf '%s\n' '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"next-protocol-context","arguments":{}}}'
    printf '%s\n' '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"claim","arguments":{"instance_id":"claim-1","scope":"claim this work"}}}'
    printf '%s\n' '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"advance","arguments":{}}}'
    sleep 1
} | "$2" --session --work-unit work-unit-168 > "$3"
if grep -q '"error"' "$3"; then
    cat "$3" >&2
    exit 23
fi
"#,
    );
}

fn setup_ready_scoped_project(dir: &Path) -> PathBuf {
    let manifest_path = common::write_methodology(
        dir,
        common::scoped_work_unit_manifest_toml(),
        common::scoped_work_unit_schemas(),
        &["take"],
    );
    let project_dir = dir.join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("work-unit")).unwrap();
    fs::write(
        workspace.join("work-unit/work-unit-168.json"),
        common::github_work_unit_json(168),
    )
    .unwrap();

    project_dir
}

fn scoped_state_json(project_dir: &Path) -> serde_json::Value {
    let output = runa_bin()
        .arg("state")
        .arg("--json")
        .arg("--work-unit")
        .arg("work-unit-168")
        .env_remove("RUNA_FORGE_TYPE")
        .env_remove("RUNA_FORGE_TRACKER_ID")
        .env("RUNA_FORGE_OWNER", "tesserine")
        .env("RUNA_FORGE_NAME", "runa")
        .current_dir(project_dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "state failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn workspace_claim_json(project_dir: &Path) -> serde_json::Value {
    serde_json::from_str(
        &fs::read_to_string(project_dir.join(".runa/workspace/claim/claim-1.json")).unwrap(),
    )
    .unwrap()
}

fn execution_records_json(project_dir: &Path) -> serde_json::Value {
    serde_json::from_str(
        &fs::read_to_string(project_dir.join(".runa/store/execution-records.json")).unwrap(),
    )
    .unwrap()
}

#[test]
fn go_launches_configured_agent_with_session_mcp_config_for_one_tick() {
    let dir = tempfile::tempdir().unwrap();
    let project_dir = setup_ready_scoped_project(dir.path());
    let agent_path = dir.path().join("agent.sh");
    let prompt_path = dir.path().join("prompt.txt");
    let config_path = dir.path().join("mcp-config.json");
    let mcp_log_path = dir.path().join("mcp.log");
    write_executable(
        &agent_path,
        r#"#!/bin/sh
set -eu
cat > "$1"
printf '%s' "$RUNA_MCP_CONFIG" > "$2"
{
    printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"go-test","version":"1.0.0"}}}'
    printf '%s\n' '{"jsonrpc":"2.0","method":"notifications/initialized"}'
    printf '%s\n' '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"next-protocol-context","arguments":{}}}'
    printf '%s\n' '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"claim","arguments":{"instance_id":"claim-1","scope":"claim this work"}}}'
    printf '%s\n' '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"advance","arguments":{}}}'
    sleep 1
} | "$3" --session --work-unit work-unit-168 > "$4"
if grep -q '"error"' "$4"; then
    cat "$4" >&2
    exit 23
fi
"#,
    );
    let runa_mcp_path = runa_mcp_bin_path();
    append_agent_command_config(
        &project_dir,
        &[
            &agent_path,
            &prompt_path,
            &config_path,
            &runa_mcp_path,
            &mcp_log_path,
        ],
    );

    let output = runa_bin()
        .arg("go")
        .arg("--work-unit")
        .arg("work-unit-168")
        .env_remove("RUNA_FORGE_TYPE")
        .env_remove("RUNA_FORGE_TRACKER_ID")
        .env("RUNA_FORGE_OWNER", "tesserine")
        .env("RUNA_FORGE_NAME", "runa")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}\nmcp log: {}",
        String::from_utf8_lossy(&output.stderr),
        fs::read_to_string(&mcp_log_path).unwrap_or_else(|_| "<missing>".to_string())
    );

    let prompt = fs::read_to_string(prompt_path).unwrap();
    assert!(
        prompt.contains("next-protocol-context"),
        "prompt should instruct the agent to get context: {prompt}"
    );
    assert!(
        prompt.contains("advance"),
        "prompt should instruct the agent to advance exactly once: {prompt}"
    );

    let config: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(config_path).unwrap()).unwrap();
    assert_eq!(
        config["args"],
        serde_json::json!(["--session", "--work-unit", "work-unit-168"])
    );
    assert!(config["command"].as_str().unwrap().contains("runa-mcp"));
    assert_eq!(
        config["env"]["RUNA_WORKING_DIR"].as_str().unwrap(),
        project_dir.to_string_lossy()
    );
    assert!(
        config["env"]["RUNA_CONFIG"]
            .as_str()
            .unwrap()
            .ends_with(".runa/config.toml")
    );

    let claim = fs::read_to_string(project_dir.join(".runa/workspace/claim/claim-1.json")).unwrap();
    assert!(
        claim.contains("\"work_unit\": \"work-unit-168\""),
        "{claim}"
    );
    let execution_records =
        fs::read_to_string(project_dir.join(".runa/store/execution-records.json")).unwrap();
    assert!(
        execution_records.contains(r#""protocol": "take""#),
        "{execution_records}"
    );
}

#[test]
fn go_fails_when_agent_exits_without_advancing_the_session_step() {
    let dir = tempfile::tempdir().unwrap();
    let project_dir = setup_ready_scoped_project(dir.path());
    let agent_path = dir.path().join("agent.sh");
    let prompt_path = dir.path().join("prompt.txt");
    write_executable(&agent_path, "#!/bin/sh\nset -eu\ncat > \"$1\"\n");
    append_agent_command_config(&project_dir, &[&agent_path, &prompt_path]);

    let output = runa_bin()
        .arg("go")
        .arg("--work-unit")
        .arg("work-unit-168")
        .env_remove("RUNA_FORGE_TYPE")
        .env_remove("RUNA_FORGE_TRACKER_ID")
        .env("RUNA_FORGE_OWNER", "tesserine")
        .env("RUNA_FORGE_NAME", "runa")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "go should fail when the session was not advanced"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("did not advance"),
        "stderr should explain the missing advance: {stderr}"
    );
}

#[test]
fn go_matches_direct_session_surface_when_regenerating_deleted_output_with_unchanged_inputs() {
    let dir = tempfile::tempdir().unwrap();
    let go_dir = dir.path().join("go");
    let direct_dir = dir.path().join("direct");
    fs::create_dir(&go_dir).unwrap();
    fs::create_dir(&direct_dir).unwrap();
    let go_project_dir = setup_ready_scoped_project(&go_dir);
    let direct_project_dir = setup_ready_scoped_project(&direct_dir);
    let runa_mcp_path = runa_mcp_bin_path();

    drive_session_mcp_once(
        &go_project_dir,
        &runa_mcp_path,
        &dir.path().join("go-baseline-mcp.log"),
    );
    drive_session_mcp_once(
        &direct_project_dir,
        &runa_mcp_path,
        &dir.path().join("direct-baseline-mcp.log"),
    );

    fs::remove_file(go_project_dir.join(".runa/workspace/claim/claim-1.json")).unwrap();
    fs::remove_file(direct_project_dir.join(".runa/workspace/claim/claim-1.json")).unwrap();

    let agent_path = dir.path().join("agent.sh");
    let prompt_path = dir.path().join("prompt.txt");
    let go_log_path = dir.path().join("go-rerun-mcp.log");
    write_go_session_agent(&agent_path);
    append_agent_command_config(
        &go_project_dir,
        &[&agent_path, &prompt_path, &runa_mcp_path, &go_log_path],
    );

    let go_output = runa_bin()
        .arg("go")
        .arg("--work-unit")
        .arg("work-unit-168")
        .env_remove("RUNA_FORGE_TYPE")
        .env_remove("RUNA_FORGE_TRACKER_ID")
        .env("RUNA_FORGE_OWNER", "tesserine")
        .env("RUNA_FORGE_NAME", "runa")
        .current_dir(&go_project_dir)
        .output()
        .unwrap();

    assert!(
        go_output.status.success(),
        "go failed after authoritative advance regenerated the deleted output\nstdout: {}\nstderr: {}\nmcp log: {}",
        String::from_utf8_lossy(&go_output.stdout),
        String::from_utf8_lossy(&go_output.stderr),
        fs::read_to_string(&go_log_path).unwrap_or_else(|_| "<missing>".to_string())
    );

    drive_session_mcp_once(
        &direct_project_dir,
        &runa_mcp_path,
        &dir.path().join("direct-rerun-mcp.log"),
    );

    assert_eq!(
        workspace_claim_json(&go_project_dir),
        workspace_claim_json(&direct_project_dir)
    );
    assert_eq!(
        execution_records_json(&go_project_dir),
        execution_records_json(&direct_project_dir)
    );
    assert_eq!(
        scoped_state_json(&go_project_dir),
        scoped_state_json(&direct_project_dir)
    );
}
