use std::collections::{BTreeSet, HashSet};
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

    let has_blocked = evaluated.cycle.is_some()
        || !evaluated.blocked.is_empty()
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
    initial_scan_result: &libagent::ScanResult,
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

    let preview_command = preview_runa_mcp_command();
    let concrete_entries: std::collections::HashMap<_, _> = build_plan_entries(
        execution_state.planned_entries.clone(),
        &preview_command,
        working_dir,
        config_path,
    )
    .into_iter()
    .map(|entry| {
        (
            candidate_key(&entry.protocol, entry.work_unit.as_deref()),
            entry,
        )
    })
    .collect();
    let initial_ready: HashSet<_> = concrete_entries.keys().cloned().collect();
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
    let inherited_scan = scan_result_with_inherited_gaps(initial_scan_result, Vec::new());

    loop {
        let state = evaluate_execution_state(&shadow, working_dir, &inherited_scan);
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
            let concrete = concrete_entries
                .get(&key)
                .expect("initially ready candidate must have a concrete plan entry");
            projected.push(RunPlanJson {
                protocol: concrete.protocol.clone(),
                work_unit: concrete.work_unit.clone(),
                trigger: concrete.trigger.clone(),
                projection,
                mcp_config: Some(concrete.mcp_config.clone()),
                context: Some(concrete.context.clone()),
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
                .record_projected_with_timestamp(
                    produced_type,
                    &instance_id,
                    &path,
                    &value,
                    timestamp_ms,
                )
                .map_err(RunError::Store)?;
            modified.push(libagent::ArtifactRef {
                artifact_type: produced_type.clone(),
                instance_id,
                path,
                work_unit: next_entry.work_unit.clone(),
            });
            timestamp_ms += 1;
        }

        let projection_scan = scan_result_with_inherited_gaps(initial_scan_result, modified);
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
    if schema.get("allOf").is_some() {
        let merged = merge_all_of_schema(schema);
        return minimal_value_for_schema(&merged, work_unit);
    }
    for key in ["oneOf", "anyOf"] {
        if let Some(branches) = schema.get(key).and_then(serde_json::Value::as_array)
            && let Some(first) = branches.first()
        {
            return minimal_value_for_schema(first, work_unit);
        }
    }

    match schema.get("type").and_then(serde_json::Value::as_str) {
        Some("object") => minimal_object_value(schema, work_unit),
        Some("array") => minimal_array_value(schema, work_unit),
        Some("string") => minimal_string_value(schema),
        Some("integer") => minimal_integer_value(schema),
        Some("number") => minimal_number_value(schema),
        Some("boolean") => serde_json::Value::Bool(false),
        Some("null") => serde_json::Value::Null,
        _ => serde_json::Value::Null,
    }
}

#[derive(Clone, Copy)]
enum LowerBound {
    Inclusive(f64),
    Exclusive(f64),
}

fn merge_all_of_schema(schema: &serde_json::Value) -> serde_json::Value {
    let mut merged = schema.as_object().cloned().unwrap_or_default();
    let branches = schema
        .get("allOf")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    merged.remove("allOf");

    let mut required: BTreeSet<String> = merged
        .get("required")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .map(str::to_owned)
        .collect();
    let mut properties = merged
        .get("properties")
        .and_then(serde_json::Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut explicit_types: Vec<String> = merged
        .get("type")
        .and_then(serde_json::Value::as_str)
        .map(|value| vec![value.to_string()])
        .unwrap_or_default();
    let mut min_length = schema_keyword_u64(schema, "minLength");
    let mut min_items = schema_keyword_u64(schema, "minItems");
    let mut min_properties = schema_keyword_u64(schema, "minProperties");
    let mut lower_bound = schema_lower_bound(schema);

    for branch in branches {
        let normalized = if branch.get("allOf").is_some() {
            merge_all_of_schema(&branch)
        } else {
            branch
        };

        if let Some(values) = normalized
            .get("required")
            .and_then(serde_json::Value::as_array)
        {
            required.extend(
                values
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(str::to_owned),
            );
        }
        if let Some(branch_properties) = normalized
            .get("properties")
            .and_then(serde_json::Value::as_object)
        {
            for (name, property_schema) in branch_properties {
                properties.insert(name.clone(), property_schema.clone());
            }
        }
        if let Some(branch_type) = normalized.get("type").and_then(serde_json::Value::as_str) {
            explicit_types.push(branch_type.to_string());
        }

        min_length = max_u64(min_length, schema_keyword_u64(&normalized, "minLength"));
        min_items = max_u64(min_items, schema_keyword_u64(&normalized, "minItems"));
        min_properties = max_u64(
            min_properties,
            schema_keyword_u64(&normalized, "minProperties"),
        );
        lower_bound = stricter_lower_bound(lower_bound, schema_lower_bound(&normalized));
    }

    if required.is_empty() {
        merged.remove("required");
    } else {
        merged.insert(
            "required".into(),
            serde_json::Value::Array(
                required
                    .into_iter()
                    .map(serde_json::Value::String)
                    .collect(),
            ),
        );
    }

    if properties.is_empty() {
        merged.remove("properties");
    } else {
        merged.insert("properties".into(), serde_json::Value::Object(properties));
    }

    match explicit_types.split_first().and_then(|(first, rest)| {
        rest.iter()
            .all(|candidate| candidate == first)
            .then_some(first)
    }) {
        Some(value) => {
            merged.insert("type".into(), serde_json::Value::String(value.clone()));
        }
        None => {
            merged.remove("type");
        }
    }

    set_optional_u64(&mut merged, "minLength", min_length);
    set_optional_u64(&mut merged, "minItems", min_items);
    set_optional_u64(&mut merged, "minProperties", min_properties);
    set_lower_bound(&mut merged, lower_bound);

    serde_json::Value::Object(merged)
}

fn schema_keyword_u64(schema: &serde_json::Value, key: &str) -> Option<u64> {
    schema.get(key).and_then(serde_json::Value::as_u64)
}

fn max_u64(left: Option<u64>, right: Option<u64>) -> Option<u64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

fn schema_lower_bound(schema: &serde_json::Value) -> Option<LowerBound> {
    stricter_lower_bound(
        schema
            .get("minimum")
            .and_then(serde_json::Value::as_f64)
            .map(LowerBound::Inclusive),
        schema
            .get("exclusiveMinimum")
            .and_then(serde_json::Value::as_f64)
            .map(LowerBound::Exclusive),
    )
}

fn stricter_lower_bound(left: Option<LowerBound>, right: Option<LowerBound>) -> Option<LowerBound> {
    match (left, right) {
        (Some(left), Some(right)) => Some(match left.value().total_cmp(&right.value()) {
            std::cmp::Ordering::Less => right,
            std::cmp::Ordering::Greater => left,
            std::cmp::Ordering::Equal => {
                if left.is_exclusive() || !right.is_exclusive() {
                    left
                } else {
                    right
                }
            }
        }),
        (Some(bound), None) | (None, Some(bound)) => Some(bound),
        (None, None) => None,
    }
}

impl LowerBound {
    fn value(self) -> f64 {
        match self {
            LowerBound::Inclusive(value) | LowerBound::Exclusive(value) => value,
        }
    }

    fn is_exclusive(self) -> bool {
        matches!(self, LowerBound::Exclusive(_))
    }
}

fn set_optional_u64(
    map: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    value: Option<u64>,
) {
    match value {
        Some(value) => {
            map.insert(key.to_string(), serde_json::Value::Number(value.into()));
        }
        None => {
            map.remove(key);
        }
    }
}

fn set_lower_bound(
    map: &mut serde_json::Map<String, serde_json::Value>,
    bound: Option<LowerBound>,
) {
    map.remove("minimum");
    map.remove("exclusiveMinimum");

    match bound {
        Some(LowerBound::Inclusive(value)) => {
            if let Some(number) = serde_json::Number::from_f64(value) {
                map.insert("minimum".into(), serde_json::Value::Number(number));
            }
        }
        Some(LowerBound::Exclusive(value)) => {
            if let Some(number) = serde_json::Number::from_f64(value) {
                map.insert("exclusiveMinimum".into(), serde_json::Value::Number(number));
            }
        }
        None => {}
    }
}

fn scan_result_with_inherited_gaps(
    scan_result: &libagent::ScanResult,
    modified: Vec<libagent::ArtifactRef>,
) -> libagent::ScanResult {
    libagent::ScanResult {
        modified,
        unreadable: scan_result.unreadable.clone(),
        partially_scanned_types: scan_result.partially_scanned_types.clone(),
        ..Default::default()
    }
}

fn minimal_object_value(schema: &serde_json::Value, work_unit: Option<&str>) -> serde_json::Value {
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
    let min_properties = schema
        .get("minProperties")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0) as usize;
    let mut property_names: Vec<_> = properties.keys().cloned().collect();
    property_names.sort();

    let mut object = serde_json::Map::new();
    for name in &property_names {
        let property_schema = properties
            .get(name)
            .expect("property names must resolve within the same object");

        if name == "work_unit" {
            if let Some(work_unit) = work_unit {
                object.insert(
                    name.clone(),
                    serde_json::Value::String(work_unit.to_string()),
                );
            } else if required.contains("work_unit") {
                object.insert(
                    name.clone(),
                    serde_json::Value::String("projected".to_string()),
                );
            }
            continue;
        }

        if required.contains(name.as_str()) {
            object.insert(
                name.clone(),
                minimal_value_for_schema(property_schema, work_unit),
            );
        }
    }

    for name in &property_names {
        if object.len() >= min_properties || object.contains_key(name) || name == "work_unit" {
            continue;
        }

        let property_schema = properties
            .get(name)
            .expect("property names must resolve within the same object");
        object.insert(
            name.clone(),
            minimal_value_for_schema(property_schema, work_unit),
        );
    }

    serde_json::Value::Object(object)
}

fn minimal_array_value(schema: &serde_json::Value, work_unit: Option<&str>) -> serde_json::Value {
    let min_items = schema
        .get("minItems")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0) as usize;
    let prefix_items = schema
        .get("prefixItems")
        .and_then(serde_json::Value::as_array);
    let items = schema.get("items");

    let mut values = Vec::new();
    while values.len() < min_items {
        let index = values.len();
        let item_schema = prefix_items
            .and_then(|prefix_items| prefix_items.get(index))
            .or(items);
        values.push(
            item_schema
                .map(|item_schema| minimal_value_for_schema(item_schema, work_unit))
                .unwrap_or(serde_json::Value::Null),
        );
    }

    serde_json::Value::Array(values)
}

