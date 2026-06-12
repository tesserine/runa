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
use crate::model::ProtocolDeclaration;
use crate::scoped_identity::{
    ResolvedForgeIdentity, ScopedWorkUnitError, find_work_unit_by_tracker_identity,
    validate_tracker_consistency,
};
use crate::selection::precondition_scan_incomplete_types;
use crate::store::ArtifactStore;

/// Artifact type the runtime treats as the scope-identity seed. The acquisition
/// surface is the sole unscoped protocol that produces this type.
pub const WORK_UNIT_ARTIFACT_TYPE: &str = "work-unit";

/// Environment atom carrying the entry ticket number to acquisition mechanics.
pub const RUNA_ENTRY_TICKET: &str = "RUNA_ENTRY_TICKET";

/// A forge ticket reference resolved against the active deployment identity.
///
/// `tracker_identity` is the canonical match key (`github:<owner>/<name>:<n>`
/// or `sourcehut:<tracker_id>:<n>`), identical to what a recorded work-unit
/// handle yields. `display` is the operator-facing rendering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketRef {
    pub number: u64,
    pub tracker_identity: String,
    pub display: String,
}

/// What the operator asserted in the reference, before deployment resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
enum AssertedForge {
    /// Bare number or `#N`: forge identity comes wholly from the deployment.
    None,
    /// `owner/repo#N` or a GitHub issue URL.
    Github { owner: String, name: String },
    /// `sourcehut:<tracker_id>#N`.
    Sourcehut { tracker_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedReference {
    number: u64,
    asserted: AssertedForge,
}

/// Errors raised while opening a session from a forge ticket reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryError {
    /// The reference string does not parse as a supported ticket form.
    InvalidReference { supplied: String },
    /// A required deployment-identity atom is absent for this reference form.
    MissingDeploymentIdentity { variable: &'static str },
    /// The reference asserts a forge deployment other than the active one.
    DeploymentDisagreement {
        reference_identity: String,
        active_identity: String,
    },
    /// No unscoped protocol declares the `work-unit` artifact as an output.
    NoAcquisitionSurface { scoped_producers: Vec<String> },
    /// More than one unscoped protocol declares the `work-unit` artifact.
    AmbiguousAcquisitionSurface { candidates: Vec<String> },
    /// Acquisition completed its contract but materialized no matching work-unit.
    Unresolved { reference: String },
}

impl fmt::Display for EntryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EntryError::InvalidReference { supplied } => write!(
                f,
                "'{supplied}' is not a recognized forge ticket reference; expected a ticket number, 'owner/repo#N', a forge issue URL, or 'sourcehut:<tracker_id>#N'"
            ),
            EntryError::MissingDeploymentIdentity { variable } => write!(
                f,
                "ticket reference omits forge identity and required deployment atom '{variable}' is unset"
            ),
            EntryError::DeploymentDisagreement {
                reference_identity,
                active_identity,
            } => write!(
                f,
                "ticket reference names {reference_identity}, which disagrees with active deployment {active_identity}"
            ),
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

/// Parse and resolve a forge ticket reference against the active deployment.
///
/// The grammar accepts a bare number, `#N`, `owner/repo#N`, a GitHub issue URL,
/// or `sourcehut:<tracker_id>#N`. An asserted forge identity must agree with the
/// active deployment; a bare reference inherits the deployment identity. No
/// forge access occurs — only the reference string and the resolved identity.
pub fn resolve_ticket_reference(
    raw: &str,
    identity: &ResolvedForgeIdentity,
) -> Result<TicketRef, EntryError> {
    let parsed = parse_ticket_reference(raw)?;
    bind_reference_identity(&parsed, identity)
}

fn parse_ticket_reference(raw: &str) -> Result<ParsedReference, EntryError> {
    let trimmed = raw.trim();
    let invalid = || EntryError::InvalidReference {
        supplied: raw.to_string(),
    };

    if let Some(rest) = trimmed.strip_prefix("https://github.com/") {
        let (repository, tail) = rest.split_once("/issues/").ok_or_else(invalid)?;
        let (owner, name) = repository.split_once('/').ok_or_else(invalid)?;
        let number = parse_number(tail).ok_or_else(invalid)?;
        if owner.is_empty() || name.is_empty() {
            return Err(invalid());
        }
        return Ok(ParsedReference {
            number,
            asserted: AssertedForge::Github {
                owner: owner.to_string(),
                name: name.to_string(),
            },
        });
    }

    if let Some(rest) = trimmed.strip_prefix("sourcehut:") {
        let (tracker_id, tail) = rest.split_once('#').ok_or_else(invalid)?;
        let number = parse_number(tail).ok_or_else(invalid)?;
        if tracker_id.is_empty() {
            return Err(invalid());
        }
        return Ok(ParsedReference {
            number,
            asserted: AssertedForge::Sourcehut {
                tracker_id: tracker_id.to_string(),
            },
        });
    }

    if let Some((repository, tail)) = trimmed.split_once('#')
        && !repository.is_empty()
    {
        let (owner, name) = repository.split_once('/').ok_or_else(invalid)?;
        let number = parse_number(tail).ok_or_else(invalid)?;
        if owner.is_empty() || name.is_empty() {
            return Err(invalid());
        }
        return Ok(ParsedReference {
            number,
            asserted: AssertedForge::Github {
                owner: owner.to_string(),
                name: name.to_string(),
            },
        });
    }

    let number = parse_number(trimmed.strip_prefix('#').unwrap_or(trimmed)).ok_or_else(invalid)?;
    Ok(ParsedReference {
        number,
        asserted: AssertedForge::None,
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
    identity: &ResolvedForgeIdentity,
) -> Result<TicketRef, EntryError> {
    let number = parsed.number;
    match &parsed.asserted {
        AssertedForge::Github { owner, name } => {
            require_forge(identity, "github")?;
            let active = require_atom(identity.owner.as_deref(), "RUNA_FORGE_OWNER")?;
            let active_name = require_atom(identity.name.as_deref(), "RUNA_FORGE_NAME")?;
            if owner != active || name != active_name {
                return Err(EntryError::DeploymentDisagreement {
                    reference_identity: format!("github:{owner}/{name}"),
                    active_identity: format!("github:{active}/{active_name}"),
                });
            }
            Ok(github_ticket(owner, name, number))
        }
        AssertedForge::Sourcehut { tracker_id } => {
            require_forge(identity, "sourcehut")?;
            let active = require_atom(identity.tracker_id.as_deref(), "RUNA_FORGE_TRACKER_ID")?;
            if tracker_id != active {
                return Err(EntryError::DeploymentDisagreement {
                    reference_identity: format!("sourcehut:{tracker_id}"),
                    active_identity: format!("sourcehut:{active}"),
                });
            }
            Ok(sourcehut_ticket(tracker_id, number))
        }
        AssertedForge::None => match identity.forge_type.as_str() {
            "sourcehut" => {
                let tracker_id =
                    require_atom(identity.tracker_id.as_deref(), "RUNA_FORGE_TRACKER_ID")?;
                Ok(sourcehut_ticket(tracker_id, number))
            }
            _ => {
                let owner = require_atom(identity.owner.as_deref(), "RUNA_FORGE_OWNER")?;
                let name = require_atom(identity.name.as_deref(), "RUNA_FORGE_NAME")?;
                Ok(github_ticket(owner, name, number))
            }
        },
    }
}

fn require_forge(identity: &ResolvedForgeIdentity, expected: &str) -> Result<(), EntryError> {
    if identity.forge_type == expected {
        return Ok(());
    }
    Err(EntryError::DeploymentDisagreement {
        reference_identity: expected.to_string(),
        active_identity: identity.forge_type.clone(),
    })
}

fn require_atom<'a>(value: Option<&'a str>, variable: &'static str) -> Result<&'a str, EntryError> {
    value
        .filter(|value| !value.is_empty())
        .ok_or(EntryError::MissingDeploymentIdentity { variable })
}

fn github_ticket(owner: &str, name: &str, number: u64) -> TicketRef {
    TicketRef {
        number,
        tracker_identity: format!("github:{owner}/{name}:{number}"),
        display: format!("github:{owner}/{name}#{number}"),
    }
}

fn sourcehut_ticket(tracker_id: &str, number: u64) -> TicketRef {
    TicketRef {
        number,
        tracker_identity: format!("sourcehut:{tracker_id}:{number}"),
        display: format!("sourcehut:{tracker_id}#{number}"),
    }
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
/// Returns the instance id whose tracker handle identity equals the reference,
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
    identity: &ResolvedForgeIdentity,
    ticket: &TicketRef,
) -> Result<Option<String>, ScopedWorkUnitError> {
    validate_tracker_consistency(store, identity)?;
    match find_work_unit_by_tracker_identity(store, &ticket.tracker_identity) {
        Some(instance_id) => Ok(Some(instance_id)),
        None if store.has_any_scan_gap_for_type(WORK_UNIT_ARTIFACT_TYPE) => {
            Err(ScopedWorkUnitError::WorkUnitScanIncomplete)
        }
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::TriggerCondition;

    fn github_identity(owner: &str, name: &str) -> ResolvedForgeIdentity {
        ResolvedForgeIdentity {
            forge_type: "github".to_string(),
            owner: Some(owner.to_string()),
            name: Some(name.to_string()),
            tracker_id: None,
        }
    }

    fn sourcehut_identity(tracker_id: &str) -> ResolvedForgeIdentity {
        ResolvedForgeIdentity {
            forge_type: "sourcehut".to_string(),
            owner: None,
            name: None,
            tracker_id: Some(tracker_id.to_string()),
        }
    }

    fn protocol(name: &str, produces: &[&str], scoped: bool) -> ProtocolDeclaration {
        ProtocolDeclaration {
            name: name.into(),
            requires: Vec::new(),
            accepts: Vec::new(),
            produces: produces.iter().map(|value| value.to_string()).collect(),
            may_produce: Vec::new(),
            required_output_choices: Vec::new(),
            scoped,
            trigger: TriggerCondition::OnArtifact {
                name: "seed".into(),
            },
            instructions: None,
        }
    }

    #[test]
    fn bare_number_inherits_github_deployment() {
        let ticket =
            resolve_ticket_reference("188", &github_identity("tesserine", "runa")).unwrap();
        assert_eq!(ticket.number, 188);
        assert_eq!(ticket.tracker_identity, "github:tesserine/runa:188");
        assert_eq!(ticket.display, "github:tesserine/runa#188");
    }

    #[test]
    fn hash_number_inherits_deployment() {
        let ticket =
            resolve_ticket_reference("#14", &github_identity("tesserine", "runa")).unwrap();
        assert_eq!(ticket.tracker_identity, "github:tesserine/runa:14");
    }

    #[test]
    fn owner_repo_form_matches_active_deployment() {
        let ticket =
            resolve_ticket_reference("tesserine/runa#14", &github_identity("tesserine", "runa"))
                .unwrap();
        assert_eq!(ticket.tracker_identity, "github:tesserine/runa:14");
    }

    #[test]
    fn github_url_form_parses() {
        let ticket = resolve_ticket_reference(
            "https://github.com/tesserine/runa/issues/188",
            &github_identity("tesserine", "runa"),
        )
        .unwrap();
        assert_eq!(ticket.number, 188);
        assert_eq!(ticket.tracker_identity, "github:tesserine/runa:188");
    }

    #[test]
    fn owner_repo_form_rejects_foreign_deployment() {
        let error = resolve_ticket_reference(
            "tesserine/groundwork#14",
            &github_identity("tesserine", "runa"),
        )
        .unwrap_err();
        assert!(matches!(error, EntryError::DeploymentDisagreement { .. }));
    }

    #[test]
    fn sourcehut_form_matches_active_tracker() {
        let ticket = resolve_ticket_reference("sourcehut:4#9", &sourcehut_identity("4")).unwrap();
        assert_eq!(ticket.tracker_identity, "sourcehut:4:9");
        assert_eq!(ticket.display, "sourcehut:4#9");
    }

    #[test]
    fn sourcehut_form_rejects_github_deployment() {
        let error =
            resolve_ticket_reference("sourcehut:4#9", &github_identity("tesserine", "runa"))
                .unwrap_err();
        assert!(matches!(error, EntryError::DeploymentDisagreement { .. }));
    }

    #[test]
    fn bare_number_under_sourcehut_inherits_tracker() {
        let ticket = resolve_ticket_reference("9", &sourcehut_identity("4")).unwrap();
        assert_eq!(ticket.tracker_identity, "sourcehut:4:9");
    }

    #[test]
    fn missing_owner_atom_is_rejected() {
        let identity = ResolvedForgeIdentity {
            forge_type: "github".to_string(),
            owner: None,
            name: Some("runa".to_string()),
            tracker_id: None,
        };
        let error = resolve_ticket_reference("14", &identity).unwrap_err();
        assert!(matches!(
            error,
            EntryError::MissingDeploymentIdentity {
                variable: "RUNA_FORGE_OWNER"
            }
        ));
    }

    #[test]
    fn garbage_reference_is_rejected() {
        for raw in [
            "",
            "not-a-ticket",
            "#",
            "0",
            "owner/repo#",
            "owner#3",
            "#abc",
        ] {
            assert!(
                resolve_ticket_reference(raw, &github_identity("tesserine", "runa")).is_err(),
                "expected '{raw}' to be rejected"
            );
        }
    }

    fn acquisition_protocol(requires: &[&str]) -> ProtocolDeclaration {
        ProtocolDeclaration {
            name: "decompose".into(),
            requires: requires.iter().map(|value| value.to_string()).collect(),
            accepts: Vec::new(),
            produces: vec![WORK_UNIT_ARTIFACT_TYPE.to_string()],
            may_produce: Vec::new(),
            required_output_choices: Vec::new(),
            scoped: false,
            trigger: TriggerCondition::OnArtifact {
                name: "request".into(),
            },
            instructions: None,
        }
    }

    #[test]
    fn acquisition_admissible_when_preconditions_met_and_scan_complete() {
        let tmp = tempfile::tempdir().unwrap();
        let store = crate::test_helpers::make_store(&tmp.path().join("store"), vec!["work-unit"]);
        let acquisition = acquisition_protocol(&[]);

        assert!(check_acquisition_admissible(&acquisition, &store, &HashSet::new()).is_ok());
    }

    #[test]
    fn acquisition_blocked_when_required_input_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let store = crate::test_helpers::make_store(
            &tmp.path().join("store"),
            vec!["request", "work-unit"],
        );
        let acquisition = acquisition_protocol(&["request"]);

        assert!(matches!(
            check_acquisition_admissible(&acquisition, &store, &HashSet::new()),
            Err(AcquisitionBlock::Precondition(_))
        ));
    }

    #[test]
    fn acquisition_blocked_when_required_input_partially_scanned() {
        let tmp = tempfile::tempdir().unwrap();
        let mut store = crate::test_helpers::make_store(
            &tmp.path().join("store"),
            vec!["request", "work-unit"],
        );
        // A valid `request` satisfies preconditions...
        store
            .record(
                "request",
                "good",
                std::path::Path::new("good.json"),
                &serde_json::json!({"title": "good"}),
            )
            .unwrap();
        // ...but `request` is a required input that was only partially scanned.
        let acquisition = acquisition_protocol(&["request"]);
        let partials = HashSet::from(["request".to_string()]);

        assert!(matches!(
            check_acquisition_admissible(&acquisition, &store, &partials),
            Err(AcquisitionBlock::ScanIncomplete(types)) if types == vec!["request".to_string()]
        ));
    }

    #[test]
    fn acquisition_admissible_when_only_substituted_trigger_partially_scanned() {
        let tmp = tempfile::tempdir().unwrap();
        let store = crate::test_helpers::make_store(
            &tmp.path().join("store"),
            vec!["request", "work-unit"],
        );
        // `request` is only the trigger (not required). The ticket replaces the
        // trigger, so a partial scan of `request` must not block acquisition.
        let acquisition = acquisition_protocol(&[]);
        let partials = HashSet::from(["request".to_string()]);

        assert!(check_acquisition_admissible(&acquisition, &store, &partials).is_ok());
    }

    #[test]
    fn resolve_promise_blocks_on_work_unit_scan_gap_without_match() {
        let tmp = tempfile::tempdir().unwrap();
        let mut store =
            crate::test_helpers::make_store(&tmp.path().join("store"), vec!["work-unit"]);
        // An unreadable work-unit file leaves the type only partially scanned, so
        // a no-match result is untrustworthy and must not authorize cold-start.
        store.mark_instance_scan_gap("work-unit", "hidden");
        let identity = github_identity("tesserine", "runa");
        let ticket = resolve_ticket_reference("14", &identity).unwrap();

        assert_eq!(
            resolve_promise(&store, &identity, &ticket),
            Err(ScopedWorkUnitError::WorkUnitScanIncomplete)
        );
    }

    #[test]
    fn resolve_promise_returns_none_when_scan_complete_and_no_match() {
        let tmp = tempfile::tempdir().unwrap();
        let store = crate::test_helpers::make_store(&tmp.path().join("store"), vec!["work-unit"]);
        let identity = github_identity("tesserine", "runa");
        let ticket = resolve_ticket_reference("14", &identity).unwrap();

        assert_eq!(resolve_promise(&store, &identity, &ticket), Ok(None));
    }

    #[test]
    fn resolve_promise_returns_match_despite_scan_gap() {
        let tmp = tempfile::tempdir().unwrap();
        let mut store =
            crate::test_helpers::make_store(&tmp.path().join("store"), vec!["work-unit"]);
        let artifact = serde_json::json!({
            "title": "Cold start",
            "handle": {
                "forge_tag": "github",
                "url": "https://github.com/tesserine/runa/issues/14",
                "number": 14
            }
        });
        let path = tmp.path().join("work-unit-14.json");
        std::fs::write(&path, artifact.to_string()).unwrap();
        store
            .record_with_timestamp("work-unit", "work-unit-14", &path, &artifact, 1)
            .unwrap();
        // A gap on an unrelated hidden instance must not suppress the found match
        // — re-entry still binds.
        store.mark_instance_scan_gap("work-unit", "hidden");
        let identity = github_identity("tesserine", "runa");
        let ticket = resolve_ticket_reference("14", &identity).unwrap();

        assert_eq!(
            resolve_promise(&store, &identity, &ticket),
            Ok(Some("work-unit-14".to_string()))
        );
    }

    #[test]
    fn discovers_sole_unscoped_producer() {
        let protocols = vec![
            protocol("decompose", &["work-unit"], false),
            protocol("take", &["claim"], true),
        ];
        let surface = discover_acquisition_surface(&protocols).unwrap();
        assert_eq!(surface.name, "decompose");
    }

    #[test]
    fn rejects_when_only_scoped_producer_exists() {
        let protocols = vec![protocol("take", &["work-unit"], true)];
        let error = discover_acquisition_surface(&protocols).unwrap_err();
        assert!(matches!(
            error,
            EntryError::NoAcquisitionSurface { scoped_producers } if scoped_producers == vec!["take".to_string()]
        ));
    }

    #[test]
    fn rejects_when_no_producer_exists() {
        let protocols = vec![protocol("plan", &["plan-doc"], false)];
        assert!(matches!(
            discover_acquisition_surface(&protocols),
            Err(EntryError::NoAcquisitionSurface { .. })
        ));
    }

    #[test]
    fn rejects_ambiguous_unscoped_producers() {
        let protocols = vec![
            protocol("decompose", &["work-unit"], false),
            protocol("intake", &["work-unit"], false),
        ];
        let error = discover_acquisition_surface(&protocols).unwrap_err();
        assert!(matches!(
            error,
            EntryError::AmbiguousAcquisitionSurface { candidates } if candidates.len() == 2
        ));
    }

    #[test]
    fn discovers_producer_via_required_output_choice() {
        let mut protocol = protocol("decompose", &[], false);
        protocol.required_output_choices = vec![crate::model::RequiredOutputChoice {
            name: "delivery".into(),
            members: vec!["work-unit".into(), "deferral".into()],
        }];
        let protocols = vec![protocol];
        assert_eq!(
            discover_acquisition_surface(&protocols).unwrap().name,
            "decompose"
        );
    }
}
