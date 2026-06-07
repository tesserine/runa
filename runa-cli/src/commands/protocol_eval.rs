pub(crate) use libagent::session::{
    EvaluatedProtocols, ProtocolEntry, ProtocolJson, ProtocolStatus, ScanFindings,
    collect_scan_findings,
};
#[cfg(test)]
pub(crate) use libagent::session::{FailureEntry, InputEntry, TriggerState};

pub(crate) fn evaluate_protocols(
    loaded: &crate::project::LoadedProject,
    working_dir: &std::path::Path,
    scan_findings: &ScanFindings,
    scope: libagent::EvaluationScope<'_>,
) -> EvaluatedProtocols {
    let evaluated =
        libagent::session::evaluate_protocols(loaded, working_dir, scan_findings, scope);
    if let Some(cycle) = &evaluated.cycle {
        eprintln!("warning: {cycle}");
    }
    evaluated
}

pub(crate) fn print_group(label: &str, entries: &[ProtocolEntry]) {
    println!("{label}:");
    if entries.is_empty() {
        println!("  (none)");
        return;
    }

    for entry in entries {
        println!("  {}", display_protocol_name(entry));
        match entry.status {
            ProtocolStatus::Ready => {
                for input in &entry.inputs {
                    println!(
                        "    - {}/{} ({})",
                        input.artifact_type, input.instance_id, input.relationship
                    );
                }
            }
            ProtocolStatus::Blocked => {
                for failure in &entry.precondition_failures {
                    println!("    - {} ({})", failure.artifact_type, failure.reason);
                }
            }
            ProtocolStatus::Waiting => {
                for condition in &entry.unsatisfied_conditions {
                    println!("    - {condition}");
                }
            }
        }
    }
}

fn display_protocol_name(entry: &ProtocolEntry) -> String {
    match &entry.work_unit {
        Some(work_unit) => format!("{} (work_unit={work_unit})", entry.name),
        None => entry.name.clone(),
    }
}
