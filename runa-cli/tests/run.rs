mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::os::unix::process::CommandExt;

fn runa_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_runa"))
}

fn init_project(project_dir: &std::path::Path, manifest_path: &std::path::Path) {
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
    let existing = fs::read_to_string(&config_path).unwrap();
    let command_entries = command
        .iter()
        .map(|entry| format!("  {:?},", entry.display().to_string()))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(
        config_path,
        format!("{existing}\n[agent]\ncommand = [\n{command_entries}\n]\n"),
    )
    .unwrap();
}

#[cfg(unix)]
fn write_reconciling_agent(dir: &Path) -> PathBuf {
    let script_path = dir.join("reconciling-agent.sh");
    fs::write(
        &script_path,
        "#!/bin/sh\nlog_file=\"$1\"\npayload=$(cat)\ncase \"$payload\" in\n  *\"# Protocol: implement\"*)\n    printf 'implement\\n' >> \"$log_file\"\n    mkdir -p .runa/workspace/implementation\n    printf '%s\\n' '{\"done\":true}' > .runa/workspace/implementation/impl-1.json\n    ;;\n  *\"# Protocol: verify\"*)\n    printf 'verify\\n' >> \"$log_file\"\n    mkdir -p .runa/workspace/verified\n    printf '%s\\n' '{\"done\":true}' > .runa/workspace/verified/check-1.json\n    ;;\n  *)\n    printf '%s\\n' \"$payload\" > \"$log_file.unexpected\"\n    exit 19\n    ;;\nesac\n",
    )
    .unwrap();
    fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755)).unwrap();
    script_path
}

#[cfg(unix)]
fn write_prepare_then_implement_agent(dir: &Path) -> PathBuf {
    let script_path = dir.join("prepare-then-implement-agent.sh");
    fs::write(
        &script_path,
        "#!/bin/sh\nlog_file=\"$1\"\npayload=$(cat)\ncase \"$payload\" in\n  *\"# Protocol: alpha_prepare\"*)\n    printf 'alpha_prepare\\n' >> \"$log_file\"\n    ;;\n  *\"# Protocol: beta_implement\"*)\n    printf 'beta_implement\\n' >> \"$log_file\"\n    mkdir -p .runa/workspace/implementation\n    printf '%s\\n' '{\"done\":true}' > .runa/workspace/implementation/impl-1.json\n    ;;\n  *)\n    printf '%s\\n' \"$payload\" > \"$log_file.unexpected\"\n    exit 19\n    ;;\nesac\n",
    )
    .unwrap();
    fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755)).unwrap();
    script_path
}

#[cfg(unix)]
fn write_scoped_prepare_then_revise_agent(dir: &Path) -> PathBuf {
    let script_path = dir.join("scoped-prepare-then-revise-agent.sh");
    fs::write(
        &script_path,
        "#!/bin/sh\nlog_file=\"$1\"\npayload=$(cat)\ncase \"$payload\" in\n  *\"# Protocol: prepare (work_unit=wu-a)\"*)\n    printf 'prepare:wu-a\\n' >> \"$log_file\"\n    ;;\n  *\"# Protocol: prepare (work_unit=wu-b)\"*)\n    printf 'prepare:wu-b\\n' >> \"$log_file\"\n    ;;\n  *\"# Protocol: revise (work_unit=wu-b)\"*)\n    printf 'revise:wu-b\\n' >> \"$log_file\"\n    mkdir -p .runa/workspace/constraints\n    printf '%s\\n' '{\"title\":\"updated-b\",\"work_unit\":\"wu-b\"}' > .runa/workspace/constraints/b.json\n    ;;\n  *)\n    printf '%s\\n' \"$payload\" > \"$log_file.unexpected\"\n    exit 19\n    ;;\nesac\n",
    )
    .unwrap();
    fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755)).unwrap();
    script_path
}

#[cfg(unix)]
fn write_scoped_prepare_then_failed_revise_agent(dir: &Path) -> PathBuf {
    let script_path = dir.join("scoped-prepare-then-failed-revise-agent.sh");
    fs::write(
        &script_path,
        "#!/bin/sh\nlog_file=\"$1\"\npayload=$(cat)\ncase \"$payload\" in\n  *\"# Protocol: prepare (work_unit=wu-a)\"*)\n    printf 'prepare:wu-a\\n' >> \"$log_file\"\n    ;;\n  *\"# Protocol: prepare (work_unit=wu-b)\"*)\n    printf 'prepare:wu-b\\n' >> \"$log_file\"\n    ;;\n  *\"# Protocol: revise (work_unit=wu-b)\"*)\n    printf 'revise:wu-b\\n' >> \"$log_file\"\n    mkdir -p .runa/workspace/constraints\n    printf '%s\\n' '{\"title\":\"updated-b\",\"work_unit\":\"wu-b\"}' > .runa/workspace/constraints/b.json\n    ;;\n  *)\n    printf '%s\\n' \"$payload\" > \"$log_file.unexpected\"\n    exit 19\n    ;;\nesac\n",
    )
    .unwrap();
    fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755)).unwrap();
    script_path
}

#[cfg(unix)]
fn write_scoped_prepare_then_agent_failed_revise_agent(dir: &Path) -> PathBuf {
    let script_path = dir.join("scoped-prepare-then-agent-failed-revise-agent.sh");
    fs::write(
        &script_path,
        "#!/bin/sh\nlog_file=\"$1\"\npayload=$(cat)\ncase \"$payload\" in\n  *\"# Protocol: prepare (work_unit=wu-a)\"*)\n    printf 'prepare:wu-a\\n' >> \"$log_file\"\n    ;;\n  *\"# Protocol: prepare (work_unit=wu-b)\"*)\n    printf 'prepare:wu-b\\n' >> \"$log_file\"\n    ;;\n  *\"# Protocol: revise (work_unit=wu-b)\"*)\n    printf 'revise:wu-b\\n' >> \"$log_file\"\n    mkdir -p .runa/workspace/constraints\n    printf '%s\\n' '{\"title\":\"updated-b\",\"work_unit\":\"wu-b\"}' > .runa/workspace/constraints/b.json\n    exit 17\n    ;;\n  *)\n    printf '%s\\n' \"$payload\" > \"$log_file.unexpected\"\n    exit 19\n    ;;\nesac\n",
    )
    .unwrap();
    fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755)).unwrap();
    script_path
}

