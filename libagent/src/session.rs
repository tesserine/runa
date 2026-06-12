//! Scoped session state machine: one work unit, one current step, and
//! `advance` as a single transactional operation.
//!
//! Governing contract: [`docs/session-surface-contract.md`] — this module
//! is the inner cascade behind the one operator verb, *advance the session
//! by one step* (commons ADR-0015: mode is a property of the session).
//! Drivers — `runa go`, the `runa-mcp` session mode — are clients of this
//! state machine; no driver reimplements readiness, context delivery, or
//! transition authority.
//!
//! Invariants:
//!
//! - **State derives from artifacts, never from driver assertion.** Every
//!   lifecycle operation (`open*`, `readiness`, `next_context`, `advance`)
//!   reconciles against a real workspace scan before acting; a driver
//!   cannot move the session by claiming a state.
//! - **One current step.** The session holds at most one
//!   `(protocol, work_unit)` pair; an empty session lets readiness select
//!   the first ready step.
//! - **`advance` is one operation:** rescan, enforce the current step's
//!   postconditions, stage its execution record (the freshness snapshot
//!   `selection.rs` uses for input-set currentness), validate the next
//!   selected step, then persist and move. A postcondition failure leaves
//!   the session on the current step with no transition.
//! - **Exhaustion is session-scoped.** Failed candidates are skipped for
//!   the lifetime of this `SessionState` only; nothing about failure is
//!   persisted as artifact state.
//! - **Promised scope admits exactly one step.** A session opened from a
//!   ticket reference (`SessionScope::Promised`) can only run the
//!   methodology's acquisition surface until the work-unit materializes
//!   and the scope binds.
//!
//! [`docs/session-surface-contract.md`]: https://github.com/tesserine/runa/blob/main/docs/session-surface-contract.md

use std::collections::HashSet;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::context::{ContextInjection, render_context_prompt};

pub const SESSION_ADVANCE_RECEIPT_ENV: &str = "RUNA_SESSION_ADVANCE_RECEIPT";

#[derive(Debug)]
pub enum SessionError {
    Project(crate::ProjectError),
    Scan(crate::ScanError),
    WorkUnitScope(crate::ScopedWorkUnitError),
    Entry(crate::EntryError),
    MissingWorkUnit,
    NoCurrentStep,
    CurrentStepMissing(String),
    CurrentStepNotReady(String),
    CurrentStepUnservable(String),
    Precondition(crate::EnforcementError),
    ScanIncomplete {
        protocol: String,
        types: Vec<String>,
    },
    Postcondition(crate::EnforcementError),
    Record(crate::StoreError),
}

impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SessionError::Project(err) => write!(f, "{err}"),
            SessionError::Scan(err) => write!(f, "{err}"),
            SessionError::WorkUnitScope(err) => write!(f, "{err}"),
            SessionError::Entry(err) => write!(f, "{err}"),
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
            SessionError::CurrentStepUnservable(message) => {
                write!(f, "current session step cannot be served: {message}")
            }
            SessionError::Precondition(err) => write!(f, "{err}"),
            SessionError::ScanIncomplete { protocol, types } => write!(
                f,
                "acquisition protocol '{protocol}' cannot be served: input artifact type(s) {} were only partially scanned",
                types.join(", ")
            ),
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
            SessionError::Entry(err) => Some(err),
            SessionError::Precondition(err) => Some(err),
            SessionError::Postcondition(err) => Some(err),
            SessionError::Record(err) => Some(err),
            SessionError::MissingWorkUnit
            | SessionError::NoCurrentStep
            | SessionError::CurrentStepMissing(_)
            | SessionError::CurrentStepNotReady(_)
            | SessionError::CurrentStepUnservable(_)
            | SessionError::ScanIncomplete { .. } => None,
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

