mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

fn runa_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_runa"))
}

fn runa_bin_path() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_runa"))
}

fn built_runa_mcp_path() -> PathBuf {
    runa_bin_path()
        .parent()
        .unwrap()
        .join(format!("runa-mcp{}", std::env::consts::EXE_SUFFIX))
}

fn copy_binary(src: &Path, dest: &Path) {
    fs::copy(src, dest).unwrap();
}

fn copy_isolated_runa(dir: &Path) -> PathBuf {
    let bin_dir = dir.join("isolated-bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let isolated = bin_dir.join(format!("runa{}", std::env::consts::EXE_SUFFIX));
    copy_binary(runa_bin_path(), &isolated);
    isolated
}

fn command_output_retry_busy(mut command: Command) -> std::process::Output {
    for attempt in 0..5 {
        match command.output() {
            Ok(output) => return output,
            Err(err)
                if err.kind() == std::io::ErrorKind::ExecutableFileBusy
                    || err.raw_os_error() == Some(26) =>
            {
                if attempt == 4 {
                    panic!("command failed after retrying ETXTBSY: {err}");
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(err) => panic!("failed to run command: {err}"),
        }
    }

    unreachable!("retry loop must return or panic")
}

fn manifest_toml() -> &'static str {
    r#"
name = "groundwork"

[[artifact_types]]
name = "constraints"

[[artifact_types]]
name = "prior-art"

[[artifact_types]]
name = "implementation"

[[protocols]]
name = "implement"
requires = ["constraints"]
accepts = ["prior-art"]
produces = ["implementation"]
trigger = { type = "on_artifact", name = "constraints" }

[[protocols]]
name = "verify"
requires = ["implementation"]
trigger = { type = "on_artifact", name = "constraints" }

[[protocols]]
name = "ground"
trigger = { type = "on_invalid", name = "implementation" }
"#
}

fn implement_only_manifest_toml() -> &'static str {
    r#"
name = "groundwork"

[[artifact_types]]
name = "constraints"

[[artifact_types]]
name = "prior-art"

[[artifact_types]]
name = "implementation"

[[protocols]]
name = "implement"
requires = ["constraints"]
accepts = ["prior-art"]
produces = ["implementation"]
trigger = { type = "on_artifact", name = "constraints" }
"#
}

fn methodology_schemas() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "constraints",
            r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#,
        ),
        (
            "prior-art",
            r#"{"type":"object","required":["source"],"properties":{"source":{"type":"string"}}}"#,
        ),
        (
            "implementation",
            r#"{"type":"object","required":["done"],"properties":{"done":{"type":"boolean"}}}"#,
        ),
    ]
}

fn methodology_protocols() -> Vec<&'static str> {
    vec!["implement", "verify", "ground"]
}

fn implement_only_methodology_protocols() -> Vec<&'static str> {
    vec!["implement"]
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

#[test]
fn step_dry_run_filters_execution_plan_by_declared_scope() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "constraints"

[[artifact_types]]
name = "implementation"

[[protocols]]
name = "ground"
trigger = { type = "on_artifact", name = "constraints" }

[[protocols]]
name = "implement"
requires = ["constraints"]
produces = ["implementation"]
scoped = true
trigger = { type = "on_artifact", name = "constraints" }
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
        ],
        &["ground", "implement"],
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("constraints")).unwrap();
    fs::write(
        workspace.join("constraints/spec-a.json"),
        r#"{"title":"ship step","work_unit":"wu-a"}"#,
    )
    .unwrap();

    let unscoped = runa_bin()
        .arg("step")
        .arg("--dry-run")
        .arg("--json")
        .current_dir(&project_dir)
        .output()
        .unwrap();
    assert!(
        unscoped.status.success(),
        "step failed: {}",
        String::from_utf8_lossy(&unscoped.stderr)
    );
    let unscoped_json: serde_json::Value = serde_json::from_slice(&unscoped.stdout).unwrap();
    assert_eq!(unscoped_json["execution_plan"].as_array().unwrap().len(), 1);
    assert_eq!(unscoped_json["execution_plan"][0]["protocol"], "ground");
    assert!(
        unscoped_json["execution_plan"][0]
            .get("work_unit")
            .is_none()
    );

    let scoped = runa_bin()
        .arg("step")
        .arg("--dry-run")
        .arg("--json")
        .arg("--work-unit")
        .arg("wu-a")
        .current_dir(&project_dir)
        .output()
        .unwrap();
    assert!(
        scoped.status.success(),
        "step --work-unit failed: {}",
        String::from_utf8_lossy(&scoped.stderr)
    );
    let scoped_json: serde_json::Value = serde_json::from_slice(&scoped.stdout).unwrap();
    assert_eq!(scoped_json["execution_plan"].as_array().unwrap().len(), 1);
    assert_eq!(scoped_json["execution_plan"][0]["protocol"], "implement");
    assert_eq!(scoped_json["execution_plan"][0]["work_unit"], "wu-a");
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
fn write_capture_agent(dir: &Path) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let script_path = dir.join("capture-agent.sh");
    fs::write(
        &script_path,
        "#!/bin/sh\ncapture=\"$1\"\nout=/dev/null\nif [ ! -s \"$capture\" ]; then\n  out=\"$capture\"\nfi\nwhile IFS= read -r line || [ -n \"$line\" ]; do\n  printf '%s\\n' \"$line\" >> \"$out\"\ndone\nif [ -n \"$2\" ] && [ ! -s \"$2\" ]; then\n  printf '%s' \"$RUNA_MCP_CONFIG\" > \"$2\"\nfi\nprintf '%s\\n' '{\"done\":true}' > .runa/workspace/implementation/impl-1.json\n",
    )
    .unwrap();
    fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755)).unwrap();
    script_path
}
fn write_no_output_agent(dir: &Path) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let script_path = dir.join("no-output-agent.sh");
    fs::write(
        &script_path,
        "#!/bin/sh\ncapture=\"$1\"\n: > \"$capture\"\nwhile IFS= read -r line || [ -n \"$line\" ]; do\n  printf '%s\\n' \"$line\" >> \"$capture\"\ndone\nif [ -n \"$2\" ]; then\n  printf '%s' \"$RUNA_MCP_CONFIG\" > \"$2\"\nfi\n",
    )
    .unwrap();
    fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755)).unwrap();
    script_path
}
fn write_second_run_fails_agent(dir: &Path) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let script_path = dir.join("second-run-fails-agent.sh");
    fs::write(
        &script_path,
        "#!/bin/sh\ncount_file=\"$1\"\ncount=0\nif [ -f \"$count_file\" ]; then count=$(cat \"$count_file\"); fi\ncount=$((count + 1))\nprintf '%s' \"$count\" > \"$count_file\"\ncat > /dev/null\nif [ \"$count\" -ge 2 ]; then\n  exit 17\nfi\n",
    )
    .unwrap();
    fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755)).unwrap();
    script_path
}
fn write_failing_agent(dir: &Path) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let script_path = dir.join("failing-agent.sh");
    fs::write(
        &script_path,
        "#!/bin/sh\ncount_file=\"$1\"\npayload_dir=\"$2\"\ncount=0\nif [ -f \"$count_file\" ]; then count=$(cat \"$count_file\"); fi\ncount=$((count + 1))\nprintf '%s' \"$count\" > \"$count_file\"\ncat > \"$payload_dir/$count.json\"\nexit 17\n",
    )
    .unwrap();
    fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755)).unwrap();
    script_path
}
fn write_reconciling_agent(dir: &Path) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let script_path = dir.join("reconciling-agent.sh");
    fs::write(
        &script_path,
        "#!/bin/sh\nlog_file=\"$1\"\npayload=$(cat)\ncase \"$payload\" in\n  *\"# Protocol: implement\"*)\n    printf 'implement\\n' >> \"$log_file\"\n    mkdir -p .runa/workspace/implementation\n    printf '%s\\n' '{\"done\":true}' > .runa/workspace/implementation/impl-1.json\n    ;;\n  *\"# Protocol: verify\"*)\n    printf 'verify\\n' >> \"$log_file\"\n    mkdir -p .runa/workspace/verified\n    printf '%s\\n' '{\"done\":true}' > .runa/workspace/verified/check-1.json\n    ;;\n  *)\n    printf '%s\\n' \"$payload\" > \"$log_file.unexpected\"\n    exit 19\n    ;;\nesac\n",
    )
    .unwrap();
    fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755)).unwrap();
    script_path
}
fn write_prepare_then_implement_agent(dir: &Path) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let script_path = dir.join("prepare-then-implement-agent.sh");
    fs::write(
        &script_path,
        "#!/bin/sh\nlog_file=\"$1\"\npayload=$(cat)\ncase \"$payload\" in\n  *\"# Protocol: alpha_prepare\"*)\n    printf 'alpha_prepare\\n' >> \"$log_file\"\n    ;;\n  *\"# Protocol: beta_implement\"*)\n    printf 'beta_implement\\n' >> \"$log_file\"\n    mkdir -p .runa/workspace/implementation\n    printf '%s\\n' '{\"done\":true}' > .runa/workspace/implementation/impl-1.json\n    ;;\n  *)\n    printf '%s\\n' \"$payload\" > \"$log_file.unexpected\"\n    exit 19\n    ;;\nesac\n",
    )
    .unwrap();
    fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755)).unwrap();
    script_path
}
fn write_scoped_prepare_then_revise_agent(dir: &Path) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let script_path = dir.join("scoped-prepare-then-revise-agent.sh");
    fs::write(
        &script_path,
        "#!/bin/sh\nlog_file=\"$1\"\npayload=$(cat)\ncase \"$payload\" in\n  *\"# Protocol: prepare (work_unit=wu-a)\"*)\n    printf 'prepare:wu-a\\n' >> \"$log_file\"\n    ;;\n  *\"# Protocol: prepare (work_unit=wu-b)\"*)\n    printf 'prepare:wu-b\\n' >> \"$log_file\"\n    ;;\n  *\"# Protocol: revise (work_unit=wu-b)\"*)\n    printf 'revise:wu-b\\n' >> \"$log_file\"\n    mkdir -p .runa/workspace/constraints\n    printf '%s\\n' '{\"title\":\"updated-b\",\"work_unit\":\"wu-b\"}' > .runa/workspace/constraints/b.json\n    ;;\n  *)\n    printf '%s\\n' \"$payload\" > \"$log_file.unexpected\"\n    exit 19\n    ;;\nesac\n",
    )
    .unwrap();
    fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755)).unwrap();
    script_path
}

