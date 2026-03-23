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

#[derive(Debug)]
pub enum LoggingError {
    SetGlobalDefault(String),
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

impl std::error::Error for LoggingError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            LoggingError::SetGlobalDefault(_) => None,
            LoggingError::Reload(_) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedLoggingConfig {
    pub format: LogFormat,
    pub filter: String,
}

pub fn resolve_logging_config(config: Option<&LoggingConfig>) -> ResolvedLoggingConfig {
    let filter = std::env::var("RUST_LOG")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| config.and_then(|logging| logging.filter.clone()))
        .unwrap_or_else(|| DEFAULT_FILTER.to_string());

    ResolvedLoggingConfig {
        format: config
            .map(|logging| logging.format)
            .unwrap_or(LogFormat::Text),
        filter,
    }
}

pub fn configure_tracing(config: Option<&LoggingConfig>) -> Result<(), LoggingError> {
    let resolved = resolve_logging_config(config);
    if let Some(handle) = TRACING_HANDLE.get() {
        return handle.reload(&resolved);
    }

    let handle = TracingHandle::new(&resolved)?;
    match TRACING_HANDLE.set(handle) {
        Ok(()) => TRACING_HANDLE
            .get()
            .expect("tracing handle set")
            .reload(&resolved),
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

        let (filter_layer, filter) = reload::Layer::new(build_filter(&resolved.filter));

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

        Ok(Self { format, filter })
    }

    fn reload(&self, resolved: &ResolvedLoggingConfig) -> Result<(), LoggingError> {
        self.filter
            .reload(build_filter(&resolved.filter))
            .map_err(|err| LoggingError::Reload(err.to_string()))?;
        self.format
            .store(format_code(resolved.format), Ordering::Relaxed);
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
        if self.format.load(Ordering::Relaxed) == format_code(self.active_format) {
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
    EnvFilter::try_new(spec).unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER))
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
        unsafe {
            std::env::remove_var("RUST_LOG");
        }

        let resolved = resolve_logging_config(None);

        assert_eq!(resolved.format, LogFormat::Text);
        assert_eq!(resolved.filter, "warn");
    }

    #[test]
    fn resolve_logging_config_prefers_env_over_config_filter() {
        unsafe {
            std::env::set_var("RUST_LOG", "error");
        }
        let config = LoggingConfig {
            format: LogFormat::Json,
            filter: Some("info".to_string()),
        };

        let resolved = resolve_logging_config(Some(&config));

        assert_eq!(resolved.format, LogFormat::Json);
        assert_eq!(resolved.filter, "error");
        unsafe {
            std::env::remove_var("RUST_LOG");
        }
    }
}
