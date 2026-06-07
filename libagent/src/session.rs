use std::collections::HashSet;
use std::fmt;
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
            | SessionError::CurrentStepMissing(_) => None,
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

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct CurrentStep {
    pub protocol: String,
    pub work_unit: Option<String>,
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
    exhausted: HashSet<CurrentStep>,
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
        let scan_findings = crate::collect_scan_findings(&scan_result, &self.loaded.workspace_dir);
        let evaluated = self.evaluate(&scan_findings);
        Ok(self.readiness_from(scan_findings, evaluated))
    }

    pub fn next_context(&mut self) -> Result<(ContextInjection, String), SessionError> {
        let scan_result = crate::scan(&self.loaded.workspace_dir, &mut self.loaded.store)?;
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
        Ok((context, rendered))
    }

    pub fn advance_with_validator<F>(
        &mut self,
        validate_step: F,
    ) -> Result<AdvanceOutcome<'_>, SessionError>
    where
        F: FnOnce(Option<&crate::ProtocolDeclaration>, &crate::ArtifactStore) -> Result<(), String>,
    {
        let completed_step = self
            .current_step
            .clone()
            .ok_or(SessionError::NoCurrentStep)?;
        let scan_result = crate::scan(&self.loaded.workspace_dir, &mut self.loaded.store)?;
        let scan_findings = crate::collect_scan_findings(&scan_result, &self.loaded.workspace_dir);

        let protocol = self.protocol(&completed_step.protocol)?;
        crate::enforce_postconditions(
            protocol,
            &self.loaded.store,
            completed_step.work_unit.as_deref(),
        )
        .map_err(SessionError::Postcondition)?;
        let execution_record = crate::protocol_execution_record(
            protocol,
            &self.loaded.store,
            completed_step.work_unit.as_deref(),
            &scan_findings.affected_types,
        );
        self.loaded.store.record_execution(
            &completed_step.protocol,
            completed_step.work_unit.as_deref(),
            execution_record,
        )?;

        let mut evaluated = self.evaluate(&scan_findings);
        self.exhausted.insert(completed_step.clone());
        let next_step = self.select_next(&evaluated);
        let next_protocol = match &next_step {
            Some(step) => Some(self.protocol(&step.protocol)?),
            None => None,
        };
        validate_step(next_protocol, &self.loaded.store)
            .map_err(|message| SessionError::Record(crate::StoreError::Serialization(message)))?;

        self.current_step = next_step;
        let readiness = self.readiness_from(scan_findings, {
            evaluated = self.evaluate_without_exhaustion_refresh(evaluated);
            evaluated
        });

        Ok(AdvanceOutcome {
            version: 1,
            completed_step,
            next_step: self.current_step.clone(),
            readiness,
        })
    }

    fn refresh_current_from_scan(&mut self, _selector: StepSelector) -> Result<(), SessionError> {
        let scan_result = crate::scan(&self.loaded.workspace_dir, &mut self.loaded.store)?;
        let scan_findings = crate::collect_scan_findings(&scan_result, &self.loaded.workspace_dir);
        let evaluated = self.evaluate(&scan_findings);
        self.current_step = self.select_next(&evaluated);
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

    fn evaluate_without_exhaustion_refresh(
        &self,
        evaluated: crate::EvaluatedProtocols,
    ) -> crate::EvaluatedProtocols {
        evaluated
    }

    fn select_next(&self, evaluated: &crate::EvaluatedProtocols) -> Option<CurrentStep> {
        evaluated
            .ready
            .iter()
            .map(|entry| CurrentStep {
                protocol: entry.name.clone(),
                work_unit: entry.work_unit.clone(),
            })
            .find(|step| !self.exhausted.contains(step))
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
