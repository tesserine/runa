//! Cold-start entry from a forge ticket reference.
//!
//! The scoped pipeline activates on a `work-unit` artifact, but the natural
//! developer entry is a ticket reference — "start on runa#14". This module
//! supplies the runtime half: parsing a forge ticket reference into a tracker
//! identity, discovering the methodology's acquisition surface, and resolving
//! the reference to the materialized `work-unit` instance once acquisition has
//! produced it.
//!
//! The runtime never reads ticket content. A reference carries identity only;
//! the methodology performs all forge reads through its own mechanics.

use std::collections::HashSet;
use std::fmt;

use crate::enforcement::{EnforcementError, enforce_preconditions};
use crate::forge_address::{ForgeAddressError, ForgeProject};
use crate::model::ProtocolDeclaration;
use crate::scoped_identity::{
    ScopedWorkUnitError, find_work_unit_by_tracker_identity, validate_tracker_consistency,
};
use crate::selection::precondition_scan_incomplete_types;
use crate::store::ArtifactStore;

/// Artifact type the runtime treats as the scope-identity seed. The acquisition
/// surface is the sole unscoped protocol that produces this type.
pub const WORK_UNIT_ARTIFACT_TYPE: &str = "work-unit";

/// A forge ticket reference resolved against the configured forge-address set.
///
/// `tracker_identity` is the unnumbered tracker identity. `work_unit_identity`
/// is the numbered identity that matches a recorded work-unit handle. `display`
/// is the operator-facing rendering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketRef {
    pub number: u64,
    pub tracker_identity: String,
    pub work_unit_identity: String,
    pub display: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedReference {
    number: u64,
    tracker_selector: Option<String>,
}

/// Errors raised while opening a session from a forge ticket reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryError {
    /// The reference string does not parse as a supported ticket form.
    InvalidReference {
        supplied: String,
    },
    ForgeAddress(ForgeAddressError),
    /// No unscoped protocol declares the `work-unit` artifact as an output.
    NoAcquisitionSurface {
        scoped_producers: Vec<String>,
    },
    /// More than one unscoped protocol declares the `work-unit` artifact.
    AmbiguousAcquisitionSurface {
        candidates: Vec<String>,
    },
    /// Acquisition completed its contract but materialized no matching work-unit.
    Unresolved {
        reference: String,
    },
}

impl fmt::Display for EntryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EntryError::InvalidReference { supplied } => write!(
                f,
                "'{supplied}' is not a recognized forge ticket reference; expected a ticket number, '#N', or '<tracker>#N'"
            ),
            EntryError::ForgeAddress(error) => write!(f, "{error}"),
            EntryError::NoAcquisitionSurface { scoped_producers } => {
                if scoped_producers.is_empty() {
                    write!(
                        f,
                        "methodology declares no unscoped producer of '{WORK_UNIT_ARTIFACT_TYPE}'; cold-start ticket entry is unavailable"
                    )
                } else {
                    write!(
                        f,
                        "methodology declares no unscoped producer of '{WORK_UNIT_ARTIFACT_TYPE}'; cold-start ticket entry is unavailable (scoped producers: {})",
                        scoped_producers.join(", ")
                    )
                }
            }
            EntryError::AmbiguousAcquisitionSurface { candidates } => write!(
                f,
                "more than one unscoped protocol produces '{WORK_UNIT_ARTIFACT_TYPE}' ({}); cold-start ticket entry requires a single acquisition surface",
                candidates.join(", ")
            ),
            EntryError::Unresolved { reference } => write!(
                f,
                "acquisition from ticket {reference} completed but produced no '{WORK_UNIT_ARTIFACT_TYPE}' matching the reference"
            ),
        }
    }
}

impl std::error::Error for EntryError {}

impl From<ForgeAddressError> for EntryError {
    fn from(error: ForgeAddressError) -> Self {
        Self::ForgeAddress(error)
    }
}

/// Parse and resolve a forge ticket reference against configured trackers.
///
/// The grammar accepts a bare number, `#N`, or `<tracker>#N`. Bare references
/// are accepted only when the project has exactly one tracker.
pub fn resolve_ticket_reference(
    raw: &str,
    project: &ForgeProject,
) -> Result<TicketRef, EntryError> {
    let parsed = parse_ticket_reference(raw)?;
    bind_reference_identity(&parsed, project).map_err(EntryError::from)
}

