//! Human-readable rendering of `RootCauseCluster`s.
//!
//! Each cluster prints as a Sugiyama-layered ASCII DAG with three tiers:
//! the trigger (e.g. `$PATH`), the source crates whose units cargo flagged
//! and/or whose build scripts watch the trigger, and the downstream crates
//! that rebuilt transitively. Edges are labeled `watched by`, `reads`, or
//! `transitive`.

use std::{
    collections::{BTreeSet, HashMap, HashSet},
    iter,
};

use ascii_dag::graph::{Graph, RenderMode};

use crate::{
    rebuild_graph::RebuildNode,
    rebuild_reason::RebuildReason,
    root_cause_cluster::{ClusterTrigger, RootCauseCluster},
};

/// Render every cluster into `out`. Clusters are separated by a blank line.
pub fn render(clusters: &[RootCauseCluster], out: &mut String) {
    if clusters.is_empty() {
        out.push_str("No rebuild triggers detected.\n");
        return;
    }

    let rendered = clusters
        .iter()
        .map(|cluster| {
            let mut buf = String::new();
            render_cluster(cluster, &mut buf);
            buf
        })
        .collect::<Vec<_>>()
        .join("\n");
    out.push_str(&rendered);
}

fn render_cluster(cluster: &RootCauseCluster, out: &mut String) {
    let source_names = unique_source_crate_names(cluster);
    let node_labels = build_node_labels(cluster, &source_names);
    let dag = build_dag(cluster, &source_names, &node_labels);
    out.push_str(&dag.render());
    out.push('\n');
}

/// Node labels indexed by DAG node id: `[trigger, source_0, ..., affected_0, ...]`.
fn build_node_labels(cluster: &RootCauseCluster, source_names: &[String]) -> Vec<String> {
    iter::once(trigger_node_label(cluster))
        .chain(source_names.iter().map(|name| source_node_label(cluster, name)))
        .chain(
            cluster
                .affected_packages
                .iter()
                .map(|n| short_package_label(&n.package.package_id)),
        )
        .collect()
}

fn unique_source_crate_names(cluster: &RootCauseCluster) -> Vec<String> {
    let from_packages = cluster
        .source_packages
        .iter()
        .map(|p| short_package_label(&p.package_id));
    let from_watched = cluster.watched_by.iter().cloned();
    from_packages
        .chain(from_watched)
        .collect::<BTreeSet<String>>()
        .into_iter()
        .collect()
}

fn trigger_node_label(cluster: &RootCauseCluster) -> String {
    let base = match &cluster.trigger {
        ClusterTrigger::EnvVar(name) => format!("${name}"),
        ClusterTrigger::File(path) => short_path(path),
        ClusterTrigger::Rustflags => "rustflags".to_string(),
        ClusterTrigger::Features => "features".to_string(),
        ClusterTrigger::Profile => "profile".to_string(),
        ClusterTrigger::TargetConfig => "target-config".to_string(),
        ClusterTrigger::Unknown(msg) => msg.clone(),
    };
    if cluster.volatile {
        format!("{base} (volatile)")
    } else {
        base
    }
}

fn source_node_label(cluster: &RootCauseCluster, name: &str) -> String {
    let is_build_script = cluster.source_packages.iter().any(|p| {
        short_package_label(&p.package_id) == name
            && p.target.as_deref() == Some("build-script-build")
    });
    if is_build_script {
        format!("{name} (build script)")
    } else {
        name.to_string()
    }
}

fn short_package_label(package_id: &str) -> String {
    package_id
        .split_whitespace()
        .next()
        .unwrap_or(package_id)
        .to_string()
}

fn short_path(path: &str) -> String {
    path.rsplit('/')
        .take(2)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("/")
}

fn normalize(name: &str) -> String {
    name.replace('-', "_")
}