#[cfg(unix)]
fn write_fail_first_then_continue_agent(dir: &Path) -> PathBuf {
    let script_path = dir.join("fail-first-then-continue-agent.sh");
    fs::write(
        &script_path,
        "#!/bin/sh\nlog_file=\"$1\"\npayload=$(cat)\ncase \"$payload\" in\n  *\"# Protocol: alpha_fail\"*)\n    printf 'alpha_fail\\n' >> \"$log_file\"\n    exit 17\n    ;;\n  *\"# Protocol: beta_succeed\"*)\n    printf 'beta_succeed\\n' >> \"$log_file\"\n    mkdir -p .runa/workspace/beta_done\n    printf '%s\\n' '{\"done\":true}' > .runa/workspace/beta_done/out.json\n    ;;\n  *)\n    printf '%s\\n' \"$payload\" > \"$log_file.unexpected\"\n    exit 19\n    ;;\nesac\n",
    )
    .unwrap();
    fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755)).unwrap();
    script_path
}

#[cfg(unix)]
fn write_prepare_notes_only_agent(dir: &Path) -> PathBuf {
    let script_path = dir.join("prepare-notes-only-agent.sh");
    fs::write(
        &script_path,
        "#!/bin/sh\nlog_file=\"$1\"\npayload=$(cat)\ncase \"$payload\" in\n  *\"# Protocol: prepare\"*)\n    printf 'prepare\\n' >> \"$log_file\"\n    mkdir -p .runa/workspace/notes\n    printf '%s\\n' '{\"title\":\"note\"}' > .runa/workspace/notes/note-1.json\n    ;;\n  *\"# Protocol: publish\"*)\n    printf 'publish\\n' >> \"$log_file\"\n    mkdir -p .runa/workspace/published\n    printf '%s\\n' '{\"done\":true}' > .runa/workspace/published/out-1.json\n    ;;\n  *)\n    printf '%s\\n' \"$payload\" > \"$log_file.unexpected\"\n    exit 19\n    ;;\nesac\n",
    )
    .unwrap();
    fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755)).unwrap();
    script_path
}

#[cfg(unix)]
fn write_interruptible_prepare_agent(dir: &Path) -> PathBuf {
    let script_path = dir.join("interruptible-prepare-agent.sh");
    fs::write(
        &script_path,
        "#!/bin/sh\nlog_file=\"$1\"\nsentinel=\"$2\"\npayload=$(cat)\ncase \"$payload\" in\n  *\"# Protocol: prepare\"*)\n    printf 'prepare-start\\n' >> \"$log_file\"\n    mkdir -p .runa/workspace/implementation\n    printf '%s\\n' '{\"done\":true}' > .runa/workspace/implementation/impl-1.json\n    : > \"$sentinel\"\n    sleep 2\n    printf 'prepare-end\\n' >> \"$log_file\"\n    ;;\n  *\"# Protocol: verify\"*)\n    printf 'verify\\n' >> \"$log_file\"\n    mkdir -p .runa/workspace/verified\n    printf '%s\\n' '{\"done\":true}' > .runa/workspace/verified/check-1.json\n    ;;\n  *)\n    printf '%s\\n' \"$payload\" > \"$log_file.unexpected\"\n    exit 19\n    ;;\nesac\n",
    )
    .unwrap();
    fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755)).unwrap();
    script_path
}

#[cfg(unix)]
fn write_parent_interrupting_agent(dir: &Path) -> PathBuf {
    let script_path = dir.join("parent-interrupting-agent.sh");
    fs::write(
        &script_path,
        "#!/bin/sh\nlog_file=\"$1\"\npayload=$(cat)\ncase \"$payload\" in\n  *\"# Protocol: prepare\"*)\n    printf 'prepare\\n' >> \"$log_file\"\n    mkdir -p .runa/workspace/implementation\n    printf '%s\\n' '{\"done\":true}' > .runa/workspace/implementation/impl-1.json\n    kill -INT \"$PPID\"\n    ;;\n  *)\n    printf '%s\\n' \"$payload\" > \"$log_file.unexpected\"\n    exit 19\n    ;;\nesac\n",
    )
    .unwrap();
    fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755)).unwrap();
    script_path
}

#[cfg(unix)]
fn wait_for_path(path: &Path, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if path.exists() {
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }

    panic!("timed out waiting for {}", path.display());
}

#[cfg(unix)]
fn send_sigint_to_process_group(pid: u32) {
    unsafe extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }

    const SIGINT: i32 = 2;
    let process_group = -(pid as i32);
    let rc = unsafe { kill(process_group, SIGINT) };
    assert_eq!(
        rc, 0,
        "failed to send SIGINT to process group {process_group}"
    );
}

#[cfg(unix)]
#[test]
fn run_without_dry_run_cascades_through_ready_protocols() {
    let dir = tempfile::tempdir().unwrap();
    let bool_schema =
        r#"{"type":"object","required":["done"],"properties":{"done":{"type":"boolean"}}}"#;
    let manifest_path = common::write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "constraints"

[[artifact_types]]
name = "implementation"

[[artifact_types]]
name = "verified"

[[protocols]]
name = "implement"
requires = ["constraints"]
produces = ["implementation"]
trigger = { type = "on_artifact", name = "constraints" }

[[protocols]]
name = "verify"
requires = ["implementation"]
produces = ["verified"]
trigger = { type = "on_artifact", name = "implementation" }
"#,
        &[
            (
                "constraints",
                r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#,
            ),
            ("implementation", bool_schema),
            ("verified", bool_schema),
        ],
        &["implement", "verify"],
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("constraints")).unwrap();
    fs::write(
        workspace.join("constraints/spec-1.json"),
        r#"{"title":"ship step"}"#,
    )
    .unwrap();

    let log_path = dir.path().join("executed.log");
    let agent_path = write_reconciling_agent(dir.path());
    append_agent_command_config(&project_dir, &[agent_path.as_path(), log_path.as_path()]);

    let output = runa_bin()
        .arg("run")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let executed = fs::read_to_string(&log_path).unwrap();
    assert_eq!(executed, "implement\nverify\n");
    assert!(workspace.join("implementation/impl-1.json").is_file());
    assert!(workspace.join("verified/check-1.json").is_file());
}

