use std::fmt::{Display, Formatter, Result as FmtResult};

use serde::Serialize;

use crate::env_value_token_diff::EnvValueTokenDiff;

/// Rebuild reasons parsed from Cargo's fingerprint log output.
///
/// This enum represents the different reasons why Cargo rebuilds a crate,
/// as reported in `CARGO_LOG=cargo::core::compiler::fingerprint=info` output.
///
/// Based on Cargo's internal `DirtyReason` enum:
/// <https://doc.rust-lang.org/stable/nightly-rustc/src/cargo/core/compiler/fingerprint/dirty_reason.rs.html>
///
/// Note: This is not using Cargo's internal types directly for stability
/// reasons. The variants are based on the string format in Cargo's log output.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[non_exhaustive]
pub enum RebuildReason {
    EnvVarChanged {
        name: String,
        old_value: Option<String>,
        new_value: Option<String>,
    },
    UnitDependencyInfoChanged {
        name: String,
        old_fingerprint: String,
        new_fingerprint: String,
        context: Option<DependencyChangeContext>,
    },
    RustflagsChanged {
        old: Vec<String>,
        new: Vec<String>,
    },
    FeaturesChanged {
        old: String,
        new: String,
    },
    ProfileConfigurationChanged,
    TargetConfigurationChanged,
    FileChanged {
        path: String,
    },
    Unknown(String),
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DependencyChangeContext {
    pub package_id: Option<String>,
    pub target_type: Option<String>,
    pub root_cause: Option<String>,
}

impl Display for RebuildReason {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        match self {
            Self::EnvVarChanged {
                name,
                old_value,
                new_value,
            } => match EnvValueTokenDiff::between(old_value.as_deref(), new_value.as_deref()) {
                Some(token_diff) => write!(f, "env:{name} ({token_diff})"),
                None => write!(f, "env:{name} (changed)"),
            },
            Self::UnitDependencyInfoChanged { name, .. } => write!(f, "dep:{name}"),
            Self::RustflagsChanged { .. } => write!(f, "rustflags changed"),
            Self::FeaturesChanged { old, new } => write!(f, "features: {old} -> {new}"),
            Self::ProfileConfigurationChanged => write!(f, "profile changed"),
            Self::TargetConfigurationChanged => write!(f, "target config changed"),
            Self::FileChanged { path } => {
                let short_path = path
                    .split('/')
                    .rev()
                    .take(2)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect::<Vec<_>>()
                    .join("/");
                write!(f, "file:{short_path}")
            }
            Self::Unknown(msg) => write!(f, "unknown:{msg}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn displays_environment_variable_changes() {
        let env_change = RebuildReason::EnvVarChanged {
            name: "CC".to_string(),
            old_value: Some("gcc".to_string()),
            new_value: None,
        };

        assert!(env_change.to_string().contains("env:CC"));
        assert!(env_change.to_string().contains("gcc -> unset"));

        let dep_change = RebuildReason::UnitDependencyInfoChanged {
            name: "rusqlite".to_string(),
            old_fingerprint: "123".to_string(),
            new_fingerprint: "456".to_string(),
            context: None,
        };

        assert!(dep_change.to_string().contains("dep:rusqlite"));

        let target_change = RebuildReason::TargetConfigurationChanged;
        assert!(target_change.to_string().contains("target config changed"));
    }

    #[test]
    fn displays_features_and_profile_changes() {
        let features_change = RebuildReason::FeaturesChanged {
            old: "default".to_string(),
            new: "default,serde".to_string(),
        };

        assert!(features_change.to_string().contains("features"));
        assert!(features_change.to_string().contains("default"));
        assert!(features_change.to_string().contains("serde"));

        let profile_change = RebuildReason::ProfileConfigurationChanged;
        assert!(profile_change.to_string().contains("profile changed"));
    }

    #[test]
    fn displays_dependency_name() {
        let dep_with_context = RebuildReason::UnitDependencyInfoChanged {
            name: "libz-sys".to_string(),
            old_fingerprint: "123".to_string(),
            new_fingerprint: "456".to_string(),
            context: Some(DependencyChangeContext {
                package_id: Some("libz-sys v1.1.23".to_string()),
                target_type: Some("build-script-build".to_string()),
                root_cause: Some("CC environment variable changed".to_string()),
            }),
        };

        assert!(dep_with_context.to_string().contains("dep:libz-sys"));
    }

    #[test]
    fn displays_rustflags_changed() {
        let rustflags_change = RebuildReason::RustflagsChanged {
            old: vec!["--cfg".to_string(), "test".to_string()],
            new: vec![
                "--cfg".to_string(),
                "test".to_string(),
                "-C".to_string(),
                "target-cpu=native".to_string(),
            ],
        };

        assert!(rustflags_change.to_string().contains("rustflags changed"));
    }
}
