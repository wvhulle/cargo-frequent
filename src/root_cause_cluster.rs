//! Post-processing pass over a `RebuildGraph` that groups root-cause nodes
//! sharing an underlying trigger, and cross-references env-var triggers
//! against build-script `cargo:rerun-if-env-changed` directives so the
//! report can attribute "PATH changed → watched by pyo3-build-config".

use std::{
    collections::{HashMap, HashSet},
    fmt::{Display, Formatter, Result as FmtResult},
};

use serde::Serialize;

use crate::{
    build_script_rerun_directives::{self, RerunDirectiveMap},
    rebuild_graph::{PackageTarget, RebuildGraph, RebuildNode},
    rebuild_reason::RebuildReason,
};

/// What ultimately caused a cluster of rebuilds. Two roots with the same
/// trigger collapse into one cluster.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub enum ClusterTrigger {
    EnvVar(String),
    File(String),
    Rustflags,
    Features,
    Profile,
    TargetConfig,
    Unknown(String),
}

impl ClusterTrigger {
    fn from_reason(reason: &RebuildReason) -> Self {
        match reason {
            RebuildReason::EnvVarChanged { name, .. } => Self::EnvVar(name.clone()),
            RebuildReason::FileChanged { path } => Self::File(path.clone()),
            RebuildReason::RustflagsChanged { .. } => Self::Rustflags,
            RebuildReason::FeaturesChanged { .. } => Self::Features,
            RebuildReason::ProfileConfigurationChanged => Self::Profile,
            RebuildReason::TargetConfigurationChanged => Self::TargetConfig,
            RebuildReason::Unknown(msg) => Self::Unknown(msg.clone()),
            RebuildReason::UnitDependencyInfoChanged { .. } => {
                Self::Unknown("dependency change".to_string())
            }
        }
    }
}

impl Display for ClusterTrigger {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        match self {
            Self::EnvVar(name) => write!(f, "env:{name}"),
            Self::File(path) => {
                let mut tail = path.rsplit('/').take(2).collect::<Vec<_>>();
                tail.reverse();
                write!(f, "file:{}", tail.join("/"))
            }
            Self::Rustflags => write!(f, "rustflags"),
            Self::Features => write!(f, "features"),
            Self::Profile => write!(f, "profile"),
            Self::TargetConfig => write!(f, "target config"),
            Self::Unknown(msg) => write!(f, "{msg}"),
        }
    }
}

/// One row in the clustered report: a trigger, the source packages whose
/// build scripts (or units) fired it, and the union of every package
/// transitively rebuilt as a result.
#[derive(Debug, Clone, Serialize)]
pub struct RootCauseCluster {
    pub trigger: ClusterTrigger,
    pub source_packages: Vec<PackageTarget>,
    pub affected_packages: Vec<RebuildNode>,
    /// Crates whose most recent build-script output declared
    /// `cargo:rerun-if-env-changed=<this var>`. Empty for non-env triggers.
    pub watched_by: Vec<String>,
    /// True when the env var is on the curated volatile list (PATH, HOME, …).
    pub volatile: bool,
}

#[derive(Default)]
struct ClusterAccumulator {
    source_packages: Vec<PackageTarget>,
    source_indices: Vec<usize>,
}

/// Group root causes that share an underlying trigger into clusters.
#[must_use]
pub fn cluster(graph: &RebuildGraph, directives: &RerunDirectiveMap) -> Vec<RootCauseCluster> {
    graph
        .nodes()
        .iter()
        .enumerate()
        .filter(|(_, node)| node.is_root_cause())
        .fold(
            HashMap::<ClusterTrigger, ClusterAccumulator>::new(),
            |mut by_trigger, (idx, node)| {
                let entry = by_trigger
                    .entry(ClusterTrigger::from_reason(&node.reason))
                    .or_default();
                entry.source_packages.push(node.package.clone());
                entry.source_indices.push(idx);
                by_trigger
            },
        )
        .into_iter()
        .map(|(trigger, acc)| RootCauseCluster {
            watched_by: match &trigger {
                ClusterTrigger::EnvVar(name) => crates_watching_env(directives, name),
                _ => Vec::new(),
            },
            volatile: matches!(
                &trigger,
                ClusterTrigger::EnvVar(name) if build_script_rerun_directives::is_volatile(name)
            ),
            affected_packages: union_affected(graph, &acc.source_indices),
            source_packages: acc.source_packages,
            trigger,
        })
        .collect()
}