#[cfg(unix)]
#[test]
fn run_without_dry_run_stops_after_current_cycle_when_sigint_arrives_mid_execution() {
    let dir = tempfile::tempdir().unwrap();
    let bool_schema =
        r#"{"type":"object","required":["done"],"properties":{"done":{"type":"boolean"}}}"#;
    let manifest_path = common::write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "constraints"

[[artifact_types]]
name = "implementation"

[[artifact_types]]
name = "verified"

[[protocols]]
name = "prepare"
requires = ["constraints"]
produces = ["implementation"]
trigger = { type = "on_artifact", name = "constraints" }

[[protocols]]
name = "verify"
requires = ["implementation"]
produces = ["verified"]
trigger = { type = "on_artifact", name = "implementation" }
"#,
        &[
            (
                "constraints",
                r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#,
            ),
            ("implementation", bool_schema),
            ("verified", bool_schema),
        ],
        &["prepare", "verify"],
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("constraints")).unwrap();
    fs::write(
        workspace.join("constraints/spec-1.json"),
        r#"{"title":"ship run"}"#,
    )
    .unwrap();

    let log_path = dir.path().join("executed.log");
    let sentinel_path = dir.path().join("prepare.ready");
    let agent_path = write_interruptible_prepare_agent(dir.path());
    append_agent_command_config(
        &project_dir,
        &[
            agent_path.as_path(),
            log_path.as_path(),
            sentinel_path.as_path(),
        ],
    );

    let mut child = runa_bin();
    let child = child
        .arg("run")
        .current_dir(&project_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .unwrap();

    wait_for_path(&sentinel_path, Duration::from_secs(5));
    send_sigint_to_process_group(child.id());

    let output = child.wait_with_output().unwrap();

    assert_eq!(output.status.code(), Some(130), "{output:?}");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Run outcome: interrupted"),
        "stdout: {stdout}"
    );

    let executed = fs::read_to_string(&log_path).unwrap();
    assert_eq!(executed, "prepare-start\nprepare-end\n");
    assert!(workspace.join("implementation/impl-1.json").is_file());
    assert!(!workspace.join("verified/check-1.json").exists());
}

#[cfg(unix)]
#[test]
fn run_without_dry_run_prefers_quiescent_completion_when_sigint_arrives_in_final_cycle() {
    let dir = tempfile::tempdir().unwrap();
    let bool_schema =
        r#"{"type":"object","required":["done"],"properties":{"done":{"type":"boolean"}}}"#;
    let manifest_path = common::write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "constraints"

[[artifact_types]]
name = "implementation"

[[protocols]]
name = "prepare"
requires = ["constraints"]
produces = ["implementation"]
trigger = { type = "on_artifact", name = "constraints" }
"#,
        &[
            (
                "constraints",
                r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#,
            ),
            ("implementation", bool_schema),
        ],
        &["prepare"],
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("constraints")).unwrap();
    fs::write(
        workspace.join("constraints/spec-1.json"),
        r#"{"title":"ship run"}"#,
    )
    .unwrap();

    let log_path = dir.path().join("executed.log");
    let agent_path = write_parent_interrupting_agent(dir.path());
    append_agent_command_config(&project_dir, &[agent_path.as_path(), log_path.as_path()]);

    let output = runa_bin()
        .arg("run")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Run outcome: all_complete"),
        "stdout: {stdout}"
    );
    assert!(
        !stdout.contains("Run outcome: interrupted"),
        "stdout: {stdout}"
    );

    let executed = fs::read_to_string(&log_path).unwrap();
    assert_eq!(executed, "prepare\n");
    assert!(workspace.join("implementation/impl-1.json").is_file());
}

#[test]
fn run_dry_run_json_projects_the_full_cascade() {
    let dir = tempfile::tempdir().unwrap();
    let bool_schema =
        r#"{"type":"object","required":["done"],"properties":{"done":{"type":"boolean"}}}"#;
    let manifest_path = common::write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "constraints"

[[artifact_types]]
name = "implementation"

[[artifact_types]]
name = "verified"

[[protocols]]
name = "implement"
requires = ["constraints"]
produces = ["implementation"]
trigger = { type = "on_artifact", name = "constraints" }

[[protocols]]
name = "verify"
requires = ["implementation"]
produces = ["verified"]
trigger = { type = "on_artifact", name = "implementation" }
"#,
        &[
            (
                "constraints",
                r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#,
            ),
            ("implementation", bool_schema),
            ("verified", bool_schema),
        ],
        &["implement", "verify"],
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("constraints")).unwrap();
    fs::write(
        workspace.join("constraints/spec-1.json"),
        r#"{"title":"ship step"}"#,
    )
    .unwrap();

    let output = runa_bin()
        .arg("run")
        .arg("--dry-run")
        .arg("--json")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(3), "{output:?}");

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let execution_plan = value["execution_plan"].as_array().unwrap();
    assert_eq!(execution_plan.len(), 2, "{value:#}");
    assert_eq!(execution_plan[0]["protocol"], "implement");
    assert_eq!(execution_plan[1]["protocol"], "verify");
}

#[test]
fn run_dry_run_with_blocked_work_and_no_ready_protocols_returns_exit_3() {
    let dir = tempfile::tempdir().unwrap();
    let bool_schema =
        r#"{"type":"object","required":["done"],"properties":{"done":{"type":"boolean"}}}"#;
    let manifest_path = common::write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "constraints"

[[artifact_types]]
name = "implementation"

[[protocols]]
name = "verify"
requires = ["implementation"]
trigger = { type = "on_artifact", name = "constraints" }
"#,
        &[
            (
                "constraints",
                r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#,
            ),
            ("implementation", bool_schema),
        ],
        &["verify"],
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("constraints")).unwrap();
    fs::write(
        workspace.join("constraints/spec-1.json"),
        r#"{"title":"ship step"}"#,
    )
    .unwrap();

    let output = runa_bin()
        .arg("run")
        .arg("--dry-run")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(3), "{output:?}");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Execution plan: none"), "stdout: {stdout}");
}

