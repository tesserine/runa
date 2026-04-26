mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

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

fn setup_quiescent_run_project() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "seed"

[[artifact_types]]
name = "result"

[[artifact_types]]
name = "a"

[[artifact_types]]
name = "b"

[[protocols]]
name = "publish"
requires = ["seed"]
produces = ["result"]
trigger = { type = "on_artifact", name = "seed" }

[[protocols]]
name = "first"
requires = ["b"]
produces = ["a"]
scoped = true
trigger = { type = "on_artifact", name = "b" }

[[protocols]]
name = "second"
requires = ["a"]
produces = ["b"]
scoped = true
trigger = { type = "on_artifact", name = "a" }
"#,
        &[
            (
                "seed",
                r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#,
            ),
            (
                "result",
                r#"{"type":"object","required":["done"],"properties":{"done":{"type":"boolean"}}}"#,
            ),
            (
                "a",
                r#"{"type":"object","required":["title","work_unit"],"properties":{"title":{"type":"string"},"work_unit":{"type":"string"}}}"#,
            ),
            (
                "b",
                r#"{"type":"object","required":["title","work_unit"],"properties":{"title":{"type":"string"},"work_unit":{"type":"string"}}}"#,
            ),
        ],
        &["publish", "first", "second"],
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("seed")).unwrap();
    fs::create_dir_all(workspace.join("result")).unwrap();
    fs::write(workspace.join("seed/input.json"), r#"{"title":"ship"}"#).unwrap();
    fs::write(workspace.join("result/current.json"), r#"{"done":true}"#).unwrap();
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
    fs::create_dir_all(workspace.join("a")).unwrap();
    fs::create_dir_all(workspace.join("b")).unwrap();
    fs::write(
        workspace.join("a/current.json"),
        r#"{"title":"a","work_unit":"wu-a"}"#,
    )
    .unwrap();
    fs::write(
        workspace.join("b/current.json"),
        r#"{"title":"b","work_unit":"wu-a"}"#,
    )
    .unwrap();

    (dir, project_dir)
}

#[test]
fn run_without_dry_run_rejects_json_output() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "constraints"

[[protocols]]
name = "implement"
requires = ["constraints"]
trigger = { type = "on_artifact", name = "constraints" }
"#,
        &[(
            "constraints",
            r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#,
        )],
        &["implement"],
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let output = runa_bin()
        .arg("run")
        .arg("--json")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--json is only supported with --dry-run"));
}

#[test]
fn run_without_dry_run_reports_project_load_failure_as_infrastructure_failure() {
    let dir = tempfile::tempdir().unwrap();
    let external_config = dir.path().join("external-config.toml");
    fs::write(
        &external_config,
        r#"
methodology_path = "/tmp/methodology.toml"
"#,
    )
    .unwrap();

    let project_dir = dir.path().join("not-a-project");
    fs::create_dir(&project_dir).unwrap();

    let output = runa_bin()
        .arg("--config")
        .arg(&external_config)
        .arg("run")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(6), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("not a runa project"), "stderr: {stderr}");
}

