pub mod graph;
pub mod manifest;
pub mod model;
pub mod store;
pub mod validation;
pub use graph::{CycleError, DependencyGraph, GraphError};
pub use manifest::ManifestError;
pub use model::{ArtifactType, Manifest, SkillDeclaration, TriggerCondition};
pub use store::{ArtifactState, ArtifactStore, StoreError, ValidationStatus};
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