#[test]
fn run_dry_run_with_cyclic_ready_work_returns_exit_3() {
    let dir = tempfile::tempdir().unwrap();
    let title_schema =
        r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#;
    let manifest_path = common::write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "seed-a"

[[artifact_types]]
name = "seed-b"

[[artifact_types]]
name = "a"

[[artifact_types]]
name = "b"

[[protocols]]
name = "first"
requires = ["b"]
produces = ["a"]
trigger = { type = "on_artifact", name = "seed-a" }

[[protocols]]
name = "second"
requires = ["a"]
produces = ["b"]
trigger = { type = "on_artifact", name = "seed-b" }
"#,
        &[
            ("seed-a", title_schema),
            ("seed-b", title_schema),
            ("a", title_schema),
            ("b", title_schema),
        ],
        &["first", "second"],
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("a")).unwrap();
    fs::create_dir_all(workspace.join("b")).unwrap();
    fs::create_dir_all(workspace.join("seed-a")).unwrap();
    fs::create_dir_all(workspace.join("seed-b")).unwrap();
    fs::write(workspace.join("a/current.json"), r#"{"title":"a"}"#).unwrap();
    fs::write(workspace.join("b/current.json"), r#"{"title":"b"}"#).unwrap();
    let first_scan = runa_bin()
        .arg("scan")
        .current_dir(&project_dir)
        .output()
        .unwrap();
    assert!(
        first_scan.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&first_scan.stderr)
    );
    fs::write(workspace.join("seed-a/input.json"), r#"{"title":"seed-a"}"#).unwrap();
    fs::write(workspace.join("seed-b/input.json"), r#"{"title":"seed-b"}"#).unwrap();

    let output = runa_bin()
        .arg("run")
        .arg("--dry-run")
        .arg("--json")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(3), "{output:?}");

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        value["cycle"],
        serde_json::json!(["first", "second"]),
        "{value:#}"
    );
    assert_eq!(value["execution_plan"], serde_json::json!([]), "{value:#}");
    let protocols = value["protocols"].as_array().unwrap();
    assert_eq!(protocols.len(), 2, "{value:#}");
    assert_eq!(protocols[0]["name"], "first");
    assert_eq!(protocols[0]["status"], "ready");
    assert_eq!(protocols[1]["name"], "second");
    assert_eq!(protocols[1]["status"], "ready");
}

#[test]
fn run_with_cyclic_ready_work_returns_exit_3_without_agent_config() {
    let dir = tempfile::tempdir().unwrap();
    let title_schema =
        r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#;
    let manifest_path = common::write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "seed-a"

[[artifact_types]]
name = "seed-b"

[[artifact_types]]
name = "a"

[[artifact_types]]
name = "b"

[[protocols]]
name = "first"
requires = ["b"]
produces = ["a"]
trigger = { type = "on_artifact", name = "seed-a" }

[[protocols]]
name = "second"
requires = ["a"]
produces = ["b"]
trigger = { type = "on_artifact", name = "seed-b" }
"#,
        &[
            ("seed-a", title_schema),
            ("seed-b", title_schema),
            ("a", title_schema),
            ("b", title_schema),
        ],
        &["first", "second"],
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("a")).unwrap();
    fs::create_dir_all(workspace.join("b")).unwrap();
    fs::create_dir_all(workspace.join("seed-a")).unwrap();
    fs::create_dir_all(workspace.join("seed-b")).unwrap();
    fs::write(workspace.join("a/current.json"), r#"{"title":"a"}"#).unwrap();
    fs::write(workspace.join("b/current.json"), r#"{"title":"b"}"#).unwrap();
    let first_scan = runa_bin()
        .arg("scan")
        .current_dir(&project_dir)
        .output()
        .unwrap();
    assert!(
        first_scan.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&first_scan.stderr)
    );
    fs::write(workspace.join("seed-a/input.json"), r#"{"title":"seed-a"}"#).unwrap();
    fs::write(workspace.join("seed-b/input.json"), r#"{"title":"seed-b"}"#).unwrap();

    let output = runa_bin()
        .arg("run")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(3), "{output:?}");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Run outcome: quiescent_with_blocked_work"),
        "stdout: {stdout}"
    );
}

#[test]
fn run_dry_run_does_not_project_may_produce_outputs() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "doc"

[[artifact_types]]
name = "reviewed"

[[artifact_types]]
name = "notes"

[[artifact_types]]
name = "published"

[[protocols]]
name = "review"
requires = ["doc"]
produces = ["reviewed"]
may_produce = ["notes"]
trigger = { type = "on_artifact", name = "doc" }

[[protocols]]
name = "publish"
requires = ["notes"]
produces = ["published"]
trigger = { type = "on_artifact", name = "notes" }
"#,
        &[
            (
                "doc",
                r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#,
            ),
            (
                "reviewed",
                r#"{"type":"object","required":["done"],"properties":{"done":{"type":"boolean"}}}"#,
            ),
            (
                "notes",
                r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#,
            ),
            (
                "published",
                r#"{"type":"object","required":["done"],"properties":{"done":{"type":"boolean"}}}"#,
            ),
        ],
        &["review", "publish"],
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("doc")).unwrap();
    fs::write(workspace.join("doc/input.json"), r#"{"title":"draft"}"#).unwrap();

    let output = runa_bin()
        .arg("run")
        .arg("--dry-run")
        .arg("--json")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(3), "{output:?}");

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let execution_plan = value["execution_plan"].as_array().unwrap();
    assert_eq!(execution_plan.len(), 1, "{value:#}");
    assert_eq!(execution_plan[0]["protocol"], "review");
    assert_eq!(execution_plan[0]["projection"], "current");
}

#[test]
fn run_dry_run_json_current_entries_do_not_include_projected_accepted_inputs() {
    let dir = tempfile::tempdir().unwrap();
    let title_schema =
        r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#;
    let bool_schema =
        r#"{"type":"object","required":["done"],"properties":{"done":{"type":"boolean"}}}"#;
    let manifest_path = common::write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "seed"

[[artifact_types]]
name = "notes"

[[artifact_types]]
name = "published"

[[protocols]]
name = "prepare"
requires = ["seed"]
produces = ["notes"]
trigger = { type = "on_artifact", name = "seed" }

[[protocols]]
name = "publish"
requires = ["seed"]
accepts = ["notes"]
produces = ["published"]
trigger = { type = "on_artifact", name = "seed" }
"#,
        &[
            ("seed", title_schema),
            ("notes", title_schema),
            ("published", bool_schema),
        ],
        &["prepare", "publish"],
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("seed")).unwrap();
    fs::write(workspace.join("seed/input.json"), r#"{"title":"ship"}"#).unwrap();

    let output = runa_bin()
        .arg("run")
        .arg("--dry-run")
        .arg("--json")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0), "{output:?}");

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let execution_plan = value["execution_plan"].as_array().unwrap();
    assert_eq!(execution_plan.len(), 2, "{value:#}");
    assert_eq!(execution_plan[0]["protocol"], "prepare");
    assert_eq!(execution_plan[0]["projection"], "current");
    assert_eq!(execution_plan[1]["protocol"], "publish");
    assert_eq!(execution_plan[1]["projection"], "current");
    let inputs = execution_plan[1]["context"]["inputs"].as_array().unwrap();
    assert_eq!(inputs.len(), 1, "{value:#}");
    assert_eq!(inputs[0]["artifact_type"], "seed");
    assert_eq!(inputs[0]["instance_id"], "input");
    assert_eq!(
        inputs[0]["display_path"],
        serde_json::Value::String(workspace.join("seed/input.json").display().to_string())
    );
    assert_eq!(inputs[0]["relationship"], "requires");
}