fn write_fake_claude(dir: &Path) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let script_path = dir.join("claude");
    fs::write(
        &script_path,
        "#!/bin/sh\ncapture=\"$FAKE_CLAUDE_CAPTURE\"\nconfig=\"\"\nwhile [ \"$#\" -gt 0 ]; do\n  if [ \"$1\" = \"--mcp-config\" ]; then\n    shift\n    config=\"$1\"\n  fi\n  shift\ndone\ncat \"$config\" > \"$capture\"\n",
    )
    .unwrap();
    fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755)).unwrap();
    script_path
}
fn write_producing_fake_claude(dir: &Path) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let script_path = dir.join("claude");
    fs::write(
        &script_path,
        "#!/bin/sh\nset -eu\nconfig=\"\"\nstrict=0\nprintf '%s\\n' \"$@\" > \"$FAKE_CLAUDE_ARGV_CAPTURE\"\nwhile [ \"$#\" -gt 0 ]; do\n  if [ \"$1\" = \"--mcp-config\" ]; then\n    shift\n    config=\"$1\"\n  elif [ \"$1\" = \"--strict-mcp-config\" ]; then\n    strict=1\n  fi\n  shift\ndone\nif [ -z \"$config\" ] || [ \"$strict\" -ne 1 ]; then\n  exit 37\nfi\ncat \"$config\" > \"$FAKE_CLAUDE_CAPTURE\"\ncat >/dev/null\nmkdir -p .runa/workspace/implementation\nprintf '%s\\n' '{\"done\":true}' > .runa/workspace/implementation/impl-1.json\n",
    )
    .unwrap();
    fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755)).unwrap();
    script_path
}

#[test]
fn step_dry_run_json_reports_ready_execution_plan_and_full_skill_status() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        manifest_toml(),
        &methodology_schemas(),
        &methodology_protocols(),
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("constraints")).unwrap();
    fs::create_dir_all(workspace.join("prior-art")).unwrap();
    fs::create_dir_all(workspace.join("implementation")).unwrap();
    fs::write(
        workspace.join("constraints/spec-1.json"),
        r#"{"title":"ship step"}"#,
    )
    .unwrap();
    fs::write(
        workspace.join("prior-art/survey-1.json"),
        r#"{"source":"notes"}"#,
    )
    .unwrap();

    let output = runa_bin()
        .arg("step")
        .arg("--dry-run")
        .arg("--json")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0), "{output:?}");

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["version"], 4);
    assert_eq!(value["methodology"], "groundwork");
    assert_eq!(value["scan_warnings"], serde_json::json!([]));
    assert!(value.get("cycle").is_none(), "{value:#}");

    let execution_plan = value["execution_plan"].as_array().unwrap();
    assert_eq!(execution_plan.len(), 1, "{value:#}");
    assert_eq!(execution_plan[0]["protocol"], "implement");
    assert_eq!(execution_plan[0]["trigger"], "on_artifact(constraints)");
    assert_eq!(
        execution_plan[0]["mcp_config"]["args"],
        serde_json::json!(["--protocol", "implement"])
    );
    assert_eq!(
        execution_plan[0]["mcp_config"]["env"],
        serde_json::json!({
            "RUNA_CONFIG": project_dir.join(".runa/config.toml"),
            "RUNA_WORKING_DIR": project_dir
        })
    );
    let mcp_command = execution_plan[0]["mcp_config"]["command"]
        .as_str()
        .expect("mcp command should be a string");
    assert!(
        mcp_command.ends_with(&format!(
            "{}runa-mcp{}",
            std::path::MAIN_SEPARATOR,
            std::env::consts::EXE_SUFFIX
        )),
        "{mcp_command}"
    );
    assert_eq!(execution_plan[0]["context"]["protocol"], "implement");
    assert!(
        execution_plan[0]["context"]["work_unit"].is_null(),
        "{value:#}"
    );
    assert_eq!(
        execution_plan[0]["context"]["instructions"],
        "# implement\n"
    );
    assert_eq!(
        execution_plan[0]["context"]["expected_outputs"],
        serde_json::json!({
            "produces": ["implementation"],
            "may_produce": []
        })
    );
    assert_eq!(
        execution_plan[0]["context"]["inputs"],
        serde_json::json!([
            {
                "artifact_type": "constraints",
                "instance_id": "spec-1",
                "display_path": workspace.join("constraints/spec-1.json"),
                "content_hash": "sha256:dd4077b358533c789242e86ac7f5e7dffa0a587d5b4acfd343c612ae9ddfd315",
                "relationship": "requires"
            },
            {
                "artifact_type": "prior-art",
                "instance_id": "survey-1",
                "display_path": workspace.join("prior-art/survey-1.json"),
                "content_hash": "sha256:07de5216ca2c3ee50838fd24a2032bc4a9d77e73ba1de36a1cbdcd56b666946a",
                "relationship": "accepts"
            }
        ])
    );

    let protocols = value["protocols"].as_array().unwrap();
    assert_eq!(protocols.len(), 3, "{value:#}");
    assert_eq!(protocols[0]["name"], "implement");
    assert_eq!(protocols[0]["status"], "ready");
    assert_eq!(protocols[1]["name"], "verify");
    assert_eq!(protocols[1]["status"], "blocked");
    assert_eq!(protocols[2]["name"], "ground");
    assert_eq!(protocols[2]["status"], "waiting");
    assert_eq!(
        protocols[2]["unsatisfied_conditions"],
        serde_json::json!([
            "on_invalid(implementation): no invalid instances of artifact type 'implementation'"
        ])
    );
}

