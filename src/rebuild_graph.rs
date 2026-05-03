//! Rebuild causality graph for tracking root causes of rebuilds
//!
//! Cargo's rebuild triggers form a directed acyclic graph where:
//! - Root causes are nodes with no incoming edges (file changes, env var
//!   changes)
//! - `UnitDependencyInfoChanged` creates edges between dependent packages
//! - Finding root causes means traversing back to nodes with in-degree 0

use std::{
    collections::{HashMap, HashSet},
    fmt::{Display, Formatter, Result as FmtResult},
};

use serde::Serialize;

use crate::{
    build_script_watches::{self, WatchMap},
    rebuild_reason::RebuildReason,
};

/// Identifies a compilation unit in the rebuild graph
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct PackageTarget {
    pub package_id: String,
    pub target: Option<String>,
}

impl PackageTarget {
    pub fn new(package_id: impl Into<String>, target: Option<String>) -> Self {
        Self {
            package_id: package_id.into(),
            target,
        }
    }
}

impl Display for PackageTarget {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        let package_name = self
            .package_id
            .split_whitespace()
            .next()
            .unwrap_or(&self.package_id);

        match &self.target {
            Some(target) => write!(f, "{package_name} [{target}]"),
            None => write!(f, "{package_name}"),
        }
    }
}

/// A node in the rebuild graph: a package with its direct rebuild reason
#[derive(Debug, Clone, Serialize)]
pub struct RebuildNode {
    pub package: PackageTarget,
    pub reason: RebuildReason,
}

impl RebuildNode {
    #[must_use]
    pub const fn new(package: PackageTarget, reason: RebuildReason) -> Self {
        Self { package, reason }
    }

    /// Returns true if this is a root cause (not caused by another package
    /// rebuild)
    #[must_use]
    pub const fn is_root_cause(&self) -> bool {
        !matches!(self.reason, RebuildReason::UnitDependencyInfoChanged { .. })
    }
}

/// Directed graph of rebuild causality
///
/// Edges point from cause to effect:
/// - Package A (root cause) -> Package B (depends on A)
/// - An edge exists when Package B's rebuild reason is
///   `UnitDependencyInfoChanged` mentioning A
#[derive(Debug, Default)]
pub struct RebuildGraph {
    nodes: Vec<RebuildNode>,
    /// Map from dependency name to indices of nodes that caused its rebuild
    dependency_causes: HashMap<String, Vec<usize>>,
    /// Map from package to its node index
    package_to_node: HashMap<PackageTarget, usize>,
    /// Track seen (`package_name`, `reason_key`) to deduplicate
    seen_entries: HashSet<(String, String)>,
}

impl RebuildGraph {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a rebuild node to the graph, deduplicating by package name and
    /// reason
    pub fn add_node(&mut self, node: RebuildNode) -> Option<usize> {
        let package_name = extract_package_name(&node.package.package_id);
        let reason_key = node.reason.to_string();
        let entry_key = (package_name.clone(), reason_key);

        if !self.seen_entries.insert(entry_key) {
            return None;
        }

        let idx = self.nodes.len();
        self.package_to_node.insert(node.package.clone(), idx);

        // If this is a root cause, record it as a potential cause for dependencies
        if node.is_root_cause() {
            self.dependency_causes
                .entry(package_name)
                .or_default()
                .push(idx);
        }

        self.nodes.push(node);
        Some(idx)
    }

    /// Find all packages affected by a root cause (BFS traversal)
    fn find_affected_packages(&self, root_idx: usize) -> Vec<RebuildNode> {
        let root_name = extract_package_name(&self.nodes[root_idx].package.package_id);
        let root_name_normalized = normalize_crate_name(&root_name);
        let mut affected = Vec::new();
        let mut visited = HashSet::new();
        visited.insert(root_idx);

        // Find nodes that were rebuilt because of this root cause
        for (idx, node) in self.nodes.iter().enumerate() {
            if visited.contains(&idx) {
                continue;
            }

            if let RebuildReason::UnitDependencyInfoChanged { name, .. } = &node.reason {
                let dep_name_normalized = normalize_crate_name(name);
                let is_affected = dep_name_normalized == root_name_normalized
                    || self.is_transitively_affected(name, &root_name);

                if is_affected {
                    affected.push(node.clone());
                    visited.insert(idx);
                }
            }
        }

        affected
    }

