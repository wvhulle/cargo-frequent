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

use crate::rebuild_reason::RebuildReason;

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

        if node.is_root_cause() {
            self.dependency_causes
                .entry(package_name)
                .or_default()
                .push(idx);
        }

        self.nodes.push(node);
        Some(idx)
    }

    pub fn nodes(&self) -> &[RebuildNode] {
        &self.nodes
    }

    /// Find all packages affected by a root cause (BFS traversal)
    pub fn find_affected_packages(&self, root_idx: usize) -> Vec<RebuildNode> {
        let root_name = extract_package_name(&self.nodes[root_idx].package.package_id);
        let root_name_normalized = normalize_crate_name(&root_name);
        self.nodes
            .iter()
            .enumerate()
            .filter(|(idx, _)| *idx != root_idx)
            .filter_map(|(_, node)| match &node.reason {
                RebuildReason::UnitDependencyInfoChanged { name, .. } => Some((node, name)),
                _ => None,
            })
            .filter(|(_, name)| {
                normalize_crate_name(name) == root_name_normalized
                    || self.is_transitively_affected(name, &root_name)
            })
            .map(|(node, _)| node.clone())
            .collect()
    }

    fn is_transitively_affected(&self, dep_name: &str, root_name: &str) -> bool {
        let root_name_normalized = normalize_crate_name(root_name);
        let dep_name_normalized = normalize_crate_name(dep_name);
        self.nodes
            .iter()
            .filter(|node| {
                normalize_crate_name(&extract_package_name(&node.package.package_id))
                    == dep_name_normalized
            })
            .filter_map(|node| match &node.reason {
                RebuildReason::UnitDependencyInfoChanged { name, .. } => Some(name),
                _ => None,
            })
            .any(|name| {
                normalize_crate_name(name) == root_name_normalized
                    || self.is_transitively_affected(name, root_name)
            })
    }
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
