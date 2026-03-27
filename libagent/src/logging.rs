//! Shared tracing bootstrap and runtime reconfiguration.
//!
//! Installs a global `tracing` subscriber with a reloadable filter and
//! runtime-selectable text/JSON stderr formatter. Callers invoke
//! [`configure_tracing`] at startup and again after config is loaded.

use std::fmt;
use std::io::{self, Write};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, OnceLock};

use tracing_subscriber::Registry;
use tracing_subscriber::filter::EnvFilter;
use tracing_subscriber::fmt::{self as tracing_fmt, MakeWriter};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::reload;

use crate::project::{LogFormat, LoggingConfig};

const DEFAULT_FILTER: &str = "warn";

type FilterHandle = reload::Handle<EnvFilter, Registry>;

/// Errors from tracing subscriber installation or reconfiguration.
#[derive(Debug)]
pub enum LoggingError {
    /// Failed to set the global default subscriber.
    SetGlobalDefault(String),
    /// Failed to reload the filter on an existing subscriber.
    Reload(String),
}

impl fmt::Display for LoggingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoggingError::SetGlobalDefault(err) => write!(f, "{err}"),
            LoggingError::Reload(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for LoggingError {}

/// The resolved logging configuration after applying the precedence chain:
/// `RUST_LOG` env var > `config.logging.filter` > default (`warn`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedLoggingConfig {
    pub format: LogFormat,
    pub filter: String,
}

/// Resolve the logging filter and format using the fixed precedence chain:
/// `RUST_LOG` env var > `config.logging.filter` > default (`warn`).
/// Format comes from config, defaulting to text.
pub fn resolve_logging_config(config: Option<&LoggingConfig>) -> ResolvedLoggingConfig {
    let env_filter = std::env::var("RUST_LOG").ok();
    resolve_logging_config_with_env(env_filter.as_deref(), config)
}

pub(crate) fn resolve_logging_config_with_env(
    env_filter: Option<&str>,
    config: Option<&LoggingConfig>,
) -> ResolvedLoggingConfig {
    let filter = env_filter
        .filter(|value| !value.is_empty())
        .map(String::from)
        .or_else(|| config.and_then(|logging| logging.filter.clone()))
        .unwrap_or_else(|| DEFAULT_FILTER.to_string());

    ResolvedLoggingConfig {
        format: config
            .map(|logging| logging.format)
            .unwrap_or(LogFormat::Text),
        filter,
    }
}

/// Install or reconfigure the global tracing subscriber.
///
/// On first call, installs a subscriber with text and JSON layers writing to
/// stderr. On subsequent calls, reloads the filter and switches the active
/// format. If the filter string is invalid, falls back to `warn` with a
/// logged warning.
pub fn configure_tracing(config: Option<&LoggingConfig>) -> Result<(), LoggingError> {
    let resolved = resolve_logging_config(config);
    if let Some(handle) = TRACING_HANDLE.get() {
        return handle.reload(&resolved);
    }

    let handle = TracingHandle::new(&resolved)?;
    match TRACING_HANDLE.set(handle) {
        Ok(()) => Ok(()),
        Err(existing) => existing.reload(&resolved),
    }
}

struct TracingHandle {
    format: Arc<AtomicU8>,
    filter: FilterHandle,
}