#[test]
fn run_dry_run_filters_projection_by_declared_scope() {
    let dir = tempfile::tempdir().unwrap();
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
name = "ground"
trigger = { type = "on_artifact", name = "constraints" }

[[protocols]]
name = "implement"
requires = ["constraints"]
produces = ["implementation"]
scoped = true
trigger = { type = "on_artifact", name = "constraints" }

[[protocols]]
name = "verify"
requires = ["implementation"]
produces = ["verified"]
scoped = true
trigger = { type = "on_artifact", name = "implementation" }
"#,
        &[
            (
                "constraints",
                r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#,
            ),
            (
                "implementation",
                r#"{"type":"object","required":["done","work_unit"],"properties":{"done":{"type":"boolean"},"work_unit":{"type":"string"}}}"#,
            ),
            (
                "verified",
                r#"{"type":"object","required":["done","work_unit"],"properties":{"done":{"type":"boolean"},"work_unit":{"type":"string"}}}"#,
            ),
        ],
        &["ground", "implement", "verify"],
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("constraints")).unwrap();
    fs::write(
        workspace.join("constraints/spec-a.json"),
        r#"{"title":"ship run","work_unit":"wu-a"}"#,
    )
    .unwrap();
    fs::write(
        workspace.join("constraints/spec-b.json"),
        r#"{"title":"ship run","work_unit":"wu-b"}"#,
    )
    .unwrap();

    let unscoped = runa_bin()
        .arg("run")
        .arg("--dry-run")
        .arg("--json")
        .current_dir(&project_dir)
        .output()
        .unwrap();
    assert!(
        unscoped.status.success(),
        "run failed: {}",
        String::from_utf8_lossy(&unscoped.stderr)
    );
    let unscoped_json: serde_json::Value = serde_json::from_slice(&unscoped.stdout).unwrap();
    let unscoped_plan = unscoped_json["execution_plan"].as_array().unwrap();
    assert_eq!(unscoped_plan.len(), 1);
    assert_eq!(unscoped_plan[0]["protocol"], "ground");
    assert!(unscoped_plan[0].get("work_unit").is_none());

    let scoped = runa_bin()
        .arg("run")
        .arg("--dry-run")
        .arg("--json")
        .arg("--work-unit")
        .arg("wu-a")
        .current_dir(&project_dir)
        .output()
        .unwrap();
    assert!(
        scoped.status.code() == Some(3),
        "run --work-unit exited with {:?}: stdout={} stderr={}",
        scoped.status.code(),
        String::from_utf8_lossy(&scoped.stdout),
        String::from_utf8_lossy(&scoped.stderr)
    );
    let scoped_json: serde_json::Value = serde_json::from_slice(&scoped.stdout).unwrap();
    let scoped_plan = scoped_json["execution_plan"].as_array().unwrap();
    assert_eq!(scoped_plan.len(), 2);
    assert_eq!(scoped_plan[0]["protocol"], "implement");
    assert_eq!(scoped_plan[0]["work_unit"], "wu-a");
    assert_eq!(scoped_plan[1]["protocol"], "verify");
    assert_eq!(scoped_plan[1]["work_unit"], "wu-a");
    let plan_text = serde_json::to_string(scoped_plan).unwrap();
    assert!(!plan_text.contains("wu-b"), "{plan_text}");
    assert!(!plan_text.contains("ground"), "{plan_text}");
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
fn write_single_protocol_agent(dir: &Path) -> PathBuf {
    let script_path = dir.join("single-protocol-agent.sh");
    fs::write(
        &script_path,
        "#!/bin/sh\nlog_file=\"$1\"\npayload=$(cat)\ncase \"$payload\" in\n  *\"# Protocol: implement\"*)\n    printf 'implement\\n' >> \"$log_file\"\n    mkdir -p .runa/workspace/implementation\n    printf '%s\\n' '{\"done\":true}' > .runa/workspace/implementation/impl-1.json\n    ;;\n  *)\n    printf '%s\\n' \"$payload\" > \"$log_file.unexpected\"\n    exit 19\n    ;;\nesac\n",
    )
    .unwrap();
    fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755)).unwrap();
    script_path
}
fn write_arg_logging_single_protocol_agent(dir: &Path) -> PathBuf {
    let script_path = dir.join("arg-logging-single-protocol-agent.sh");
    fs::write(
        &script_path,
        "#!/bin/sh\nlog_file=\"$1\"\nargs_file=\"$2\"\nshift 2\nprintf '%s\\n' \"$@\" > \"$args_file\"\npayload=$(cat)\ncase \"$payload\" in\n  *\"# Protocol: implement\"*)\n    printf 'implement\\n' >> \"$log_file\"\n    mkdir -p .runa/workspace/implementation\n    printf '%s\\n' '{\"done\":true}' > .runa/workspace/implementation/impl-1.json\n    ;;\n  *)\n    printf '%s\\n' \"$payload\" > \"$log_file.unexpected\"\n    exit 19\n    ;;\nesac\n",
    )
    .unwrap();
    fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755)).unwrap();
    script_path
}
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
#[test]
fn run_without_dry_run_uses_configured_agent_command() {
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
name = "implement"
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
        &["implement"],
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

    let log_path = dir.path().join("configured.log");
    let agent_path = write_single_protocol_agent(dir.path());
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
    assert_eq!(executed, "implement\n");
    assert!(workspace.join("implementation/impl-1.json").is_file());
}
#[test]
fn run_without_dry_run_cli_agent_command_overrides_configured_agent_command() {
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
name = "implement"
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
        &["implement"],
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

    let config_log_path = dir.path().join("configured.log");
    let cli_log_path = dir.path().join("cli.log");
    let configured_agent = write_single_protocol_agent(dir.path());
    let cli_agent = write_single_protocol_agent(dir.path());
    append_agent_command_config(
        &project_dir,
        &[configured_agent.as_path(), config_log_path.as_path()],
    );

    let output = runa_bin()
        .arg("run")
        .arg("--agent-command")
        .arg("--")
        .arg(&cli_agent)
        .arg(&cli_log_path)
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let cli_executed = fs::read_to_string(&cli_log_path).unwrap();
    assert_eq!(cli_executed, "implement\n");
    assert!(!config_log_path.exists(), "configured agent should not run");
    assert!(workspace.join("implementation/impl-1.json").is_file());
}
#[test]
fn run_without_dry_run_cli_agent_command_preserves_hyphenated_tokens() {
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
name = "implement"
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
        &["implement"],
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
    let args_path = dir.path().join("argv.log");
    let agent_path = write_arg_logging_single_protocol_agent(dir.path());

    let output = runa_bin()
        .arg("run")
        .arg("--agent-command")
        .arg("--")
        .arg(&agent_path)
        .arg(&log_path)
        .arg(&args_path)
        .arg("--dangerously-skip-permissions")
        .arg("-p")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let argv = fs::read_to_string(&args_path).unwrap();
    assert_eq!(argv, "--dangerously-skip-permissions\n-p\n");
    let executed = fs::read_to_string(&log_path).unwrap();
    assert_eq!(executed, "implement\n");
    assert!(workspace.join("implementation/impl-1.json").is_file());
}
#[test]
fn run_without_dry_run_rejects_empty_cli_agent_command_even_when_configured() {
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
name = "implement"
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
        &["implement"],
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

    let config_log_path = dir.path().join("configured.log");
    let agent_path = write_single_protocol_agent(dir.path());
    append_agent_command_config(
        &project_dir,
        &[agent_path.as_path(), config_log_path.as_path()],
    );

    let output = runa_bin()
        .arg("run")
        .arg("--agent-command")
        .arg("--")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(6), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("no agent command configured"),
        "stderr: {stderr}"
    );
    assert!(!config_log_path.exists(), "configured agent should not run");
    assert!(!workspace.join("implementation/impl-1.json").exists());
}

