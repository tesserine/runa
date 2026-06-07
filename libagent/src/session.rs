use std::collections::HashSet;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::context::{ContextInjection, render_context_prompt};

#[derive(Debug)]
pub enum SessionError {
    Project(crate::ProjectError),
    Scan(crate::ScanError),
    WorkUnitScope(crate::ScopedWorkUnitError),
    MissingWorkUnit,
    NoCurrentStep,
    CurrentStepMissing(String),
    CurrentStepNotReady(String),
    Postcondition(crate::EnforcementError),
    Record(crate::StoreError),
}

impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SessionError::Project(err) => write!(f, "{err}"),
            SessionError::Scan(err) => write!(f, "{err}"),
            SessionError::WorkUnitScope(err) => write!(f, "{err}"),
            SessionError::MissingWorkUnit => write!(f, "--session requires --work-unit"),
            SessionError::NoCurrentStep => write!(f, "session has no current ready step"),
            SessionError::CurrentStepMissing(protocol) => {
                write!(
                    f,
                    "current session protocol '{protocol}' is no longer in the manifest"
                )
            }
            SessionError::CurrentStepNotReady(protocol) => {
                write!(
                    f,
                    "current session protocol '{protocol}' is no longer ready"
                )
            }
            SessionError::Postcondition(err) => write!(f, "{err}"),
            SessionError::Record(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for SessionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SessionError::Project(err) => Some(err),
            SessionError::Scan(err) => Some(err),
            SessionError::WorkUnitScope(err) => Some(err),
            SessionError::Postcondition(err) => Some(err),
            SessionError::Record(err) => Some(err),
            SessionError::MissingWorkUnit
            | SessionError::NoCurrentStep
            | SessionError::CurrentStepMissing(_)
            | SessionError::CurrentStepNotReady(_) => None,
        }
    }
}

impl From<crate::ProjectError> for SessionError {
    fn from(err: crate::ProjectError) -> Self {
        SessionError::Project(err)
    }
}

impl From<crate::ScanError> for SessionError {
    fn from(err: crate::ScanError) -> Self {
        SessionError::Scan(err)
    }
}

impl From<crate::ScopedWorkUnitError> for SessionError {
    fn from(err: crate::ScopedWorkUnitError) -> Self {
        SessionError::WorkUnitScope(err)
    }
}

impl From<crate::StoreError> for SessionError {
    fn from(err: crate::StoreError) -> Self {
        SessionError::Record(err)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CurrentStep {
    pub protocol: String,
    pub work_unit: Option<String>,
    #[serde(skip)]
    pub provenance_snapshot: Option<crate::ExecutionRecord>,
}

impl PartialEq for CurrentStep {
    fn eq(&self, other: &Self) -> bool {
        self.protocol == other.protocol && self.work_unit == other.work_unit
    }
}

impl Eq for CurrentStep {}

impl Hash for CurrentStep {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.protocol.hash(state);
        self.work_unit.hash(state);
    }
}

#[derive(Debug, Clone, Copy)]
pub enum StepSelector {
    FirstReady,
}

#[derive(Serialize)]
pub struct SessionReadiness<'a> {
    pub version: u32,
    pub methodology: &'a str,
    pub scan_warnings: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_step: Option<CurrentStep>,
    pub protocols: Vec<crate::ProtocolJson>,
}

#[derive(Serialize)]
pub struct AdvanceOutcome<'a> {
    pub version: u32,
    pub completed_step: CurrentStep,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_step: Option<CurrentStep>,
    pub readiness: SessionReadiness<'a>,
}

pub struct SessionState {
    working_dir: PathBuf,
    pub loaded: crate::LoadedProject,
    work_unit: String,
    current_step: Option<CurrentStep>,
    exhausted: HashSet<crate::CandidateKey>,
}

impl From<&CurrentStep> for crate::CandidateKey {
    fn from(step: &CurrentStep) -> Self {
        Self::new(&step.protocol, step.work_unit.as_deref())
    }
}

