/// Session-outcome exit codes — canonical: commons/EXIT-CODES.md.
///
/// runa implements the shared vocabulary; it does not define it. The
/// conformance test below verifies this enum against the vendored copy of
/// the commons table at `tests/fixtures/commons-exit-codes.json`, which
/// carries immutable provenance to its canonical version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitCode {
    Success,
    GenericFailure,
    UsageError,
    Blocked,
    NothingReady,
    WorkFailed,
    InfrastructureFailure,
}

impl ExitCode {
    pub const fn code(self) -> i32 {
        match self {
            ExitCode::Success => 0,
            ExitCode::GenericFailure => 1,
            ExitCode::UsageError => 2,
            ExitCode::Blocked => 3,
            ExitCode::NothingReady => 4,
            ExitCode::WorkFailed => 5,
            ExitCode::InfrastructureFailure => 6,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ExitCode;

    /// The vendored commons exit-code table; provenance inside the fixture.
    const VENDORED_COMMONS_TABLE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/commons-exit-codes.json"
    ));

    #[test]
    fn commons_exit_codes_match_specification() {
        let table: serde_json::Value = serde_json::from_str(VENDORED_COMMONS_TABLE)
            .expect("vendored commons exit-code table should be valid JSON");
        let application_defined = table["application_defined"]
            .as_object()
            .expect("vendored table should carry an application_defined object");

        let implemented = [
            ("success", ExitCode::Success),
            ("generic_failure", ExitCode::GenericFailure),
            ("usage_error", ExitCode::UsageError),
            ("blocked", ExitCode::Blocked),
            ("nothing_ready", ExitCode::NothingReady),
            ("work_failed", ExitCode::WorkFailed),
            ("infrastructure_failure", ExitCode::InfrastructureFailure),
        ];

        assert_eq!(
            application_defined.len(),
            implemented.len(),
            "the vendored commons table and the ExitCode enum should cover the same labels"
        );
        for (label, exit_code) in implemented {
            let expected = application_defined
                .get(label)
                .and_then(serde_json::Value::as_i64)
                .unwrap_or_else(|| panic!("vendored commons table should define `{label}`"));
            assert_eq!(
                i64::from(exit_code.code()),
                expected,
                "ExitCode::{exit_code:?} disagrees with the vendored commons table for `{label}`"
            );
        }
    }
}
