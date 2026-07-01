//! Cold-start ticket entry: `runa run --ticket` / `runa go --ticket`.

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

fn runa_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_runa"))
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

/// A methodology with an unscoped acquisition surface (`decompose` produces
/// `work-unit`) and a scoped `take`.
const ENTRY_MANIFEST: &str = r#"
name = "groundwork"

[[artifact_types]]
name = "intent"

[[artifact_types]]
name = "work-unit"

[[artifact_types]]
name = "claim"

[[protocols]]
name = "decompose"
produces = ["work-unit"]
scoped = false
trigger = { type = "on_artifact", name = "work-unit" }

[[protocols]]
name = "take"
requires = ["work-unit"]
produces = ["claim"]
scoped = true
trigger = { type = "on_artifact", name = "work-unit" }
"#;

/// Like `ENTRY_MANIFEST`, but the acquisition surface requires an absent `seed`
/// artifact, so its preconditions block cold-start entry.
const BLOCKED_ENTRY_MANIFEST: &str = r#"
name = "groundwork"

[[artifact_types]]
name = "seed"

[[artifact_types]]
name = "work-unit"

[[artifact_types]]
name = "claim"

[[protocols]]
name = "decompose"
requires = ["seed"]
produces = ["work-unit"]
scoped = false
trigger = { type = "on_artifact", name = "seed" }

[[protocols]]
name = "take"
requires = ["work-unit"]
produces = ["claim"]
scoped = true
trigger = { type = "on_artifact", name = "work-unit" }
"#;

fn blocked_entry_schemas() -> &'static [(&'static str, &'static str)] {
    &[
        (
            "seed",
            r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#,
        ),
        (
            "work-unit",
            r#"{"type":"object","required":["title","handle"],"properties":{"title":{"type":"string"},"handle":{"type":"object"}}}"#,
        ),
        (
            "claim",
            r#"{"type":"object","required":["work_unit","scope"],"properties":{"work_unit":{"type":"string"},"scope":{"type":"string"}}}"#,
        ),
    ]
}

fn setup_blocked_entry_project(dir: &Path) -> PathBuf {
    let manifest_path = common::write_methodology(
        dir,
        BLOCKED_ENTRY_MANIFEST,
        blocked_entry_schemas(),
        &["decompose", "take"],
    );
    let project_dir = dir.join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);
    common::append_github_forge_config(&project_dir, "tesserine", "runa");
    project_dir
}

/// Reuses the `seed`-requiring manifest, but here a valid `seed` satisfies
/// preconditions and a partial scan (not a missing input) blocks entry.
fn setup_partial_scan_entry_project(dir: &Path) -> PathBuf {
    let manifest_path = common::write_methodology(
        dir,
        BLOCKED_ENTRY_MANIFEST,
        blocked_entry_schemas(),
        &["decompose", "take"],
    );
    let project_dir = dir.join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);
    common::append_github_forge_config(&project_dir, "tesserine", "runa");

    // A valid `seed` satisfies preconditions, but an unreadable sibling under
    // the same required type leaves it only partially scanned.
    let seed_dir = project_dir.join(".runa/workspace/seed");
    fs::create_dir_all(&seed_dir).unwrap();
    fs::write(seed_dir.join("good.json"), r#"{"title":"good"}"#).unwrap();
    let unreadable = seed_dir.join("unreadable.json");
    fs::write(&unreadable, r#"{"title":"hidden"}"#).unwrap();
    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o000)).unwrap();

    project_dir
}

fn entry_schemas() -> &'static [(&'static str, &'static str)] {
    &[
        (
            "intent",
            r#"{"type":"object","required":["statement","source"],"properties":{"statement":{"type":"string"},"source":{"type":"string"},"target":{"type":"string"}}}"#,
        ),
        (
            "work-unit",
            r#"{"type":"object","required":["title","handle"],"properties":{"title":{"type":"string"},"handle":{"type":"object"}}}"#,
        ),
        (
            "claim",
            r#"{"type":"object","required":["work_unit","scope"],"properties":{"work_unit":{"type":"string"},"scope":{"type":"string"}}}"#,
        ),
    ]
}

fn setup_entry_project(dir: &Path) -> PathBuf {
    let manifest_path =
        common::write_methodology(dir, ENTRY_MANIFEST, entry_schemas(), &["decompose", "take"]);
    let project_dir = dir.join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);
    common::append_github_forge_config(&project_dir, "tesserine", "runa");
    project_dir
}

fn write_intent_target(project_dir: &Path, target: &str) {
    write_intent_target_instance(project_dir, "intent-1", target);
}