#[test]
fn run_without_dry_run_fails_when_agent_command_is_not_configured() {
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
name = "implement"
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
        &["implement"],
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

    let output = runa_bin()
        .arg("run")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(6), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("no agent command configured"),
        "stderr: {stderr}"
    );
    assert!(!workspace.join("implementation/impl-1.json").exists());
}

#[test]
fn run_help_describes_agent_command_passthrough() {
    let output = runa_bin().arg("run").arg("--help").output().unwrap();

    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Usage: runa run [OPTIONS] [-- [ARGV]...]"),
        "stdout: {stdout}"
    );
    assert!(stdout.contains("--agent-command"), "stdout: {stdout}");
    assert!(stdout.contains("-- <argv"), "stdout: {stdout}");
    assert!(
        !stdout.contains("Usage: runa run [OPTIONS] [ARGV]..."),
        "stdout: {stdout}"
    );
}
#[test]
fn run_without_separator_rejects_agent_command_before_dry_run_flag() {
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
name = "implement"
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
        &["implement"],
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
    let agent_path = write_single_protocol_agent(dir.path());
    append_agent_command_config(&project_dir, &[agent_path.as_path(), log_path.as_path()]);

    let output = runa_bin()
        .arg("run")
        .arg("--agent-command")
        .arg(&agent_path)
        .arg("--dry-run")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unexpected argument"), "stderr: {stderr}");
    assert!(
        stderr.contains(agent_path.to_string_lossy().as_ref()),
        "stderr: {stderr}"
    );
    assert!(!log_path.exists(), "agent should not run");
    assert!(!workspace.join("implementation/impl-1.json").exists());
}
#[test]
fn run_without_separator_rejects_agent_command_before_work_unit_flag() {
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
name = "implement"
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
        &["implement"],
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
    let agent_path = write_single_protocol_agent(dir.path());
    append_agent_command_config(&project_dir, &[agent_path.as_path(), log_path.as_path()]);

    let output = runa_bin()
        .arg("run")
        .arg("--agent-command")
        .arg(&agent_path)
        .arg("--work-unit")
        .arg("wu-xyz")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unexpected argument"), "stderr: {stderr}");
    assert!(
        stderr.contains(agent_path.to_string_lossy().as_ref()),
        "stderr: {stderr}"
    );
    assert!(!log_path.exists(), "agent should not run");
    assert!(!workspace.join("implementation/impl-1.json").exists());
}
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
    assert!(stdout.contains("Run outcome: success"), "stdout: {stdout}");
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
fn run_dry_run_ignores_out_of_scope_cycle_when_unscoped_work_is_quiescent() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "seed"

