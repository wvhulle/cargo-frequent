use core::error::Error;
use std::{
    fmt::{Display, Formatter, Result as FmtResult},
    io,
    path::PathBuf,
    process::ExitStatus,
};

#[derive(Debug)]
pub enum FrequentError {
    CargoTomlMissing {
        manifest_path: PathBuf,
    },
    CargoSpawnFailed {
        io_error: io::Error,
    },
    CargoMetadataExited {
        exit_status: ExitStatus,
    },
    CargoMetadataUnparsable {
        json_error: serde_json::Error,
    },
    DirectoryUnreadable {
        directory_path: PathBuf,
        io_error: io::Error,
    },
    BuildLogUnreadable {
        io_error: io::Error,
    },
    ReportSerialization {
        json_error: serde_json::Error,
    },
}

impl Display for FrequentError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        match self {
            Self::CargoTomlMissing { manifest_path } => {
                write!(
                    formatter,
                    "Cargo.toml not found at {}",
                    manifest_path.display()
                )
            }
            Self::CargoSpawnFailed { io_error } => {
                write!(formatter, "could not start cargo: {io_error}")
            }
            Self::CargoMetadataExited { exit_status } => {
                write!(formatter, "cargo metadata failed with {exit_status}")
            }
            Self::CargoMetadataUnparsable { json_error } => {
                write!(
                    formatter,
                    "cargo metadata printed unexpected JSON: {json_error}"
                )
            }
            Self::DirectoryUnreadable {
                directory_path,
                io_error,
            } => write!(
                formatter,
                "could not read {}: {io_error}",
                directory_path.display()
            ),
            Self::BuildLogUnreadable { io_error } => {
                write!(formatter, "could not read the cargo build log: {io_error}")
            }
            Self::ReportSerialization { json_error } => {
                write!(formatter, "could not serialize the report: {json_error}")
            }
        }
    }
}

impl Error for FrequentError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::CargoSpawnFailed { io_error }
            | Self::DirectoryUnreadable { io_error, .. }
            | Self::BuildLogUnreadable { io_error } => Some(io_error),
            Self::CargoMetadataUnparsable { json_error }
            | Self::ReportSerialization { json_error } => Some(json_error),
            Self::CargoTomlMissing { .. } | Self::CargoMetadataExited { .. } => None,
        }
    }
}