fn write_intent_target_instance(project_dir: &Path, instance_id: &str, target: &str) {
    let intent_dir = project_dir.join(".runa/workspace/intent");
    fs::create_dir_all(&intent_dir).unwrap();
    fs::write(
        intent_dir.join(format!("{instance_id}.json")),
        format!(r#"{{"statement":"Start ticket work","source":"operator","target":"{target}"}}"#),
    )
    .unwrap();
}

/// Like `ENTRY_MANIFEST`, but the acquisition declares `work-unit` via
/// `may_produce` (the groundwork shape) instead of `produces`.
const MAY_PRODUCE_ENTRY_MANIFEST: &str = r#"
name = "groundwork"

[[artifact_types]]
name = "work-unit"

[[artifact_types]]
name = "claim"

[[protocols]]
name = "decompose"
may_produce = ["work-unit"]
scoped = false
trigger = { type = "on_artifact", name = "work-unit" }

[[protocols]]
name = "take"
requires = ["work-unit"]
produces = ["claim"]
scoped = true
trigger = { type = "on_artifact", name = "work-unit" }
"#;

fn setup_may_produce_entry_project(dir: &Path) -> PathBuf {
    let manifest_path = common::write_methodology(
        dir,
        MAY_PRODUCE_ENTRY_MANIFEST,
        entry_schemas(),
        &["decompose", "take"],
    );
    let project_dir = dir.join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);
    common::append_github_forge_config(&project_dir, "tesserine", "runa");
    project_dir
}

/// Drop forge atoms from the inherited environment so the test resolves the
/// deployment identity from `.runa/config.toml` deterministically.
fn clear_forge_env(command: &mut Command) -> &mut Command {
    command
        .env_remove("RUNA_FORGE_TYPE")
        .env_remove("RUNA_FORGE_OWNER")
        .env_remove("RUNA_FORGE_NAME")
        .env_remove("RUNA_FORGE_TRACKER_ID")
}

/// An agent that materializes the work-unit on the acquisition step and a claim
/// on `take`. The acquisition prompt must carry the entry reference.
fn write_entry_agent(dir: &Path) -> PathBuf {
    let script_path = dir.join("entry-agent.sh");
    fs::write(
        &script_path,
        r####"#!/bin/sh
log_file="$1"
payload=$(cat)
case "$payload" in
  *"# Protocol: decompose"*)
    case "$payload" in
      *"## Session entry"*"github:tesserine/runa#14"*) : ;;
      *) printf '%s\n' "$payload" > "$log_file.no-entry"; exit 23 ;;
    esac
    printf 'decompose\n' >> "$log_file"
    mkdir -p .runa/workspace/work-unit
    printf '%s\n' '{"title":"Cold start","handle":{"forge_tag":"github","url":"https://github.com/tesserine/runa/issues/14","number":14}}' > .runa/workspace/work-unit/work-unit-14-cold-start.json
    ;;
  *"# Protocol: take"*)
    printf 'take\n' >> "$log_file"
    mkdir -p .runa/workspace/claim
    printf '%s\n' '{"work_unit":"work-unit-14-cold-start","scope":"acquired"}' > .runa/workspace/claim/claim-1.json
    ;;
  *)
    printf '%s\n' "$payload" > "$log_file.unexpected"
    exit 19
    ;;
esac
"####,
    )
    .unwrap();
    fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755)).unwrap();
    script_path
}

/// An agent that satisfies `decompose`'s postconditions by producing a valid
/// work-unit, but for a *different* ticket (number 99), so the promised #14
/// never binds.
fn write_mismatched_ticket_agent(dir: &Path) -> PathBuf {
    let script_path = dir.join("mismatched-agent.sh");
    fs::write(
        &script_path,
        r####"#!/bin/sh
log_file="$1"
payload=$(cat)
case "$payload" in
  *"# Protocol: decompose"*)
    printf 'decompose\n' >> "$log_file"
    mkdir -p .runa/workspace/work-unit
    printf '%s\n' '{"title":"Other","handle":{"forge_tag":"github","url":"https://github.com/tesserine/runa/issues/99","number":99}}' > .runa/workspace/work-unit/work-unit-99-other.json
    ;;
  *)
    printf '%s\n' "$payload" > "$log_file.unexpected"
    exit 19
    ;;
esac
"####,
    )
    .unwrap();
    fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755)).unwrap();
    script_path
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