fn parse_ticket_reference(raw: &str) -> Result<ParsedReference, EntryError> {
    let trimmed = raw.trim();
    let invalid = || EntryError::InvalidReference {
        supplied: raw.to_string(),
    };

    if let Some((repository, tail)) = trimmed.split_once('#')
        && !repository.is_empty()
    {
        let number = parse_number(tail).ok_or_else(invalid)?;
        if repository.contains('/') {
            return Err(invalid());
        }
        return Ok(ParsedReference {
            number,
            tracker_selector: Some(repository.to_string()),
        });
    }

    let number = parse_number(trimmed.strip_prefix('#').unwrap_or(trimmed)).ok_or_else(invalid)?;
    Ok(ParsedReference {
        number,
        tracker_selector: None,
    })
}

fn parse_number(value: &str) -> Option<u64> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    value.parse().ok().filter(|number| *number > 0)
}

fn bind_reference_identity(
    parsed: &ParsedReference,
    project: &ForgeProject,
) -> Result<TicketRef, ForgeAddressError> {
    let number = parsed.number;
    let tracker = project.tracker(parsed.tracker_selector.as_deref())?;
    Ok(TicketRef {
        number,
        tracker_identity: tracker.identity.clone(),
        work_unit_identity: format!("{}#{number}", tracker.identity),
        display: format!("{}#{number}", tracker.id),
    })
}

/// Discover the single unscoped protocol that produces the `work-unit` artifact.
///
/// This is the surface the runtime serves so acquisition can deliver its
/// materialized work-unit. The derivation is methodology-neutral: it names no
/// protocol, only the runtime-owned scope-identity artifact type.
pub fn discover_acquisition_surface(
    protocols: &[ProtocolDeclaration],
) -> Result<&ProtocolDeclaration, EntryError> {
    let producers: Vec<&ProtocolDeclaration> = protocols
        .iter()
        .filter(|protocol| {
            protocol
                .output_artifact_types()
                .any(|artifact_type| artifact_type == WORK_UNIT_ARTIFACT_TYPE)
        })
        .collect();
    let unscoped: Vec<&ProtocolDeclaration> = producers
        .iter()
        .copied()
        .filter(|protocol| !protocol.scoped)
        .collect();

    match unscoped.as_slice() {
        [surface] => Ok(surface),
        [] => Err(EntryError::NoAcquisitionSurface {
            scoped_producers: producers
                .iter()
                .filter(|protocol| protocol.scoped)
                .map(|protocol| protocol.name.clone())
                .collect(),
        }),
        many => Err(EntryError::AmbiguousAcquisitionSurface {
            candidates: many.iter().map(|protocol| protocol.name.clone()).collect(),
        }),
    }
}

/// Why a cold-start acquisition is not admissible, if it is not.
///
/// Mirrors the canonical readiness gates ([`crate::classify_candidates`]) with
/// the trigger substituted by the operator's reference: a missing/invalid
/// required input, or an input type left untrusted by a partial scan.
#[derive(Debug)]
pub enum AcquisitionBlock {
    Precondition(EnforcementError),
    ScanIncomplete(Vec<String>),
}

impl fmt::Display for AcquisitionBlock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AcquisitionBlock::Precondition(error) => write!(f, "{error}"),
            AcquisitionBlock::ScanIncomplete(types) => write!(
                f,
                "acquisition input artifact type(s) {} were only partially scanned",
                types.join(", ")
            ),
        }
    }
}

/// The single admission gate for a cold-start acquisition — the one place that
/// enumerates the readiness gates entry must honor, so the session, projection,
/// and CLI entry surfaces cannot drift apart.
///
/// Entry substitutes only the acquisition protocol's trigger (the operator's
/// ticket reference stands in for it). Every other gate the canonical readiness
/// path applies still holds: its `requires` preconditions, and scan trust over
/// those required inputs. Scan gaps on a trigger-only artifact type are ignored
/// — the reference replaces the trigger, so that artifact is never consulted.
/// Currentness is intentionally omitted — the reference forces a fresh
/// acquisition regardless of unrelated outputs. The acquisition is always
/// unscoped, so `work_unit` is `None`.
pub fn check_acquisition_admissible(
    acquisition: &ProtocolDeclaration,
    store: &ArtifactStore,
    partially_scanned_types: &HashSet<String>,
) -> Result<(), AcquisitionBlock> {
    enforce_preconditions(acquisition, store, None).map_err(AcquisitionBlock::Precondition)?;
    let incomplete = precondition_scan_incomplete_types(acquisition, partially_scanned_types);
    if !incomplete.is_empty() {
        return Err(AcquisitionBlock::ScanIncomplete(incomplete));
    }
    Ok(())
}

