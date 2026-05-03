//! Human-readable rendering of `RootCauseCluster`s.
//!
//! Each cluster prints as a header line (trigger + flags + watched-by
//! attribution) followed by a Sugiyama-layered ASCII DAG built with
//! `ascii-dag` showing the trigger at the top fanning out to every package
//! it caused to rebuild.

use std::fmt::Write;

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
    let labels = build_labels(cluster);
    let dag = build_dag(cluster, &labels);
    out.push_str(&dag.render());
    out.push('\n');
}

struct Labels {
    trigger: String,
    nodes: Vec<String>,
}

fn build_labels(cluster: &RootCauseCluster) -> Labels {
    Labels {
        trigger: trigger_node_label(cluster),
        nodes: cluster
            .affected_packages
            .iter()
            .map(|n| short_package_label(&n.package.package_id))
            .collect(),
    }
}

fn trigger_node_label(cluster: &RootCauseCluster) -> String {
    let mut label = match &cluster.trigger {
        ClusterTrigger::EnvVar(name) => format!("env:{name}"),
        ClusterTrigger::File(path) => format!("file:{}", short_path(path)),
        ClusterTrigger::Rustflags => "rustflags".to_string(),
        ClusterTrigger::Features => "features".to_string(),
        ClusterTrigger::Profile => "profile".to_string(),
        ClusterTrigger::TargetConfig => "target-config".to_string(),
        ClusterTrigger::Unknown(msg) => msg.clone(),
    };
    if cluster.volatile {
        label.push_str(" [volatile]");
    }
    if !cluster.watched_by.is_empty() {
        let _ = write!(label, " watched-by:{}", cluster.watched_by.join(","));
    }
    label
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

fn build_dag<'a>(cluster: &RootCauseCluster, labels: &'a Labels) -> Graph<'a> {
    let mut dag = Graph::with_mode(RenderMode::Vertical);
    dag.add_node(0, &labels.trigger);

    labels
        .nodes
        .iter()
        .enumerate()
        .for_each(|(idx, label)| dag.add_node(idx + 1, label));

    cluster
        .affected_packages
        .iter()
        .enumerate()
        .flat_map(|(idx, affected)| {
            let to = idx + 1;
            let causes = cause_indices_within_cluster(cluster, affected);
            if causes.is_empty() {
                vec![(0, to)]
            } else {
                causes.into_iter().map(|from| (from + 1, to)).collect()
            }
        })
        .for_each(|(from, to)| dag.add_edge(from, to, None));

    dag
}

fn cause_indices_within_cluster(
    cluster: &RootCauseCluster,
    affected: &RebuildNode,
) -> Vec<usize> {
    let RebuildReason::UnitDependencyInfoChanged { name: cause_name, .. } = &affected.reason
    else {
        return Vec::new();
    };
    let normalized_cause = cause_name.replace('-', "_");

    cluster
        .affected_packages
        .iter()
        .enumerate()
        .filter(|(_, n)| {
            let pkg_name = n
                .package
                .package_id
                .split_whitespace()
                .next()
                .unwrap_or("");
            pkg_name.replace('-', "_") == normalized_cause
        })
        .map(|(i, _)| i)
        .collect()
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
    fn trigger_label_includes_volatile_and_watched_by() {
        let cluster = sample_cluster();
        let label = trigger_node_label(&cluster);
        assert!(label.contains("env:PATH"));
        assert!(label.contains("[volatile]"));
        assert!(label.contains("watched-by:pyo3-build-config"));
    }

    #[test]
    fn render_emits_header_and_dag_nodes() {
        let mut out = String::new();
        render(&[sample_cluster()], &mut out);
        assert!(out.contains("env:PATH"));
        assert!(out.contains("numpy"));
        assert!(out.contains("python_utils"));
    }

    #[test]
    fn empty_clusters_message() {
        let mut out = String::new();
        render(&[], &mut out);
        assert!(out.contains("No rebuild triggers detected"));
    }

    #[test]
    fn dependency_chain_renders_trigger_above_leaves() {
        let cluster = sample_cluster();
        let mut out = String::new();
        render_cluster(&cluster, &mut out);
        let path_pos = out.find("env:PATH").expect("PATH node");
        let utils_pos = out.find("[python_utils]").expect("python_utils node");
        assert!(
            path_pos < utils_pos,
            "trigger should appear above downstream package: {out}"
        );
    }
}