#[test]
fn step_dry_run_json_uses_display_path_for_context_inputs() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        manifest_toml(),
        &methodology_schemas(),
        &methodology_protocols(),
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
        .arg("step")
        .arg("--dry-run")
        .arg("--json")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let input = &value["execution_plan"][0]["context"]["inputs"][0];
    assert_eq!(
        input["display_path"],
        serde_json::Value::String(
            workspace
                .join("constraints/spec-1.json")
                .display()
                .to_string()
        )
    );
    assert!(input.get("path").is_none(), "{input:#}");
}

#[test]
fn step_dry_run_json_quickstart_review_receives_requirements_and_design() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../examples/quickstart-methodology/manifest.toml");

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("requirements")).unwrap();
    fs::create_dir_all(workspace.join("design")).unwrap();

    let requirements_path = workspace.join("requirements/request-1.json");
    fs::write(&requirements_path, r#"{"title":"Ship a review flow"}"#).unwrap();

    let design_path = workspace.join("design/design-1.json");
    fs::write(
        &design_path,
        r#"{"summary":"Review the design against the requirements"}"#,
    )
    .unwrap();

    let output = runa_bin()
        .arg("step")
        .arg("--dry-run")
        .arg("--json")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let execution_plan = value["execution_plan"].as_array().unwrap();
    assert_eq!(execution_plan.len(), 1, "{value:#}");
    assert_eq!(execution_plan[0]["protocol"], "review");

    let inputs = execution_plan[0]["context"]["inputs"].as_array().unwrap();
    assert_eq!(inputs.len(), 2, "{value:#}");

    assert_eq!(inputs[0]["artifact_type"], "requirements");
    assert_eq!(inputs[0]["instance_id"], "request-1");
    assert_eq!(
        inputs[0]["display_path"],
        serde_json::Value::String(requirements_path.display().to_string())
    );
    assert_eq!(inputs[0]["relationship"], "requires");

    assert_eq!(inputs[1]["artifact_type"], "design");
    assert_eq!(inputs[1]["instance_id"], "design-1");
    assert_eq!(
        inputs[1]["display_path"],
        serde_json::Value::String(design_path.display().to_string())
    );
    assert_eq!(inputs[1]["relationship"], "requires");

    let protocols = value["protocols"].as_array().unwrap();
    let draft = protocols
        .iter()
        .find(|protocol| protocol["name"] == "draft")
        .unwrap();
    assert_eq!(draft["status"], "waiting");
    assert_eq!(draft["trigger"], "satisfied");
    assert_eq!(
        draft["unsatisfied_conditions"],
        serde_json::json!(["outputs are current"])
    );

    let review = protocols
        .iter()
        .find(|protocol| protocol["name"] == "review")
        .unwrap();
    assert_eq!(review["status"], "ready");
}

#[test]
fn step_dry_run_text_reports_why_when_no_skills_are_ready() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        manifest_toml(),
        &methodology_schemas(),
        &methodology_protocols(),
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let output = runa_bin()
        .arg("step")
        .arg("--dry-run")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(3), "{output:?}");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Execution plan: none"), "stdout: {stdout}");
    assert!(stdout.contains("No READY protocols."), "stdout: {stdout}");
    assert!(stdout.contains("WAITING:"), "stdout: {stdout}");
    assert!(
        stdout.contains(
            "on_artifact(constraints): no valid instances of artifact type 'constraints' exist"
        ),
        "stdout: {stdout}"
    );
    assert!(
        stdout.contains(
            "on_invalid(implementation): no invalid instances of artifact type 'implementation'"
        ),
        "stdout: {stdout}"
    );
}

#[test]
fn step_dry_run_text_shows_preloaded_protocol_instructions_for_ready_protocols() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        manifest_toml(),
        &methodology_schemas(),
        &methodology_protocols(),
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("constraints")).unwrap();
    fs::create_dir_all(workspace.join("prior-art")).unwrap();
    fs::create_dir_all(workspace.join("implementation")).unwrap();
    fs::write(
        workspace.join("constraints/spec-1.json"),
        r#"{"title":"ship step"}"#,
    )
    .unwrap();
    fs::write(
        workspace.join("prior-art/survey-1.json"),
        r#"{"source":"notes"}"#,
    )
    .unwrap();

    let output = runa_bin()
        .arg("step")
        .arg("--dry-run")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0), "{output:?}");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("\"protocol\": \"implement\""),
        "stdout: {stdout}"
    );
    assert!(
        stdout.contains("\"instructions\": \"# implement\\n\""),
        "stdout: {stdout}"
    );
    assert!(stdout.contains("mcp_config:"), "stdout: {stdout}");
    assert!(stdout.contains("\"args\": ["), "stdout: {stdout}");
}

#[test]
fn step_dry_run_json_succeeds_without_discoverable_runa_mcp() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        manifest_toml(),
        &methodology_schemas(),
        &methodology_protocols(),
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

    let isolated_runa = copy_isolated_runa(dir.path());
    let empty_path = dir.path().join("empty-path");
    fs::create_dir(&empty_path).unwrap();

    let mut command = Command::new(&isolated_runa);
    command
        .arg("step")
        .arg("--dry-run")
        .arg("--json")
        .current_dir(&project_dir)
        .env("PATH", &empty_path);
    let output = command_output_retry_busy(command);

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let execution_plan = value["execution_plan"].as_array().unwrap();
    assert_eq!(execution_plan.len(), 1, "{value:#}");
    assert_eq!(
        execution_plan[0]["mcp_config"]["command"],
        serde_json::Value::String(format!("runa-mcp{}", std::env::consts::EXE_SUFFIX))
    );
}