#[test]
fn run_ticket_dry_run_projects_acquisition_then_take() {
    let dir = tempfile::tempdir().unwrap();
    let project_dir = setup_entry_project(dir.path());

    let output = clear_forge_env(&mut runa_bin())
        .arg("run")
        .arg("--ticket")
        .arg("#14")
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
    assert_eq!(value["version"], 3, "{value:#}");
    assert_eq!(value["entry"]["reference"], "github:tesserine/runa#14");
    assert_eq!(value["entry"]["ticket_number"], 14);
    assert_eq!(value["entry"]["acquisition_protocol"], "decompose");

    let plan = value["execution_plan"].as_array().unwrap();
    assert_eq!(plan.len(), 2, "{value:#}");
    assert_eq!(plan[0]["protocol"], "decompose");
    assert_eq!(plan[0]["projection"], "current");
    // The acquisition step carries the entry reference in its context.
    assert_eq!(
        plan[0]["context"]["entry"]["reference"],
        "github:tesserine/runa#14"
    );
    assert_eq!(plan[1]["protocol"], "take");
    assert_eq!(plan[1]["projection"], "projected");
    assert_eq!(plan[1]["work_unit"], "work-unit-14");
}

#[test]
fn run_intent_target_dry_run_projects_same_entry_as_ticket() {
    let dir = tempfile::tempdir().unwrap();
    let project_dir = setup_entry_project(dir.path());
    write_intent_target(&project_dir, "#14");

    let output = clear_forge_env(&mut runa_bin())
        .arg("run")
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
    assert_eq!(value["version"], 3, "{value:#}");
    assert_eq!(value["entry"]["reference"], "github:tesserine/runa#14");
    assert_eq!(value["entry"]["ticket_number"], 14);
    assert_eq!(value["entry"]["acquisition_protocol"], "decompose");

    let plan = value["execution_plan"].as_array().unwrap();
    assert_eq!(plan.len(), 2, "{value:#}");
    assert_eq!(plan[0]["protocol"], "decompose");
    assert_eq!(plan[0]["projection"], "current");
    assert_eq!(
        plan[0]["context"]["entry"]["reference"],
        "github:tesserine/runa#14"
    );
    assert_eq!(plan[1]["protocol"], "take");
    assert_eq!(plan[1]["projection"], "projected");
    assert_eq!(plan[1]["work_unit"], "work-unit-14");
}

#[test]
fn run_intent_target_partial_scan_blocks_instead_of_routing_visible_sibling() {
    let dir = tempfile::tempdir().unwrap();
    let project_dir = setup_entry_project(dir.path());
    write_intent_target_instance(&project_dir, "intent-14", "#14");
    write_intent_target_instance(&project_dir, "intent-99", "#99");

    let scan = clear_forge_env(&mut runa_bin())
        .arg("scan")
        .current_dir(&project_dir)
        .output()
        .unwrap();
    assert!(
        scan.status.success(),
        "scan stderr: {}",
        String::from_utf8_lossy(&scan.stderr)
    );
    fs::set_permissions(
        project_dir.join(".runa/workspace/intent/intent-14.json"),
        fs::Permissions::from_mode(0o000),
    )
    .unwrap();

    let output = clear_forge_env(&mut runa_bin())
        .arg("run")
        .arg("--dry-run")
        .arg("--json")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(3), "{output:?}");
    assert!(
        output.stdout.is_empty(),
        "must not emit a routed JSON plan: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("entry input artifact type(s) intent were only partially scanned"),
        "stderr: {stderr}"
    );
}

#[test]
fn run_intent_target_invalid_reference_is_usage_error_without_agent() {
    let dir = tempfile::tempdir().unwrap();
    let project_dir = setup_entry_project(dir.path());
    write_intent_target(&project_dir, "not-a-ticket");

    let output = clear_forge_env(&mut runa_bin())
        .arg("run")
        .arg("--dry-run")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not a recognized forge ticket reference"),
        "stderr: {stderr}"
    );
}

#[test]
fn run_intent_target_rejects_foreign_deployment_reference() {
    let dir = tempfile::tempdir().unwrap();
    let project_dir = setup_entry_project(dir.path());
    write_intent_target(&project_dir, "tesserine/groundwork#14");

    let output = clear_forge_env(&mut runa_bin())
        .arg("run")
        .arg("--dry-run")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("disagrees"), "stderr: {stderr}");
}

#[test]
fn run_ticket_cold_acquires_then_cascades_to_take() {
    let dir = tempfile::tempdir().unwrap();
    let project_dir = setup_entry_project(dir.path());
    let log_path = dir.path().join("executed.log");
    let agent_path = write_entry_agent(dir.path());
    append_agent_command_config(&project_dir, &[agent_path.as_path(), log_path.as_path()]);

    let output = clear_forge_env(&mut runa_bin())
        .arg("run")
        .arg("--ticket")
        .arg("tesserine/runa#14")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let executed = fs::read_to_string(&log_path).unwrap();
    assert_eq!(executed, "decompose\ntake\n");
    let workspace = project_dir.join(".runa/workspace");
    assert!(
        workspace
            .join("work-unit/work-unit-14-cold-start.json")
            .is_file()
    );
    assert!(workspace.join("claim/claim-1.json").is_file());
}