impl From<crate::EntryError> for SessionError {
    fn from(err: crate::EntryError) -> Self {
        SessionError::Entry(err)
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
pub struct SessionReadiness {
    pub version: u32,
    pub methodology: String,
    pub scan_warnings: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_step: Option<CurrentStep>,
    pub protocols: Vec<crate::ProtocolJson>,
}

#[derive(Serialize)]
pub struct AdvanceOutcome {
    pub version: u32,
    pub completed_step: CurrentStep,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_step: Option<CurrentStep>,
    pub readiness: SessionReadiness,
}

pub struct SessionTransition<T> {
    pub payload: T,
    pub current_step_changed: bool,
}

/// A session's scope: a recorded work-unit (`Bound`) or a forge ticket
/// reference standing for a work-unit not yet materialized (`Promised`).
///
/// A promised scope admits exactly one step — the methodology's acquisition
/// surface, evaluated unscoped — and resolves to a `Bound` scope when that step
/// materializes the work-unit. Downstream of binding the two are
/// indistinguishable.
#[derive(Clone)]
enum SessionScope {
    Bound(String),
    Promised { ticket: crate::TicketRef },
}

pub struct SessionState {
    working_dir: PathBuf,
    pub loaded: crate::LoadedProject,
    scope: SessionScope,
    current_step: Option<CurrentStep>,
    exhausted: HashSet<crate::CandidateKey>,
}

struct ReconciledScan {
    scan_result: crate::ScanResult,
    scan_findings: crate::ScanFindings,
    evaluated: crate::EvaluatedProtocols,
}

impl From<&CurrentStep> for crate::CandidateKey {
    fn from(step: &CurrentStep) -> Self {
        Self::new(&step.protocol, step.work_unit.as_deref())
    }
}

/// The set of artifact type names that were only partially scanned (some
/// entries unreadable), used to gate readiness on scan trust.
fn partially_scanned_set(scan_result: &crate::ScanResult) -> HashSet<String> {
    scan_result
        .partially_scanned_types
        .iter()
        .map(|partial| partial.artifact_type.clone())
        .collect()
}

/// Map a canonical acquisition-admission block onto the session error surface.
fn session_error_from_block(block: crate::AcquisitionBlock, protocol: String) -> SessionError {
    match block {
        crate::AcquisitionBlock::Precondition(error) => SessionError::Precondition(error),
        crate::AcquisitionBlock::ScanIncomplete(types) => {
            SessionError::ScanIncomplete { protocol, types }
        }
    }
}

impl SessionState {
    pub fn open(
        working_dir: PathBuf,
        config_override: Option<&Path>,
        work_unit: Option<String>,
    ) -> Result<Self, SessionError> {
        Self::open_with_validator(
            working_dir,
            config_override,
            work_unit,
            |_next_protocol, _store| Ok(()),
        )
    }

    pub fn open_with_validator<F>(
        working_dir: PathBuf,
        config_override: Option<&Path>,
        work_unit: Option<String>,
        validate_step: F,
    ) -> Result<Self, SessionError>
    where
        F: FnOnce(Option<&crate::ProtocolDeclaration>, &crate::ArtifactStore) -> Result<(), String>,
    {
        let work_unit = work_unit.ok_or(SessionError::MissingWorkUnit)?;
        let loaded = crate::project::load(&working_dir, config_override)?;
        let mut session = Self {
            working_dir,
            loaded,
            scope: SessionScope::Bound(work_unit),
            current_step: None,
            exhausted: HashSet::new(),
        };
        let reconciled = session.reconcile_after_scan(false)?;
        let next_step = session.select_next(
            &reconciled.evaluated,
            &reconciled.scan_findings,
            &session.exhausted,
        )?;
        session.current_step = session.validate_selected_step(next_step, validate_step)?;
        Ok(session)
    }

    /// Open a session from a forge ticket reference.
    ///
    /// When the referenced work-unit already exists, the session opens bound to
    /// it — indistinguishable from [`open`](Self::open). Otherwise the session
    /// opens in a promised scope pinned to the methodology's acquisition
    /// surface; the reference is that step's activation, and the agent's
    /// acquisition materializes the work-unit, which [`advance`] then binds.
    pub fn open_entry<F>(
        working_dir: PathBuf,
        config_override: Option<&Path>,
        ticket: crate::TicketRef,
        validate_step: F,
    ) -> Result<Self, SessionError>
    where
        F: FnOnce(Option<&crate::ProtocolDeclaration>, &crate::ArtifactStore) -> Result<(), String>,
    {
        let mut loaded = crate::project::load(&working_dir, config_override)?;
        let scan_result = crate::scan(&loaded.workspace_dir, &mut loaded.store)?;
        let scan_findings = crate::collect_scan_findings(&scan_result, &loaded.workspace_dir);
        let identity = crate::resolve_forge_identity(&loaded.config.forge);

        // Resolve the promise first. Re-entry (the work-unit already exists)
        // degrades to an ordinary bound session and needs no acquisition surface;
        // only a cold start requires one.
        if let Some(work_unit) = crate::resolve_promise(&loaded.store, &identity, &ticket)? {
            crate::validate_scoped_work_unit_with_identity(&loaded.store, &work_unit, &identity)?;
            let mut session = Self {
                working_dir,
                loaded,
                scope: SessionScope::Bound(work_unit),
                current_step: None,
                exhausted: HashSet::new(),
            };
            session.refresh_exhaustion_after_scan(&scan_result);
            let evaluated = session.evaluate(&scan_findings);
            let next_step = session.select_next(&evaluated, &scan_findings, &session.exhausted)?;
            session.current_step = session.validate_selected_step(next_step, validate_step)?;
            return Ok(session);
        }

        // Cold start: the acquisition surface is required.
        crate::validate_tracker_consistency(&loaded.store, &identity)?;
        let acquisition_name = crate::discover_acquisition_surface(&loaded.manifest.protocols)?
            .name
            .clone();
        let mut session = Self {
            working_dir,
            loaded,
            scope: SessionScope::Promised { ticket },
            current_step: None,
            exhausted: HashSet::new(),
        };
        session.refresh_exhaustion_after_scan(&scan_result);

        // The reference substitutes only the acquisition's trigger; the canonical
        // admission gate (preconditions + scan trust) still applies.
        let acquisition = session.protocol(&acquisition_name)?;
        if let Err(block) = crate::check_acquisition_admissible(
            acquisition,
            &session.loaded.store,
            &partially_scanned_set(&scan_result),
        ) {
            return Err(session_error_from_block(block, acquisition_name));
        }
        // The ticket substitutes the trigger, so record the full trigger
        // freshness baseline; otherwise a later normal activation of this
        // acquisition could be falsely treated as current.
        let provenance_snapshot =
            crate::protocol_entry_execution_record(acquisition, &session.loaded.store, None);
        let step = CurrentStep {
            protocol: acquisition_name,
            work_unit: None,
            provenance_snapshot: Some(provenance_snapshot),
        };
        session.current_step = session.validate_selected_step(Some(step), validate_step)?;
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

    pub fn readiness<F>(
        &mut self,
        validate_step: F,
    ) -> Result<SessionTransition<SessionReadiness>, SessionError>
    where
        F: FnOnce(Option<&crate::ProtocolDeclaration>, &crate::ArtifactStore) -> Result<(), String>,
    {
        let before_step = self.current_step.clone();
        let reconciled = self.reconcile_after_scan(false)?;
        if self.current_step.is_none() {
            let next_step = self.select_next(
                &reconciled.evaluated,
                &reconciled.scan_findings,
                &self.exhausted,
            )?;
            self.current_step = self.validate_selected_step(next_step, validate_step)?;
        }
        let current_step_changed = before_step != self.current_step;
        let payload = self.readiness_from(reconciled.scan_findings, reconciled.evaluated);
        Ok(SessionTransition {
            payload,
            current_step_changed,
        })
    }

    pub fn next_context(&mut self) -> Result<(ContextInjection, String), SessionError> {
        let reconciled = self.reconcile_after_scan(true)?;
        let current_step = self
            .current_step
            .clone()
            .ok_or(SessionError::NoCurrentStep)?;
        let protocol = self.protocol(&current_step.protocol)?;
        let mut context = crate::context::build_execution_context(
            protocol,
            &self.loaded.store,
            self.context_work_unit(),
            &reconciled.scan_findings.affected_types,
        );
        if let SessionScope::Promised { ticket, .. } = &self.scope {
            context.entry = Some(crate::context::EntryDelivery {
                reference: ticket.display.clone(),
                ticket_number: ticket.number,
                tracker_identity: ticket.tracker_identity.clone(),
            });
        }
        let rendered = render_context_prompt(&context);
        self.refresh_current_provenance_snapshot(&reconciled.scan_findings)?;
        Ok((context, rendered))
    }

    pub fn advance_with_validator<F>(
        &mut self,
        validate_step: F,
    ) -> Result<SessionTransition<AdvanceOutcome>, SessionError>
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
    ) -> Result<SessionTransition<AdvanceOutcome>, SessionError>
    where
        S: FnOnce(
            &Self,
            &crate::EvaluatedProtocols,
            &crate::ScanFindings,
            &HashSet<crate::CandidateKey>,
        ) -> Result<Option<CurrentStep>, SessionError>,
        F: FnOnce(Option<&crate::ProtocolDeclaration>, &crate::ArtifactStore) -> Result<(), String>,
    {
        let before_step = self.current_step.clone();
        let reconciled = self.reconcile_after_scan(true)?;
        let completed_step = self
            .current_step
            .clone()
            .ok_or(SessionError::NoCurrentStep)?;

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
                    &reconciled.scan_findings.affected_types,
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
            &reconciled.scan_result,
        );

        // A promised scope binds here: acquisition has produced the work-unit,
        // and the session becomes an ordinary bound scoped session for the next
        // step. Binding must precede selection (select_next needs the bound
        // scope to find the next scoped step), so capture the prior scope and
        // restore it — along with the staged execution record — on any failure,
        // leaving a retried tick still representing the promised entry.
        let previous_scope = self.scope.clone();
        if matches!(self.scope, SessionScope::Promised { .. })
            && let Err(error) = self.bind_promise()
        {
            // bind_promise mutates scope only on success, so no scope restore.
            self.loaded.store.restore_execution_record(
                &completed_step.protocol,
                completed_step.work_unit.as_deref(),
                previous_record,
            );
            return Err(error);
        }

        let evaluated = self.evaluate(&reconciled.scan_findings);
        let next_step = match select_next(
            self,
            &evaluated,
            &reconciled.scan_findings,
            &staged_exhausted,
        ) {
            Ok(next_step) => next_step,
            Err(error) => {
                self.loaded.store.restore_execution_record(
                    &completed_step.protocol,
                    completed_step.work_unit.as_deref(),
                    previous_record,
                );
                self.scope = previous_scope;
                return Err(error);
            }
        };
        let next_step = match self.validate_selected_step(next_step, validate_step) {
            Ok(next_step) => next_step,
            Err(error) => {
                self.loaded.store.restore_execution_record(
                    &completed_step.protocol,
                    completed_step.work_unit.as_deref(),
                    previous_record,
                );
                self.scope = previous_scope;
                return Err(error);
            }
        };
        if let Err(error) = self.loaded.store.persist_staged_execution_records() {
            self.loaded.store.restore_execution_record(
                &completed_step.protocol,
                completed_step.work_unit.as_deref(),
                previous_record,
            );
            self.scope = previous_scope;
            return Err(SessionError::Record(error));
        }

        self.current_step = next_step;
        self.exhausted = staged_exhausted;
        let readiness = self.readiness_from(reconciled.scan_findings, evaluated);
        let current_step_changed = before_step != self.current_step;

        let payload = AdvanceOutcome {
            version: 1,
            completed_step,
            next_step: self.current_step.clone(),
            readiness,
        };
        Ok(SessionTransition {
            payload,
            current_step_changed,
        })
    }

    fn reconcile_after_scan(
        &mut self,
        require_current_ready: bool,
    ) -> Result<ReconciledScan, SessionError> {
        let scan_result = self.scan_workspace()?;
        let identity = crate::resolve_forge_identity(&self.loaded.config.forge);
        match &self.scope {
            SessionScope::Bound(work_unit) => {
                crate::validate_scoped_work_unit_with_identity(
                    &self.loaded.store,
                    work_unit,
                    &identity,
                )?;
            }
            SessionScope::Promised { .. } => {
                crate::validate_tracker_consistency(&self.loaded.store, &identity)?;
            }
        }
        self.refresh_exhaustion_after_scan(&scan_result);
        let scan_findings = crate::collect_scan_findings(&scan_result, &self.loaded.workspace_dir);
        let evaluated = self.evaluate(&scan_findings);
        if require_current_ready {
            let current_step = self
                .current_step
                .clone()
                .ok_or(SessionError::NoCurrentStep)?;
            // The operator's reference stands in for the acquisition step's
            // trigger, but its preconditions and scan-trust gates still apply. A
            // bound step uses the normal readiness check; a promised step uses
            // the canonical acquisition-admission gate.
            if matches!(self.scope, SessionScope::Bound(_)) {
                self.ensure_current_step_can_complete(&current_step, &evaluated)?;
            } else {
                let protocol = self.protocol(&current_step.protocol)?;
                if let Err(block) = crate::check_acquisition_admissible(
                    protocol,
                    &self.loaded.store,
                    &partially_scanned_set(&scan_result),
                ) {
                    return Err(session_error_from_block(
                        block,
                        current_step.protocol.clone(),
                    ));
                }
            }
        }
        Ok(ReconciledScan {
            scan_result,
            scan_findings,
            evaluated,
        })
    }

    fn scan_workspace(&mut self) -> Result<crate::ScanResult, SessionError> {
        Ok(crate::scan(
            &self.loaded.workspace_dir,
            &mut self.loaded.store,
        )?)
    }

    fn evaluate(&self, scan_findings: &crate::ScanFindings) -> crate::EvaluatedProtocols {
        crate::evaluate_protocols(
            &self.loaded,
            &self.working_dir,
            scan_findings,
            self.evaluation_scope(),
        )
    }

    fn evaluation_scope(&self) -> crate::EvaluationScope<'_> {
        match &self.scope {
            SessionScope::Bound(work_unit) => crate::EvaluationScope::Scoped(work_unit),
            SessionScope::Promised { .. } => crate::EvaluationScope::Unscoped,
        }
    }