impl SessionState {
    pub fn open(
        working_dir: PathBuf,
        config_override: Option<&Path>,
        work_unit: Option<String>,
    ) -> Result<Self, SessionError> {
        let work_unit = work_unit.ok_or(SessionError::MissingWorkUnit)?;
        let mut loaded = crate::project::load(&working_dir, config_override)?;
        crate::scan(&loaded.workspace_dir, &mut loaded.store)?;
        crate::validate_scoped_work_unit(&loaded.store, &work_unit)?;
        let mut session = Self {
            working_dir,
            loaded,
            work_unit,
            current_step: None,
            exhausted: HashSet::new(),
        };
        session.refresh_current_from_scan(StepSelector::FirstReady)?;
        Ok(session)
    }

    pub fn workspace_dir(&self) -> &Path {
        &self.loaded.workspace_dir
    }

    pub fn store(&self) -> &crate::ArtifactStore {
        &self.loaded.store
    }

    pub fn store_mut(&mut self) -> &mut crate::ArtifactStore {
        &mut self.loaded.store
    }

    pub fn current_step(&self) -> Option<&CurrentStep> {
        self.current_step.as_ref()
    }

    pub fn current_protocol(&self) -> Result<&crate::ProtocolDeclaration, SessionError> {
        let current = self
            .current_step
            .as_ref()
            .ok_or(SessionError::NoCurrentStep)?;
        self.protocol(&current.protocol)
    }