    /// Check if a dependency was transitively affected by a root cause
    fn is_transitively_affected(&self, dep_name: &str, root_name: &str) -> bool {
        let root_name_normalized = normalize_crate_name(root_name);
        // Check if dep_name was rebuilt because of root_name through the chain
        for node in &self.nodes {
            let package_name = extract_package_name(&node.package.package_id);
            let package_name_normalized = normalize_crate_name(&package_name);
            let dep_name_normalized = normalize_crate_name(dep_name);

            if package_name_normalized != dep_name_normalized {
                continue;
            }

            if let RebuildReason::UnitDependencyInfoChanged { name, .. } = &node.reason {
                let name_normalized = normalize_crate_name(name);
                if name_normalized == root_name_normalized {
                    return true;
                }
                if self.is_transitively_affected(name, root_name) {
                    return true;
                }
            }
        }
        false
    }

    /// Serialize clusters (with build-script watch attribution) to JSON.
    pub fn clusters_to_json(&self, watches: &WatchMap) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(&self.clusters(watches))
    }

    /// Group root causes that share an underlying trigger into clusters.
    ///
    /// Two `EnvVarChanged` roots naming the same variable collapse into one
    /// cluster; same for any other reason variant whose `ClusterTrigger`
    /// matches. The cluster's `affected_packages` is the union of every
    /// member's downstream impact.
    ///
    /// When a `WatchMap` is supplied, env-var clusters are cross-referenced
    /// against build-script watch directives so the report can attribute
    /// "PATH changed → watched by pyo3-build-config".
    #[must_use]
    pub fn clusters(&self, watches: &WatchMap) -> Vec<RootCauseCluster> {
        let mut by_trigger: HashMap<ClusterTrigger, ClusterAccumulator> = HashMap::new();

        for (idx, node) in self.nodes.iter().enumerate() {
            if !node.is_root_cause() {
                continue;
            }
            let trigger = ClusterTrigger::from_reason(&node.reason);
            let entry = by_trigger.entry(trigger).or_default();
            entry.source_packages.push(node.package.clone());
            entry.source_indices.push(idx);
        }

        by_trigger
            .into_iter()
            .map(|(trigger, acc)| {
                let affected = self.union_affected(&acc.source_indices);
                let watched_by = match &trigger {
                    ClusterTrigger::EnvVar(name) => crates_watching_env(watches, name),
                    _ => Vec::new(),
                };
                let volatile = matches!(
                    &trigger,
                    ClusterTrigger::EnvVar(name) if build_script_watches::is_volatile(name)
                );
                RootCauseCluster {
                    trigger,
                    source_packages: acc.source_packages,
                    affected_packages: affected,
                    watched_by,
                    volatile,
                }
            })
            .collect()
    }

    fn union_affected(&self, source_indices: &[usize]) -> Vec<RebuildNode> {
        let mut seen_keys = HashSet::new();
        let mut out = Vec::new();
        for &idx in source_indices {
            for node in self.find_affected_packages(idx) {
                let key = (node.package.clone(), node.reason.to_string());
                if seen_keys.insert(key) {
                    out.push(node);
                }
            }
        }
        out
    }
}

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

fn crates_watching_env(watches: &WatchMap, var_name: &str) -> Vec<String> {
    watches
        .iter()
        .filter(|(_, w)| w.env_vars.iter().any(|v| v == var_name))
        .map(|(name, _)| name.clone())
        .collect()
}

/// Extract just the package name from a `package_id` like "libz-sys v1.1.23"
fn extract_package_name(package_id: &str) -> String {
    package_id
        .split_whitespace()
        .next()
        .unwrap_or(package_id)
        .to_string()
}

