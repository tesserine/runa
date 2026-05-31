//! Shared completion-evidence checks for live and projected readiness.

use std::collections::HashSet;

use crate::model::ProtocolDeclaration;
use crate::store::ArtifactStore;

pub(crate) fn completion_scan_gap_affects_work_unit(
    protocol: &ProtocolDeclaration,
    completion_outputs: &[&String],
    store: &ArtifactStore,
    work_unit: Option<&str>,
    partially_scanned_types: &HashSet<String>,
) -> bool {
    completion_scan_gap_types(protocol, completion_outputs)
        .iter()
        .any(|artifact_type| {
            store.scan_gap_affects_work_unit(artifact_type, work_unit)
                || (partially_scanned_types.contains(artifact_type.as_str())
                    && !store.has_any_scan_gap_for_type(artifact_type))
        })
}

fn completion_scan_gap_types<'a>(
    protocol: &'a ProtocolDeclaration,
    completion_outputs: &[&'a String],
) -> Vec<&'a String> {
    let mut output_types = completion_outputs.to_vec();
    output_types.extend(protocol.required_choice_members());
    output_types
}