#[test]
fn step_without_dry_run_fails_when_agent_command_is_not_configured() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        manifest_toml(),
        &methodology_schemas(),
        &methodology_protocols(),
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let output = runa_bin()
        .arg("step")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(6), "{output:?}");
    assert!(
        !output.status.success(),
        "step should fail without --dry-run"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("ERROR"), "stderr: {stderr}");
    assert!(stderr.contains("command"), "stderr: {stderr}");
    assert!(stderr.contains("step"), "stderr: {stderr}");
    assert!(
        stderr.contains("no agent command configured"),
        "stderr: {stderr}"
    );
    assert!(stderr.contains("[agent]"), "stderr: {stderr}");
    assert!(stderr.contains("config.toml"), "stderr: {stderr}");
}
#[test]
fn step_without_dry_run_invokes_configured_agent_with_execution_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        implement_only_manifest_toml(),
        &methodology_schemas(),
        &implement_only_methodology_protocols(),
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("constraints")).unwrap();
    fs::create_dir_all(workspace.join("prior-art")).unwrap();
    fs::create_dir_all(workspace.join("implementation")).unwrap();
    fs::write(
        workspace.join("constraints/spec-1.json"),
        r#"{"title":"ship step"}"#,
    )
    .unwrap();
    fs::write(
        workspace.join("prior-art/survey-1.json"),
        r#"{"source":"notes"}"#,
    )
    .unwrap();

    let payload_path = dir.path().join("captured-payload.json");
    let mcp_config_path = dir.path().join("captured-mcp-config.json");
    let agent_path = write_capture_agent(dir.path());
    append_agent_command_config(
        &project_dir,
        &[
            agent_path.as_path(),
            payload_path.as_path(),
            mcp_config_path.as_path(),
        ],
    );

    let output = runa_bin()
        .arg("step")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let captured = fs::read_to_string(&payload_path).unwrap();
    assert!(captured.contains("# Protocol: implement"), "{captured}");
    assert!(captured.contains("## Protocol instructions"), "{captured}");
    assert!(captured.contains("# implement"), "{captured}");
    assert!(captured.contains("## What you've been given"), "{captured}");
    assert!(captured.contains("**Title:** ship step"), "{captured}");
    assert!(captured.contains("## Additional context"), "{captured}");
    assert!(captured.contains("**Source:** notes"), "{captured}");
    assert!(
        captured.contains("## What you need to deliver"),
        "{captured}"
    );
    assert!(
        captured.contains("You must produce: implementation"),
        "{captured}"
    );
    assert!(
        captured.contains(
            "To deliver each required output, call the tool with the matching name and fill in the required fields."
        ),
        "{captured}"
    );

    let mcp_config: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&mcp_config_path).unwrap()).unwrap();
    let mcp_command = mcp_config["command"]
        .as_str()
        .expect("mcp command should be a string");
    assert!(
        mcp_command.ends_with(&format!(
            "{}runa-mcp{}",
            std::path::MAIN_SEPARATOR,
            std::env::consts::EXE_SUFFIX
        )),
        "{mcp_command}"
    );
    assert_eq!(
        mcp_config["args"],
        serde_json::json!(["--protocol", "implement"])
    );
    assert_eq!(
        mcp_config["env"],
        serde_json::json!({
            "RUNA_CONFIG": project_dir.join(".runa/config.toml"),
            "RUNA_WORKING_DIR": project_dir
        })
    );
}
#[test]
fn step_without_dry_run_wires_direct_claude_to_mcp_config() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        implement_only_manifest_toml(),
        &methodology_schemas(),
        &implement_only_methodology_protocols(),
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

    let fake_claude = write_producing_fake_claude(dir.path());
    let argv_capture = dir.path().join("captured-claude-argv.txt");
    let mcp_config_path = dir.path().join("captured-claude-config.json");
    append_agent_command_config(&project_dir, &[fake_claude.as_path()]);

    let output = runa_bin()
        .arg("step")
        .current_dir(&project_dir)
        .env("FAKE_CLAUDE_ARGV_CAPTURE", &argv_capture)
        .env("FAKE_CLAUDE_CAPTURE", &mcp_config_path)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let argv = fs::read_to_string(&argv_capture).unwrap();
    assert!(argv.contains("--mcp-config\n"), "{argv}");
    assert!(argv.contains("--strict-mcp-config\n"), "{argv}");

    let mcp_config: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&mcp_config_path).unwrap()).unwrap();
    assert_eq!(
        mcp_config["mcpServers"]["runa"]["args"],
        serde_json::json!(["--protocol", "implement"])
    );
    assert_eq!(
        mcp_config["mcpServers"]["runa"]["env"],
        serde_json::json!({
            "RUNA_CONFIG": project_dir.join(".runa/config.toml"),
            "RUNA_WORKING_DIR": project_dir
        })
    );
    let mcp_command = mcp_config["mcpServers"]["runa"]["command"]
        .as_str()
        .expect("mcp command should be a string");
    assert!(
        mcp_command.ends_with(&format!(
            "{}runa-mcp{}",
            std::path::MAIN_SEPARATOR,
            std::env::consts::EXE_SUFFIX
        )),
        "{mcp_command}"
    );
}
#[test]
fn step_without_dry_run_executes_one_protocol_and_leaves_downstream_work_ready() {
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
        .arg("step")
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
    assert!(!workspace.join("verified/check-1.json").exists());

    let state_output = runa_bin()
        .arg("state")
        .arg("--json")
        .current_dir(&project_dir)
        .output()
        .unwrap();
    assert!(
        state_output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&state_output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&state_output.stdout).unwrap();
    let verify = value["protocols"]
        .as_array()
        .unwrap()
        .iter()
        .find(|protocol| protocol["name"] == "verify")
        .unwrap();
    assert_eq!(verify["status"], "ready");
}
#[test]
fn step_without_dry_run_stops_after_the_first_ready_protocol_when_multiple_are_ready() {
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
        .arg("step")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let executed = fs::read_to_string(&log_path).unwrap();
    assert_eq!(executed, "alpha_prepare\n");
    assert!(!workspace.join("implementation/impl-1.json").exists());
}

#[test]
fn step_dry_run_json_only_reports_the_next_ready_execution() {
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
name = "alpha_prepare"
requires = ["constraints"]
trigger = { type = "on_artifact", name = "constraints" }

[[protocols]]
name = "beta_implement"
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

    let output = runa_bin()
        .arg("step")
        .arg("--dry-run")
        .arg("--json")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0), "{output:?}");

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let execution_plan = value["execution_plan"].as_array().unwrap();
    assert_eq!(execution_plan.len(), 1, "{value:#}");
    let planned = execution_plan[0]["protocol"].as_str().unwrap();
    assert!(matches!(planned, "alpha_prepare" | "beta_implement"));
}
#[test]
fn step_without_dry_run_stops_after_the_first_scoped_ready_protocol() {
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
        .arg("step")
        .arg("--work-unit")
        .arg("wu-b")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0), "{output:?}");

    let executed = fs::read_to_string(&log_path).unwrap();
    assert_eq!(executed, "prepare:wu-b\n");
}