/// Normalize a crate name for comparison (hyphens and underscores are
/// equivalent)
fn normalize_crate_name(name: &str) -> String {
    name.replace('-', "_")
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::Path,
        process::{Command, Stdio},
    };

    use assert_cmd::prelude::*;
    use tempfile::TempDir;

    use super::*;
    use crate::fingerprint_parser::parse_rebuild_entry;

    #[test]
    fn collapses_same_env_var_into_one_cluster() {
        let mut graph = RebuildGraph::new();

        graph.add_node(RebuildNode::new(
            PackageTarget::new("pyo3-build-config v0.21.0", Some("build-script-build".to_string())),
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

        let mut watches = WatchMap::new();
        watches.insert(
            "pyo3-build-config".to_string(),
            build_script_watches::BuildScriptWatches {
                env_vars: vec!["PATH".to_string()],
                paths: vec![],
            },
        );

        let clusters = graph.clusters(&watches);
        assert_eq!(clusters.len(), 1, "two PATH events should collapse");
        let cluster = &clusters[0];
        assert!(matches!(&cluster.trigger, ClusterTrigger::EnvVar(name) if name == "PATH"));
        assert!(cluster.volatile, "PATH must be flagged volatile");
        assert_eq!(cluster.watched_by, vec!["pyo3-build-config".to_string()]);
        assert_eq!(cluster.source_packages.len(), 2);
        assert_eq!(cluster.affected_packages.len(), 1);
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

        let clusters = graph.clusters(&WatchMap::new());
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

        let clusters = graph.clusters(&WatchMap::new());
        assert_eq!(clusters.len(), 1);
        assert!(matches!(&clusters[0].trigger, ClusterTrigger::EnvVar(name) if name == "CC"));
        assert_eq!(clusters[0].source_packages.len(), 1);
        assert_eq!(clusters[0].affected_packages.len(), 1);
    }

    fn create_workspace_with_dependencies() -> TempDir {
        let temp_dir = TempDir::new().unwrap();

        fs::write(
            temp_dir.path().join("Cargo.toml"),
            r#"
[workspace]
members = ["lib-a", "lib-b", "app"]
resolver = "2"
"#,
        )
        .unwrap();

        let lib_a_dir = temp_dir.path().join("lib-a");
        fs::create_dir_all(lib_a_dir.join("src")).unwrap();
        fs::write(
            lib_a_dir.join("Cargo.toml"),
            r#"
[package]
name = "lib-a"
version = "0.1.0"
edition = "2021"
"#,
        )
        .unwrap();
        fs::write(
            lib_a_dir.join("src/lib.rs"),
            r#"
pub fn greet() -> &'static str {
    "Hello from lib-a"
}
"#,
        )
        .unwrap();

        let middle_lib_dir = temp_dir.path().join("lib-b");
        fs::create_dir_all(middle_lib_dir.join("src")).unwrap();
        fs::write(
            middle_lib_dir.join("Cargo.toml"),
            r#"
[package]
name = "lib-b"
version = "0.1.0"
edition = "2021"

[dependencies]
lib-a = { path = "../lib-a" }
"#,
        )
        .unwrap();
        fs::write(
            middle_lib_dir.join("src/lib.rs"),
            r#"
pub fn message() -> String {
    format!("lib-b says: {}", lib_a::greet())
}
"#,
        )
        .unwrap();

        let app_dir = temp_dir.path().join("app");
        fs::create_dir_all(app_dir.join("src")).unwrap();
        fs::write(
            app_dir.join("Cargo.toml"),
            r#"
[package]
name = "app"
version = "0.1.0"
edition = "2021"

[dependencies]
lib-b = { path = "../lib-b" }
"#,
        )
        .unwrap();
        fs::write(
            app_dir.join("src/main.rs"),
            r#"
fn main() {
    println!("{}", lib_b::message());
}
"#,
        )
        .unwrap();

        temp_dir
    }

    fn collect_cargo_fingerprint_logs(project_path: &Path) -> Vec<String> {
        let output = Command::new("cargo")
            .arg("build")
            .current_dir(project_path)
            .env("CARGO_LOG", "cargo::core::compiler::fingerprint=info")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .expect("Failed to run cargo build");

        let stderr = String::from_utf8_lossy(&output.stderr);
        stderr
            .lines()
            .filter(|line| {
                line.contains("fingerprint") && (line.contains("dirty:") || line.contains("stale:"))
            })
            .map(String::from)
            .collect()
    }

    fn build_graph_from_logs(log_lines: &[String]) -> RebuildGraph {
        let mut graph = RebuildGraph::new();
        for line in log_lines {
            if let Some(entry) = parse_rebuild_entry(line) {
                graph.add_node(RebuildNode::new(entry.package, entry.reason));
            }
        }
        graph
    }

    #[test]
    fn json_structure_is_valid_for_workspace_rebuild() {
        let workspace = create_workspace_with_dependencies();

        let mut build_cmd = Command::new("cargo");
        build_cmd.arg("build").current_dir(workspace.path());
        build_cmd.assert().success();

        let lib_a_src = workspace.path().join("lib-a/src/lib.rs");
        fs::write(
            &lib_a_src,
            r#"
pub fn greet() -> &'static str {
    "Hello from modified lib-a!"
}
"#,
        )
        .unwrap();

        let log_lines = collect_cargo_fingerprint_logs(workspace.path());
        let graph = build_graph_from_logs(&log_lines);

        let json = graph
            .clusters_to_json(&WatchMap::new())
            .expect("JSON serialization should succeed");
        let parsed: serde_json::Value =
            serde_json::from_str(&json).expect("JSON should be valid and parseable");

        let cluster_array = parsed.as_array().expect("JSON should be an array");
        assert!(
            !cluster_array.is_empty(),
            "should have at least one cluster"
        );

        for cluster in cluster_array {
            assert!(cluster.get("trigger").is_some(), "trigger present");
            let affected = cluster
                .get("affected_packages")
                .and_then(serde_json::Value::as_array)
                .expect("affected_packages array");
            for pkg in affected {
                let pkg_reason = &pkg["reason"];
                assert!(
                    pkg_reason.get("UnitDependencyInfoChanged").is_some(),
                    "affected entries are dependency changes: {pkg_reason}"
                );
            }
        }
    }
}