#[test]
fn run_dry_run_projects_fan_out_in_graph_order() {
    let dir = tempfile::tempdir().unwrap();
    let title_schema =
        r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#;
    let bool_schema =
        r#"{"type":"object","required":["done"],"properties":{"done":{"type":"boolean"}}}"#;
    let manifest_path = common::write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "seed"

[[artifact_types]]
name = "left"

[[artifact_types]]
name = "right"

[[protocols]]
name = "build"
requires = ["seed"]
produces = ["left", "right"]
trigger = { type = "on_artifact", name = "seed" }

[[protocols]]
name = "left_verify"
requires = ["left"]
trigger = { type = "on_artifact", name = "left" }

[[protocols]]
name = "right_verify"
requires = ["right"]
produces = ["done"]
trigger = { type = "on_artifact", name = "right" }

[[artifact_types]]
name = "done"
"#,
        &[
            ("seed", title_schema),
            ("left", title_schema),
            ("right", title_schema),
            ("done", bool_schema),
        ],
        &["build", "left_verify", "right_verify"],
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("seed")).unwrap();
    fs::write(workspace.join("seed/source.json"), r#"{"title":"draft"}"#).unwrap();

    let output = runa_bin()
        .arg("run")
        .arg("--dry-run")
        .arg("--json")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(3), "{output:?}");

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let execution_plan = value["execution_plan"].as_array().unwrap();
    assert_eq!(execution_plan.len(), 3, "{value:#}");
    assert_eq!(execution_plan[0]["protocol"], "build");
    assert_eq!(execution_plan[0]["projection"], "current");
    assert!(
        execution_plan[1]["protocol"] == "left_verify"
            || execution_plan[1]["protocol"] == "right_verify",
        "{value:#}"
    );
    assert_eq!(execution_plan[1]["projection"], "projected");
    assert!(
        execution_plan[2]["protocol"] == "left_verify"
            || execution_plan[2]["protocol"] == "right_verify",
        "{value:#}"
    );
    assert_ne!(execution_plan[1]["protocol"], execution_plan[2]["protocol"]);
    assert_eq!(execution_plan[2]["projection"], "projected");
}

#[test]
fn run_dry_run_projects_fan_in_after_all_dependencies_exist() {
    let dir = tempfile::tempdir().unwrap();
    let title_schema =
        r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#;
    let bool_schema =
        r#"{"type":"object","required":["done"],"properties":{"done":{"type":"boolean"}}}"#;
    let manifest_path = common::write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "seed"

[[artifact_types]]
name = "left"

[[artifact_types]]
name = "right"

[[artifact_types]]
name = "joined"

[[protocols]]
name = "build_left"
requires = ["seed"]
produces = ["left"]
trigger = { type = "on_artifact", name = "seed" }

[[protocols]]
name = "build_right"
requires = ["seed"]
produces = ["right"]
trigger = { type = "on_artifact", name = "seed" }

[[protocols]]
name = "join"
requires = ["left", "right"]
produces = ["joined"]
trigger = { type = "all_of", conditions = [
  { type = "on_artifact", name = "left" },
  { type = "on_artifact", name = "right" }
] }
"#,
        &[
            ("seed", title_schema),
            ("left", title_schema),
            ("right", title_schema),
            ("joined", bool_schema),
        ],
        &["build_left", "build_right", "join"],
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("seed")).unwrap();
    fs::write(workspace.join("seed/source.json"), r#"{"title":"draft"}"#).unwrap();

    let output = runa_bin()
        .arg("run")
        .arg("--dry-run")
        .arg("--json")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(3), "{output:?}");

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let execution_plan = value["execution_plan"].as_array().unwrap();
    assert_eq!(execution_plan.len(), 3, "{value:#}");
    assert!(
        execution_plan[0]["protocol"] == "build_left"
            || execution_plan[0]["protocol"] == "build_right",
        "{value:#}"
    );
    assert_eq!(execution_plan[0]["projection"], "current");
    assert!(
        execution_plan[1]["protocol"] == "build_left"
            || execution_plan[1]["protocol"] == "build_right",
        "{value:#}"
    );
    assert_ne!(execution_plan[0]["protocol"], execution_plan[1]["protocol"]);
    assert_eq!(execution_plan[1]["projection"], "current");
    assert_eq!(execution_plan[2]["protocol"], "join");
    assert_eq!(execution_plan[2]["projection"], "projected");
}