[[artifact_types]]
name = "result"

[[artifact_types]]
name = "a"

[[artifact_types]]
name = "b"

[[protocols]]
name = "publish"
requires = ["seed"]
produces = ["result"]
trigger = { type = "on_artifact", name = "seed" }

[[protocols]]
name = "first"
requires = ["b"]
produces = ["a"]
scoped = true
trigger = { type = "on_artifact", name = "b" }

[[protocols]]
name = "second"
requires = ["a"]
produces = ["b"]
scoped = true
trigger = { type = "on_artifact", name = "a" }
"#,
        &[
            (
                "seed",
                r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#,
            ),
            (
                "result",
                r#"{"type":"object","required":["done"],"properties":{"done":{"type":"boolean"}}}"#,
            ),
            (
                "a",
                r#"{"type":"object","required":["title","work_unit"],"properties":{"title":{"type":"string"},"work_unit":{"type":"string"}}}"#,
            ),
            (
                "b",
                r#"{"type":"object","required":["title","work_unit"],"properties":{"title":{"type":"string"},"work_unit":{"type":"string"}}}"#,
            ),
        ],
        &["publish", "first", "second"],
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("seed")).unwrap();
    fs::create_dir_all(workspace.join("result")).unwrap();
    fs::write(workspace.join("seed/input.json"), r#"{"title":"ship"}"#).unwrap();
    fs::write(workspace.join("result/current.json"), r#"{"done":true}"#).unwrap();
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
    fs::create_dir_all(workspace.join("a")).unwrap();
    fs::create_dir_all(workspace.join("b")).unwrap();
    fs::write(
        workspace.join("a/current.json"),
        r#"{"title":"a","work_unit":"wu-a"}"#,
    )
    .unwrap();
    fs::write(
        workspace.join("b/current.json"),
        r#"{"title":"b","work_unit":"wu-a"}"#,
    )
    .unwrap();

    let output = runa_bin()
        .arg("run")
        .arg("--dry-run")
        .arg("--json")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0), "{output:?}");

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(value.get("cycle").is_none(), "{value:#}");
    assert_eq!(value["execution_plan"], serde_json::json!([]), "{value:#}");
    let protocols = value["protocols"].as_array().unwrap();
    assert_eq!(protocols.len(), 1, "{value:#}");
    assert_eq!(protocols[0]["name"], "publish");
    assert_eq!(protocols[0]["status"], "waiting");
    assert_eq!(
        protocols[0]["unsatisfied_conditions"],
        serde_json::json!(["outputs are current"])
    );
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
fn run_without_dry_run_with_no_ready_protocols_returns_exit_4() {
    let (dir, project_dir) = setup_quiescent_run_project();
    let log_path = dir.path().join("executed.log");
    let agent_path = write_single_protocol_agent(dir.path());
    append_agent_command_config(&project_dir, &[agent_path.as_path(), log_path.as_path()]);

    let output = runa_bin()
        .arg("run")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(4), "{output:?}");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Run outcome: nothing_ready"),
        "stdout: {stdout}"
    );
    assert!(!log_path.exists(), "agent should not run");
}

