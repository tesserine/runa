use std::collections::HashSet;
use std::fmt;
use std::path::Path;

use libagent::context::ContextInjectionView;
use serde::{Serialize, Serializer};

use super::CommandError;
use crate::commands::protocol_eval;
use crate::commands::step::{
    ExecutionState, McpServerConfig, PlanEntry, PlannedEntry, StepError, build_plan_entries,
    evaluate_execution_state, execute_entry, locate_runa_mcp, preview_runa_mcp_command,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOutcome {
    AllComplete,
    QuiescentFailures,
    QuiescentBlocked,
}

impl RunOutcome {
    pub fn exit_code(self) -> i32 {
        match self {
            RunOutcome::AllComplete => 0,
            RunOutcome::QuiescentFailures => 2,
            RunOutcome::QuiescentBlocked => 3,
        }
    }

    fn label(self) -> &'static str {
        match self {
            RunOutcome::AllComplete => "all_complete",
            RunOutcome::QuiescentFailures => "quiescent_with_failures",
            RunOutcome::QuiescentBlocked => "quiescent_with_blocked_work",
        }
    }
}

#[derive(Debug)]
pub enum RunError {
    Step(StepError),
    Store(libagent::StoreError),
    Graph(libagent::GraphError),
    Json(serde_json::Error),
    TempDir(std::io::Error),
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RunError::Step(err) => write!(f, "{err}"),
            RunError::Store(err) => write!(f, "{err}"),
            RunError::Graph(err) => write!(f, "{err}"),
            RunError::Json(err) => write!(f, "{err}"),
            RunError::TempDir(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for RunError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RunError::Step(err) => Some(err),
            RunError::Store(err) => Some(err),
            RunError::Graph(err) => Some(err),
            RunError::Json(err) => Some(err),
            RunError::TempDir(err) => Some(err),
        }
    }
}

impl From<StepError> for RunError {
    fn from(err: StepError) -> Self {
        RunError::Step(err)
    }
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum ProjectionKind {
    Current,
    Projected,
}

#[derive(Serialize)]
struct RunJson<'a> {
    version: u32,
    methodology: &'a str,
    scan_warnings: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cycle: Option<Vec<String>>,
    execution_plan: Vec<RunPlanJson>,
    protocols: Vec<protocol_eval::ProtocolJson>,
}

#[derive(Serialize)]
struct RunPlanJson {
    protocol: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    work_unit: Option<String>,
    trigger: String,
    projection: ProjectionKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    mcp_config: Option<McpServerConfig>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_optional_context"
    )]
    context: Option<libagent::context::ContextInjection>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CandidateKey {
    protocol: String,
    work_unit: Option<String>,
}

fn serialize_optional_context<S>(
    context: &Option<libagent::context::ContextInjection>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    match context {
        Some(context) => ContextInjectionView::from(context).serialize(serializer),
        None => serializer.serialize_none(),
    }
}

fn candidate_key(protocol: &str, work_unit: Option<&str>) -> CandidateKey {
    CandidateKey {
        protocol: protocol.to_string(),
        work_unit: work_unit.map(str::to_owned),
    }
}

fn resolve_agent_command(
    working_dir: &Path,
    config_override: Option<&Path>,
) -> Result<Vec<String>, RunError> {
    let config = crate::project::read_config(working_dir, config_override)
        .map_err(CommandError::from)
        .map_err(StepError::from)
        .map_err(RunError::from)?;
    config
        .agent
        .command
        .filter(|command| {
            !command.is_empty() && !command.first().is_some_and(|part| part.is_empty())
        })
        .ok_or(RunError::from(StepError::AgentCommandNotConfigured))
}

fn classify_outcome(
    evaluated: &protocol_eval::EvaluatedProtocols,
    had_failures: bool,
) -> RunOutcome {
    if had_failures {
        return RunOutcome::QuiescentFailures;
    }

    let has_blocked = !evaluated.blocked.is_empty()
        || evaluated.waiting.iter().any(|entry| {
            entry
                .unsatisfied_conditions
                .iter()
                .any(|c| c != "outputs are current")
        });

    if has_blocked {
        RunOutcome::QuiescentBlocked
    } else {
        RunOutcome::AllComplete
    }
}