#[test]
fn run_dry_run_projects_scoped_downstream_work_from_graph_work_units() {
    let dir = tempfile::tempdir().unwrap();
    let wu_title_schema = r#"{"type":"object","required":["title","work_unit"],"properties":{"title":{"type":"string"},"work_unit":{"type":"string"}}}"#;
    let wu_bool_schema = r#"{"type":"object","required":["done","work_unit"],"properties":{"done":{"type":"boolean"},"work_unit":{"type":"string"}}}"#;
    let manifest_path = common::write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "input"

[[artifact_types]]
name = "constrained"

[[artifact_types]]
name = "verified"

[[protocols]]
name = "build"
requires = ["input"]
produces = ["constrained"]
trigger = { type = "on_artifact", name = "input" }

[[protocols]]
name = "verify"
requires = ["constrained"]
produces = ["verified"]
trigger = { type = "on_artifact", name = "constrained" }
"#,
        &[
            ("input", wu_title_schema),
            ("constrained", r#"{"type":"string","minLength":1}"#),
            ("verified", wu_bool_schema),
        ],
        &["build", "verify"],
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("input")).unwrap();
    fs::write(
        workspace.join("input/a.json"),
        r#"{"title":"draft-a","work_unit":"wu-a"}"#,
    )
    .unwrap();
    fs::write(
        workspace.join("input/b.json"),
        r#"{"title":"draft-b","work_unit":"wu-b"}"#,
    )
    .unwrap();

    let output = runa_bin()
        .arg("run")
        .arg("--dry-run")
        .arg("--json")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(3), "{output:?}");

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let execution_plan = value["execution_plan"].as_array().unwrap();
    assert_eq!(execution_plan.len(), 4, "{value:#}");
    assert_eq!(execution_plan[0]["protocol"], "build");
    assert_eq!(execution_plan[0]["work_unit"], "wu-a");
    assert_eq!(execution_plan[0]["projection"], "current");
    assert_eq!(execution_plan[1]["protocol"], "build");
    assert_eq!(execution_plan[1]["work_unit"], "wu-b");
    assert_eq!(execution_plan[1]["projection"], "current");
    assert_eq!(execution_plan[2]["protocol"], "verify");
    assert_eq!(execution_plan[2]["work_unit"], "wu-a");
    assert_eq!(execution_plan[2]["projection"], "projected");
    assert_eq!(execution_plan[3]["protocol"], "verify");
    assert_eq!(execution_plan[3]["work_unit"], "wu-b");
    assert_eq!(execution_plan[3]["projection"], "projected");
}

#[cfg(unix)]
#[test]
fn run_dry_run_preserves_partial_scan_blocking_in_projection() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "seed"

[[artifact_types]]
name = "constraints"

[[artifact_types]]
name = "implementation"

[[artifact_types]]
name = "verified"

[[protocols]]
name = "prepare"
requires = ["seed"]
produces = ["implementation"]
trigger = { type = "on_artifact", name = "seed" }

[[protocols]]
name = "verify"
requires = ["implementation", "constraints"]
produces = ["verified"]
trigger = { type = "on_artifact", name = "implementation" }
"#,
        &[
            (
                "seed",
                r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#,
            ),
            (
                "constraints",
                r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#,
            ),
            (
                "implementation",
                r#"{"type":"object","required":["done"],"properties":{"done":{"type":"boolean"}}}"#,
            ),
            (
                "verified",
                r#"{"type":"object","required":["done"],"properties":{"done":{"type":"boolean"}}}"#,
            ),
        ],
        &["prepare", "verify"],
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("seed")).unwrap();
    fs::create_dir_all(workspace.join("constraints")).unwrap();
    fs::write(workspace.join("seed/source.json"), r#"{"title":"draft"}"#).unwrap();
    fs::write(
        workspace.join("constraints/visible.json"),
        r#"{"title":"visible"}"#,
    )
    .unwrap();
    let unreadable = workspace.join("constraints/hidden.json");
    fs::write(&unreadable, r#"{"title":"hidden"}"#).unwrap();
    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o0)).unwrap();

    let output = runa_bin()
        .arg("run")
        .arg("--dry-run")
        .arg("--json")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o644)).unwrap();

    assert_eq!(output.status.code(), Some(3), "{output:?}");

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let execution_plan = value["execution_plan"].as_array().unwrap();
    assert_eq!(execution_plan.len(), 1, "{value:#}");
    assert_eq!(execution_plan[0]["protocol"], "prepare");
}

#[cfg(unix)]
#[test]
fn run_without_dry_run_does_not_rerun_outputless_protocols_after_unrelated_transitions() {
    let dir = tempfile::tempdir().unwrap();
    let bool_schema =
        r#"{"type":"object","required":["done"],"properties":{"done":{"type":"boolean"}}}"#;
    let manifest_path = common::write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "constraints"

[[artifact_types]]
name = "implementation"

[[protocols]]
name = "beta_implement"
requires = ["constraints"]
produces = ["implementation"]
trigger = { type = "on_artifact", name = "constraints" }

[[protocols]]
name = "alpha_prepare"
requires = ["constraints"]
trigger = { type = "on_artifact", name = "constraints" }
"#,
        &[
            (
                "constraints",
                r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#,
            ),
            ("implementation", bool_schema),
        ],
        &["alpha_prepare", "beta_implement"],
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("constraints")).unwrap();
    fs::write(
        workspace.join("constraints/spec-1.json"),
        r#"{"title":"ship step"}"#,
    )
    .unwrap();

    let log_path = dir.path().join("executed.log");
    let agent_path = write_prepare_then_implement_agent(dir.path());
    append_agent_command_config(&project_dir, &[agent_path.as_path(), log_path.as_path()]);

    let output = runa_bin()
        .arg("run")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let executed = fs::read_to_string(&log_path).unwrap();
    assert_eq!(executed, "alpha_prepare\nbeta_implement\n");
    assert!(workspace.join("implementation/impl-1.json").is_file());
}

#[cfg(unix)]
#[test]
fn run_without_dry_run_only_reopens_scoped_outputless_work_for_matching_work_unit() {
    let dir = tempfile::tempdir().unwrap();
    let wu_schema = r#"{"type":"object","required":["title","work_unit"],"properties":{"title":{"type":"string"},"work_unit":{"type":"string"}}}"#;
    let manifest_path = common::write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "constraints"

[[artifact_types]]
name = "seed"

[[protocols]]
name = "revise"
requires = ["seed"]
produces = ["constraints"]
trigger = { type = "on_artifact", name = "seed" }

[[protocols]]
name = "prepare"
trigger = { type = "on_artifact", name = "constraints" }
"#,
        &[("constraints", wu_schema), ("seed", wu_schema)],
        &["revise", "prepare"],
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("constraints")).unwrap();
    fs::create_dir_all(workspace.join("seed")).unwrap();
    fs::write(
        workspace.join("constraints/a.json"),
        r#"{"title":"draft-a","work_unit":"wu-a"}"#,
    )
    .unwrap();
    fs::write(
        workspace.join("constraints/b.json"),
        r#"{"title":"draft-b","work_unit":"wu-b"}"#,
    )
    .unwrap();
    let first_scan = runa_bin()
        .arg("scan")
        .current_dir(&project_dir)
        .output()
        .unwrap();
    assert!(
        first_scan.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&first_scan.stderr)
    );
    fs::write(
        workspace.join("seed/b.json"),
        r#"{"title":"seed-b","work_unit":"wu-b"}"#,
    )
    .unwrap();

    let log_path = dir.path().join("executed.log");
    let agent_path = write_scoped_prepare_then_revise_agent(dir.path());
    append_agent_command_config(&project_dir, &[agent_path.as_path(), log_path.as_path()]);

    let output = runa_bin()
        .arg("run")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let executed = fs::read_to_string(&log_path).unwrap();
    assert_eq!(
        executed,
        "prepare:wu-a\nprepare:wu-b\nrevise:wu-b\nprepare:wu-b\n"
    );
}