    fn context_work_unit(&self) -> Option<&str> {
        match &self.scope {
            SessionScope::Bound(work_unit) => Some(work_unit),
            SessionScope::Promised { .. } => None,
        }
    }

    /// Resolve a promised scope to the materialized work-unit and bind it.
    ///
    /// No-op when already bound. Errors when acquisition produced no matching
    /// work-unit ([`EntryError::Unresolved`]) or the recorded work-unit fails
    /// scoped-identity validation.
    fn bind_promise(&mut self) -> Result<(), SessionError> {
        let identity = crate::resolve_forge_identity(&self.loaded.config.forge);
        let ticket = match &self.scope {
            SessionScope::Promised { ticket, .. } => ticket.clone(),
            SessionScope::Bound(_) => return Ok(()),
        };
        match crate::resolve_promise(&self.loaded.store, &identity, &ticket)? {
            Some(work_unit) => {
                crate::validate_scoped_work_unit_with_identity(
                    &self.loaded.store,
                    &work_unit,
                    &identity,
                )?;
                self.scope = SessionScope::Bound(work_unit);
                Ok(())
            }
            None => Err(SessionError::Entry(crate::EntryError::Unresolved {
                reference: ticket.display,
            })),
        }
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

    fn validate_selected_step<F>(
        &self,
        step: Option<CurrentStep>,
        validate_step: F,
    ) -> Result<Option<CurrentStep>, SessionError>
    where
        F: FnOnce(Option<&crate::ProtocolDeclaration>, &crate::ArtifactStore) -> Result<(), String>,
    {
        let protocol = match &step {
            Some(step) => Some(self.protocol(&step.protocol)?),
            None => None,
        };
        validate_step(protocol, &self.loaded.store).map_err(SessionError::CurrentStepUnservable)?;
        Ok(step)
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
        // A promised acquisition's trigger is substituted by the ticket, so it
        // records the full trigger freshness baseline rather than the
        // satisfied-only set, keeping later normal activations honest.
        let provenance_snapshot = if matches!(self.scope, SessionScope::Promised { .. }) {
            crate::protocol_entry_execution_record(protocol, &self.loaded.store, None)
        } else {
            crate::protocol_execution_record(
                protocol,
                &self.loaded.store,
                current.work_unit.as_deref(),
                &scan_findings.affected_types,
            )
        };
        if let Some(current_step) = &mut self.current_step {
            current_step.provenance_snapshot = Some(provenance_snapshot);
        }
        Ok(())
    }

    fn readiness_from(
        &self,
        scan_findings: crate::ScanFindings,
        evaluated: crate::EvaluatedProtocols,
    ) -> SessionReadiness {
        SessionReadiness {
            version: 1,
            methodology: self.loaded.manifest.name.clone(),
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

    fn write_session_project_with_work_unit(dir: &Path, create_work_unit: bool) -> PathBuf {
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
        if create_work_unit {
            fs::write(
                workspace.join("work-unit/work-unit-166.json"),
                r#"{"title":"Scope"}"#,
            )
            .unwrap();
        }
        project_dir
    }

    fn write_session_project(dir: &Path) -> PathBuf {
        write_session_project_with_work_unit(dir, true)
    }

    /// A methodology with an unscoped acquisition surface (`decompose` produces
    /// `work-unit`) and a scoped `take`, plus a GitHub forge deployment. When
    /// `materialize` is set, a matching `work-unit` for ticket 14 is present.
    fn write_entry_project(dir: &Path, materialize: bool) -> PathBuf {
        write_entry_project_with_acquisition_requires(dir, materialize, false)
    }

    /// `acquisition_requires_request` declares an unmet `requires` on the
    /// acquisition surface so its preconditions block at cold-start entry.
    fn write_entry_project_with_acquisition_requires(
        dir: &Path,
        materialize: bool,
        acquisition_requires_request: bool,
    ) -> PathBuf {
        let decompose_requires = if acquisition_requires_request {
            "requires = [\"request\"]\n"
        } else {
            ""
        };
        let manifest = format!(
            r#"
name = "groundwork"

[[artifact_types]]
name = "work-unit"

[[artifact_types]]
name = "request"

[[artifact_types]]
name = "claim"

[[protocols]]
name = "decompose"
{decompose_requires}produces = ["work-unit"]
scoped = false
trigger = {{ type = "on_artifact", name = "request" }}

[[protocols]]
name = "take"
requires = ["work-unit"]
produces = ["claim"]
scoped = true
trigger = {{ type = "on_artifact", name = "work-unit" }}
"#
        );
        let manifest_path = crate::test_helpers::write_methodology(
            dir,
            &manifest,
            &[
                (
                    "work-unit",
                    r#"{"type":"object","required":["title","handle"],"properties":{"title":{"type":"string"},"handle":{"type":"object"}}}"#,
                ),
                (
                    "request",
                    r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#,
                ),
                (
                    "claim",
                    r#"{"type":"object","required":["work_unit","scope"],"properties":{"work_unit":{"type":"string"},"scope":{"type":"string"}}}"#,
                ),
            ],
            &["decompose", "take"],
        );
        scaffold_entry_project(dir, &manifest_path, materialize)
    }

    /// A methodology whose only protocol is the scoped `take` — there is no
    /// unscoped `work-unit` producer, so a cold start has no acquisition surface.
    fn write_take_only_entry_project(dir: &Path, materialize: bool) -> PathBuf {
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
                    r#"{"type":"object","required":["title","handle"],"properties":{"title":{"type":"string"},"handle":{"type":"object"}}}"#,
                ),
                (
                    "claim",
                    r#"{"type":"object","required":["work_unit","scope"],"properties":{"work_unit":{"type":"string"},"scope":{"type":"string"}}}"#,
                ),
            ],
            &["take"],
        );
        scaffold_entry_project(dir, &manifest_path, materialize)
    }