fn minimal_string_value(schema: &serde_json::Value) -> serde_json::Value {
    let min_length = schema
        .get("minLength")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0) as usize;
    serde_json::Value::String("x".repeat(min_length))
}

fn minimal_integer_value(schema: &serde_json::Value) -> serde_json::Value {
    let value = schema
        .get("exclusiveMinimum")
        .and_then(serde_json::Value::as_f64)
        .map(|exclusive_minimum| exclusive_minimum.floor() as i64 + 1)
        .or_else(|| {
            schema
                .get("minimum")
                .and_then(serde_json::Value::as_f64)
                .map(|minimum| minimum.ceil() as i64)
        })
        .unwrap_or(0);
    serde_json::Value::Number(value.into())
}

fn minimal_number_value(schema: &serde_json::Value) -> serde_json::Value {
    let value = schema
        .get("exclusiveMinimum")
        .and_then(serde_json::Value::as_f64)
        .map(next_greater_f64)
        .or_else(|| schema.get("minimum").and_then(serde_json::Value::as_f64))
        .unwrap_or(0.0);
    serde_json::Number::from_f64(value)
        .map(serde_json::Value::Number)
        .unwrap_or_else(|| serde_json::json!(0.0))
}

fn next_greater_f64(value: f64) -> f64 {
    if !value.is_finite() {
        return 0.0;
    }
    if value == 0.0 {
        return f64::from_bits(1);
    }

    let bits = value.to_bits();
    if value.is_sign_positive() {
        f64::from_bits(bits + 1)
    } else {
        f64::from_bits(bits - 1)
    }
}