#[test]
fn run_dry_run_marks_reopened_initial_candidates_as_projected() {
    let dir = tempfile::tempdir().unwrap();
    let title_schema =
        r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#;
    let bool_schema =
        r#"{"type":"object","required":["done"],"properties":{"done":{"type":"boolean"}}}"#;
    let manifest_path = common::write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "draft"

[[artifact_types]]
name = "seed"

[[artifact_types]]
name = "notes"

[[artifact_types]]
name = "prepared"

[[protocols]]
name = "beta_collect"
produces = ["notes"]
trigger = { type = "on_artifact", name = "seed" }

[[protocols]]
name = "alpha_prepare"
produces = ["prepared"]
trigger = { type = "on_artifact", name = "draft" }

[[protocols]]
name = "gamma_revise"
requires = ["notes"]
produces = ["draft"]
trigger = { type = "on_artifact", name = "notes" }
"#,
        &[
            ("draft", title_schema),
            ("seed", title_schema),
            ("notes", title_schema),
            ("prepared", bool_schema),
        ],
        &["beta_collect", "alpha_prepare", "gamma_revise"],
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("draft")).unwrap();
    fs::create_dir_all(workspace.join("seed")).unwrap();
    fs::write(workspace.join("draft/current.json"), r#"{"title":"draft"}"#).unwrap();
    fs::write(workspace.join("seed/current.json"), r#"{"title":"seed"}"#).unwrap();

    let output = runa_bin()
        .arg("run")
        .arg("--dry-run")
        .arg("--json")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(3), "{output:?}");

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let execution_plan = value["execution_plan"].as_array().unwrap();
    assert_eq!(execution_plan.len(), 4, "{value:#}");
    assert_eq!(execution_plan[0]["protocol"], "alpha_prepare");
    assert_eq!(execution_plan[0]["projection"], "current");
    assert!(execution_plan[0]["context"].is_object(), "{value:#}");
    assert!(execution_plan[0]["mcp_config"].is_object(), "{value:#}");
    assert_eq!(execution_plan[1]["protocol"], "beta_collect");
    assert_eq!(execution_plan[1]["projection"], "current");
    assert_eq!(execution_plan[2]["protocol"], "gamma_revise");
    assert_eq!(execution_plan[2]["projection"], "projected");
    assert_eq!(execution_plan[3]["protocol"], "alpha_prepare");
    assert_eq!(execution_plan[3]["projection"], "projected");
    assert!(execution_plan[3]["context"].is_null(), "{value:#}");
    assert!(execution_plan[3]["mcp_config"].is_null(), "{value:#}");
}

#[cfg(unix)]
#[test]
fn run_without_dry_run_reopens_exhausted_work_after_postcondition_failure() {
    let dir = tempfile::tempdir().unwrap();
    let wu_schema = r#"{"type":"object","required":["title","work_unit"],"properties":{"title":{"type":"string"},"work_unit":{"type":"string"}}}"#;
    let bool_schema =
        r#"{"type":"object","required":["done"],"properties":{"done":{"type":"boolean"}}}"#;
    let manifest_path = common::write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "constraints"

[[artifact_types]]
name = "seed"

[[artifact_types]]
name = "implementation"

[[protocols]]
name = "revise"
requires = ["seed"]
produces = ["constraints", "implementation"]
trigger = { type = "on_artifact", name = "seed" }

[[protocols]]
name = "prepare"
trigger = { type = "on_artifact", name = "constraints" }
"#,
        &[
            ("constraints", wu_schema),
            ("seed", wu_schema),
            ("implementation", bool_schema),
        ],
        &["revise", "prepare"],
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("constraints")).unwrap();
    fs::create_dir_all(workspace.join("seed")).unwrap();
    fs::write(
        workspace.join("constraints/a.json"),
        r#"{"title":"draft-a","work_unit":"wu-a"}"#,
    )
    .unwrap();
    fs::write(
        workspace.join("constraints/b.json"),
        r#"{"title":"draft-b","work_unit":"wu-b"}"#,
    )
    .unwrap();
    let first_scan = runa_bin()
        .arg("scan")
        .current_dir(&project_dir)
        .output()
        .unwrap();
    assert!(
        first_scan.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&first_scan.stderr)
    );
    fs::write(
        workspace.join("seed/b.json"),
        r#"{"title":"seed-b","work_unit":"wu-b"}"#,
    )
    .unwrap();

    let log_path = dir.path().join("executed.log");
    let agent_path = write_scoped_prepare_then_failed_revise_agent(dir.path());
    append_agent_command_config(&project_dir, &[agent_path.as_path(), log_path.as_path()]);

    let output = runa_bin()
        .arg("run")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2), "{output:?}");

    let executed = fs::read_to_string(&log_path).unwrap();
    assert_eq!(
        executed,
        "prepare:wu-a\nprepare:wu-b\nrevise:wu-b\nprepare:wu-b\n"
    );
    assert!(!workspace.join("implementation/out.json").exists());
}