impl TracingHandle {
    fn new(resolved: &ResolvedLoggingConfig) -> Result<Self, LoggingError> {
        let format = Arc::new(AtomicU8::new(format_code(resolved.format)));
        let text_writer = ActiveFormatWriter::new(format.clone(), LogFormat::Text);
        let json_writer = ActiveFormatWriter::new(format.clone(), LogFormat::Json);

        // Validate filter before subscriber installation. If invalid, use the
        // default and defer the warning until the subscriber is installed.
        let (initial_filter, fallback_warning) = match EnvFilter::try_new(&resolved.filter) {
            Ok(f) => (f, None),
            Err(err) => (
                EnvFilter::new(DEFAULT_FILTER),
                Some((resolved.filter.clone(), err.to_string())),
            ),
        };
        let (filter_layer, filter) = reload::Layer::new(initial_filter);

        let text_layer = tracing_fmt::layer()
            .with_writer(text_writer)
            .with_ansi(false)
            .without_time()
            .compact();
        let json_layer = tracing_fmt::layer()
            .json()
            .with_writer(json_writer)
            .with_ansi(false)
            .without_time();

        let subscriber = Registry::default()
            .with(filter_layer)
            .with(text_layer)
            .with(json_layer);
        tracing::subscriber::set_global_default(subscriber)
            .map_err(|err| LoggingError::SetGlobalDefault(err.to_string()))?;

        if let Some((spec, error)) = fallback_warning {
            tracing::warn!(
                spec = %spec,
                error = %error,
                fallback = DEFAULT_FILTER,
                "configured log filter is invalid, falling back to default"
            );
        }

        Ok(Self { format, filter })
    }

    fn reload(&self, resolved: &ResolvedLoggingConfig) -> Result<(), LoggingError> {
        self.filter
            .reload(build_filter(&resolved.filter))
            .map_err(|err| LoggingError::Reload(err.to_string()))?;
        self.format
            .store(format_code(resolved.format), Ordering::Release);
        Ok(())
    }
}

#[derive(Clone)]
struct ActiveFormatWriter {
    format: Arc<AtomicU8>,
    active_format: LogFormat,
}

impl ActiveFormatWriter {
    fn new(format: Arc<AtomicU8>, active_format: LogFormat) -> Self {
        Self {
            format,
            active_format,
        }
    }
}

impl<'a> MakeWriter<'a> for ActiveFormatWriter {
    type Writer = Box<dyn Write + Send + 'a>;

    fn make_writer(&'a self) -> Self::Writer {
        if self.format.load(Ordering::Acquire) == format_code(self.active_format) {
            Box::new(io::stderr())
        } else {
            Box::new(NullWriter)
        }
    }
}

struct NullWriter;

impl Write for NullWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn build_filter(spec: &str) -> EnvFilter {
    EnvFilter::try_new(spec).unwrap_or_else(|err| {
        tracing::warn!(
            spec = spec,
            error = %err,
            fallback = DEFAULT_FILTER,
            "configured log filter is invalid, falling back to default"
        );
        EnvFilter::new(DEFAULT_FILTER)
    })
}

fn format_code(format: LogFormat) -> u8 {
    match format {
        LogFormat::Text => 0,
        LogFormat::Json => 1,
    }
}

static TRACING_HANDLE: OnceLock<TracingHandle> = OnceLock::new();

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_logging_config_defaults_to_warn_text() {
        let resolved = resolve_logging_config_with_env(None, None);

        assert_eq!(resolved.format, LogFormat::Text);
        assert_eq!(resolved.filter, "warn");
    }

    #[test]
    fn resolve_logging_config_prefers_env_over_config_filter() {
        let config = LoggingConfig {
            format: LogFormat::Json,
            filter: Some("info".to_string()),
        };

        let resolved = resolve_logging_config_with_env(Some("error"), Some(&config));

        assert_eq!(resolved.format, LogFormat::Json);
        assert_eq!(resolved.filter, "error");
    }

    #[test]
    fn resolve_logging_config_ignores_empty_env_filter() {
        let config = LoggingConfig {
            format: LogFormat::Text,
            filter: Some("debug".to_string()),
        };

        let resolved = resolve_logging_config_with_env(Some(""), Some(&config));

        assert_eq!(resolved.filter, "debug");
    }

    #[test]
    fn resolve_logging_config_uses_config_filter_when_env_absent() {
        let config = LoggingConfig {
            format: LogFormat::Text,
            filter: Some("trace".to_string()),
        };

        let resolved = resolve_logging_config_with_env(None, Some(&config));

        assert_eq!(resolved.filter, "trace");
    }
}