/// Resolve a ticket reference to the materialized `work-unit` instance.
///
/// Returns the instance id whose work-unit handle identity equals the reference,
/// or `None` when no such instance exists yet (cold store). Tracker-handle
/// consistency is enforced first, so a `ScopedWorkUnitError` here signals a
/// malformed or conflicting recorded work-unit rather than a missing one.
///
/// A `None` result authorizes cold-start acquisition, so it must be trustworthy:
/// when the `work-unit` type was only partially scanned (an unreadable instance
/// file could hide a matching or duplicate root), resolution fails with
/// [`ScopedWorkUnitError::WorkUnitScanIncomplete`] rather than falling through to
/// acquisition and risking duplicate work for an existing ticket.
pub fn resolve_promise(
    store: &ArtifactStore,
    project: &ForgeProject,
    ticket: &TicketRef,
) -> Result<Option<String>, ScopedWorkUnitError> {
    validate_tracker_consistency(store, project)?;
    match find_work_unit_by_tracker_identity(store, &ticket.work_unit_identity) {
        Some(instance_id) => Ok(Some(instance_id)),
        None if store.has_any_scan_gap_for_type(WORK_UNIT_ARTIFACT_TYPE) => {
            Err(ScopedWorkUnitError::WorkUnitScanIncomplete)
        }
        None => Ok(None),
    }
}

#[cfg(test)]
mod forge_address_tests {
    use super::*;
    use crate::forge_address::{
        ForgeProject, RawForgeInstance, RawForges, RawRepository, RawTracker,
    };

    fn project(trackers: Vec<RawTracker>) -> ForgeProject {
        ForgeProject::resolve(RawForges {
            instances: vec![
                RawForgeInstance {
                    id: "github-com".to_string(),
                    forge_type: "github".to_string(),
                    host: Some("github.com".to_string()),
                    git_host: None,
                    tracker_host: None,
                },
                RawForgeInstance {
                    id: "weforge".to_string(),
                    forge_type: "sourcehut".to_string(),
                    host: None,
                    git_host: Some("git.weforge.build".to_string()),
                    tracker_host: Some("todo.weforge.build".to_string()),
                },
            ],
            repositories: vec![RawRepository {
                id: "runa".to_string(),
                instance: "github-com".to_string(),
                owner: "tesserine".to_string(),
                name: "runa".to_string(),
            }],
            trackers,
        })
        .unwrap()
    }

    #[test]
    fn bare_ticket_reference_resolves_through_one_configured_tracker() {
        let ticket = resolve_ticket_reference("#14", &project(Vec::new())).unwrap();

        assert_eq!(ticket.number, 14);
        assert_eq!(
            ticket.tracker_identity,
            "github@github.com/tracker/tesserine/runa"
        );
        assert_eq!(
            ticket.work_unit_identity,
            "github@github.com/tracker/tesserine/runa#14"
        );
        assert_eq!(ticket.display, "runa#14");
    }

    #[test]
    fn bare_ticket_reference_is_rejected_when_multiple_trackers_exist() {
        let error = resolve_ticket_reference(
            "14",
            &project(vec![RawTracker {
                id: "weforge".to_string(),
                instance: "weforge".to_string(),
                owner: "operator".to_string(),
                name: "weforge".to_string(),
                tracker_id: Some("4".to_string()),
            }]),
        )
        .unwrap_err();

        assert!(matches!(
            error,
            EntryError::ForgeAddress(ForgeAddressError::AmbiguousBareTrackerReference)
        ));
    }

    #[test]
    fn qualified_ticket_reference_selects_configured_tracker() {
        let ticket = resolve_ticket_reference(
            "weforge#9",
            &project(vec![RawTracker {
                id: "weforge".to_string(),
                instance: "weforge".to_string(),
                owner: "operator".to_string(),
                name: "weforge".to_string(),
                tracker_id: Some("4".to_string()),
            }]),
        )
        .unwrap();

        assert_eq!(
            ticket.tracker_identity,
            "sourcehut@git=git.weforge.build,tracker=todo.weforge.build/tracker/~operator/weforge/4"
        );
        assert_eq!(
            ticket.work_unit_identity,
            "sourcehut@git=git.weforge.build,tracker=todo.weforge.build/tracker/~operator/weforge/4#9"
        );
    }
}