#[test]
fn run_without_dry_run_with_no_ready_protocols_still_requires_agent_command() {
    let (_dir, project_dir) = setup_quiescent_run_project();

    let output = runa_bin()
        .arg("run")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(6), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("no agent command configured"),
        "stderr: {stderr}"
    );
}
#[test]
fn run_without_dry_run_rejects_empty_cli_agent_command_when_no_protocols_are_ready() {
    let (dir, project_dir) = setup_quiescent_run_project();
    let config_log_path = dir.path().join("configured.log");
    let agent_path = write_single_protocol_agent(dir.path());
    append_agent_command_config(
        &project_dir,
        &[agent_path.as_path(), config_log_path.as_path()],
    );

    let output = runa_bin()
        .arg("run")
        .arg("--agent-command")
        .arg("--")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(6), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("no agent command configured"),
        "stderr: {stderr}"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("Run outcome: nothing_ready"),
        "stdout: {stdout}"
    );
    assert!(!config_log_path.exists(), "configured agent should not run");
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
    assert_eq!(protocols[0]["status"], "waiting");
    assert_eq!(
        protocols[0]["unsatisfied_conditions"],
        serde_json::json!(["dependency cycle detected: first -> second"])
    );
    assert_eq!(protocols[1]["name"], "second");
    assert_eq!(protocols[1]["status"], "waiting");
    assert_eq!(
        protocols[1]["unsatisfied_conditions"],
        serde_json::json!(["dependency cycle detected: first -> second"])
    );
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
    let log_path = dir.path().join("executed.log");
    let agent_path = write_single_protocol_agent(dir.path());
    append_agent_command_config(&project_dir, &[agent_path.as_path(), log_path.as_path()]);

    let output = runa_bin()
        .arg("run")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(3), "{output:?}");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Run outcome: blocked"), "stdout: {stdout}");
    assert!(!log_path.exists(), "agent should not run");
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
scoped = true
trigger = { type = "on_artifact", name = "input" }

[[protocols]]
name = "verify"
requires = ["constrained"]
produces = ["verified"]
scoped = true
trigger = { type = "on_artifact", name = "constrained" }
"#,
        &[
            ("input", wu_title_schema),
            ("constrained", wu_title_schema),
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
        .arg("--work-unit")
        .arg("wu-a")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(3), "{output:?}");

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let execution_plan = value["execution_plan"].as_array().unwrap();
    assert_eq!(execution_plan.len(), 2, "{value:#}");
    assert_eq!(execution_plan[0]["protocol"], "build");
    assert_eq!(execution_plan[0]["work_unit"], "wu-a");
    assert_eq!(execution_plan[0]["projection"], "current");
    assert_eq!(execution_plan[1]["protocol"], "verify");
    assert_eq!(execution_plan[1]["work_unit"], "wu-a");
    assert_eq!(execution_plan[1]["projection"], "projected");
    let plan_text = serde_json::to_string(execution_plan).unwrap();
    assert!(!plan_text.contains("wu-b"), "{plan_text}");
}
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