    pub fn readiness(&mut self) -> Result<SessionReadiness<'_>, SessionError> {
        let scan_result = crate::scan(&self.loaded.workspace_dir, &mut self.loaded.store)?;
        self.refresh_exhaustion_after_scan(&scan_result);
        let scan_findings = crate::collect_scan_findings(&scan_result, &self.loaded.workspace_dir);
        let evaluated = self.evaluate(&scan_findings);
        if self.current_step.is_none() {
            self.current_step = self.select_next(&evaluated, &scan_findings, &self.exhausted)?;
        }
        Ok(self.readiness_from(scan_findings, evaluated))
    }

    pub fn next_context(&mut self) -> Result<(ContextInjection, String), SessionError> {
        let scan_result = crate::scan(&self.loaded.workspace_dir, &mut self.loaded.store)?;
        self.refresh_exhaustion_after_scan(&scan_result);
        let scan_findings = crate::collect_scan_findings(&scan_result, &self.loaded.workspace_dir);
        let protocol = self.current_protocol()?;
        let mut context =
            crate::context::build_context(protocol, &self.loaded.store, Some(&self.work_unit));
        context.inputs.retain(|input| {
            input.relationship == crate::context::ArtifactRelationship::Requires
                || !scan_findings
                    .affected_types
                    .contains(input.artifact_type.as_str())
        });
        let rendered = render_context_prompt(&context);
        self.refresh_current_provenance_snapshot(&scan_findings)?;
        Ok((context, rendered))
    }

    pub fn advance_with_validator<F>(
        &mut self,
        validate_step: F,
    ) -> Result<AdvanceOutcome<'_>, SessionError>
    where
        F: FnOnce(Option<&crate::ProtocolDeclaration>, &crate::ArtifactStore) -> Result<(), String>,
    {
        self.advance_with_selector_and_validator(
            |session, evaluated, scan_findings, exhausted| {
                session.select_next(evaluated, scan_findings, exhausted)
            },
            validate_step,
        )
    }

    fn advance_with_selector_and_validator<S, F>(
        &mut self,
        select_next: S,
        validate_step: F,
    ) -> Result<AdvanceOutcome<'_>, SessionError>
    where
        S: FnOnce(
            &Self,
            &crate::EvaluatedProtocols,
            &crate::ScanFindings,
            &HashSet<crate::CandidateKey>,
        ) -> Result<Option<CurrentStep>, SessionError>,
        F: FnOnce(Option<&crate::ProtocolDeclaration>, &crate::ArtifactStore) -> Result<(), String>,
    {
        let completed_step = self
            .current_step
            .clone()
            .ok_or(SessionError::NoCurrentStep)?;
        let scan_result = crate::scan(&self.loaded.workspace_dir, &mut self.loaded.store)?;
        let scan_findings = crate::collect_scan_findings(&scan_result, &self.loaded.workspace_dir);
        let current_evaluated = self.evaluate(&scan_findings);
        self.ensure_current_step_can_complete(&completed_step, &current_evaluated)?;

        let protocol = self.protocol(&completed_step.protocol)?;
        crate::enforce_postconditions(
            protocol,
            &self.loaded.store,
            completed_step.work_unit.as_deref(),
        )
        .map_err(SessionError::Postcondition)?;
        let execution_record = completed_step
            .provenance_snapshot
            .clone()
            .unwrap_or_else(|| {
                crate::protocol_execution_record(
                    protocol,
                    &self.loaded.store,
                    completed_step.work_unit.as_deref(),
                    &scan_findings.affected_types,
                )
            });
        let current_inputs_match_provenance =
            self.current_inputs_match_provenance(&completed_step, &execution_record);

        let previous_record = self.loaded.store.stage_execution_record(
            &completed_step.protocol,
            completed_step.work_unit.as_deref(),
            execution_record,
        );

        let mut staged_exhausted = self.exhausted.clone();
        if current_inputs_match_provenance {
            staged_exhausted.insert(crate::CandidateKey::from(&completed_step));
        } else {
            staged_exhausted.remove(&crate::CandidateKey::from(&completed_step));
        }
        crate::refresh_exhausted_candidates_after_scan(
            &self.loaded.manifest.protocols,
            &mut staged_exhausted,
            &scan_result,
        );

        let evaluated = self.evaluate(&scan_findings);
        let next_step = match select_next(self, &evaluated, &scan_findings, &staged_exhausted) {
            Ok(next_step) => next_step,
            Err(error) => {
                self.loaded.store.restore_execution_record(
                    &completed_step.protocol,
                    completed_step.work_unit.as_deref(),
                    previous_record,
                );
                return Err(error);
            }
        };
        let next_protocol = match &next_step {
            Some(step) => match self.protocol(&step.protocol) {
                Ok(protocol) => Some(protocol),
                Err(error) => {
                    self.loaded.store.restore_execution_record(
                        &completed_step.protocol,
                        completed_step.work_unit.as_deref(),
                        previous_record,
                    );
                    return Err(error);
                }
            },
            None => None,
        };
        if let Err(message) = validate_step(next_protocol, &self.loaded.store) {
            self.loaded.store.restore_execution_record(
                &completed_step.protocol,
                completed_step.work_unit.as_deref(),
                previous_record,
            );
            return Err(SessionError::Record(crate::StoreError::Serialization(
                message,
            )));
        }
        if let Err(error) = self.loaded.store.persist_staged_execution_records() {
            self.loaded.store.restore_execution_record(
                &completed_step.protocol,
                completed_step.work_unit.as_deref(),
                previous_record,
            );
            return Err(SessionError::Record(error));
        }

        self.current_step = next_step;
        self.exhausted = staged_exhausted;
        let readiness = self.readiness_from(scan_findings, evaluated);

        Ok(AdvanceOutcome {
            version: 1,
            completed_step,
            next_step: self.current_step.clone(),
            readiness,
        })
    }

    fn refresh_current_from_scan(&mut self, _selector: StepSelector) -> Result<(), SessionError> {
        let scan_result = crate::scan(&self.loaded.workspace_dir, &mut self.loaded.store)?;
        self.refresh_exhaustion_after_scan(&scan_result);
        let scan_findings = crate::collect_scan_findings(&scan_result, &self.loaded.workspace_dir);
        let evaluated = self.evaluate(&scan_findings);
        self.current_step = self.select_next(&evaluated, &scan_findings, &self.exhausted)?;
        Ok(())
    }

    fn evaluate(&self, scan_findings: &crate::ScanFindings) -> crate::EvaluatedProtocols {
        crate::evaluate_protocols(
            &self.loaded,
            &self.working_dir,
            scan_findings,
            crate::EvaluationScope::Scoped(&self.work_unit),
        )
    }

    fn select_next(
        &self,
        evaluated: &crate::EvaluatedProtocols,
        scan_findings: &crate::ScanFindings,
        exhausted: &HashSet<crate::CandidateKey>,
    ) -> Result<Option<CurrentStep>, SessionError> {
        for entry in &evaluated.ready {
            let mut step = CurrentStep {
                protocol: entry.name.clone(),
                work_unit: entry.work_unit.clone(),
                provenance_snapshot: None,
            };
            if exhausted.contains(&crate::CandidateKey::from(&step)) {
                continue;
            }
            let protocol = self.protocol(&step.protocol)?;
            step.provenance_snapshot = Some(crate::protocol_execution_record(
                protocol,
                &self.loaded.store,
                step.work_unit.as_deref(),
                &scan_findings.affected_types,
            ));
            return Ok(Some(step));
        }

        Ok(None)
    }

    fn ensure_current_step_can_complete(
        &self,
        step: &CurrentStep,
        evaluated: &crate::EvaluatedProtocols,
    ) -> Result<(), SessionError> {
        let is_current_step = |entry: &crate::ProtocolEntry| {
            entry.name == step.protocol && entry.work_unit == step.work_unit
        };
        if evaluated.ready.iter().any(&is_current_step) {
            return Ok(());
        }
        if evaluated.waiting.iter().any(|entry| {
            is_current_step(entry)
                && matches!(
                    entry.waiting_reason,
                    Some(crate::WaitingReason::OutputsCurrent)
                )
        }) {
            return Ok(());
        }
        Err(SessionError::CurrentStepNotReady(step.protocol.clone()))
    }

    fn current_inputs_match_provenance(
        &self,
        step: &CurrentStep,
        provenance: &crate::ExecutionRecord,
    ) -> bool {
        let current_inputs = crate::selection::execution_input_snapshot_for_freshness_inputs(
            &self.loaded.store,
            provenance.input_modes.iter(),
            step.work_unit.as_deref(),
        );
        provenance.inputs == current_inputs
    }

    fn refresh_exhaustion_after_scan(&mut self, scan_result: &crate::ScanResult) {
        crate::refresh_exhausted_candidates_after_scan(
            &self.loaded.manifest.protocols,
            &mut self.exhausted,
            scan_result,
        );
    }

    fn refresh_current_provenance_snapshot(
        &mut self,
        scan_findings: &crate::ScanFindings,
    ) -> Result<(), SessionError> {
        let Some(current) = self.current_step.clone() else {
            return Ok(());
        };
        let protocol = self.protocol(&current.protocol)?;
        let provenance_snapshot = crate::protocol_execution_record(
            protocol,
            &self.loaded.store,
            current.work_unit.as_deref(),
            &scan_findings.affected_types,
        );
        if let Some(current_step) = &mut self.current_step {
            current_step.provenance_snapshot = Some(provenance_snapshot);
        }
        Ok(())
    }

    fn readiness_from(
        &self,
        scan_findings: crate::ScanFindings,
        evaluated: crate::EvaluatedProtocols,
    ) -> SessionReadiness<'_> {
        SessionReadiness {
            version: 1,
            methodology: &self.loaded.manifest.name,
            scan_warnings: scan_findings.warnings,
            current_step: self.current_step.clone(),
            protocols: evaluated.json_protocols(),
        }
    }

    fn protocol(&self, name: &str) -> Result<&crate::ProtocolDeclaration, SessionError> {
        self.loaded
            .manifest
            .protocols
            .iter()
            .find(|protocol| protocol.name == name)
            .ok_or_else(|| SessionError::CurrentStepMissing(name.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ExecutionInput, ExecutionInputMode, ExecutionInputSnapshot, ExecutionRecord};
    use std::collections::{BTreeMap, HashSet};
    use std::fs;

    fn write_session_project(dir: &Path) -> PathBuf {
        let manifest_path = crate::test_helpers::write_methodology(
            dir,
            r#"
name = "groundwork"

[[artifact_types]]
name = "work-unit"

[[artifact_types]]
name = "claim"

[[protocols]]
name = "take"
requires = ["work-unit"]
produces = ["claim"]
scoped = true
trigger = { type = "on_artifact", name = "work-unit" }
"#,
            &[
                (
                    "work-unit",
                    r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#,
                ),
                (
                    "claim",
                    r#"{"type":"object","required":["work_unit","scope"],"properties":{"work_unit":{"type":"string"},"scope":{"type":"string"}}}"#,
                ),
            ],
            &["take"],
        );
        let project_dir = dir.join("project");
        fs::create_dir(&project_dir).unwrap();
        let runa_dir = project_dir.join(".runa");
        fs::create_dir_all(&runa_dir).unwrap();
        let manifest_path = fs::canonicalize(manifest_path).unwrap();
        fs::write(
            runa_dir.join("config.toml"),
            format!(
                "methodology_path = {:?}\n",
                manifest_path.display().to_string()
            ),
        )
        .unwrap();
        fs::write(
            runa_dir.join("state.toml"),
            "initialized_at = \"2026-03-25T00:00:00Z\"\nruna_version = \"0.1.0\"\n",
        )
        .unwrap();
        let workspace = project_dir.join(".runa/workspace");
        fs::create_dir_all(workspace.join("work-unit")).unwrap();
        fs::write(
            workspace.join("work-unit/work-unit-166.json"),
            r#"{"title":"Scope"}"#,
        )
        .unwrap();
        project_dir
    }

    #[test]
    fn exhausted_candidate_key_identity_ignores_provenance_snapshot() {
        let step_with_snapshot = CurrentStep {
            protocol: "take".to_string(),
            work_unit: Some("work-unit-166".to_string()),
            provenance_snapshot: Some(ExecutionRecord {
                input_modes: BTreeMap::from([(
                    "work-unit".to_string(),
                    ExecutionInputMode::ValidOnly,
                )]),
                inputs: ExecutionInputSnapshot {
                    artifact_types: BTreeMap::from([(
                        "work-unit".to_string(),
                        vec![ExecutionInput {
                            instance_id: "work-unit-166".to_string(),
                            content_hash: "sha256:context-time".to_string(),
                        }],
                    )]),
                },
            }),
        };

        let mut exhausted = HashSet::new();
        exhausted.insert(crate::CandidateKey::new("take", Some("work-unit-166")));

        assert!(exhausted.contains(&crate::CandidateKey::from(&step_with_snapshot)));
    }

    #[test]
    fn advance_selection_error_restores_staged_execution_state() {
        let dir = tempfile::tempdir().unwrap();
        let project_dir = write_session_project(dir.path());
        let mut session =
            SessionState::open(project_dir.clone(), None, Some("work-unit-166".to_string()))
                .unwrap();
        let workspace = project_dir.join(".runa/workspace");
        fs::create_dir_all(workspace.join("claim")).unwrap();
        fs::write(
            workspace.join("claim/claim-1.json"),
            r#"{"work_unit":"work-unit-166","scope":"claim this work"}"#,
        )
        .unwrap();

        let advance = session.advance_with_selector_and_validator(
            |_session, _evaluated, _scan_findings, _exhausted| {
                Err(SessionError::CurrentStepMissing("synthetic".to_string()))
            },
            |_next_protocol, _store| Ok(()),
        );

        assert!(advance.is_err(), "advance unexpectedly succeeded");
        assert_eq!(
            session.current_step().map(|step| step.protocol.as_str()),
            Some("take")
        );
        assert!(
            !session
                .exhausted
                .contains(&crate::CandidateKey::new("take", Some("work-unit-166")))
        );
        assert!(
            session
                .store()
                .execution_record("take", Some("work-unit-166"))
                .is_none()
        );
        let execution_record_path = project_dir.join(".runa/store/execution-records.json");
        if execution_record_path.is_file() {
            let execution_records = fs::read_to_string(execution_record_path).unwrap();
            assert!(
                !execution_records.contains(r#""protocol": "take""#),
                "{execution_records}"
            );
        }
    }
}
