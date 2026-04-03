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
//! - [`trigger`] — Trigger condition evaluation against runtime state
//! - [`enforcement`] — Pre/post-execution enforcement of protocol contracts
//!
//! See `ARCHITECTURE.md` in the repository root for data flow and design details.

pub mod context;
pub mod enforcement;
pub mod graph;
pub mod logging;
pub mod manifest;
pub mod model;
pub mod project;
pub mod projection;
pub mod scan;
pub mod selection;
pub mod store;
#[cfg(test)]
pub(crate) mod test_helpers;
pub mod trigger;
pub(crate) mod util;
pub mod validation;
pub use enforcement::{
    ArtifactFailure, EnforcementError, Phase, Relationship, enforce_postconditions,
    enforce_preconditions,
};
pub use graph::{CycleError, DependencyGraph, GraphError};
pub use logging::{LoggingError, ResolvedLoggingConfig, configure_tracing, resolve_logging_config};
pub use manifest::ManifestError;
pub use model::{
    ArtifactType, Manifest, ProtocolDeclaration, TriggerCondition,
    UnscopedOutputRequiresWorkUnitError, validate_output_scope,
};
pub use project::{Config, LoadedProject, LogFormat, LoggingConfig, ProjectError, State};
pub use projection::{ProjectionCandidate, ProjectionClass, project_cascade};
pub use scan::{
    ArtifactRef, InvalidArtifact, MalformedArtifact, PartiallyScannedType, ScanError, ScanResult,
    UnreadableArtifact, scan,
};
pub use selection::{
    Candidate, CandidateStatus, ClassifiedCandidate, EvaluationScope, EvaluationTopology,
    ScanTrust, WaitingReason, classify_candidates, collect_unsatisfied_conditions,
    discover_ready_candidates, protocol_execution_input_snapshot, protocol_relevant_input_types,
    protocol_relevant_inputs_changed, resolve_evaluation_topology,
};
pub use store::{
    ArtifactState, ArtifactStore, ExecutionInput, ExecutionInputSnapshot, ExecutionRecord,
    StoreError, ValidationStatus, execution_contract_hash,
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
