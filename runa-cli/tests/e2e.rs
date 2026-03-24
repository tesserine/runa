mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn runa_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_runa"))
}

fn manifest_toml() -> &'static str {
    r#"
name = "groundwork"

[[artifact_types]]
name = "constraints"

[[artifact_types]]
name = "prior-art"

[[artifact_types]]
name = "design-doc"

[[artifact_types]]
name = "notes"

[[artifact_types]]
name = "implementation"

[[protocols]]
name = "research"
requires = ["constraints"]
accepts = ["prior-art"]
produces = ["design-doc"]
may_produce = ["notes"]
trigger = { type = "on_artifact", name = "constraints" }

[[protocols]]
name = "implement"
requires = ["design-doc"]
accepts = ["notes"]
produces = ["implementation"]
trigger = { type = "all_of", conditions = [
    { type = "on_artifact", name = "design-doc" },
    { type = "on_artifact", name = "prior-art" }
] }

[[protocols]]
name = "verify"
requires = ["implementation"]
trigger = { type = "on_artifact", name = "implementation" }
"#
}

const SCHEMAS: &[(&str, &str)] = &[
    (
        "constraints",
        r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#,
    ),
    (
        "prior-art",
        r#"{"type":"object","required":["source"],"properties":{"source":{"type":"string"}}}"#,
    ),
    (
        "design-doc",
        r#"{"type":"object","required":["summary"],"properties":{"summary":{"type":"string"}}}"#,
    ),
    (
        "notes",
        r#"{"type":"object","required":["text"],"properties":{"text":{"type":"string"}}}"#,
    ),
    (
        "implementation",
        r#"{"type":"object","required":["done"],"properties":{"done":{"type":"boolean"}}}"#,
    ),
];

const PROTOCOLS: &[&str] = &["research", "implement", "verify"];

fn run_command(project_dir: &Path, args: &[&str]) -> Output {
    runa_bin()
        .args(args)
        .current_dir(project_dir)
        .output()
        .unwrap()
}