    /// Write `.runa/{config,state}.toml` and the workspace for an entry project,
    /// optionally materializing the work-unit for ticket 14.
    fn scaffold_entry_project(dir: &Path, manifest_path: &Path, materialize: bool) -> PathBuf {
        let project_dir = dir.join("project");
        fs::create_dir(&project_dir).unwrap();
        let runa_dir = project_dir.join(".runa");
        fs::create_dir_all(&runa_dir).unwrap();
        let manifest_path = fs::canonicalize(manifest_path).unwrap();
        fs::write(
            runa_dir.join("config.toml"),
            format!(
                "methodology_path = {:?}\n\n[forge]\ntype = \"github\"\nowner = \"tesserine\"\nname = \"runa\"\n",
                manifest_path.display().to_string()
            ),
        )
        .unwrap();
        fs::write(
            runa_dir.join("state.toml"),
            "initialized_at = \"2026-06-11T00:00:00Z\"\nruna_version = \"0.1.0\"\n",
        )
        .unwrap();
        let workspace = project_dir.join(".runa/workspace");
        fs::create_dir_all(workspace.join("work-unit")).unwrap();
        if materialize {
            write_acquired_work_unit(&workspace);
        }
        project_dir
    }

    fn write_acquired_work_unit(workspace: &Path) {
        fs::create_dir_all(workspace.join("work-unit")).unwrap();
        fs::write(
            workspace.join("work-unit/work-unit-14-cold-start.json"),
            r#"{"title":"Cold start","handle":{"forge_tag":"github","url":"https://github.com/tesserine/runa/issues/14","number":14}}"#,
        )
        .unwrap();
    }

