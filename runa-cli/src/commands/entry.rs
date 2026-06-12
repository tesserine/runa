//! Cold-start ticket entry wiring shared by `go` and `run`.
//!
//! These helpers parse a forge ticket reference, resolve it against recorded
//! work-units, discover the methodology's acquisition surface, and synthesize
//! the acquisition step the runtime serves. All forge identity logic lives in
//! libagent; this module only adapts it to the CLI's plan/execution types.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use libagent::{
    Config, ProtocolDeclaration, ResolvedForgeIdentity, ScanFindings, ScanResult, TicketRef,
};

use crate::commands::CommandError;
use crate::commands::step::{PlannedEntry, StepError, resolved_runtime_env};
use crate::project::LoadedProject;

/// Why a cold-start acquisition cannot be served, if it cannot.
///
/// Entry substitutes only the acquisition's trigger; its `requires`
/// preconditions and scan-trust gates still apply, exactly as normal readiness
/// would block a protocol. Returns `None` when the acquisition is servable.
pub(crate) fn acquisition_block_reason(
    loaded: &LoadedProject,
    acquisition: &ProtocolDeclaration,
    scan_result: &ScanResult,
) -> Option<String> {
    let partially_scanned: HashSet<String> = scan_result
        .partially_scanned_types
        .iter()
        .map(|partial| partial.artifact_type.clone())
        .collect();
    libagent::check_acquisition_admissible(acquisition, &loaded.store, &partially_scanned)
        .err()
        .map(|block| block.to_string())
}

/// Parse and resolve a ticket reference against the project's forge deployment.
pub(crate) fn resolve_reference(
    loaded: &LoadedProject,
    raw: &str,
) -> Result<(TicketRef, ResolvedForgeIdentity), StepError> {
    let identity = libagent::resolve_forge_identity(&loaded.config.forge);
    let ticket =
        libagent::resolve_ticket_reference(raw, &identity).map_err(StepError::TicketReference)?;
    Ok((ticket, identity))
}

/// Resolve the reference to an already-recorded work-unit (re-entry), or `None`
/// when the work-unit does not exist yet (cold start).
pub(crate) fn resolve_existing(
    loaded: &LoadedProject,
    identity: &ResolvedForgeIdentity,
    ticket: &TicketRef,
) -> Result<Option<String>, StepError> {
    libagent::resolve_promise(&loaded.store, identity, ticket)
        .map_err(CommandError::from)
        .map_err(StepError::from)
}

/// The methodology's acquisition surface — the sole unscoped producer of the
/// `work-unit` artifact — cloned for plan construction.
pub(crate) fn acquisition_surface(
    loaded: &LoadedProject,
) -> Result<ProtocolDeclaration, StepError> {
    libagent::discover_acquisition_surface(&loaded.manifest.protocols)
        .cloned()
        .map_err(StepError::TicketReference)
}

/// The provisional scope id used in projections before the work-unit exists.
pub(crate) fn promised_scope_token(ticket: &TicketRef) -> String {
    format!("work-unit-{}", ticket.number)
}

/// Build the synthesized acquisition planned entry for a cold ticket entry.
///
/// The entry's trigger is the ticket reference; its context carries the
/// reference (and nothing of the ticket's content) plus the acquisition
/// protocol's own instructions and inputs.
pub(crate) fn acquisition_planned_entry(
    loaded: &LoadedProject,
    acquisition: &ProtocolDeclaration,
    ticket: &TicketRef,
    scan_findings: &ScanFindings,
) -> PlannedEntry {
    let mut context = libagent::context::build_context(acquisition, &loaded.store, None);
    context.entry = Some(libagent::context::EntryDelivery {
        reference: ticket.display.clone(),
        ticket_number: ticket.number,
        tracker_identity: ticket.tracker_identity.clone(),
    });
    PlannedEntry {
        protocol: acquisition.name.clone(),
        work_unit: None,
        trigger: "ticket_entry".to_string(),
        context,
        execution_record: libagent::protocol_execution_record(
            acquisition,
            &loaded.store,
            None,
            &scan_findings.affected_types,
        ),
    }
}

/// Runtime env for an entry step: the forge atoms plus the entry ticket number.
pub(crate) fn entry_runtime_env(
    working_dir: &Path,
    config: &Config,
    ticket: &TicketRef,
) -> BTreeMap<String, String> {
    let mut env = resolved_runtime_env(working_dir, config);
    env.insert(
        libagent::RUNA_ENTRY_TICKET.to_string(),
        ticket.number.to_string(),
    );
    env
}