#[test]
fn run_ticket_dry_run_projects_take_for_may_produce_acquisition() {
    let dir = tempfile::tempdir().unwrap();
    let project_dir = setup_may_produce_entry_project(dir.path());

    let output = clear_forge_env(&mut runa_bin())
        .arg("run")
        .arg("--ticket")
        .arg("#14")
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
    let plan = value["execution_plan"].as_array().unwrap();
    // The acquisition declares work-unit via may_produce, yet `take` must still
    // project on the acquired work-unit.
    assert_eq!(plan.len(), 2, "{value:#}");
    assert_eq!(plan[0]["protocol"], "decompose");
    assert_eq!(plan[1]["protocol"], "take");
    assert_eq!(plan[1]["work_unit"], "work-unit-14");
    assert_eq!(plan[1]["projection"], "projected");
}

#[test]
fn run_ticket_unresolved_leaves_no_acquisition_record() {
    let dir = tempfile::tempdir().unwrap();
    let project_dir = setup_entry_project(dir.path());
    let log_path = dir.path().join("executed.log");
    let agent_path = write_mismatched_ticket_agent(dir.path());
    append_agent_command_config(&project_dir, &[agent_path.as_path(), log_path.as_path()]);

    let output = clear_forge_env(&mut runa_bin())
        .arg("run")
        .arg("--ticket")
        .arg("#14")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    // The acquisition ran and satisfied postconditions, but produced a work-unit
    // for a different ticket: the entry is unresolved (exit 5)...
    assert_eq!(output.status.code(), Some(5), "{output:?}");
    assert_eq!(fs::read_to_string(&log_path).unwrap(), "decompose\n");

    // ...and no execution record claims the acquisition step completed.
    let records_path = project_dir.join(".runa/store/execution-records.json");
    if records_path.is_file() {
        let records = fs::read_to_string(&records_path).unwrap();
        assert!(
            !records.contains("decompose"),
            "stale acquisition record: {records}"
        );
    }
}

#[test]
fn run_ticket_blocks_when_acquisition_input_partially_scanned() {
    let dir = tempfile::tempdir().unwrap();
    let project_dir = setup_partial_scan_entry_project(dir.path());
    let log_path = dir.path().join("executed.log");
    let agent_path = write_entry_agent(dir.path());
    append_agent_command_config(&project_dir, &[agent_path.as_path(), log_path.as_path()]);

    let output = clear_forge_env(&mut runa_bin())
        .arg("run")
        .arg("--ticket")
        .arg("#14")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    // Preconditions are met (a valid seed exists), but the partial scan makes
    // the input untrusted: blocked (exit 3), agent never launched.
    assert_eq!(output.status.code(), Some(3), "{output:?}");
    assert!(!log_path.exists(), "acquisition agent should not run");
}

#[test]
fn go_ticket_blocks_when_acquisition_input_partially_scanned() {
    let dir = tempfile::tempdir().unwrap();
    let project_dir = setup_partial_scan_entry_project(dir.path());
    let log_path = dir.path().join("executed.log");
    let agent_path = write_entry_agent(dir.path());
    append_agent_command_config(&project_dir, &[agent_path.as_path(), log_path.as_path()]);

    let output = clear_forge_env(&mut runa_bin())
        .arg("go")
        .arg("--ticket")
        .arg("#14")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(3), "{output:?}");
    assert!(!log_path.exists(), "acquisition agent should not run");
}

/// A methodology whose only protocol is the acquisition itself — there is no
/// further scoped work after the work-unit is materialized.
const ACQUISITION_ONLY_MANIFEST: &str = r#"
name = "groundwork"

[[artifact_types]]
name = "work-unit"

[[protocols]]
name = "decompose"
produces = ["work-unit"]
scoped = false
trigger = { type = "on_artifact", name = "work-unit" }
"#;

fn setup_acquisition_only_project(dir: &Path) -> PathBuf {
    let manifest_path = common::write_methodology(
        dir,
        ACQUISITION_ONLY_MANIFEST,
        &[(
            "work-unit",
            r#"{"type":"object","required":["title","handle"],"properties":{"title":{"type":"string"},"handle":{"type":"object"}}}"#,
        )],
        &["decompose"],
    );
    let project_dir = dir.join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);
    common::append_github_forge_config(&project_dir, "tesserine", "runa");
    project_dir
}