    fn ticket_14() -> crate::TicketRef {
        crate::TicketRef {
            number: 14,
            tracker_identity: "github:tesserine/runa:14".to_string(),
            display: "github:tesserine/runa#14".to_string(),
        }
    }

    #[test]
    fn open_entry_cold_pins_acquisition_step() {
        let dir = tempfile::tempdir().unwrap();
        let project_dir = write_entry_project(dir.path(), false);

        let session =
            SessionState::open_entry(project_dir, None, ticket_14(), |_, _| Ok(())).unwrap();

        let step = session.current_step().expect("acquisition step pinned");
        assert_eq!(step.protocol, "decompose");
        assert_eq!(step.work_unit, None);
        assert!(matches!(session.scope, SessionScope::Promised { .. }));
    }

    #[test]
    fn open_entry_re_entry_binds_without_acquisition_surface() {
        let dir = tempfile::tempdir().unwrap();
        // No unscoped work-unit producer exists, but the work-unit is already
        // recorded: re-entry must degrade to a bound session rather than fail on
        // acquisition-surface discovery.
        let project_dir = write_take_only_entry_project(dir.path(), true);

        let session =
            SessionState::open_entry(project_dir, None, ticket_14(), |_, _| Ok(())).unwrap();

        let step = session.current_step().expect("bound session selects take");
        assert_eq!(step.protocol, "take");
        assert!(matches!(session.scope, SessionScope::Bound(_)));
    }