fn build_dag<'a>(
    cluster: &RootCauseCluster,
    source_names: &[String],
    node_labels: &'a [String],
) -> Graph<'a> {
    let source_offset = 1;
    let affected_offset = source_offset + source_names.len();
    let mut dag = Graph::with_mode(RenderMode::Vertical);

    node_labels
        .iter()
        .enumerate()
        .for_each(|(idx, label)| dag.add_node(idx, label));

    let watched_set: HashSet<&String> = cluster.watched_by.iter().collect();
    source_names
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let edge = if watched_set.contains(name) { "watched by" } else { "reads" };
            (0_usize, source_offset + i, Some(edge))
        })
        .for_each(|(from, to, label)| dag.add_edge(from, to, label));

    let source_idx_by_normalized: HashMap<String, usize> = source_names
        .iter()
        .enumerate()
        .map(|(i, name)| (normalize(name), source_offset + i))
        .collect();
    let affected_idx_by_normalized: HashMap<String, usize> = cluster
        .affected_packages
        .iter()
        .enumerate()
        .map(|(i, n)| {
            (
                normalize(&short_package_label(&n.package.package_id)),
                affected_offset + i,
            )
        })
        .collect();

    cluster
        .affected_packages
        .iter()
        .enumerate()
        .map(|(i, affected)| {
            let to = affected_offset + i;
            let cause = cause_name(affected).and_then(|n| {
                let key = normalize(n);
                source_idx_by_normalized
                    .get(&key)
                    .or_else(|| affected_idx_by_normalized.get(&key))
                    .copied()
            });
            cause.map_or((0_usize, to, None), |from| (from, to, Some("transitive")))
        })
        .for_each(|(from, to, label)| dag.add_edge(from, to, label));

    dag
}

fn cause_name(affected: &RebuildNode) -> Option<&str> {
    match &affected.reason {
        RebuildReason::UnitDependencyInfoChanged { name, .. } => Some(name),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        rebuild_graph::{PackageTarget, RebuildNode},
        rebuild_reason::RebuildReason,
    };

    fn sample_cluster() -> RootCauseCluster {
        RootCauseCluster {
            trigger: ClusterTrigger::EnvVar("PATH".to_string()),
            source_packages: vec![PackageTarget::new(
                "pyo3-build-config v0.21.0",
                Some("build-script-build".to_string()),
            )],
            affected_packages: vec![
                RebuildNode::new(
                    PackageTarget::new("numpy v0.21.0", None),
                    RebuildReason::EnvVarChanged {
                        name: "PATH".to_string(),
                        old_value: None,
                        new_value: Some("x".to_string()),
                    },
                ),
                RebuildNode::new(
                    PackageTarget::new("python_utils v0.1.0", None),
                    RebuildReason::UnitDependencyInfoChanged {
                        name: "numpy".to_string(),
                        old_fingerprint: "1".to_string(),
                        new_fingerprint: "2".to_string(),
                        context: None,
                    },
                ),
            ],
            watched_by: vec!["pyo3-build-config".to_string()],
            volatile: true,
        }
    }

    #[test]
    fn trigger_label_uses_dollar_and_volatile() {
        let cluster = sample_cluster();
        let label = trigger_node_label(&cluster);
        assert_eq!(label, "$PATH (volatile)");
    }

    #[test]
    fn source_label_marks_build_scripts() {
        let cluster = sample_cluster();
        assert_eq!(
            source_node_label(&cluster, "pyo3-build-config"),
            "pyo3-build-config (build script)"
        );
    }

    #[test]
    fn render_emits_three_tiers_with_edge_labels() {
        let mut out = String::new();
        render(&[sample_cluster()], &mut out);
        assert!(out.contains("$PATH"), "trigger node: {out}");
        assert!(out.contains("(volatile)"), "volatile annotation: {out}");
        assert!(
            out.contains("pyo3-build-config (build script)"),
            "source-tier build-script node: {out}"
        );
        assert!(out.contains("watched by"), "edge label: {out}");
        assert!(out.contains("transitive"), "edge label: {out}");
        assert!(out.contains("numpy"), "affected: {out}");
        assert!(out.contains("python_utils"), "affected: {out}");
    }

    #[test]
    fn empty_clusters_message() {
        let mut out = String::new();
        render(&[], &mut out);
        assert!(out.contains("No rebuild triggers detected"));
    }

    #[test]
    fn trigger_renders_above_downstream_packages() {
        let cluster = sample_cluster();
        let mut out = String::new();
        render_cluster(&cluster, &mut out);
        let path_pos = out.find("$PATH").expect("PATH node");
        let utils_pos = out.find("python_utils").expect("python_utils node");
        assert!(
            path_pos < utils_pos,
            "trigger should appear above downstream package: {out}"
        );
    }
}
