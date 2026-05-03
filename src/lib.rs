use core::error::Error;
use std::{fmt, io, path::PathBuf};

mod build_script_watches;
mod dirty_analyzer;
mod fingerprint_parser;
mod rebuild_graph;
mod rebuild_reason;
mod report;

pub use dirty_analyzer::Config;

#[derive(Debug)]
pub enum AnalyzerError {
    CargoTomlNotFound(PathBuf),
    EmptyCommand,
    Io(io::Error),
    Json(serde_json::Error),
    BuildScriptOutputUnreadable(PathBuf, io::Error),
    CargoMetadataFailed(String),
}

impl fmt::Display for AnalyzerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CargoTomlNotFound(path) => {
                write!(f, "Cargo.toml not found at {}", path.display())
            }
            Self::EmptyCommand => write!(f, "empty cargo command"),
            Self::Io(e) => write!(f, "IO error: {e}"),
            Self::Json(e) => write!(f, "JSON error: {e}"),
            Self::BuildScriptOutputUnreadable(path, e) => {
                write!(
                    f,
                    "could not read build-script output at {}: {e}",
                    path.display()
                )
            }
            Self::CargoMetadataFailed(msg) => write!(f, "cargo metadata failed: {msg}"),
        }
    }
}

impl Error for AnalyzerError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(e) | Self::BuildScriptOutputUnreadable(_, e) => Some(e),
            Self::Json(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for AnalyzerError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<serde_json::Error> for AnalyzerError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}