/// Serialize clusters (with build-script attribution) to JSON.
pub fn cluster_to_json(
    graph: &RebuildGraph,
    directives: &RerunDirectiveMap,
) -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(&cluster(graph, directives))
}

fn union_affected(graph: &RebuildGraph, source_indices: &[usize]) -> Vec<RebuildNode> {
    let mut seen = HashSet::new();
    source_indices
        .iter()
        .flat_map(|&idx| graph.find_affected_packages(idx))
        .filter(|node| seen.insert((node.package.clone(), node.reason.to_string())))
        .collect()
}

fn crates_watching_env(directives: &RerunDirectiveMap, var_name: &str) -> Vec<String> {
    directives
        .iter()
        .filter(|(_, w)| w.env_vars.iter().any(|v| v == var_name))
        .map(|(name, _)| name.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build_script_rerun_directives::BuildScriptRerunDirectives;

    #[test]
    fn collapses_same_env_var_into_one_cluster() {
        let mut graph = RebuildGraph::new();

        graph.add_node(RebuildNode::new(
            PackageTarget::new(
                "pyo3-build-config v0.21.0",
                Some("build-script-build".to_string()),
            ),
            RebuildReason::EnvVarChanged {
                name: "PATH".to_string(),
                old_value: Some("/a".to_string()),
                new_value: Some("/b".to_string()),
            },
        ));
        graph.add_node(RebuildNode::new(
            PackageTarget::new("numpy v0.21.0", None),
            RebuildReason::EnvVarChanged {
                name: "PATH".to_string(),
                old_value: Some("/a".to_string()),
                new_value: Some("/b".to_string()),
            },
        ));
        graph.add_node(RebuildNode::new(
            PackageTarget::new("python_utils v0.1.0", None),
            RebuildReason::UnitDependencyInfoChanged {
                name: "numpy".to_string(),
                old_fingerprint: "1".to_string(),
                new_fingerprint: "2".to_string(),
                context: None,
            },
        ));

        let mut directives = RerunDirectiveMap::new();
        directives.insert(
            "pyo3-build-config".to_string(),
            BuildScriptRerunDirectives {
                env_vars: vec!["PATH".to_string()],
                paths: vec![],
            },
        );

        let clusters = cluster(&graph, &directives);
        assert_eq!(clusters.len(), 1, "two PATH events should collapse");
        let c = &clusters[0];
        assert!(matches!(&c.trigger, ClusterTrigger::EnvVar(name) if name == "PATH"));
        assert!(c.volatile, "PATH must be flagged volatile");
        assert_eq!(c.watched_by, vec!["pyo3-build-config".to_string()]);
        assert_eq!(c.source_packages.len(), 2);
        assert_eq!(c.affected_packages.len(), 1);
    }

    #[test]
    fn separate_triggers_remain_separate_clusters() {
        let mut graph = RebuildGraph::new();
        graph.add_node(RebuildNode::new(
            PackageTarget::new("a v1.0.0", None),
            RebuildReason::EnvVarChanged {
                name: "PATH".to_string(),
                old_value: None,
                new_value: Some("x".to_string()),
            },
        ));
        graph.add_node(RebuildNode::new(
            PackageTarget::new("b v1.0.0", None),
            RebuildReason::FileChanged {
                path: "src/lib.rs".to_string(),
            },
        ));

        let clusters = cluster(&graph, &RerunDirectiveMap::new());
        assert_eq!(clusters.len(), 2);
    }

    #[test]
    fn builds_and_analyzes_rebuild_graph() {
        let mut graph = RebuildGraph::new();

        graph.add_node(RebuildNode::new(
            PackageTarget::new("libz-sys v1.1.23", None),
            RebuildReason::EnvVarChanged {
                name: "CC".to_string(),
                old_value: Some("gcc".to_string()),
                new_value: None,
            },
        ));

        graph.add_node(RebuildNode::new(
            PackageTarget::new("rusqlite v0.31.0", None),
            RebuildReason::UnitDependencyInfoChanged {
                name: "libz-sys".to_string(),
                old_fingerprint: "123".to_string(),
                new_fingerprint: "456".to_string(),
                context: None,
            },
        ));

        let clusters = cluster(&graph, &RerunDirectiveMap::new());
        assert_eq!(clusters.len(), 1);
        assert!(matches!(&clusters[0].trigger, ClusterTrigger::EnvVar(name) if name == "CC"));
        assert_eq!(clusters[0].source_packages.len(), 1);
        assert_eq!(clusters[0].affected_packages.len(), 1);
    }
}