fn build_run_json_plan(
    loaded: &crate::project::LoadedProject,
    working_dir: &Path,
    config_path: &Path,
    execution_state: &ExecutionState,
) -> Result<Vec<RunPlanJson>, RunError> {
    let temp = tempfile::tempdir().map_err(RunError::TempDir)?;
    let mut shadow = crate::project::LoadedProject {
        manifest: loaded.manifest.clone(),
        graph: libagent::DependencyGraph::build(&loaded.manifest.protocols)
            .map_err(RunError::Graph)?,
        store: loaded
            .store
            .fork(temp.path().join("store"))
            .map_err(RunError::Store)?,
        workspace_dir: loaded.workspace_dir.clone(),
    };

    let initial_ready: HashSet<_> = execution_state
        .planned_entries
        .iter()
        .map(|entry| candidate_key(&entry.protocol, entry.work_unit.as_deref()))
        .collect();
    let protocol_map: std::collections::HashMap<&str, &libagent::ProtocolDeclaration> = shadow
        .manifest
        .protocols
        .iter()
        .map(|protocol| (protocol.name.as_str(), protocol))
        .collect();
    let mut exhausted = HashSet::new();
    let mut projected = Vec::new();
    let mut timestamp_ms = shadow
        .manifest
        .artifact_types
        .iter()
        .filter_map(|artifact| shadow.store.latest_modification_ms(&artifact.name, None))
        .max()
        .unwrap_or(0)
        + 1;
    let preview_command = preview_runa_mcp_command();
    let empty_scan = libagent::ScanResult::default();

    loop {
        let state = evaluate_execution_state(&shadow, working_dir, &empty_scan);
        let Some(next_entry) = state.planned_entries.into_iter().find(|entry| {
            !exhausted.contains(&candidate_key(&entry.protocol, entry.work_unit.as_deref()))
        }) else {
            break;
        };

        let key = candidate_key(&next_entry.protocol, next_entry.work_unit.as_deref());
        let projection = if initial_ready.contains(&key) {
            ProjectionKind::Current
        } else {
            ProjectionKind::Projected
        };

        if matches!(projection, ProjectionKind::Current) {
            let concrete = build_plan_entries(
                vec![next_entry.clone()],
                &preview_command,
                working_dir,
                config_path,
            )
            .into_iter()
            .next()
            .expect("single planned entry must produce one plan entry");
            projected.push(RunPlanJson {
                protocol: concrete.protocol,
                work_unit: concrete.work_unit,
                trigger: concrete.trigger,
                projection,
                mcp_config: Some(concrete.mcp_config),
                context: Some(concrete.context),
            });
        } else {
            projected.push(RunPlanJson {
                protocol: next_entry.protocol.clone(),
                work_unit: next_entry.work_unit.clone(),
                trigger: next_entry.trigger.clone(),
                projection,
                mcp_config: None,
                context: None,
            });
        }

        exhausted.insert(key);

        let protocol = protocol_map
            .get(next_entry.protocol.as_str())
            .expect("planned protocol must exist in manifest");
        let mut modified = Vec::new();
        for produced_type in &protocol.produces {
            let artifact_type = shadow
                .manifest
                .artifact_types
                .iter()
                .find(|artifact| artifact.name == *produced_type)
                .expect("produced artifact type must exist in manifest");
            let value = minimal_artifact_value(artifact_type, next_entry.work_unit.as_deref());
            let instance_id = format!(
                "projected-{}-{}-{}",
                protocol.name,
                produced_type,
                projected.len()
            );
            let path = temp
                .path()
                .join("workspace")
                .join(produced_type)
                .join(format!("{instance_id}.json"));
            shadow
                .store
                .record_with_timestamp(produced_type, &instance_id, &path, &value, timestamp_ms)
                .map_err(RunError::Store)?;
            modified.push(libagent::ArtifactRef {
                artifact_type: produced_type.clone(),
                instance_id,
                path,
                work_unit: next_entry.work_unit.clone(),
            });
            timestamp_ms += 1;
        }

        let projection_scan = libagent::ScanResult {
            modified,
            ..Default::default()
        };
        exhausted.retain(|candidate| {
            let protocol = protocol_map
                .get(candidate.protocol.as_str())
                .expect("planned protocol must exist in manifest");
            !libagent::protocol_relevant_inputs_changed(
                protocol,
                candidate.work_unit.as_deref(),
                &projection_scan,
            )
        });
    }

    Ok(projected)
}

