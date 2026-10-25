use std::{collections::BTreeMap, fmt::Write};

use ascii_dag::graph::{Graph, RenderMode};

use crate::{
    trigger_detection::{RebuildTrigger, TriggerChange},
    trigger_graph::rebuilt_members::TriggerPath,
};

const TRIGGER_NODE_ID: usize = 0;
const SHOWN_MEMBER_LIMIT: usize = 5;
pub fn render_trigger_paths(trigger_paths: &[TriggerPath]) -> String {
    trigger_paths
        .iter()
        .map(render_trigger_path)
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_trigger_path(trigger_path: &TriggerPath) -> String {
    let rebuild_trigger = &trigger_path.rebuild_trigger;
    let observer_count = rebuild_trigger.observing_crates.len();
    let shown_member_routes =
        &trigger_path.member_routes[..trigger_path.member_routes.len().min(SHOWN_MEMBER_LIMIT)];
    let hidden_member_count = trigger_path.member_routes.len() - shown_member_routes.len();
    let first_member_node_id = observer_count + 1;
    let node_labels: Vec<String> = [rebuild_trigger.trigger_label.clone()]
        .into_iter()
        .chain(
            rebuild_trigger
                .observing_crates
                .iter()
                .map(|observing_crate| {
                    if observing_crate.watches_through_build_script {
                        format!("{} (build script)", observing_crate.crate_name)
                    } else {
                        observing_crate.crate_name.clone()
                    }
                }),
        )
        .chain(
            shown_member_routes
                .iter()
                .map(|member_route| member_route.member_name.clone()),
        )
        .chain((hidden_member_count > 0).then(|| format!("+{hidden_member_count} more")))
        .collect();
    let observer_node_ids = rebuild_trigger.observing_crates.iter().enumerate().fold(
        BTreeMap::new(),
        |mut observer_node_ids: BTreeMap<&str, usize>, (observer_position, observing_crate)| {
            observer_node_ids.insert(&observing_crate.crate_name, observer_position + 1);
            observer_node_ids
        },
    );
    let member_group_title = if trigger_path.intermediate_crate_count > 0 {
        format!(
            "recompiles (via {} other crates)",
            trigger_path.intermediate_crate_count
        )
    } else {
        "recompiles".to_string()
    };

    let mut trigger_graph = Graph::with_mode(RenderMode::Vertical);
    node_labels
        .iter()
        .enumerate()
        .for_each(|(node_id, node_label)| trigger_graph.add_node(node_id, node_label));
    (1..=observer_count).for_each(|observer_node_id| {
        let edge_label = (observer_node_id == 1).then_some(rebuild_trigger.observation_label);
        trigger_graph.add_edge(TRIGGER_NODE_ID, observer_node_id, edge_label);
    });
    shown_member_routes
        .iter()
        .enumerate()
        .for_each(|(member_position, member_route)| {
            let observer_node_id = observer_node_ids
                .get(member_route.observing_crate_name.as_str())
                .copied()
                .unwrap_or(TRIGGER_NODE_ID);
            trigger_graph.add_edge(
                observer_node_id,
                first_member_node_id + member_position,
                None,
            );
        });
    if hidden_member_count > 0 {
        trigger_graph.add_edge(1, node_labels.len() - 1, None);
    }

    let trigger_group = trigger_graph.add_subgraph("trigger");
    let observer_group = trigger_graph.add_subgraph("observed by");
    let observer_node_list: Vec<usize> = (1..first_member_node_id).collect();
    let member_node_list: Vec<usize> = (first_member_node_id..node_labels.len()).collect();
    let _ = trigger_graph
        .put_nodes(&[TRIGGER_NODE_ID])
        .inside(trigger_group);
    let _ = trigger_graph
        .put_nodes(&observer_node_list)
        .inside(observer_group);
    if !member_node_list.is_empty() {
        let member_group = trigger_graph.add_subgraph(&member_group_title);
        let _ = trigger_graph
            .put_nodes(&member_node_list)
            .inside(member_group);
    }

    let mut rendered_text = trigger_graph
        .compute_layout()
        .render_scanline()
        .lines()
        .map(str::trim_end)
        .filter(|rendered_line| !rendered_line.is_empty())
        .fold(String::new(), |mut rendered_text, rendered_line| {
            rendered_text.push_str(rendered_line);
            rendered_text.push('\n');
            rendered_text
        });
    render_trigger_details(rebuild_trigger, &mut rendered_text);
    rendered_text
}

fn render_trigger_details(rebuild_trigger: &RebuildTrigger, rendered_text: &mut String) {
    match &rebuild_trigger.trigger_change {
        TriggerChange::Summary { change_text } => {
            let _ = writeln!(*rendered_text, "  change:   {change_text}");
        }
        TriggerChange::Replacement {
            removed_text,
            added_text,
        } => {
            let _ = writeln!(*rendered_text, "  removed:  {removed_text}");
            let _ = writeln!(*rendered_text, "  added:    {added_text}");
        }
    }
    let _ = writeln!(*rendered_text, "  evidence: {}", rebuild_trigger.evidence);
    rebuild_trigger.fix_hints.iter().for_each(|fix_hint| {
        let _ = writeln!(*rendered_text, "  hint:     {}", fix_hint.summary);
        let _ = writeln!(*rendered_text, "            {}", fix_hint.suggestion);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        trigger_detection::{
            ObservingCrate, RebuildTrigger, TriggerRecurrence, fix_hints::FixHint,
        },
        trigger_graph::rebuilt_members::MemberRoute,
    };

    fn seed_trigger_path(intermediate_crate_count: usize) -> TriggerPath {
        TriggerPath {
            rebuild_trigger: RebuildTrigger {
                trigger_label: "$BINDGEN_EXTRA_CLANG_ARGS".to_string(),
                trigger_change: TriggerChange::Summary {
                    change_text: "-frandom-seed=a -> -frandom-seed=b".to_string(),
                },
                observation_label: "watched by",
                observing_crates: vec![ObservingCrate {
                    crate_name: "pipewire-sys".to_string(),
                    watches_through_build_script: true,
                }],
                evidence: "recorded 0s ago".to_string(),
                recurrence: TriggerRecurrence::Recurring,
                fix_hints: vec![FixHint {
                    summary: "seed".to_string(),
                    suggestion: "strip it".to_string(),
                }],
            },
            member_routes: vec![MemberRoute {
                observing_crate_name: "pipewire-sys".to_string(),
                member_name: "sottomano-shell".to_string(),
            }],
            intermediate_crate_count,
        }
    }

    #[test]
    fn draws_groups_edges_and_details() {
        let rendered_text = render_trigger_paths(&[seed_trigger_path(3)]);
        [
            "trigger",
            "observed by",
            "recompiles",
            "$BINDGEN_EXTRA_CLANG_ARGS",
            "pipewire-sys (build script)",
            "sottomano-shell",
            "watched by",
            "recompiles (via 3 other crates)",
            "change:   -frandom-seed=a -> -frandom-seed=b",
            "hint:     seed",
        ]
        .into_iter()
        .for_each(|expected_text| {
            assert!(
                rendered_text.contains(expected_text),
                "{expected_text} missing:\n{rendered_text}"
            );
        });
    }

    #[test]
    fn direct_dependents_have_no_via_label() {
        let rendered_text = render_trigger_paths(&[seed_trigger_path(0)]);
        assert!(!rendered_text.contains("via"), "{rendered_text}");
    }
}