    #[test]
    fn open_entry_cold_without_acquisition_surface_is_error() {
        let dir = tempfile::tempdir().unwrap();
        // No work-unit recorded and no acquisition surface: a cold start has no
        // way to acquire, so discovery fails.
        let project_dir = write_take_only_entry_project(dir.path(), false);

        let result = SessionState::open_entry(project_dir, None, ticket_14(), |_, _| Ok(()));

        assert!(matches!(
            result,
            Err(SessionError::Entry(
                crate::EntryError::NoAcquisitionSurface { .. }
            ))
        ));
    }

    #[test]
    fn open_entry_blocks_when_acquisition_preconditions_unmet() {
        let dir = tempfile::tempdir().unwrap();
        // decompose requires an absent `request`; entry substitutes only the
        // trigger, so the unmet precondition must still block.
        let project_dir = write_entry_project_with_acquisition_requires(dir.path(), false, true);

        let result = SessionState::open_entry(project_dir, None, ticket_14(), |_, _| Ok(()));

        assert!(matches!(result, Err(SessionError::Precondition(_))));
    }

    #[test]
    fn open_entry_binds_immediately_when_work_unit_exists() {
        let dir = tempfile::tempdir().unwrap();
        let project_dir = write_entry_project(dir.path(), true);

        let session =
            SessionState::open_entry(project_dir, None, ticket_14(), |_, _| Ok(())).unwrap();

        let step = session.current_step().expect("bound session selects take");
        assert_eq!(step.protocol, "take");
        assert_eq!(step.work_unit.as_deref(), Some("work-unit-14-cold-start"));
        assert!(matches!(session.scope, SessionScope::Bound(_)));
    }