fn minimal_artifact_value(
    artifact_type: &libagent::ArtifactType,
    work_unit: Option<&str>,
) -> serde_json::Value {
    minimal_value_for_schema(&artifact_type.schema, work_unit)
}

fn minimal_value_for_schema(
    schema: &serde_json::Value,
    work_unit: Option<&str>,
) -> serde_json::Value {
    if let Some(constant) = schema.get("const") {
        return constant.clone();
    }
    if let Some(values) = schema.get("enum").and_then(serde_json::Value::as_array)
        && let Some(first) = values.first()
    {
        return first.clone();
    }
    for key in ["oneOf", "anyOf", "allOf"] {
        if let Some(branches) = schema.get(key).and_then(serde_json::Value::as_array)
            && let Some(first) = branches.first()
        {
            return minimal_value_for_schema(first, work_unit);
        }
    }

    match schema.get("type").and_then(serde_json::Value::as_str) {
        Some("object") => {
            let required: HashSet<&str> = schema
                .get("required")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(serde_json::Value::as_str)
                .collect();
            let properties = schema
                .get("properties")
                .and_then(serde_json::Value::as_object)
                .cloned()
                .unwrap_or_default();

            let mut object = serde_json::Map::new();
            for (name, property_schema) in properties {
                if name == "work_unit" {
                    if let Some(work_unit) = work_unit {
                        object.insert(name, serde_json::Value::String(work_unit.to_string()));
                    } else if required.contains("work_unit") {
                        object.insert(name, serde_json::Value::String("projected".to_string()));
                    }
                    continue;
                }

                if required.contains(name.as_str()) {
                    object.insert(name, minimal_value_for_schema(&property_schema, work_unit));
                }
            }

            serde_json::Value::Object(object)
        }
        Some("array") => {
            let item = schema
                .get("items")
                .map(|items| minimal_value_for_schema(items, work_unit));
            let min_items = schema
                .get("minItems")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            let mut values = Vec::new();
            for _ in 0..min_items {
                values.push(item.clone().unwrap_or(serde_json::Value::Null));
            }
            serde_json::Value::Array(values)
        }
        Some("string") => serde_json::Value::String(String::new()),
        Some("integer") => serde_json::Value::Number(0.into()),
        Some("number") => serde_json::json!(0.0),
        Some("boolean") => serde_json::Value::Bool(false),
        Some("null") => serde_json::Value::Null,
        _ => serde_json::Value::Null,
    }
}

fn execute_and_reconcile(
    working_dir: &Path,
    loaded: &mut crate::project::LoadedProject,
    agent_command: &[String],
    config_path: &Path,
    mcp_command: &str,
    next_entry: PlannedEntry,
) -> Result<(ExecutionState, libagent::ScanResult, PlanEntry), StepError> {
    let execution_entry =
        build_plan_entries(vec![next_entry], mcp_command, working_dir, config_path)
            .into_iter()
            .next()
            .expect("single planned entry must produce one execution entry");

    execute_entry(working_dir, agent_command, &execution_entry)?;

    let scan_result =
        libagent::scan(&loaded.workspace_dir, &mut loaded.store).map_err(|source| {
            StepError::PostExecutionScan {
                protocol: execution_entry.protocol.clone(),
                work_unit: execution_entry.work_unit.clone(),
                source,
            }
        })?;

    let protocol = loaded
        .manifest
        .protocols
        .iter()
        .find(|protocol| protocol.name == execution_entry.protocol)
        .expect("planned protocol must exist in manifest");
    libagent::enforce_postconditions(
        protocol,
        &loaded.store,
        execution_entry.work_unit.as_deref(),
    )
    .map_err(|source| StepError::PostExecutionEnforcement {
        protocol: execution_entry.protocol.clone(),
        work_unit: execution_entry.work_unit.clone(),
        source,
    })?;

    let refreshed = evaluate_execution_state(loaded, working_dir, &scan_result);
    Ok((refreshed, scan_result, execution_entry))
}

