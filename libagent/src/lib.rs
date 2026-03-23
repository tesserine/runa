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
//! - [`scan`] — Filesystem reconciliation from artifact workspace into store state
//! - [`trigger`] — Trigger condition evaluation against runtime state
//! - [`enforcement`] — Pre/post-execution enforcement of protocol contracts
//!
//! See `ARCHITECTURE.md` in the repository root for data flow and design details.

pub mod context;
pub mod enforcement;
pub mod graph;
pub mod manifest;
pub mod model;
pub mod project;
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
pub use manifest::ManifestError;
pub use model::{ArtifactType, Manifest, ProtocolDeclaration, TriggerCondition};
pub use project::{Config, LoadedProject, ProjectError, State};
pub use scan::{
    ArtifactRef, InvalidArtifact, MalformedArtifact, PartiallyScannedType, ScanError, ScanResult,
    UnreadableArtifact, scan,
};
pub use selection::{
    Candidate, CandidateStatus, ClassifiedCandidate, ScanTrust, classify_candidates,
    collect_unsatisfied_conditions, discover_ready_candidates,
};
pub use store::{ArtifactState, ArtifactStore, StoreError, ValidationStatus};
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
