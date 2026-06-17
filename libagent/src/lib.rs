//! Core library for the runa cognitive runtime.
//!
//! libagent provides the data model, parsing, validation, and evaluation logic
//! that the runtime uses to enforce contracts between methodologies and agents.
//! The CLI is a thin layer over this library.
//!
//! - [`model`] — Core types: `Manifest`, `ArtifactType`, `ProtocolDeclaration`, `TriggerCondition`
//! - [`manifest`] — TOML manifest parsing with uniqueness validation
//! - [`validation`] — JSON Schema validation for artifact instances
//! - [`graph`] — Dependency graph: topological ordering, cycle detection, blocked-protocol identification
//! - [`store`] — Artifact state tracking: validation status, content hashing, JSON persistence
//! - [`mod@scan`] — Filesystem reconciliation from artifact workspace into store state
//! - [`completion`] — Shared completion evidence checks for live and projected readiness
//! - [`trigger`] — Trigger condition evaluation against runtime state
//! - [`enforcement`] — Pre/post-execution enforcement of protocol contracts
//! - [`scoped_identity`] — Canonical work-unit id validation for scoped entry points
//!
//! See `ARCHITECTURE.md` in the repository root for data flow and design details.

pub(crate) mod completion;
pub mod context;
pub mod enforcement;
pub mod entry;
pub mod graph;
pub mod logging;
pub mod manifest;
pub mod model;
pub mod project;
pub mod projection;
pub mod scan;
pub mod scoped_identity;
pub mod selection;
pub mod session;
pub mod status;
pub mod store;
#[cfg(test)]
pub(crate) mod test_helpers;
pub mod transcript;
pub mod trigger;
pub(crate) mod util;
pub mod validation;
pub use enforcement::{
    ArtifactFailure, EnforcementError, Phase, Relationship, enforce_postconditions,
    enforce_preconditions,
};
pub use entry::{
    AcquisitionBlock, EntryError, RUNA_ENTRY_TICKET, TicketRef, check_acquisition_admissible,
    discover_acquisition_surface, resolve_promise, resolve_ticket_reference,
};
pub use graph::{CycleError, DependencyGraph, GraphError};
pub use logging::{LoggingError, ResolvedLoggingConfig, configure_tracing, resolve_logging_config};
pub use manifest::ManifestError;
pub use model::{
    ArtifactType, Manifest, ProtocolDeclaration, RequiredOutputChoice, TriggerCondition,
    UnscopedOutputRequiresWorkUnitError, validate_output_scope,
};
pub use project::{
    Config, ForgeConfig, LoadedProject, LogFormat, LoggingConfig, ProjectError, State,
    TranscriptConfig,
};
pub use projection::{
    ProjectionCandidate, ProjectionClass, project_cascade, project_entry_cascade,
};
pub use scan::{
    ArtifactRef, InvalidArtifact, MalformedArtifact, PartiallyScannedType, ScanError, ScanResult,
    UnreadableArtifact, scan,
};
pub use scoped_identity::{
    ResolvedForgeIdentity, ScopedWorkUnitError, find_work_unit_by_tracker_identity,
    resolve_forge_environment, resolve_forge_identity, resolve_project_forge_identity,
    validate_scoped_work_unit, validate_scoped_work_unit_with_identity,
    validate_tracker_consistency,
};
pub use selection::{
    Candidate, CandidateKey, CandidateStatus, ClassifiedCandidate, EvaluationScope,
    EvaluationTopology, ScanTrust, WaitingReason, classify_candidates,
    collect_unsatisfied_conditions, discover_ready_candidates, protocol_entry_execution_record,
    protocol_execution_input_snapshot, protocol_execution_record, protocol_relevant_input_types,
    protocol_relevant_inputs_changed, refresh_exhausted_candidates_after_scan,
    resolve_evaluation_topology,
};
pub use session::{
    AdvanceOutcome, CurrentStep, SESSION_ADVANCE_RECEIPT_ENV, SessionError, SessionReadiness,
    SessionState, SessionTransition, StepSelector,
};
pub use status::{
    EvaluatedProtocols, FailureEntry, FailureJson, InputEntry, InputJson, ProtocolEntry,
    ProtocolJson, ProtocolStatus, ScanFindings, StateJson, TriggerState, collect_scan_findings,
    evaluate_protocols,
};
pub use store::{
    ArtifactState, ArtifactStore, ExecutionInput, ExecutionInputMode, ExecutionInputSnapshot,
    ExecutionRecord, StoreError, ValidationStatus, execution_contract_hash,
};
pub use trigger::{TriggerContext, TriggerResult, evaluate as evaluate_trigger};
pub use validation::{ValidationError, Violation};

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_set() {
        let v = version();
        assert!(!v.is_empty());
    }
}