enum ReconcileOutcome {
    Succeeded {
        state: Box<ExecutionState>,
        scan_result: libagent::ScanResult,
        execution_entry: Box<PlanEntry>,
    },
    PostconditionFailure {
        scan_result: libagent::ScanResult,
    },
}

fn execute_and_reconcile(
    working_dir: &Path,
    loaded: &mut crate::project::LoadedProject,
    agent_command: &[String],
    config_path: &Path,
    mcp_command: &str,
    next_entry: PlannedEntry,
) -> Result<ReconcileOutcome, StepError> {
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
    if libagent::enforce_postconditions(
        protocol,
        &loaded.store,
        execution_entry.work_unit.as_deref(),
    )
    .is_err()
    {
        return Ok(ReconcileOutcome::PostconditionFailure { scan_result });
    }

    let refreshed = evaluate_execution_state(loaded, working_dir, &scan_result);
    Ok(ReconcileOutcome::Succeeded {
        state: Box::new(refreshed),
        scan_result,
        execution_entry: Box::new(execution_entry),
    })
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
        let execution_plan = build_run_json_plan(
            &loaded,
            working_dir,
            &config_path,
            &scan_result,
            &initial_state,
        )?;

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

        return Ok(classify_outcome(&initial_state.evaluated, false));
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
            Ok(ReconcileOutcome::Succeeded {
                state: refreshed,
                scan_result,
                execution_entry,
            }) => {
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
                state = *refreshed;
                println!(
                    "Executed: {}",
                    match &execution_entry.work_unit {
                        Some(work_unit) =>
                            format!("{} (work_unit={work_unit})", execution_entry.protocol),
                        None => execution_entry.protocol,
                    }
                );
            }
            Ok(ReconcileOutcome::PostconditionFailure { scan_result }) => {
                failed.insert(key);
                state = evaluate_execution_state(&loaded, working_dir, &scan_result);
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
            }
            Err(StepError::AgentCommandFailed { .. }) => {
                failed.insert(key);
            }
            Err(err) => return Err(RunError::from(err)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::minimal_value_for_schema;
    use libagent::{ArtifactType, validation::validate_artifact};
    use serde_json::json;

    #[test]
    fn minimal_value_for_schema_satisfies_string_min_length() {
        let value = minimal_value_for_schema(&json!({"type":"string","minLength":3}), None);

        assert_eq!(value, json!("xxx"));
    }

    #[test]
    fn minimal_value_for_schema_satisfies_numeric_lower_bounds() {
        let integer = minimal_value_for_schema(&json!({"type":"integer","minimum":2}), None);
        let number =
            minimal_value_for_schema(&json!({"type":"number","exclusiveMinimum":1.5}), None);

        assert_eq!(integer, json!(2));
        assert!(number.as_f64().unwrap() > 1.5, "{number}");
    }

    #[test]
    fn minimal_value_for_schema_satisfies_min_items_with_constrained_items() {
        let value = minimal_value_for_schema(
            &json!({
                "type":"array",
                "minItems":2,
                "items":{"type":"string","minLength":1}
            }),
            None,
        );

        assert_eq!(value, json!(["x", "x"]));
    }

    #[test]
    fn minimal_value_for_schema_satisfies_min_properties() {
        let value = minimal_value_for_schema(
            &json!({
                "type":"object",
                "required":["title"],
                "minProperties":3,
                "properties":{
                    "title":{"type":"string","minLength":1},
                    "priority":{"type":"integer","minimum":1},
                    "tags":{"type":"array","minItems":1,"items":{"type":"string","minLength":1}}
                },
                "additionalProperties":false
            }),
            None,
        );

        assert_eq!(
            value,
            json!({
                "priority": 1,
                "tags": ["x"],
                "title": "x"
            })
        );
    }

    #[test]
    fn minimal_value_for_schema_merges_all_of_branches_before_synthesis() {
        let schema = json!({
            "type":"object",
            "allOf":[
                {
                    "required":["title"],
                    "properties":{
                        "title":{"type":"string","minLength":1}
                    }
                },
                {
                    "required":["priority"],
                    "properties":{
                        "priority":{"type":"integer","minimum":1}
                    }
                },
                {
                    "required":["tags"],
                    "properties":{
                        "tags":{"type":"array","minItems":2,"items":{"type":"string","minLength":1}}
                    }
                }
            ]
        });
        let value = minimal_value_for_schema(&schema, None);

        validate_artifact(
            &value,
            &ArtifactType {
                name: "constrained".into(),
                schema,
            },
        )
        .unwrap();
    }
}