#[test]
fn step_without_dry_run_reads_non_utf8_artifact_paths_into_prompt() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        implement_only_manifest_toml(),
        &methodology_schemas(),
        &implement_only_methodology_protocols(),
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("constraints")).unwrap();
    fs::create_dir_all(workspace.join("implementation")).unwrap();
    let non_utf8_name = OsString::from_vec(b"spec-\xFF.json".to_vec());
    fs::write(
        workspace.join("constraints").join(non_utf8_name),
        r#"{"title":"ship step"}"#,
    )
    .unwrap();

    let payload_path = dir.path().join("captured-payload.txt");
    let agent_path = write_capture_agent(dir.path());
    append_agent_command_config(
        &project_dir,
        &[agent_path.as_path(), payload_path.as_path()],
    );

    let output = runa_bin()
        .arg("step")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let captured = fs::read_to_string(&payload_path).unwrap();
    assert!(captured.contains("# Protocol: implement"), "{captured}");
    assert!(captured.contains("**Title:** ship step"), "{captured}");
    assert!(!captured.contains("Could not read artifact"), "{captured}");
}
#[test]
fn step_without_dry_run_uses_path_runa_mcp_when_sibling_is_missing() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        implement_only_manifest_toml(),
        &methodology_schemas(),
        &implement_only_methodology_protocols(),
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("constraints")).unwrap();
    fs::create_dir_all(workspace.join("implementation")).unwrap();
    fs::write(
        workspace.join("constraints/spec-1.json"),
        r#"{"title":"ship step"}"#,
    )
    .unwrap();

    let payload_path = dir.path().join("captured-payload.json");
    let mcp_config_path = dir.path().join("captured-mcp-config.json");
    let agent_path = write_capture_agent(dir.path());
    append_agent_command_config(
        &project_dir,
        &[
            agent_path.as_path(),
            payload_path.as_path(),
            mcp_config_path.as_path(),
        ],
    );

    let isolated_runa = copy_isolated_runa(dir.path());
    let path_bin = dir.path().join("path-bin");
    fs::create_dir(&path_bin).unwrap();
    let path_runa_mcp = path_bin.join(format!("runa-mcp{}", std::env::consts::EXE_SUFFIX));
    copy_binary(&built_runa_mcp_path(), &path_runa_mcp);

    let mut command = Command::new(&isolated_runa);
    command
        .arg("step")
        .current_dir(&project_dir)
        .env("PATH", &path_bin);
    let output = command_output_retry_busy(command);

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let mcp_config: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&mcp_config_path).unwrap()).unwrap();
    assert_eq!(
        mcp_config["command"],
        serde_json::Value::String(path_runa_mcp.to_string_lossy().into_owned())
    );
}
#[test]
fn step_without_dry_run_absolutizes_relative_config_override_and_path_entry() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        implement_only_manifest_toml(),
        &methodology_schemas(),
        &implement_only_methodology_protocols(),
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("constraints")).unwrap();
    fs::create_dir_all(workspace.join("implementation")).unwrap();
    fs::write(
        workspace.join("constraints/spec-1.json"),
        r#"{"title":"ship step"}"#,
    )
    .unwrap();

    let payload_path = dir.path().join("captured-payload.json");
    let mcp_config_path = dir.path().join("captured-mcp-config.json");
    let agent_path = write_capture_agent(dir.path());
    append_agent_command_config(
        &project_dir,
        &[
            agent_path.as_path(),
            payload_path.as_path(),
            mcp_config_path.as_path(),
        ],
    );

    let isolated_runa = copy_isolated_runa(dir.path());
    let path_bin = project_dir.join("path-bin");
    fs::create_dir(&path_bin).unwrap();
    let path_runa_mcp = path_bin.join(format!("runa-mcp{}", std::env::consts::EXE_SUFFIX));
    copy_binary(&built_runa_mcp_path(), &path_runa_mcp);

    let mut command = Command::new(&isolated_runa);
    command
        .arg("--config")
        .arg(".runa/config.toml")
        .arg("step")
        .current_dir(&project_dir)
        .env(
            "PATH",
            format!("path-bin:{}", std::env::var("PATH").unwrap_or_default()),
        );
    let output = command_output_retry_busy(command);

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let mcp_config: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&mcp_config_path).unwrap()).unwrap();
    assert_eq!(
        mcp_config["command"],
        serde_json::Value::String(path_runa_mcp.to_string_lossy().into_owned())
    );
    assert_eq!(
        mcp_config["env"]["RUNA_CONFIG"],
        serde_json::Value::String(
            project_dir
                .join(".runa/config.toml")
                .to_string_lossy()
                .into_owned()
        )
    );
    assert_eq!(
        mcp_config["env"]["RUNA_WORKING_DIR"],
        serde_json::Value::String(project_dir.to_string_lossy().into_owned())
    );
}
#[test]
fn claude_wrapper_wraps_runa_mcp_config_under_mcp_servers() {
    let dir = tempfile::tempdir().unwrap();
    let bin_dir = dir.path().join("bin");
    fs::create_dir(&bin_dir).unwrap();
    let fake_claude = write_fake_claude(&bin_dir);
    let capture_path = dir.path().join("captured-claude-config.json");
    let wrapper_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/agent-claude-code.sh");

    let output = Command::new(&wrapper_path)
        .arg("--print")
        .arg("hello")
        .env(
            "RUNA_MCP_CONFIG",
            r#"{"command":"/tmp/runa-mcp","args":["--protocol","implement"],"env":{"RUNA_CONFIG":"/tmp/config.toml","RUNA_WORKING_DIR":"/tmp/project"}}"#,
        )
        .env("FAKE_CLAUDE_CAPTURE", &capture_path)
        .env(
            "PATH",
            format!("{}:{}", bin_dir.display(), std::env::var("PATH").unwrap_or_default()),
        )
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fake_claude, bin_dir.join("claude"));

    let wrapped: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&capture_path).unwrap()).unwrap();
    assert_eq!(
        wrapped,
        serde_json::json!({
            "mcpServers": {
                "runa": {
                    "command": "/tmp/runa-mcp",
                    "args": ["--protocol", "implement"],
                    "env": {
                        "RUNA_CONFIG": "/tmp/config.toml",
                        "RUNA_WORKING_DIR": "/tmp/project"
                    }
                }
            }
        })
    );
}
#[test]
fn step_without_dry_run_reports_missing_runa_mcp_after_sibling_and_path_lookup() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        manifest_toml(),
        &methodology_schemas(),
        &methodology_protocols(),
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

    let payload_path = dir.path().join("captured-payload.json");
    let mcp_config_path = dir.path().join("captured-mcp-config.json");
    let agent_path = write_no_output_agent(dir.path());
    append_agent_command_config(
        &project_dir,
        &[
            agent_path.as_path(),
            payload_path.as_path(),
            mcp_config_path.as_path(),
        ],
    );

    let isolated_runa = copy_isolated_runa(dir.path());
    let empty_path = dir.path().join("empty-path");
    fs::create_dir(&empty_path).unwrap();

    let mut command = Command::new(&isolated_runa);
    command
        .arg("step")
        .current_dir(&project_dir)
        .env("PATH", &empty_path);
    let output = command_output_retry_busy(command);

    assert_eq!(output.status.code(), Some(6), "{output:?}");
    assert!(!output.status.success(), "step should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("runa-mcp"), "stderr: {stderr}");
    assert!(stderr.contains("sibling"), "stderr: {stderr}");
    assert!(stderr.contains("PATH"), "stderr: {stderr}");
}