/// The acquisition optionally accepts `notes`; the ticket entry must withhold it
/// when its type is untrusted.
const ACCEPTS_NOTES_MANIFEST: &str = r#"
name = "groundwork"

[[artifact_types]]
name = "notes"

[[artifact_types]]
name = "work-unit"

[[protocols]]
name = "decompose"
accepts = ["notes"]
produces = ["work-unit"]
scoped = false
trigger = { type = "on_artifact", name = "work-unit" }
"#;

fn setup_accepts_notes_entry_project(dir: &Path) -> PathBuf {
    let manifest_path = common::write_methodology(
        dir,
        ACCEPTS_NOTES_MANIFEST,
        &[
            (
                "notes",
                r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#,
            ),
            (
                "work-unit",
                r#"{"type":"object","required":["title","handle"],"properties":{"title":{"type":"string"},"handle":{"type":"object"}}}"#,
            ),
        ],
        &["decompose"],
    );
    let project_dir = dir.join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);
    common::append_github_forge_config(&project_dir, "tesserine", "runa");
    project_dir
}

#[test]
fn run_ticket_dry_run_withholds_accepts_from_partially_scanned_type() {
    let dir = tempfile::tempdir().unwrap();
    let project_dir = setup_accepts_notes_entry_project(dir.path());
    // A valid `notes` exists, but an unreadable sibling leaves the type only
    // partially scanned (untrusted).
    let notes_dir = project_dir.join(".runa/workspace/notes");
    fs::create_dir_all(&notes_dir).unwrap();
    fs::write(notes_dir.join("good.json"), r#"{"title":"good"}"#).unwrap();
    let unreadable = notes_dir.join("hidden.json");
    fs::write(&unreadable, r#"{"title":"hidden"}"#).unwrap();
    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o000)).unwrap();

    let output = clear_forge_env(&mut runa_bin())
        .arg("run")
        .arg("--ticket")
        .arg("#14")
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
    let acquisition = &value["execution_plan"][0];
    assert_eq!(acquisition["protocol"], "decompose");
    // The optional `notes` input is from an untrusted type and must be withheld.
    let inputs = acquisition["context"]["inputs"].as_array().unwrap();
    assert!(
        inputs.iter().all(|input| input["artifact_type"] != "notes"),
        "untrusted accepted input was exposed: {value:#}"
    );
}

#[test]
fn run_ticket_reports_success_when_acquisition_is_the_only_work() {
    let dir = tempfile::tempdir().unwrap();
    let project_dir = setup_acquisition_only_project(dir.path());
    let log_path = dir.path().join("executed.log");
    let agent_path = write_entry_agent(dir.path());
    append_agent_command_config(&project_dir, &[agent_path.as_path(), log_path.as_path()]);

    let output = clear_forge_env(&mut runa_bin())
        .arg("run")
        .arg("--ticket")
        .arg("#14")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    // Acquisition ran and there is no further scoped work: the command must
    // report success, not nothing-ready (exit 4).
    assert_eq!(
        output.status.code(),
        Some(0),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read_to_string(&log_path).unwrap(), "decompose\n");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Run outcome: success"), "stdout: {stdout}");
}

/// The acquisition triggers on `intent` (which it does not require); the ticket
/// substitutes that trigger.
const TRIGGER_ONLY_ENTRY_MANIFEST: &str = r#"
name = "groundwork"

[[artifact_types]]
name = "intent"

[[artifact_types]]
name = "work-unit"

[[artifact_types]]
name = "claim"

[[protocols]]
name = "decompose"
produces = ["work-unit"]
scoped = false
trigger = { type = "on_artifact", name = "intent" }

[[protocols]]
name = "take"
requires = ["work-unit"]
produces = ["claim"]
scoped = true
trigger = { type = "on_artifact", name = "work-unit" }
"#;

fn setup_trigger_only_entry_project(dir: &Path) -> PathBuf {
    let manifest_path = common::write_methodology(
        dir,
        TRIGGER_ONLY_ENTRY_MANIFEST,
        &[
            (
                "intent",
                r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#,
            ),
            (
                "work-unit",
                r#"{"type":"object","required":["title","handle"],"properties":{"title":{"type":"string"},"handle":{"type":"object"}}}"#,
            ),
            (
                "claim",
                r#"{"type":"object","required":["work_unit","scope"],"properties":{"work_unit":{"type":"string"},"scope":{"type":"string"}}}"#,
            ),
        ],
        &["decompose", "take"],
    );
    let project_dir = dir.join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);
    common::append_github_forge_config(&project_dir, "tesserine", "runa");
    project_dir
}

