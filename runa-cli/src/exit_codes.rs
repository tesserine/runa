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

    #[test]
    fn commons_exit_codes_match_specification() {
        assert_eq!(ExitCode::Success.code(), 0);
        assert_eq!(ExitCode::GenericFailure.code(), 1);
        assert_eq!(ExitCode::UsageError.code(), 2);
        assert_eq!(ExitCode::Blocked.code(), 3);
        assert_eq!(ExitCode::NothingReady.code(), 4);
        assert_eq!(ExitCode::WorkFailed.code(), 5);
        assert_eq!(ExitCode::InfrastructureFailure.code(), 6);
    }
}