#[cfg(unix)]
#[test]
fn run_without_dry_run_reopens_exhausted_work_after_agent_failure() {
    let dir = tempfile::tempdir().unwrap();
    let wu_schema = r#"{"type":"object","required":["title","work_unit"],"properties":{"title":{"type":"string"},"work_unit":{"type":"string"}}}"#;
    let bool_schema =
        r#"{"type":"object","required":["done"],"properties":{"done":{"type":"boolean"}}}"#;
    let manifest_path = common::write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "constraints"

[[artifact_types]]
name = "seed"

[[artifact_types]]
name = "implementation"

[[protocols]]
name = "revise"
requires = ["seed"]
produces = ["constraints", "implementation"]
trigger = { type = "on_artifact", name = "seed" }

[[protocols]]
name = "prepare"
trigger = { type = "on_artifact", name = "constraints" }
"#,
        &[
            ("constraints", wu_schema),
            ("seed", wu_schema),
            ("implementation", bool_schema),
        ],
        &["revise", "prepare"],
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("constraints")).unwrap();
    fs::create_dir_all(workspace.join("seed")).unwrap();
    fs::write(
        workspace.join("constraints/a.json"),
        r#"{"title":"draft-a","work_unit":"wu-a"}"#,
    )
    .unwrap();
    fs::write(
        workspace.join("constraints/b.json"),
        r#"{"title":"draft-b","work_unit":"wu-b"}"#,
    )
    .unwrap();
    let first_scan = runa_bin()
        .arg("scan")
        .current_dir(&project_dir)
        .output()
        .unwrap();
    assert!(
        first_scan.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&first_scan.stderr)
    );
    fs::write(
        workspace.join("seed/b.json"),
        r#"{"title":"seed-b","work_unit":"wu-b"}"#,
    )
    .unwrap();

    let log_path = dir.path().join("executed.log");
    let agent_path = write_scoped_prepare_then_agent_failed_revise_agent(dir.path());
    append_agent_command_config(&project_dir, &[agent_path.as_path(), log_path.as_path()]);

    let output = runa_bin()
        .arg("run")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2), "{output:?}");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Run outcome: quiescent_with_failures"),
        "stdout: {stdout}"
    );

    let executed = fs::read_to_string(&log_path).unwrap();
    assert_eq!(
        executed,
        "prepare:wu-a\nprepare:wu-b\nrevise:wu-b\nprepare:wu-b\n"
    );
    assert!(!workspace.join("implementation/out.json").exists());
}

#[cfg(unix)]
#[test]
fn run_without_dry_run_continues_after_a_protocol_failure_and_returns_exit_2() {
    let dir = tempfile::tempdir().unwrap();
    let bool_schema =
        r#"{"type":"object","required":["done"],"properties":{"done":{"type":"boolean"}}}"#;
    let manifest_path = common::write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "constraints"

[[artifact_types]]
name = "alpha_done"

[[artifact_types]]
name = "beta_done"

[[protocols]]
name = "alpha_fail"
requires = ["constraints"]
produces = ["alpha_done"]
trigger = { type = "on_artifact", name = "constraints" }

[[protocols]]
name = "beta_succeed"
requires = ["constraints"]
produces = ["beta_done"]
trigger = { type = "on_artifact", name = "constraints" }
"#,
        &[
            (
                "constraints",
                r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#,
            ),
            ("alpha_done", bool_schema),
            ("beta_done", bool_schema),
        ],
        &["alpha_fail", "beta_succeed"],
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("constraints")).unwrap();
    fs::write(
        workspace.join("constraints/spec-1.json"),
        r#"{"title":"ship step"}"#,
    )
    .unwrap();

    let log_path = dir.path().join("executed.log");
    let agent_path = write_fail_first_then_continue_agent(dir.path());
    append_agent_command_config(&project_dir, &[agent_path.as_path(), log_path.as_path()]);

    let output = runa_bin()
        .arg("run")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2), "{output:?}");

    let executed = fs::read_to_string(&log_path).unwrap();
    assert!(executed.contains("alpha_fail\n"), "{executed}");
    assert!(executed.contains("beta_succeed\n"), "{executed}");
    assert!(workspace.join("beta_done/out.json").is_file());
    assert!(!workspace.join("alpha_done/out.json").exists());
}

#[test]
fn run_with_blocked_work_and_no_ready_protocols_returns_exit_3() {
    let dir = tempfile::tempdir().unwrap();
    let bool_schema =
        r#"{"type":"object","required":["done"],"properties":{"done":{"type":"boolean"}}}"#;
    let manifest_path = common::write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "constraints"

[[artifact_types]]
name = "implementation"

[[protocols]]
name = "verify"
requires = ["implementation"]
trigger = { type = "on_artifact", name = "constraints" }
"#,
        &[
            (
                "constraints",
                r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#,
            ),
            ("implementation", bool_schema),
        ],
        &["verify"],
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("constraints")).unwrap();
    fs::write(
        workspace.join("constraints/spec-1.json"),
        r#"{"title":"ship step"}"#,
    )
    .unwrap();

    let output = runa_bin()
        .arg("run")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(3), "{output:?}");
}

#[cfg(unix)]
#[test]
fn run_preserves_scan_gap_blocking_after_postcondition_failure() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "seed"

[[artifact_types]]
name = "constraints"

[[artifact_types]]
name = "implementation"

[[artifact_types]]
name = "notes"

[[artifact_types]]
name = "published"

[[protocols]]
name = "prepare"
requires = ["seed"]
produces = ["implementation"]
may_produce = ["notes"]
trigger = { type = "on_artifact", name = "seed" }

[[protocols]]
name = "publish"
requires = ["constraints", "notes"]
produces = ["published"]
trigger = { type = "on_artifact", name = "notes" }
"#,
        &[
            (
                "seed",
                r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#,
            ),
            (
                "constraints",
                r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#,
            ),
            (
                "implementation",
                r#"{"type":"object","required":["done"],"properties":{"done":{"type":"boolean"}}}"#,
            ),
            (
                "notes",
                r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#,
            ),
            (
                "published",
                r#"{"type":"object","required":["done"],"properties":{"done":{"type":"boolean"}}}"#,
            ),
        ],
        &["prepare", "publish"],
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("seed")).unwrap();
    fs::create_dir_all(workspace.join("constraints")).unwrap();
    fs::write(workspace.join("seed/source.json"), r#"{"title":"draft"}"#).unwrap();
    fs::write(
        workspace.join("constraints/visible.json"),
        r#"{"title":"visible"}"#,
    )
    .unwrap();
    let unreadable = workspace.join("constraints/hidden.json");
    fs::write(&unreadable, r#"{"title":"hidden"}"#).unwrap();
    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o0)).unwrap();

    let log_path = dir.path().join("executed.log");
    let agent_path = write_prepare_notes_only_agent(dir.path());
    append_agent_command_config(&project_dir, &[agent_path.as_path(), log_path.as_path()]);

    let output = runa_bin()
        .arg("run")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o644)).unwrap();

    assert_eq!(output.status.code(), Some(2), "{output:?}");

    let executed = fs::read_to_string(&log_path).unwrap();
    assert_eq!(executed, "prepare\n");
    assert!(!workspace.join("published/out-1.json").exists());
}