#[test]
fn step_without_dry_run_rejects_json_output() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        manifest_toml(),
        &methodology_schemas(),
        &methodology_protocols(),
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let output = runa_bin()
        .arg("step")
        .arg("--json")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(!output.status.success(), "step should reject --json");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--json is only supported with --dry-run"));
}
#[test]
fn step_without_dry_run_with_no_ready_protocols_skips_runa_mcp_lookup() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        implement_only_manifest_toml(),
        &methodology_schemas(),
        &implement_only_methodology_protocols(),
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let payload_path = dir.path().join("captured-payload.txt");
    let agent_path = write_no_output_agent(dir.path());
    append_agent_command_config(
        &project_dir,
        &[agent_path.as_path(), payload_path.as_path()],
    );

    let isolated_runa = copy_isolated_runa(dir.path());
    let empty_path = dir.path().join("empty-path");
    fs::create_dir(&empty_path).unwrap();

    let mut command = Command::new(&isolated_runa);
    command
        .arg("step")
        .current_dir(&project_dir)
        .env("PATH", &empty_path);
    let output = command_output_retry_busy(command);

    assert_eq!(output.status.code(), Some(3), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("No READY protocols."), "stdout: {stdout}");
    assert!(!payload_path.exists(), "agent should not execute");
}

#[test]
fn step_without_dry_run_reports_project_load_failure_before_agent_config_failure() {
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
        .arg("step")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(6), "{output:?}");
    assert!(
        !output.status.success(),
        "step should fail outside an initialized project"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("not a runa project"), "stderr: {stderr}");
    assert!(
        !stderr.contains("no agent command configured"),
        "stderr: {stderr}"
    );
}
#[test]
fn step_without_dry_run_stops_after_first_non_zero_agent_exit() {
    let dir = tempfile::tempdir().unwrap();
    let wu_schema = r#"{"type":"object","required":["title","work_unit"],"properties":{"title":{"type":"string"},"work_unit":{"type":"string"}}}"#;
    let manifest_path = common::write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "doc"

[[artifact_types]]
name = "reviewed"

[[protocols]]
name = "review"
requires = ["doc"]
produces = ["reviewed"]
scoped = true
trigger = { type = "on_artifact", name = "doc" }
"#,
        &[("doc", wu_schema), ("reviewed", wu_schema)],
        &["review"],
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("doc")).unwrap();
    fs::write(
        workspace.join("doc/a.json"),
        r#"{"title":"draft-a","work_unit":"wu-a"}"#,
    )
    .unwrap();
    fs::write(
        workspace.join("doc/b.json"),
        r#"{"title":"draft-b","work_unit":"wu-b"}"#,
    )
    .unwrap();

    let count_file = dir.path().join("count.txt");
    let payload_dir = dir.path().join("payloads");
    fs::create_dir_all(&payload_dir).unwrap();
    let agent_path = write_failing_agent(dir.path());
    append_agent_command_config(
        &project_dir,
        &[
            agent_path.as_path(),
            count_file.as_path(),
            payload_dir.as_path(),
        ],
    );

    let output = runa_bin()
        .arg("step")
        .arg("--work-unit")
        .arg("wu-a")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(5), "{output:?}");
    assert!(
        !output.status.success(),
        "step should fail on non-zero agent exit"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("exited with status 17"), "stderr: {stderr}");
    assert!(stderr.contains("protocol 'review'"), "stderr: {stderr}");
    assert!(stderr.contains("work_unit=wu-a"), "stderr: {stderr}");
    assert_eq!(fs::read_to_string(&count_file).unwrap(), "1");
    let captured = fs::read_to_string(payload_dir.join("1.json")).unwrap();
    assert!(
        captured.contains("# Protocol: review (work_unit=wu-a)"),
        "{captured}"
    );
    assert!(payload_dir.join("1.json").is_file());
    assert!(!payload_dir.join("2.json").exists());
}
#[test]
fn step_without_dry_run_reports_postcondition_failure_after_successful_agent_exit() {
    let dir = tempfile::tempdir().unwrap();
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
            (
                "implementation",
                r#"{"type":"object","required":["done"],"properties":{"done":{"type":"boolean"}}}"#,
            ),
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
        r#"{"title":"ship step"}"#,
    )
    .unwrap();

    let payload_path = dir.path().join("captured-payload.txt");
    let agent_path = write_no_output_agent(dir.path());
    append_agent_command_config(
        &project_dir,
        &[agent_path.as_path(), payload_path.as_path()],
    );

    let output = runa_bin()
        .arg("step")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(5), "{output:?}");
    assert!(
        !output.status.success(),
        "step should fail when postconditions remain unsatisfied"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("post-execution reconciliation failed for protocol 'implement'"),
        "stderr: {stderr}"
    );
    assert!(
        stderr
            .contains("agent command succeeded but protocol outputs did not satisfy the contract"),
        "stderr: {stderr}"
    );
    assert!(
        stderr.contains("implementation' which is missing after execution"),
        "stderr: {stderr}"
    );

    let captured = fs::read_to_string(&payload_path).unwrap();
    assert!(captured.contains("# Protocol: implement"), "{captured}");
    assert!(!workspace.join("implementation/impl-1.json").exists());
}
#[test]
fn step_without_dry_run_returns_exit_4_when_no_actionable_work_is_ready() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        implement_only_manifest_toml(),
        &methodology_schemas(),
        &implement_only_methodology_protocols(),
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("constraints")).unwrap();
    fs::create_dir_all(workspace.join("implementation")).unwrap();
    fs::write(
        workspace.join("constraints/spec-1.json"),
        r#"{"title":"ship step"}"#,
    )
    .unwrap();
    fs::write(
        workspace.join("implementation/current.json"),
        r#"{"done":true}"#,
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

    let payload_path = dir.path().join("captured-payload.txt");
    let agent_path = write_no_output_agent(dir.path());
    append_agent_command_config(
        &project_dir,
        &[agent_path.as_path(), payload_path.as_path()],
    );

    let isolated_runa = copy_isolated_runa(dir.path());
    let empty_path = dir.path().join("empty-path");
    fs::create_dir(&empty_path).unwrap();

    let mut command = Command::new(&isolated_runa);
    command
        .arg("step")
        .current_dir(&project_dir)
        .env("PATH", &empty_path);
    let output = command_output_retry_busy(command);

    assert_eq!(output.status.code(), Some(4), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("No READY protocols."), "stdout: {stdout}");
    assert!(!payload_path.exists(), "agent should not execute");
}
#[test]
fn step_without_dry_run_does_not_rerun_ready_protocols_for_persistent_scan_warnings() {
    let dir = tempfile::tempdir().unwrap();
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
            (
                "implementation",
                r#"{"type":"object","required":["done"],"properties":{"done":{"type":"boolean"}}}"#,
            ),
        ],
        &["verify"],
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("constraints")).unwrap();
    fs::create_dir_all(workspace.join("implementation")).unwrap();
    fs::create_dir_all(workspace.join("unknown")).unwrap();
    fs::write(
        workspace.join("constraints/spec-1.json"),
        r#"{"title":"ship step"}"#,
    )
    .unwrap();
    fs::write(
        workspace.join("implementation/impl-1.json"),
        r#"{"done":true}"#,
    )
    .unwrap();

    let count_file = dir.path().join("invocations.txt");
    let agent_path = write_second_run_fails_agent(dir.path());
    append_agent_command_config(&project_dir, &[agent_path.as_path(), count_file.as_path()]);

    let output = runa_bin()
        .arg("step")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(fs::read_to_string(&count_file).unwrap(), "1");
}