    #[test]
    fn advance_from_acquisition_binds_and_selects_take() {
        let dir = tempfile::tempdir().unwrap();
        let project_dir = write_entry_project(dir.path(), false);
        let mut session =
            SessionState::open_entry(project_dir.clone(), None, ticket_14(), |_, _| Ok(()))
                .unwrap();

        // The acquisition agent materializes the work-unit.
        write_acquired_work_unit(&project_dir.join(".runa/workspace"));

        let advance = session
            .advance_with_validator(|_, _| Ok(()))
            .expect("advance binds the promised scope");

        assert_eq!(advance.payload.completed_step.protocol, "decompose");
        assert_eq!(
            advance
                .payload
                .next_step
                .as_ref()
                .map(|s| s.protocol.as_str()),
            Some("take")
        );
        assert!(matches!(session.scope, SessionScope::Bound(_)));
    }

    #[test]
    fn advance_restores_promised_scope_when_post_bind_commit_fails() {
        let dir = tempfile::tempdir().unwrap();
        let project_dir = write_entry_project(dir.path(), false);
        let mut session =
            SessionState::open_entry(project_dir.clone(), None, ticket_14(), |_, _| Ok(()))
                .unwrap();

        // Acquisition materializes the work-unit, so bind_promise succeeds; the
        // selector then fails, exercising a post-bind rollback path.
        write_acquired_work_unit(&project_dir.join(".runa/workspace"));

        let advance = session.advance_with_selector_and_validator(
            |_session, _evaluated, _scan_findings, _exhausted| {
                Err(SessionError::CurrentStepMissing("synthetic".to_string()))
            },
            |_next_protocol, _store| Ok(()),
        );

        assert!(advance.is_err(), "advance unexpectedly succeeded");
        // The promised scope must be restored, not left bound, so a retried tick
        // still represents the entry.
        assert!(
            matches!(session.scope, SessionScope::Promised { .. }),
            "scope was not restored to Promised after a failed commit"
        );
        assert_eq!(
            session.current_step().map(|step| step.protocol.as_str()),
            Some("decompose")
        );
        assert!(
            session
                .store()
                .execution_record("decompose", None)
                .is_none()
        );
        assert_no_execution_record(&project_dir, "decompose");
    }