#[test]
fn run_ticket_admits_acquisition_when_only_trigger_type_partially_scanned() {
    let dir = tempfile::tempdir().unwrap();
    let project_dir = setup_trigger_only_entry_project(dir.path());
    // An unreadable `intent` (the substituted trigger type, not a required
    // input) leaves it only partially scanned.
    let intent_dir = project_dir.join(".runa/workspace/intent");
    fs::create_dir_all(&intent_dir).unwrap();
    let unreadable = intent_dir.join("hidden.json");
    fs::write(&unreadable, r#"{"title":"hidden"}"#).unwrap();
    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o000)).unwrap();

    let log_path = dir.path().join("executed.log");
    let agent_path = write_entry_agent(dir.path());
    append_agent_command_config(&project_dir, &[agent_path.as_path(), log_path.as_path()]);

    let output = clear_forge_env(&mut runa_bin())
        .arg("run")
        .arg("--ticket")
        .arg("#14")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    // The ticket replaces the trigger, so a partial scan of the trigger-only
    // `request` must not block: acquisition runs and the cascade completes.
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read_to_string(&log_path).unwrap(), "decompose\ntake\n");
}

#[test]
fn run_ticket_blocks_when_acquisition_preconditions_unmet() {
    let dir = tempfile::tempdir().unwrap();
    let project_dir = setup_blocked_entry_project(dir.path());
    let log_path = dir.path().join("executed.log");
    let agent_path = write_entry_agent(dir.path());
    append_agent_command_config(&project_dir, &[agent_path.as_path(), log_path.as_path()]);

    let output = clear_forge_env(&mut runa_bin())
        .arg("run")
        .arg("--ticket")
        .arg("#14")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    // Exit 3 (blocked) and the agent is never launched.
    assert_eq!(output.status.code(), Some(3), "{output:?}");
    assert!(!log_path.exists(), "acquisition agent should not run");
    assert!(
        !project_dir
            .join(".runa/workspace/work-unit/work-unit-14-cold-start.json")
            .exists()
    );
}

#[test]
fn run_ticket_dry_run_blocks_when_acquisition_preconditions_unmet() {
    let dir = tempfile::tempdir().unwrap();
    let project_dir = setup_blocked_entry_project(dir.path());

    let output = clear_forge_env(&mut runa_bin())
        .arg("run")
        .arg("--ticket")
        .arg("#14")
        .arg("--dry-run")
        .arg("--json")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(3), "{output:?}");
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["entry"]["acquisition_protocol"], "decompose");
    assert_eq!(
        value["execution_plan"].as_array().unwrap().len(),
        0,
        "{value:#}"
    );
}

#[test]
fn go_ticket_blocks_when_acquisition_preconditions_unmet() {
    let dir = tempfile::tempdir().unwrap();
    let project_dir = setup_blocked_entry_project(dir.path());
    let log_path = dir.path().join("executed.log");
    let agent_path = write_entry_agent(dir.path());
    append_agent_command_config(&project_dir, &[agent_path.as_path(), log_path.as_path()]);

    let output = clear_forge_env(&mut runa_bin())
        .arg("go")
        .arg("--ticket")
        .arg("#14")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(3), "{output:?}");
    assert!(!log_path.exists(), "acquisition agent should not run");
}

/// Leave an unreadable `work-unit/*.json` so the type is only partially scanned.
fn taint_work_unit_scan(project_dir: &Path) {
    let wu_dir = project_dir.join(".runa/workspace/work-unit");
    fs::create_dir_all(&wu_dir).unwrap();
    let unreadable = wu_dir.join("hidden.json");
    fs::write(
        &unreadable,
        r#"{"title":"hidden","handle":{"forge_tag":"github","url":"https://github.com/tesserine/runa/issues/14","number":14}}"#,
    )
    .unwrap();
    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o000)).unwrap();
}

#[test]
fn run_ticket_blocks_when_work_unit_scan_incomplete() {
    let dir = tempfile::tempdir().unwrap();
    let project_dir = setup_entry_project(dir.path());
    taint_work_unit_scan(&project_dir);
    let log_path = dir.path().join("executed.log");
    let agent_path = write_entry_agent(dir.path());
    append_agent_command_config(&project_dir, &[agent_path.as_path(), log_path.as_path()]);

    let output = clear_forge_env(&mut runa_bin())
        .arg("run")
        .arg("--ticket")
        .arg("#14")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    // A no-match result is untrustworthy under a partial work-unit scan, so the
    // ticket does not cold-start acquisition.
    assert_eq!(output.status.code(), Some(6), "{output:?}");
    assert!(
        !log_path.exists(),
        "must not cold-start under an incomplete work-unit scan"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("partially scanned"), "stderr: {stderr}");
}

