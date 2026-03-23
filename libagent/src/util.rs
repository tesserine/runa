//! Shared internal utilities for libagent.
//!
//! This module is `pub(crate)` — it provides helpers used across multiple
//! domain modules but not exposed in the public API. Add functions here
//! when they serve more than one module and are not domain-specific.

use std::time::{SystemTime, UNIX_EPOCH};

/// Returns the current wall-clock time as milliseconds since the Unix epoch.
pub(crate) fn current_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before UNIX epoch")
        .as_millis() as u64
}
