use std::collections::HashSet;
use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::project::{RUNA_DIR, SIGNALS_FILENAME, STATE_FILENAME};

const SIGNAL_NAME_PATTERN: &str = "[a-z0-9][a-z0-9_-]*";

#[derive(Debug)]
pub enum SignalError {
    NotInitialized,
    InvalidSignalName(String),
    Io(std::io::Error),
    Parse(String),
    Json(serde_json::Error),
}

impl fmt::Display for SignalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SignalError::NotInitialized => {
                write!(f, "not a runa project (run 'runa init' first)")
            }
            SignalError::InvalidSignalName(name) => write!(
                f,
                "invalid signal name '{name}': expected pattern {SIGNAL_NAME_PATTERN}"
            ),
            SignalError::Io(err) => write!(f, "{err}"),
            SignalError::Parse(detail) => {
                write!(f, "failed to parse .runa/signals.json: {detail}")
            }
            SignalError::Json(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for SignalError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SignalError::Io(err) => Some(err),
            SignalError::Json(err) => Some(err),
            SignalError::NotInitialized
            | SignalError::InvalidSignalName(_)
            | SignalError::Parse(_) => None,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct SignalsFile {
    active: Vec<String>,
}

pub fn begin(working_dir: &Path, signal_name: &str) -> Result<(), SignalError> {
    validate_signal_name(signal_name)?;

    let runa_dir = ensure_initialized(working_dir)?;
    let mut active = read_signals(&runa_dir)?;
    active.insert(signal_name.to_string());
    write_signals(&runa_dir, &active)?;

    println!("Signal '{signal_name}' is active.");
    Ok(())
}

pub fn clear(working_dir: &Path, signal_name: &str) -> Result<(), SignalError> {
    validate_signal_name(signal_name)?;

    let runa_dir = ensure_initialized(working_dir)?;
    let mut active = read_signals(&runa_dir)?;
    active.remove(signal_name);
    write_signals(&runa_dir, &active)?;

    println!("Signal '{signal_name}' is inactive.");
    Ok(())
}

pub fn list(working_dir: &Path) -> Result<(), SignalError> {
    let runa_dir = ensure_initialized(working_dir)?;
    let active = sorted_signals(&read_signals(&runa_dir)?);

    if active.is_empty() {
        println!("No active signals.");
    } else {
        for signal in active {
            println!("{signal}");
        }
    }

    Ok(())
}

fn ensure_initialized(working_dir: &Path) -> Result<std::path::PathBuf, SignalError> {
    let runa_dir = working_dir.join(RUNA_DIR);
    let state_path = runa_dir.join(STATE_FILENAME);
    match std::fs::metadata(&state_path) {
        Ok(metadata) if metadata.is_file() => Ok(runa_dir),
        Ok(_) => Err(SignalError::NotInitialized),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Err(SignalError::NotInitialized),
        Err(err) => Err(SignalError::Io(err)),
    }
}

fn read_signals(runa_dir: &Path) -> Result<HashSet<String>, SignalError> {
    let path = runa_dir.join(SIGNALS_FILENAME);
    let content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(HashSet::new()),
        Err(err) => return Err(SignalError::Io(err)),
    };

    let parsed: SignalsFile =
        serde_json::from_str(&content).map_err(|err| SignalError::Parse(err.to_string()))?;
    Ok(parsed.active.into_iter().collect())
}

fn write_signals(runa_dir: &Path, active: &HashSet<String>) -> Result<(), SignalError> {
    let path = runa_dir.join(SIGNALS_FILENAME);
    let payload = SignalsFile {
        active: sorted_signals(active),
    };
    let content = serde_json::to_string_pretty(&payload).map_err(SignalError::Json)?;
    std::fs::write(path, content).map_err(SignalError::Io)
}

fn sorted_signals(active: &HashSet<String>) -> Vec<String> {
    let mut signals: Vec<String> = active.iter().cloned().collect();
    signals.sort();
    signals
}

fn validate_signal_name(signal_name: &str) -> Result<(), SignalError> {
    if !libagent::is_valid_signal_name(signal_name) {
        return Err(SignalError::InvalidSignalName(signal_name.to_string()));
    }

    Ok(())
}