    #[test]
    fn advance_without_materialized_work_unit_is_unresolved() {
        let dir = tempfile::tempdir().unwrap();
        let project_dir = write_entry_project(dir.path(), false);
        let mut session =
            SessionState::open_entry(project_dir.clone(), None, ticket_14(), |_, _| Ok(()))
                .unwrap();

        let advance = session.advance_with_validator(|_, _| Ok(()));

        assert!(matches!(
            advance,
            Err(SessionError::Postcondition(_))
                | Err(SessionError::Entry(crate::EntryError::Unresolved { .. }))
        ));
        assert!(matches!(session.scope, SessionScope::Promised { .. }));
        assert_no_execution_record(&project_dir, "decompose");
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

    #[test]
    fn next_context_refuses_current_step_after_required_input_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let project_dir = write_session_project(dir.path());
        let mut session =
            SessionState::open(project_dir.clone(), None, Some("work-unit-166".to_string()))
                .unwrap();

        fs::remove_file(project_dir.join(".runa/workspace/work-unit/work-unit-166.json")).unwrap();

        let context = session.next_context();

        assert!(matches!(
            context,
            Err(SessionError::CurrentStepNotReady(protocol)) if protocol == "take"
        ));
    }

    #[test]
    fn session_revalidates_scoped_work_unit_after_delayed_rescan() {
        let dir = tempfile::tempdir().unwrap();
        let project_dir = write_session_project_with_work_unit(dir.path(), false);
        let workspace = project_dir.join(".runa/workspace");
        let mut session =
            SessionState::open(project_dir.clone(), None, Some("work-unit-166".to_string()))
                .unwrap();

        fs::write(
            workspace.join("work-unit/work-unit-167.json"),
            r#"{"title":"Different work"}"#,
        )
        .unwrap();

        let readiness = session.readiness(|_next_protocol, _store| Ok(()));
        assert!(matches!(readiness, Err(SessionError::WorkUnitScope(_))));
        assert!(session.current_step().is_none());

        session.current_step = Some(CurrentStep {
            protocol: "take".to_string(),
            work_unit: Some("work-unit-166".to_string()),
            provenance_snapshot: None,
        });
        let context = session.next_context();
        assert!(matches!(context, Err(SessionError::WorkUnitScope(_))));

        let advance = session.advance_with_validator(|_next_protocol, _store| Ok(()));
        assert!(matches!(advance, Err(SessionError::WorkUnitScope(_))));
        assert_no_execution_record(&project_dir, "take");
    }

    fn assert_no_execution_record(project_dir: &Path, protocol: &str) {
        let execution_record_path = project_dir.join(".runa/store/execution-records.json");
        if execution_record_path.is_file() {
            let execution_records = fs::read_to_string(execution_record_path).unwrap();
            assert!(
                !execution_records.contains(&format!(r#""protocol": "{protocol}""#)),
                "{execution_records}"
            );
        }
    }
}