#[test]
fn step_dry_run_reports_blocked_reasons_when_no_skills_are_ready() {
    let dir = tempfile::tempdir().unwrap();
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
            (
                "implementation",
                r#"{"type":"object","required":["done"],"properties":{"done":{"type":"boolean"}}}"#,
            ),
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
        .arg("step")
        .arg("--dry-run")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(3), "{output:?}");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Execution plan: none"), "stdout: {stdout}");
    assert!(stdout.contains("No READY protocols."), "stdout: {stdout}");
    assert!(stdout.contains("BLOCKED:"), "stdout: {stdout}");
    assert!(
        stdout.contains("implementation (missing)"),
        "stdout: {stdout}"
    );
}
#[test]
fn step_dry_run_omits_partially_scanned_accepted_inputs_from_context() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        manifest_toml(),
        &methodology_schemas(),
        &methodology_protocols(),
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("constraints")).unwrap();
    fs::create_dir_all(workspace.join("prior-art")).unwrap();
    fs::write(
        workspace.join("constraints/spec-1.json"),
        r#"{"title":"ship step"}"#,
    )
    .unwrap();
    fs::write(
        workspace.join("prior-art/visible.json"),
        r#"{"source":"notes"}"#,
    )
    .unwrap();
    let unreadable = workspace.join("prior-art/hidden.json");
    fs::write(&unreadable, r#"{"source":"hidden"}"#).unwrap();
    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o0)).unwrap();

    let output = runa_bin()
        .arg("step")
        .arg("--dry-run")
        .arg("--json")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o644)).unwrap();

    assert_eq!(output.status.code(), Some(0), "{output:?}");

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        value["scan_warnings"],
        serde_json::json!([
            "artifact type 'prior-art' was only partially scanned: 1 unreadable entry"
        ])
    );

    let execution_plan = value["execution_plan"].as_array().unwrap();
    assert_eq!(execution_plan.len(), 1, "{value:#}");
    assert_eq!(
        execution_plan[0]["context"]["inputs"],
        serde_json::json!([
            {
                "artifact_type": "constraints",
                "instance_id": "spec-1",
                "display_path": workspace.join("constraints/spec-1.json"),
                "content_hash": "sha256:dd4077b358533c789242e86ac7f5e7dffa0a587d5b4acfd343c612ae9ddfd315",
                "relationship": "requires"
            }
        ])
    );

    let protocols = value["protocols"].as_array().unwrap();
    assert_eq!(protocols[0]["status"], "ready");
    assert_eq!(
        protocols[0]["inputs"],
        serde_json::json!([
            {
                "artifact_type": "constraints",
                "instance_id": "spec-1",
                "path": ".runa/workspace/constraints/spec-1.json",
                "relationship": "requires"
            }
        ])
    );
}

#[test]
fn step_dry_run_json_reports_cycle_and_omits_execution_plan() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "a"

[[artifact_types]]
name = "b"

[[protocols]]
name = "first"
requires = ["b"]
produces = ["a"]
trigger = { type = "on_change", name = "b" }

[[protocols]]
name = "second"
requires = ["a"]
produces = ["b"]
trigger = { type = "on_change", name = "b" }
"#,
        &[
            (
                "a",
                r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#,
            ),
            (
                "b",
                r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#,
            ),
        ],
        &["first", "second"],
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let output = runa_bin()
        .arg("step")
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
    assert_eq!(value["execution_plan"], serde_json::json!([]));
    let protocols = value["protocols"].as_array().unwrap();
    assert_eq!(protocols.len(), 2, "{value:#}");
    assert_eq!(protocols[0]["name"], "first");
    assert_eq!(protocols[1]["name"], "second");
}

#[test]
fn step_dry_run_text_reports_cycle_and_no_execution_plan() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "a"

[[artifact_types]]
name = "b"

[[protocols]]
name = "first"
requires = ["b"]
produces = ["a"]
trigger = { type = "on_change", name = "b" }

[[protocols]]
name = "second"
requires = ["a"]
produces = ["b"]
trigger = { type = "on_change", name = "b" }
"#,
        &[
            (
                "a",
                r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#,
            ),
            (
                "b",
                r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#,
            ),
        ],
        &["first", "second"],
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let output = runa_bin()
        .arg("step")
        .arg("--dry-run")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(3), "{output:?}");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("warning: dependency cycle detected: first -> second"),
        "stdout: {stdout}"
    );
    assert!(stdout.contains("Execution plan: none"), "stdout: {stdout}");
    assert!(stdout.contains("READY:"), "stdout: {stdout}");
}

#[test]
fn step_dry_run_keeps_non_cyclic_ready_skills_in_plan_when_cycle_exists() {
    let dir = tempfile::tempdir().unwrap();
    let title_schema =
        r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#;
    let manifest_path = common::write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "seed"

[[artifact_types]]
name = "a"

[[artifact_types]]
name = "b"

[[artifact_types]]
name = "result"

[[protocols]]
name = "independent"
requires = ["seed"]
produces = ["result"]
trigger = { type = "on_artifact", name = "seed" }

[[protocols]]
name = "first"
requires = ["b"]
produces = ["a"]
trigger = { type = "on_change", name = "b" }

[[protocols]]
name = "second"
requires = ["a"]
produces = ["b"]
trigger = { type = "on_change", name = "b" }
"#,
        &[
            ("seed", title_schema),
            ("a", title_schema),
            ("b", title_schema),
            (
                "result",
                r#"{"type":"object","required":["done"],"properties":{"done":{"type":"boolean"}}}"#,
            ),
        ],
        &["independent", "first", "second"],
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("seed")).unwrap();
    fs::write(workspace.join("seed/input.json"), r#"{"title":"ship"}"#).unwrap();

    let output = runa_bin()
        .arg("step")
        .arg("--dry-run")
        .arg("--json")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        value["cycle"],
        serde_json::json!(["first", "second"]),
        "{value:#}"
    );

    let execution_plan = value["execution_plan"].as_array().unwrap();
    assert_eq!(execution_plan.len(), 1, "{value:#}");
    assert_eq!(execution_plan[0]["protocol"], "independent");

    let protocols = value["protocols"].as_array().unwrap();
    assert_eq!(protocols.len(), 3, "{value:#}");
    assert_eq!(protocols[0]["name"], "independent");
}

#[test]
fn step_dry_run_scoped_ignores_unscoped_cycle_participants() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "x"

[[artifact_types]]
name = "y"

[[protocols]]
name = "publish"
requires = ["y"]
produces = ["x"]
trigger = { type = "on_artifact", name = "y" }

[[protocols]]
name = "implement"
requires = ["x"]
produces = ["y"]
scoped = true
trigger = { type = "on_artifact", name = "x" }
"#,
        &[
            (
                "x",
                r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#,
            ),
            (
                "y",
                r#"{"type":"object","required":["title","work_unit"],"properties":{"title":{"type":"string"},"work_unit":{"type":"string"}}}"#,
            ),
        ],
        &["publish", "implement"],
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("x")).unwrap();
    fs::write(workspace.join("x/input.json"), r#"{"title":"ship"}"#).unwrap();

    let output = runa_bin()
        .arg("step")
        .arg("--dry-run")
        .arg("--json")
        .arg("--work-unit")
        .arg("wu-a")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(value.get("cycle").is_none(), "{value:#}");

    let execution_plan = value["execution_plan"].as_array().unwrap();
    assert_eq!(execution_plan.len(), 1, "{value:#}");
    assert_eq!(execution_plan[0]["protocol"], "implement");
    assert_eq!(execution_plan[0]["work_unit"], "wu-a");

    let protocols = value["protocols"].as_array().unwrap();
    assert_eq!(protocols.len(), 1, "{value:#}");
    assert_eq!(protocols[0]["name"], "implement");
    assert_eq!(protocols[0]["status"], "ready");
    assert_eq!(protocols[0]["trigger"], "satisfied");
}