#[test]
fn run_dry_run_projects_downstream_work_from_mixed_validity_inputs() {
    let dir = tempfile::tempdir().unwrap();
    let bool_schema =
        r#"{"type":"object","required":["done"],"properties":{"done":{"type":"boolean"}}}"#;
    let manifest_path = common::write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "request"

[[artifact_types]]
name = "published"

[[artifact_types]]
name = "notified"

[[protocols]]
name = "publish"
requires = ["request"]
produces = ["published"]
trigger = { type = "on_artifact", name = "request" }

[[protocols]]
name = "notify"
requires = ["published"]
produces = ["notified"]
trigger = { type = "on_artifact", name = "published" }
"#,
        &[
            (
                "request",
                r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#,
            ),
            ("published", bool_schema),
            ("notified", bool_schema),
        ],
        &["publish", "notify"],
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("request")).unwrap();
    fs::write(workspace.join("request/good.json"), r#"{"title":"ok"}"#).unwrap();
    fs::write(workspace.join("request/bad.json"), r#"{"bad":true}"#).unwrap();

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
    assert_eq!(execution_plan[0]["protocol"], "publish");
    assert_eq!(execution_plan[0]["projection"], "current");
    assert_eq!(execution_plan[1]["protocol"], "notify");
    assert_eq!(execution_plan[1]["projection"], "projected");
}
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
scoped = true
trigger = { type = "on_artifact", name = "seed" }

[[protocols]]
name = "prepare"
scoped = true
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
        .arg("--work-unit")
        .arg("wu-b")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let executed = fs::read_to_string(&log_path).unwrap();
    assert_eq!(executed, "prepare:wu-b\nrevise:wu-b\nprepare:wu-b\n");
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
#[test]
fn run_without_dry_run_reopens_exhausted_work_after_postcondition_failure() {
    let dir = tempfile::tempdir().unwrap();
    let wu_schema = r#"{"type":"object","required":["title","work_unit"],"properties":{"title":{"type":"string"},"work_unit":{"type":"string"}}}"#;
    let bool_schema = r#"{"type":"object","required":["done","work_unit"],"properties":{"done":{"type":"boolean"},"work_unit":{"type":"string"}}}"#;
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
scoped = true
trigger = { type = "on_artifact", name = "seed" }

[[protocols]]
name = "prepare"
scoped = true
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
        .arg("--work-unit")
        .arg("wu-b")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(5), "{output:?}");

    let executed = fs::read_to_string(&log_path).unwrap();
    assert_eq!(executed, "prepare:wu-b\nrevise:wu-b\nprepare:wu-b\n");
    assert!(!workspace.join("implementation/out.json").exists());
}
#[test]
fn run_without_dry_run_reopens_exhausted_work_after_agent_failure() {
    let dir = tempfile::tempdir().unwrap();
    let wu_schema = r#"{"type":"object","required":["title","work_unit"],"properties":{"title":{"type":"string"},"work_unit":{"type":"string"}}}"#;
    let bool_schema = r#"{"type":"object","required":["done","work_unit"],"properties":{"done":{"type":"boolean"},"work_unit":{"type":"string"}}}"#;
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
scoped = true
trigger = { type = "on_artifact", name = "seed" }

[[protocols]]
name = "prepare"
scoped = true
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
        .arg("--work-unit")
        .arg("wu-b")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(5), "{output:?}");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Run outcome: work_failed"),
        "stdout: {stdout}"
    );

    let executed = fs::read_to_string(&log_path).unwrap();
    assert_eq!(executed, "prepare:wu-b\nrevise:wu-b\nprepare:wu-b\n");
    assert!(!workspace.join("implementation/out.json").exists());
}
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

    assert_eq!(output.status.code(), Some(5), "{output:?}");

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
    let log_path = dir.path().join("executed.log");
    let agent_path = write_single_protocol_agent(dir.path());
    append_agent_command_config(&project_dir, &[agent_path.as_path(), log_path.as_path()]);

    let output = runa_bin()
        .arg("run")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(3), "{output:?}");
    assert!(!log_path.exists(), "agent should not run");
}
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

    assert_eq!(output.status.code(), Some(5), "{output:?}");

    let executed = fs::read_to_string(&log_path).unwrap();
    assert_eq!(executed, "prepare\n");
    assert!(!workspace.join("published/out-1.json").exists());
}