fn run_json(project_dir: &Path, args: &[&str]) -> serde_json::Value {
    let output = run_command(project_dir, args);
    assert!(
        output.status.success(),
        "command `{}` failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn init_project(project_dir: &Path, manifest_path: &Path) -> Output {
    runa_bin()
        .arg("init")
        .arg("--methodology")
        .arg(manifest_path)
        .current_dir(project_dir)
        .output()
        .unwrap()
}

fn write_artifact(project_dir: &Path, artifact_type: &str, file_name: &str, json: &str) -> PathBuf {
    let path = project_dir
        .join(".runa/workspace")
        .join(artifact_type)
        .join(file_name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, json).unwrap();
    path
}

fn assert_context_inputs(inputs: &serde_json::Value, expected: &[(&str, &str, PathBuf, &str)]) {
    let actual = inputs.as_array().unwrap();
    assert_eq!(actual.len(), expected.len(), "{inputs:#}");

    for (entry, (artifact_type, instance_id, path, relationship)) in actual.iter().zip(expected) {
        assert_eq!(entry["artifact_type"], *artifact_type);
        assert_eq!(entry["instance_id"], *instance_id);
        assert_eq!(entry["path"], path.display().to_string());
        assert_eq!(entry["relationship"], *relationship);

        let content_hash = entry["content_hash"].as_str().unwrap();
        assert!(
            content_hash.starts_with("sha256:"),
            "expected sha256 content hash, got {content_hash}"
        );
        assert_eq!(
            content_hash.len(),
            71,
            "unexpected hash length: {content_hash}"
        );
    }
}

#[test]
fn e2e_progression_exercises_cli_pipeline_with_real_methodology() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(dir.path(), manifest_toml(), SCHEMAS, PROTOCOLS);

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();

    let init_output = init_project(&project_dir, &manifest_path);
    assert!(
        init_output.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&init_output.stderr)
    );
    let init_stdout = String::from_utf8_lossy(&init_output.stdout);
    assert!(init_stdout.contains("groundwork"), "stdout: {init_stdout}");
    assert!(
        init_stdout.contains("5 artifact types"),
        "stdout: {init_stdout}"
    );
    assert!(init_stdout.contains("3 protocols"), "stdout: {init_stdout}");
    assert!(project_dir.join(".runa/config.toml").is_file());
    assert!(project_dir.join(".runa/state.toml").is_file());
    assert!(project_dir.join(".runa/store").is_dir());
    assert!(project_dir.join(".runa/workspace").is_dir());

    let empty_doctor = run_command(&project_dir, &["doctor"]);
    assert!(
        !empty_doctor.status.success(),
        "doctor should fail on empty project"
    );
    let empty_doctor_stdout = String::from_utf8_lossy(&empty_doctor.stdout);
    assert!(
        empty_doctor_stdout.contains("research: cannot execute (missing: constraints)"),
        "stdout: {empty_doctor_stdout}"
    );
    assert!(
        empty_doctor_stdout.contains("implement: cannot execute (missing: design-doc)"),
        "stdout: {empty_doctor_stdout}"
    );
    assert!(
        empty_doctor_stdout.contains("verify: cannot execute (missing: implementation)"),
        "stdout: {empty_doctor_stdout}"
    );

    write_artifact(
        &project_dir,
        "constraints",
        "spec-1.json",
        r#"{"title":"Ship the runtime"}"#,
    );
    write_artifact(
        &project_dir,
        "prior-art",
        "survey-1.json",
        r#"{"source":"field-notes"}"#,
    );

    let first_scan = run_command(&project_dir, &["scan"]);
    assert!(
        first_scan.status.success(),
        "scan failed: {}",
        String::from_utf8_lossy(&first_scan.stderr)
    );
    let first_scan_stdout = String::from_utf8_lossy(&first_scan.stdout);
    assert!(
        first_scan_stdout.contains("Summary: 2 new"),
        "stdout: {first_scan_stdout}"
    );
    assert!(
        first_scan_stdout.contains("constraints/spec-1"),
        "stdout: {first_scan_stdout}"
    );
    assert!(
        first_scan_stdout.contains("prior-art/survey-1"),
        "stdout: {first_scan_stdout}"
    );

    let first_doctor = run_command(&project_dir, &["doctor"]);
    assert!(
        !first_doctor.status.success(),
        "doctor should still fail with downstream missing artifacts"
    );
    let first_doctor_stdout = String::from_utf8_lossy(&first_doctor.stdout);
    assert!(
        first_doctor_stdout.contains("research: ok"),
        "stdout: {first_doctor_stdout}"
    );
    assert!(
        first_doctor_stdout.contains("implement: cannot execute (missing: design-doc)"),
        "stdout: {first_doctor_stdout}"
    );
    assert!(
        first_doctor_stdout.contains("verify: cannot execute (missing: implementation)"),
        "stdout: {first_doctor_stdout}"
    );

    let first_status = run_json(&project_dir, &["status", "--json"]);
    let first_skills = first_status["protocols"].as_array().unwrap();
    assert_eq!(first_skills.len(), 3, "{first_status:#}");
    assert_eq!(first_skills[0]["name"], "research");
    assert_eq!(first_skills[0]["status"], "ready");
    assert_eq!(
        first_skills[0]["inputs"],
        serde_json::json!([
            {
                "artifact_type": "constraints",
                "instance_id": "spec-1",
                "path": ".runa/workspace/constraints/spec-1.json",
                "relationship": "requires"
            },
            {
                "artifact_type": "prior-art",
                "instance_id": "survey-1",
                "path": ".runa/workspace/prior-art/survey-1.json",
                "relationship": "accepts"
            }
        ])
    );
    assert_eq!(first_skills[1]["name"], "implement");
    assert_eq!(first_skills[1]["status"], "waiting");
    assert_eq!(
        first_skills[1]["unsatisfied_conditions"],
        serde_json::json!([
            "on_artifact(design-doc): no valid instances of artifact type 'design-doc' exist"
        ])
    );
    assert_eq!(first_skills[2]["name"], "verify");
    assert_eq!(first_skills[2]["status"], "waiting");
    assert_eq!(
        first_skills[2]["unsatisfied_conditions"],
        serde_json::json!([
            "on_artifact(implementation): no valid instances of artifact type 'implementation' exist"
        ])
    );

    let first_step = run_json(&project_dir, &["step", "--dry-run", "--json"]);
    let first_plan = first_step["execution_plan"].as_array().unwrap();
    assert_eq!(first_plan.len(), 1, "{first_step:#}");
    assert_eq!(first_plan[0]["protocol"], "research");
    assert_eq!(first_plan[0]["trigger"], "on_artifact(constraints)");
    assert_eq!(first_plan[0]["context"]["instructions"], "# research\n");
    assert_eq!(
        first_plan[0]["context"]["expected_outputs"],
        serde_json::json!({
            "produces": ["design-doc"],
            "may_produce": ["notes"]
        })
    );
    assert_context_inputs(
        &first_plan[0]["context"]["inputs"],
        &[
            (
                "constraints",
                "spec-1",
                project_dir.join(".runa/workspace/constraints/spec-1.json"),
                "requires",
            ),
            (
                "prior-art",
                "survey-1",
                project_dir.join(".runa/workspace/prior-art/survey-1.json"),
                "accepts",
            ),
        ],
    );

    write_artifact(
        &project_dir,
        "design-doc",
        "plan-1.json",
        r#"{"summary":"Implement the runtime"}"#,
    );
    write_artifact(
        &project_dir,
        "notes",
        "context-1.json",
        r#"{"text":"Useful context"}"#,
    );
    write_artifact(&project_dir, "notes", "bad.json", r#"{"wrong":true}"#);

    let second_scan = run_command(&project_dir, &["scan"]);
    assert!(
        second_scan.status.success(),
        "scan failed: {}",
        String::from_utf8_lossy(&second_scan.stderr)
    );
    let second_scan_stdout = String::from_utf8_lossy(&second_scan.stdout);
    assert!(
        second_scan_stdout.contains("Summary: 3 new"),
        "stdout: {second_scan_stdout}"
    );
    assert!(
        second_scan_stdout.contains("1 invalid"),
        "stdout: {second_scan_stdout}"
    );
    assert!(
        second_scan_stdout.contains("design-doc/plan-1"),
        "stdout: {second_scan_stdout}"
    );
    assert!(
        second_scan_stdout.contains("notes/context-1"),
        "stdout: {second_scan_stdout}"
    );
    assert!(
        second_scan_stdout.contains("notes/bad"),
        "stdout: {second_scan_stdout}"
    );

    let second_status = run_json(&project_dir, &["status", "--json"]);
    let second_skills = second_status["protocols"].as_array().unwrap();
    assert_eq!(second_skills.len(), 3, "{second_status:#}");
    assert_eq!(second_skills[0]["name"], "research");
    assert_eq!(second_skills[0]["status"], "ready");
    assert_eq!(second_skills[1]["name"], "implement");
    assert_eq!(second_skills[1]["status"], "ready");
    assert_eq!(
        second_skills[1]["inputs"],
        serde_json::json!([
            {
                "artifact_type": "design-doc",
                "instance_id": "plan-1",
                "path": ".runa/workspace/design-doc/plan-1.json",
                "relationship": "requires"
            },
            {
                "artifact_type": "notes",
                "instance_id": "context-1",
                "path": ".runa/workspace/notes/context-1.json",
                "relationship": "accepts"
            }
        ])
    );
    assert_eq!(second_skills[2]["name"], "verify");
    assert_eq!(second_skills[2]["status"], "waiting");
    assert!(
        second_skills[1].get("precondition_failures").is_none(),
        "{second_status:#}"
    );

    let second_step = run_json(&project_dir, &["step", "--dry-run", "--json"]);
    let second_plan = second_step["execution_plan"].as_array().unwrap();
    assert_eq!(second_plan.len(), 2, "{second_step:#}");
    assert_eq!(second_plan[0]["protocol"], "research");
    assert_eq!(second_plan[1]["protocol"], "implement");
    assert_context_inputs(
        &second_plan[1]["context"]["inputs"],
        &[
            (
                "design-doc",
                "plan-1",
                project_dir.join(".runa/workspace/design-doc/plan-1.json"),
                "requires",
            ),
            (
                "notes",
                "context-1",
                project_dir.join(".runa/workspace/notes/context-1.json"),
                "accepts",
            ),
        ],
    );

    let list_output = run_command(&project_dir, &["list"]);
    assert!(
        list_output.status.success(),
        "list failed: {}",
        String::from_utf8_lossy(&list_output.stderr)
    );
    let list_stdout = String::from_utf8_lossy(&list_output.stdout);
    assert!(list_stdout.contains("1. research"), "stdout: {list_stdout}");
    assert!(
        list_stdout.contains("2. implement"),
        "stdout: {list_stdout}"
    );
    assert!(list_stdout.contains("3. verify"), "stdout: {list_stdout}");
    assert!(
        list_stdout.contains("may_produce: notes"),
        "stdout: {list_stdout}"
    );
}