pub fn run(
    working_dir: &Path,
    config_override: Option<&Path>,
    dry_run: bool,
    json_output: bool,
) -> Result<RunOutcome, RunError> {
    if !dry_run && json_output {
        return Err(RunError::from(StepError::JsonRequiresDryRun));
    }

    let (mut loaded, scan_result) = super::load_and_scan(working_dir, config_override)
        .map_err(StepError::from)
        .map_err(RunError::from)?;
    let initial_state = evaluate_execution_state(&loaded, working_dir, &scan_result);
    let config_path = crate::project::resolve_config(working_dir, config_override)
        .map_err(CommandError::from)
        .map_err(StepError::from)
        .map_err(RunError::from)?;

    if dry_run {
        let execution_plan =
            build_run_json_plan(&loaded, working_dir, &config_path, &initial_state)?;

        if json_output {
            let payload = RunJson {
                version: 1,
                methodology: &loaded.manifest.name,
                scan_warnings: initial_state.scan_findings.warnings.clone(),
                cycle: initial_state
                    .evaluated
                    .cycle
                    .as_ref()
                    .map(|cycle| cycle.path.clone()),
                execution_plan,
                protocols: initial_state.evaluated.json_protocols(),
            };
            println!(
                "{}",
                serde_json::to_string_pretty(&payload).map_err(RunError::Json)?
            );
        } else {
            println!("Methodology: {}", loaded.manifest.name);
            println!();
            if execution_plan.is_empty() {
                println!("Execution plan: none");
            } else {
                println!("Execution plan:");
                for (index, entry) in execution_plan.iter().enumerate() {
                    match &entry.work_unit {
                        Some(work_unit) => println!(
                            "  {}. {} (work_unit={work_unit}) [{}]",
                            index + 1,
                            entry.protocol,
                            match entry.projection {
                                ProjectionKind::Current => "current",
                                ProjectionKind::Projected => "projected",
                            }
                        ),
                        None => println!(
                            "  {}. {} [{}]",
                            index + 1,
                            entry.protocol,
                            match entry.projection {
                                ProjectionKind::Current => "current",
                                ProjectionKind::Projected => "projected",
                            }
                        ),
                    }
                }
            }
            println!();
            protocol_eval::print_group("READY", &initial_state.evaluated.ready);
            println!();
            protocol_eval::print_group("BLOCKED", &initial_state.evaluated.blocked);
            println!();
            protocol_eval::print_group("WAITING", &initial_state.evaluated.waiting);
        }

        return Ok(RunOutcome::AllComplete);
    }

    let mut state = initial_state;
    if state.planned_entries.is_empty() {
        let outcome = classify_outcome(&state.evaluated, false);
        println!("Run outcome: {}", outcome.label());
        return Ok(outcome);
    }

    let agent_command = resolve_agent_command(working_dir, config_override)?;
    let mcp_command = locate_runa_mcp()
        .map_err(RunError::from)?
        .to_string_lossy()
        .into_owned();
    let mut exhausted = HashSet::new();
    let mut failed = HashSet::new();

    loop {
        let Some(next_entry) = state.planned_entries.clone().into_iter().find(|entry| {
            let key = candidate_key(&entry.protocol, entry.work_unit.as_deref());
            !exhausted.contains(&key) && !failed.contains(&key)
        }) else {
            let outcome = classify_outcome(&state.evaluated, !failed.is_empty());
            println!("Run outcome: {}", outcome.label());
            return Ok(outcome);
        };

        let key = candidate_key(&next_entry.protocol, next_entry.work_unit.as_deref());
        match execute_and_reconcile(
            working_dir,
            &mut loaded,
            &agent_command,
            &config_path,
            &mcp_command,
            next_entry,
        ) {
            Ok((refreshed, scan_result, execution_entry)) => {
                exhausted.insert(key);
                exhausted.retain(|candidate| {
                    let protocol = loaded
                        .manifest
                        .protocols
                        .iter()
                        .find(|protocol| protocol.name == candidate.protocol)
                        .expect("planned protocol must exist in manifest");
                    !libagent::protocol_relevant_inputs_changed(
                        protocol,
                        candidate.work_unit.as_deref(),
                        &scan_result,
                    )
                });
                state = refreshed;
                println!(
                    "Executed: {}",
                    match &execution_entry.work_unit {
                        Some(work_unit) =>
                            format!("{} (work_unit={work_unit})", execution_entry.protocol),
                        None => execution_entry.protocol,
                    }
                );
            }
            Err(StepError::AgentCommandFailed { .. }) => {
                failed.insert(key);
            }
            Err(StepError::PostExecutionEnforcement { .. }) => {
                failed.insert(key);
                state = evaluate_execution_state(
                    &loaded,
                    working_dir,
                    &libagent::ScanResult::default(),
                );
            }
            Err(err) => return Err(RunError::from(err)),
        }
    }
}