#[test]
fn go_ticket_blocks_when_work_unit_scan_incomplete() {
    let dir = tempfile::tempdir().unwrap();
    let project_dir = setup_entry_project(dir.path());
    taint_work_unit_scan(&project_dir);
    let log_path = dir.path().join("executed.log");
    let agent_path = write_entry_agent(dir.path());
    append_agent_command_config(&project_dir, &[agent_path.as_path(), log_path.as_path()]);

    let output = clear_forge_env(&mut runa_bin())
        .arg("go")
        .arg("--ticket")
        .arg("#14")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(6), "{output:?}");
    assert!(
        !log_path.exists(),
        "must not cold-start under an incomplete work-unit scan"
    );
}

#[test]
fn run_ticket_rejects_foreign_deployment_reference() {
    let dir = tempfile::tempdir().unwrap();
    let project_dir = setup_entry_project(dir.path());

    let output = clear_forge_env(&mut runa_bin())
        .arg("run")
        .arg("--ticket")
        .arg("tesserine/groundwork#14")
        .arg("--dry-run")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("disagrees"), "stderr: {stderr}");
}

#[test]
fn run_ticket_conflicts_with_work_unit() {
    let output = runa_bin()
        .arg("run")
        .arg("--ticket")
        .arg("#14")
        .arg("--work-unit")
        .arg("work-unit-14")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("cannot be used with"), "stderr: {stderr}");
}

fn runa_mcp_bin_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_runa"))
        .parent()
        .unwrap()
        .join(format!("runa-mcp{}", std::env::consts::EXE_SUFFIX))
}

#[test]
fn mcp_session_ticket_entry_materializes_work_unit_and_binds_to_take() {
    let dir = tempfile::tempdir().unwrap();
    let project_dir = setup_entry_project(dir.path());
    let runa_mcp_path = runa_mcp_bin_path();
    let log_path = dir.path().join("session.out");

    let output = Command::new("sh")
        .arg("-c")
        .arg(
            r####"
set -eu
{
    printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"entry-test","version":"1.0.0"}}}'
    printf '%s\n' '{"jsonrpc":"2.0","method":"notifications/initialized"}'
    printf '%s\n' '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"next-protocol-context","arguments":{}}}'
    printf '%s\n' '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"work-unit","arguments":{"instance_id":"work-unit-14-cold-start","title":"Cold start","handle":{"forge_tag":"github","url":"https://github.com/tesserine/runa/issues/14","number":14}}}}'
    printf '%s\n' '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"advance","arguments":{}}}'
    sleep 1
} | "$1" --session --ticket '#14' > "$2"
if grep -q '"error"' "$2"; then
    cat "$2" >&2
    exit 23
fi
"####,
        )
        .arg("drive-entry")
        .arg(&runa_mcp_path)
        .arg(&log_path)
        .env_remove("RUNA_FORGE_TYPE")
        .env_remove("RUNA_FORGE_OWNER")
        .env_remove("RUNA_FORGE_NAME")
        .env_remove("RUNA_FORGE_TRACKER_ID")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "entry session failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let transcript = fs::read_to_string(&log_path).unwrap();
    // The acquisition step is served, the work-unit is materialized, and the
    // session binds — advance reports `take` as the next step.
    assert!(transcript.contains("## Session entry"), "{transcript}");
    assert!(
        transcript.contains("github:tesserine/runa#14"),
        "{transcript}"
    );
    // advance reports `take` as the bound next step.
    assert!(transcript.contains("next_step"), "{transcript}");
    assert!(transcript.contains("take"), "{transcript}");
    assert!(
        project_dir
            .join(".runa/workspace/work-unit/work-unit-14-cold-start.json")
            .is_file()
    );
}

#[test]
fn mcp_session_intent_target_materializes_work_unit_and_binds_to_take() {
    let dir = tempfile::tempdir().unwrap();
    let project_dir = setup_entry_project(dir.path());
    write_intent_target(&project_dir, "#14");
    let runa_mcp_path = runa_mcp_bin_path();
    let log_path = dir.path().join("session.out");

    let output = Command::new("sh")
        .arg("-c")
        .arg(
            r####"
set -eu
{
    printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"entry-test","version":"1.0.0"}}}'
    printf '%s\n' '{"jsonrpc":"2.0","method":"notifications/initialized"}'
    printf '%s\n' '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"next-protocol-context","arguments":{}}}'
    printf '%s\n' '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"work-unit","arguments":{"instance_id":"work-unit-14-cold-start","title":"Cold start","handle":{"forge_tag":"github","url":"https://github.com/tesserine/runa/issues/14","number":14}}}}'
    printf '%s\n' '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"advance","arguments":{}}}'
    sleep 1
} | "$1" --session > "$2"
if grep -q '"error"' "$2"; then
    cat "$2" >&2
    exit 23