#[test]
fn step_dry_run_keeps_ready_skills_downstream_of_cycle_when_inputs_exist() {
    let dir = tempfile::tempdir().unwrap();
    let title_schema =
        r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#;
    let manifest_path = common::write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "a"

[[artifact_types]]
name = "b"

[[artifact_types]]
name = "result"

[[protocols]]
name = "first"
requires = ["b"]
produces = ["a"]
trigger = { type = "on_change", name = "b" }

[[protocols]]
name = "second"
requires = ["a"]
produces = ["b"]
trigger = { type = "on_change", name = "b" }

[[protocols]]
name = "publish"
requires = ["a"]
produces = ["result"]
trigger = { type = "on_artifact", name = "a" }
"#,
        &[
            ("a", title_schema),
            ("b", title_schema),
            (
                "result",
                r#"{"type":"object","required":["done"],"properties":{"done":{"type":"boolean"}}}"#,
            ),
        ],
        &["first", "second", "publish"],
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("a")).unwrap();
    fs::write(
        workspace.join("a/input.json"),
        r#"{"title":"already here"}"#,
    )
    .unwrap();

    let output = runa_bin()
        .arg("step")
        .arg("--dry-run")
        .arg("--json")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        value["cycle"],
        serde_json::json!(["first", "second"]),
        "{value:#}"
    );

    let execution_plan = value["execution_plan"].as_array().unwrap();
    assert_eq!(execution_plan.len(), 1, "{value:#}");
    assert_eq!(execution_plan[0]["protocol"], "publish");

    let protocols = value["protocols"].as_array().unwrap();
    assert_eq!(protocols[0]["name"], "publish");
    assert_eq!(protocols[0]["status"], "ready");
}

#[test]
fn step_dry_run_preserves_dependency_order_for_ready_skills_with_unrelated_cycle() {
    let dir = tempfile::tempdir().unwrap();
    let title_schema =
        r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#;
    let manifest_path = common::write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "root"

[[artifact_types]]
name = "seed"

[[artifact_types]]
name = "result"

[[artifact_types]]
name = "a"

[[artifact_types]]
name = "b"

[[protocols]]
name = "independent"
requires = ["root"]
produces = ["seed"]
trigger = { type = "on_artifact", name = "root" }

[[protocols]]
name = "publish"
requires = ["seed"]
produces = ["result"]
trigger = { type = "on_artifact", name = "seed" }

[[protocols]]
name = "first"
requires = ["b"]
produces = ["a"]
trigger = { type = "on_change", name = "b" }

[[protocols]]
name = "second"
requires = ["a"]
produces = ["b"]
trigger = { type = "on_change", name = "b" }
"#,
        &[
            ("root", title_schema),
            ("seed", title_schema),
            (
                "result",
                r#"{"type":"object","required":["done"],"properties":{"done":{"type":"boolean"}}}"#,
            ),
            ("a", title_schema),
            ("b", title_schema),
        ],
        &["independent", "publish", "first", "second"],
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("root")).unwrap();
    fs::create_dir_all(workspace.join("seed")).unwrap();
    fs::write(workspace.join("root/input.json"), r#"{"title":"root"}"#).unwrap();
    fs::write(workspace.join("seed/input.json"), r#"{"title":"ship"}"#).unwrap();

    let output = runa_bin()
        .arg("step")
        .arg("--dry-run")
        .arg("--json")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let execution_plan = value["execution_plan"].as_array().unwrap();
    assert_eq!(execution_plan.len(), 1, "{value:#}");
    assert_eq!(execution_plan[0]["protocol"], "publish");
}

#[test]
fn step_dry_run_reports_scoped_on_change_readiness_when_freshness_is_mixed() {
    let dir = tempfile::tempdir().unwrap();
    let wu_schema = r#"{"type":"object","required":["title","work_unit"],"properties":{"title":{"type":"string"},"work_unit":{"type":"string"}}}"#;
    let manifest_path = common::write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "doc"

[[artifact_types]]
name = "reviewed"

[[protocols]]
name = "review"
produces = ["reviewed"]
scoped = true
trigger = { type = "on_change", name = "doc" }
"#,
        &[("doc", wu_schema), ("reviewed", wu_schema)],
        &["review"],
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("doc")).unwrap();
    fs::create_dir_all(workspace.join("reviewed")).unwrap();

    fs::write(
        workspace.join("reviewed/a.json"),
        r#"{"title":"done-a","work_unit":"wu-a"}"#,
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
        workspace.join("doc/a.json"),
        r#"{"title":"draft-a","work_unit":"wu-a"}"#,
    )
    .unwrap();
    let second_scan = runa_bin()
        .arg("scan")
        .current_dir(&project_dir)
        .output()
        .unwrap();
    assert!(
        second_scan.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&second_scan.stderr)
    );
    fs::write(
        workspace.join("doc/b.json"),
        r#"{"title":"draft-b","work_unit":"wu-b"}"#,
    )
    .unwrap();
    let third_scan = runa_bin()
        .arg("scan")
        .current_dir(&project_dir)
        .output()
        .unwrap();
    assert!(
        third_scan.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&third_scan.stderr)
    );
    fs::write(
        workspace.join("reviewed/b.json"),
        r#"{"title":"done-b","work_unit":"wu-b"}"#,
    )
    .unwrap();

    let wu_a_output = runa_bin()
        .arg("step")
        .arg("--dry-run")
        .arg("--json")
        .arg("--work-unit")
        .arg("wu-a")
        .current_dir(&project_dir)
        .output()
        .unwrap();
    let wu_b_output = runa_bin()
        .arg("step")
        .arg("--dry-run")
        .arg("--json")
        .arg("--work-unit")
        .arg("wu-b")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert_eq!(wu_a_output.status.code(), Some(0), "{wu_a_output:?}");
    assert_eq!(wu_b_output.status.code(), Some(3), "{wu_b_output:?}");

    let wu_a_value: serde_json::Value = serde_json::from_slice(&wu_a_output.stdout).unwrap();
    let wu_a_plan = wu_a_value["execution_plan"].as_array().unwrap();
    assert_eq!(wu_a_plan.len(), 1, "{wu_a_value:#}");
    assert_eq!(wu_a_plan[0]["protocol"], "review");
    assert_eq!(wu_a_plan[0]["work_unit"], "wu-a");
    let wu_a_protocols = wu_a_value["protocols"].as_array().unwrap();
    assert_eq!(wu_a_protocols.len(), 1, "{wu_a_value:#}");
    assert_eq!(wu_a_protocols[0]["status"], "ready");
    assert_eq!(wu_a_protocols[0]["work_unit"], "wu-a");

    let wu_b_value: serde_json::Value = serde_json::from_slice(&wu_b_output.stdout).unwrap();
    let wu_b_plan = wu_b_value["execution_plan"].as_array().unwrap();
    assert!(wu_b_plan.is_empty(), "{wu_b_value:#}");
    let wu_b_protocols = wu_b_value["protocols"].as_array().unwrap();
    assert_eq!(wu_b_protocols.len(), 1, "{wu_b_value:#}");
    assert_eq!(wu_b_protocols[0]["status"], "waiting");
    assert_eq!(wu_b_protocols[0]["work_unit"], "wu-b");
}