fi
"####,
        )
        .arg("drive-entry")
        .arg(&runa_mcp_path)
        .arg(&log_path)
        .env_remove("RUNA_FORGE_TYPE")
        .env_remove("RUNA_FORGE_OWNER")
        .env_remove("RUNA_FORGE_NAME")
        .env_remove("RUNA_FORGE_TRACKER_ID")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "entry session failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let transcript = fs::read_to_string(&log_path).unwrap();
    assert!(transcript.contains("## Session entry"), "{transcript}");
    assert!(
        transcript.contains("github:tesserine/runa#14"),
        "{transcript}"
    );
    assert!(transcript.contains("next_step"), "{transcript}");
    assert!(transcript.contains("take"), "{transcript}");
    assert!(
        project_dir
            .join(".runa/workspace/work-unit/work-unit-14-cold-start.json")
            .is_file()
    );
}

#[test]
fn go_intent_target_materializes_work_unit_and_binds_to_take() {
    let dir = tempfile::tempdir().unwrap();
    let project_dir = setup_entry_project(dir.path());
    write_intent_target(&project_dir, "#14");
    let agent_path = dir.path().join("go-entry-agent.sh");
    let prompt_path = dir.path().join("prompt.txt");
    let config_path = dir.path().join("mcp-config.json");
    let runa_mcp_path = runa_mcp_bin_path();
    let mcp_log_path = dir.path().join("mcp.log");
    fs::write(
        &agent_path,
        r####"#!/bin/sh
set -eu
cat > "$1"
printf '%s' "$RUNA_MCP_CONFIG" > "$2"
{
    printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"go-entry-test","version":"1.0.0"}}}'
    printf '%s\n' '{"jsonrpc":"2.0","method":"notifications/initialized"}'
    printf '%s\n' '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"next-protocol-context","arguments":{}}}'
    printf '%s\n' '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"work-unit","arguments":{"instance_id":"work-unit-14-cold-start","title":"Cold start","handle":{"forge_tag":"github","url":"https://github.com/tesserine/runa/issues/14","number":14}}}}'
    printf '%s\n' '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"advance","arguments":{}}}'
    sleep 1
} | "$3" --session --ticket '#14' > "$4"
if grep -q '"error"' "$4"; then
    cat "$4" >&2
    exit 23
fi
"####,
    )
    .unwrap();
    fs::set_permissions(&agent_path, fs::Permissions::from_mode(0o755)).unwrap();
    append_agent_command_config(
        &project_dir,
        &[
            agent_path.as_path(),
            prompt_path.as_path(),
            config_path.as_path(),
            runa_mcp_path.as_path(),
            mcp_log_path.as_path(),
        ],
    );

    let output = clear_forge_env(&mut runa_bin())
        .arg("go")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}\nmcp log: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
        fs::read_to_string(&mcp_log_path).unwrap_or_else(|_| "<missing>".to_string())
    );

    let config: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(config_path).unwrap()).unwrap();
    assert_eq!(
        config["args"],
        serde_json::json!(["--session", "--ticket", "#14"])
    );
    assert!(
        project_dir
            .join(".runa/workspace/work-unit/work-unit-14-cold-start.json")
            .is_file()
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Acquired work-unit work-unit-14-cold-start"),
        "stdout: {stdout}"
    );
}

#[test]
fn go_ticket_invalid_reference_is_usage_error_without_agent() {
    // No `[agent].command` is configured. An invalid reference must still report
    // a usage error (2), not a missing-agent config failure (6).
    let dir = tempfile::tempdir().unwrap();
    let project_dir = setup_entry_project(dir.path());

    let output = clear_forge_env(&mut runa_bin())
        .arg("go")
        .arg("--ticket")
        .arg("not-a-ticket")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not a recognized forge ticket reference"),
        "stderr: {stderr}"
    );
}

#[test]
fn go_without_selector_reaches_unscoped_evaluation() {
    let dir = tempfile::tempdir().unwrap();
    let project_dir = setup_entry_project(dir.path());

    let output = clear_forge_env(&mut runa_bin())
        .arg("go")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(3), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("No READY protocols."), "stdout: {stdout}");
}

#[test]
fn go_ticket_conflicts_with_work_unit() {
    let output = runa_bin()
        .arg("go")
        .arg("--ticket")
        .arg("#14")
        .arg("--work-unit")
        .arg("work-unit-14")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2), "{output:?}");
}
