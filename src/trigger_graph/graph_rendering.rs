use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write,
};

use ascii_dag::{
    graph::{Graph, RenderMode},
    render::{colors::Palette, engine::RenderOptions},
};

use crate::{
    target_reading::CodeLocation,
    trigger_detection::{RebuildTrigger, TriggerChange, TriggerSources},
    trigger_graph::{
        name_lists::{compact_name_list, shorten_shared_prefixes},
        rebuilt_members::TriggerPath,
        terminal_styling::{FIX_LINES, LineRoles, PLAIN_LINES, TerminalStyle, TextRole},
    },
};

const GRAPH_NAME_LIMIT: usize = 2;
const NODE_LABEL_LIMIT: usize = 36;
const DETAIL_LABEL_WIDTH: usize = 13;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum NodeGroup {
    Environment,
    Trigger,
    Observer,
    Member,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum EdgeLabel {
    Unlabelled,
    Labelled(&'static str),
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct GraphEdge {
    from_node: usize,
    to_node: usize,
    edge_label: EdgeLabel,
}

struct GraphNode {
    node_label: String,
    node_group: NodeGroup,
}

struct NodeGroupLayout {
    node_group: NodeGroup,
    group_title: &'static str,
}

const NODE_GROUP_LAYOUTS: &[NodeGroupLayout] = &[
    NodeGroupLayout {
        node_group: NodeGroup::Environment,
        group_title: "environment",
    },
    NodeGroupLayout {
        node_group: NodeGroup::Trigger,
        group_title: "triggers",
    },
    NodeGroupLayout {
        node_group: NodeGroup::Observer,
        group_title: "observed by",
    },
    NodeGroupLayout {
        node_group: NodeGroup::Member,
        group_title: "recompiles",
    },
];

#[derive(Default)]
struct CombinedGraph {
    graph_nodes: Vec<GraphNode>,
    node_ids_by_key: BTreeMap<String, usize>,
    graph_edges: BTreeSet<GraphEdge>,
}

impl CombinedGraph {
    fn node_id(&mut self, node_key: String, node_label: String, node_group: NodeGroup) -> usize {
        if let Some(existing_node_id) = self.node_ids_by_key.get(&node_key) {
            return *existing_node_id;
        }
        let new_node_id = self.graph_nodes.len();
        self.graph_nodes.push(GraphNode {
            node_label,
            node_group,
        });
        self.node_ids_by_key.insert(node_key, new_node_id);
        new_node_id
    }

    fn connect(&mut self, from_node: usize, to_node: usize, edge_label: EdgeLabel) {
        let already_connected = self
            .graph_edges
            .iter()
            .any(|graph_edge| graph_edge.from_node == from_node && graph_edge.to_node == to_node);
        if !already_connected {
            self.graph_edges.insert(GraphEdge {
                from_node,
                to_node,
                edge_label,
            });
        }
    }

    fn group_node_ids(&self, node_group: NodeGroup) -> Vec<usize> {
        self.graph_nodes
            .iter()
            .enumerate()
            .filter(|(_, graph_node)| graph_node.node_group == node_group)
            .map(|(node_id, _)| node_id)
            .collect()
    }
}

struct GroupedNodes {
    group_title: &'static str,
    node_ids: Vec<usize>,
}

pub fn render_combined_graph(
    trigger_paths: &[TriggerPath],
    terminal_style: &TerminalStyle,
) -> String {
    let mut combined_graph = CombinedGraph::default();
    let member_parent_nodes: Vec<usize> = trigger_paths
        .iter()
        .enumerate()
        .map(|(cause_position, trigger_path)| {
            add_cause(
                &mut combined_graph,
                cause_position + 1,
                &trigger_path.rebuild_trigger,
            )
        })
        .collect();
    add_member_groups(&mut combined_graph, trigger_paths, &member_parent_nodes);
    let mut rendered_text = draw_combined_graph(&combined_graph, terminal_style);
    trigger_paths
        .iter()
        .enumerate()
        .for_each(|(cause_position, trigger_path)| {
            rendered_text.push('\n');
            render_cause_details(
                cause_position + 1,
                trigger_path,
                terminal_style,
                &mut rendered_text,
            );
        });
    rendered_text
}

fn add_cause(
    combined_graph: &mut CombinedGraph,
    cause_number: usize,
    rebuild_trigger: &RebuildTrigger,
) -> usize {
    let trigger_node = combined_graph.node_id(
        format!("trigger:{cause_number}"),
        format!("{cause_number} {}", rebuild_trigger.trigger_label),
        NodeGroup::Trigger,
    );
    add_environment_chain(combined_graph, cause_number, rebuild_trigger, trigger_node);
    if rebuild_trigger.observing_crates.is_empty() {
        return trigger_node;
    }
    let observer_names: Vec<String> = rebuild_trigger
        .observing_crates
        .iter()
        .map(|observing_crate| observing_crate.crate_name.clone())
        .collect();
    let build_script_suffix = match rebuild_trigger
        .observing_crates
        .iter()
        .filter(|observing_crate| observing_crate.watches_through_build_script)
        .count()
    {
        0 => "",
        1 => " (build script)",
        _ => " (build scripts)",
    };
    let observer_node = combined_graph.node_id(
        format!("observers:{cause_number}"),
        format!(
            "{}{build_script_suffix}",
            compact_name_list(&observer_names, GRAPH_NAME_LIMIT)
        ),
        NodeGroup::Observer,
    );
    combined_graph.connect(
        trigger_node,
        observer_node,
        EdgeLabel::Labelled(rebuild_trigger.observation_label),
    );
    observer_node
}

fn add_member_groups(
    combined_graph: &mut CombinedGraph,
    trigger_paths: &[TriggerPath],
    member_parent_nodes: &[usize],
) {
    let causes_by_member = trigger_paths.iter().enumerate().fold(
        BTreeMap::new(),
        |mut causes_by_member: BTreeMap<&str, BTreeSet<usize>>, (cause_position, trigger_path)| {
            trigger_path.member_routes.iter().for_each(|member_route| {
                causes_by_member
                    .entry(member_route.member_name.as_str())
                    .or_default()
                    .insert(cause_position);
            });
            causes_by_member
        },
    );
    let members_by_causes = causes_by_member.into_iter().fold(
        BTreeMap::new(),
        |mut members_by_causes: BTreeMap<BTreeSet<usize>, Vec<String>>,
         (member_name, cause_positions)| {
            members_by_causes
                .entry(cause_positions)
                .or_default()
                .push(member_name.to_string());
            members_by_causes
        },
    );
    members_by_causes
        .iter()
        .for_each(|(cause_positions, member_names)| {
            let member_group_node = combined_graph.node_id(
                format!("members:{cause_positions:?}"),
                compact_name_list(member_names, GRAPH_NAME_LIMIT),
                NodeGroup::Member,
            );
            cause_positions.iter().for_each(|cause_position| {
                combined_graph.connect(
                    member_parent_nodes[*cause_position],
                    member_group_node,
                    EdgeLabel::Unlabelled,
                );
            });
        });
}

fn add_environment_chain(
    combined_graph: &mut CombinedGraph,
    cause_number: usize,
    rebuild_trigger: &RebuildTrigger,
    trigger_node: usize,
) {
    let TriggerSources::CausalChain { chain_levels } = &rebuild_trigger.trigger_sources else {
        return;
    };
    chain_levels.iter().enumerate().fold(
        vec![trigger_node],
        |explained_nodes, (level_position, chain_level)| {
            let level_nodes: Vec<usize> = chain_level
                .level_steps
                .iter()
                .enumerate()
                .map(|(step_position, source_step)| {
                    combined_graph.node_id(
                        format!("environment:{cause_number}:{level_position}:{step_position}"),
                        abbreviate_node_label(&format!(
                            "{}: {}",
                            source_step.step_label,
                            source_step.step_text.lines().next().unwrap_or_default()
                        )),
                        NodeGroup::Environment,
                    )
                })
                .collect();
            level_nodes.iter().for_each(|level_node| {
                explained_nodes.iter().for_each(|explained_node| {
                    let edge_label = if level_position == 0 {
                        EdgeLabel::Labelled("set by")
                    } else {
                        EdgeLabel::Unlabelled
                    };
                    combined_graph.connect(*explained_node, *level_node, edge_label);
                });
            });
            level_nodes
        },
    );
}

fn abbreviate_node_label(node_label: &str) -> String {
    if node_label.chars().count() <= NODE_LABEL_LIMIT {
        return node_label.to_string();
    }
    let shortened_label: String = node_label.chars().take(NODE_LABEL_LIMIT - 1).collect();
    format!("{shortened_label}…")
}

fn draw_combined_graph(combined_graph: &CombinedGraph, terminal_style: &TerminalStyle) -> String {
    let mut drawn_graph = Graph::with_mode(RenderMode::Vertical);
    combined_graph
        .graph_nodes
        .iter()
        .enumerate()
        .for_each(|(node_id, graph_node)| {
            drawn_graph.add_node(node_id, &graph_node.node_label);
        });
    combined_graph.graph_edges.iter().for_each(|graph_edge| {
        let drawn_label = match graph_edge.edge_label {
            EdgeLabel::Unlabelled => None,
            EdgeLabel::Labelled(label_text) => Some(label_text),
        };
        drawn_graph.add_edge(graph_edge.from_node, graph_edge.to_node, drawn_label);
    });
    let grouped_nodes: Vec<GroupedNodes> = NODE_GROUP_LAYOUTS
        .iter()
        .map(|group_layout| GroupedNodes {
            group_title: group_layout.group_title,
            node_ids: combined_graph.group_node_ids(group_layout.node_group),
        })
        .filter(|grouped_entry| !grouped_entry.node_ids.is_empty())
        .collect();
    grouped_nodes.iter().for_each(|grouped_entry| {
        let node_subgraph = drawn_graph.add_subgraph(grouped_entry.group_title);
        let _ = drawn_graph
            .put_nodes(&grouped_entry.node_ids)
            .inside(node_subgraph);
    });
    drawn_graph
        .compute_layout()
        .render_string(&if terminal_style.styling_enabled {
            RenderOptions::colored(Palette::Ansi)
        } else {
            RenderOptions::plain()
        })
        .lines()
        .map(str::trim_end)
        .filter(|rendered_line| !rendered_line.is_empty())
        .fold(String::new(), |mut rendered_text, rendered_line| {
            rendered_text.push_str(rendered_line);
            rendered_text.push('\n');
            rendered_text
        })
}

fn render_cause_details(
    cause_number: usize,
    trigger_path: &TriggerPath,
    terminal_style: &TerminalStyle,
    rendered_text: &mut String,
) {
    let rebuild_trigger = &trigger_path.rebuild_trigger;
    let _ = writeln!(
        rendered_text,
        "{}   {}{}",
        terminal_style.paint(
            &format!("{cause_number}  {}", rebuild_trigger.trigger_label),
            TextRole::Heading
        ),
        terminal_style.location_prefix(&rebuild_trigger.trigger_location),
        terminal_style.paint(
            &format!("({})", rebuild_trigger.evidence),
            TextRole::Evidence
        )
    );
    write_change_details(
        rendered_text,
        terminal_style,
        &rebuild_trigger.trigger_change,
    );
    write_source_details(
        rendered_text,
        terminal_style,
        &rebuild_trigger.trigger_sources,
    );
    let member_names: Vec<String> = trigger_path
        .member_routes
        .iter()
        .map(|member_route| member_route.member_name.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    if !member_names.is_empty() {
        write_detail(
            rendered_text,
            terminal_style,
            "recompiles",
            &CodeLocation::Nowhere,
            &shorten_shared_prefixes(&member_names),
            &PLAIN_LINES,
        );
    }
    if trigger_path.intermediate_crate_count > 0 {
        write_detail(
            rendered_text,
            terminal_style,
            "via",
            &CodeLocation::Nowhere,
            &format!(
                "{} crates between the observers and your members",
                trigger_path.intermediate_crate_count
            ),
            &PLAIN_LINES,
        );
    }
    rebuild_trigger.fix_hints.iter().for_each(|fix_hint| {
        write_detail(
            rendered_text,
            terminal_style,
            "fix",
            &fix_hint.location,
            &format!(
                "{}\n{}{}",
                fix_hint.summary,
                fix_hint.suggestion,
                terminal_style.documentation_line(&fix_hint.documentation_link)
            ),
            &FIX_LINES,
        );
    });
}

fn write_change_details(
    rendered_text: &mut String,
    terminal_style: &TerminalStyle,
    trigger_change: &TriggerChange,
) {
    match trigger_change {
        TriggerChange::Summary { change_text } => {
            write_detail(
                rendered_text,
                terminal_style,
                "change",
                &CodeLocation::Nowhere,
                change_text,
                &PLAIN_LINES,
            );
        }
        TriggerChange::Replacement {
            removed_text,
            added_text,
        } => {
            write_detail(
                rendered_text,
                terminal_style,
                "removed",
                &CodeLocation::Nowhere,
                removed_text,
                &LineRoles {
                    first_line_role: TextRole::RemovedValue,
                    continuation_role: TextRole::RemovedValue,
                },
            );
            write_detail(
                rendered_text,
                terminal_style,
                "added",
                &CodeLocation::Nowhere,
                added_text,
                &LineRoles {
                    first_line_role: TextRole::AddedValue,
                    continuation_role: TextRole::AddedValue,
                },
            );
        }
    }
}

fn write_source_details(
    rendered_text: &mut String,
    terminal_style: &TerminalStyle,
    trigger_sources: &TriggerSources,
) {
    match trigger_sources {
        TriggerSources::CausalChain { chain_levels } => {
            chain_levels
                .iter()
                .flat_map(|chain_level| &chain_level.level_steps)
                .for_each(|source_step| {
                    write_detail(
                        rendered_text,
                        terminal_style,
                        source_step.step_label,
                        &source_step.location,
                        &source_step.step_text,
                        &PLAIN_LINES,
                    );
                });
        }
        TriggerSources::IndependentFacts { source_steps } => {
            source_steps.iter().for_each(|source_step| {
                write_detail(
                    rendered_text,
                    terminal_style,
                    source_step.step_label,
                    &source_step.location,
                    &source_step.step_text,
                    &PLAIN_LINES,
                );
            });
        }
        TriggerSources::Unknown => {}
    }
}

fn write_detail(
    rendered_text: &mut String,
    terminal_style: &TerminalStyle,
    detail_label: &str,
    detail_location: &CodeLocation,
    detail_text: &str,
    line_roles: &LineRoles,
) {
    let continuation_indent = " ".repeat(DETAIL_LABEL_WIDTH + 4);
    let padded_label = format!("{:<DETAIL_LABEL_WIDTH$}", format!("{detail_label}:"));
    detail_text
        .lines()
        .enumerate()
        .for_each(|(line_position, detail_line)| {
            if line_position == 0 {
                let _ = writeln!(
                    rendered_text,
                    "  {}  {}{}",
                    terminal_style.paint(&padded_label, TextRole::BranchLabel),
                    terminal_style.location_prefix(detail_location),
                    terminal_style.paint(detail_line, line_roles.first_line_role)
                );
            } else {
                let _ = writeln!(
                    rendered_text,
                    "{continuation_indent}{}",
                    terminal_style.paint(detail_line, line_roles.continuation_role)
                );
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        trigger_detection::{
            ChainLevel, ObservingCrate, SourceStep, TriggerRecurrence,
            fix_hints::{DocumentationLink, FixHint},
        },
        trigger_graph::rebuilt_members::MemberRoute,
    };

    fn build_script_observer(crate_name: &str) -> ObservingCrate {
        ObservingCrate {
            crate_name: crate_name.to_string(),
            watches_through_build_script: true,
        }
    }

    fn member_route(observing_crate_name: &str, member_name: &str) -> MemberRoute {
        MemberRoute {
            observing_crate_name: observing_crate_name.to_string(),
            member_name: member_name.to_string(),
        }
    }

    fn seed_trigger_path() -> TriggerPath {
        TriggerPath {
            rebuild_trigger: RebuildTrigger {
                trigger_location: CodeLocation::Nowhere,
                trigger_label: "$BINDGEN_EXTRA_CLANG_ARGS".to_string(),
                trigger_change: TriggerChange::Summary {
                    change_text: "-frandom-seed=a -> -frandom-seed=b".to_string(),
                },
                trigger_sources: TriggerSources::CausalChain {
                    chain_levels: vec![
                        ChainLevel {
                            level_steps: vec![SourceStep {
                                location: CodeLocation::Nowhere,
                                step_label: "dev shell",
                                step_text: "/nix/store/f020dci6a9…-nix-shell-env\nsecond line"
                                    .to_string(),
                            }],
                        },
                        ChainLevel {
                            level_steps: vec![
                                SourceStep {
                                    location: CodeLocation::Nowhere,
                                    step_label: "changed",
                                    step_text: "flake.nix modified".to_string(),
                                },
                                SourceStep {
                                    location: CodeLocation::Nowhere,
                                    step_label: "changed",
                                    step_text: "flake.lock modified".to_string(),
                                },
                            ],
                        },
                    ],
                },
                observation_label: "watched by",
                observing_crates: vec![
                    build_script_observer("libspa-sys"),
                    build_script_observer("pipewire-sys"),
                ],
                evidence: "recorded 0s ago".to_string(),
                recurrence: TriggerRecurrence::Recurring,
                fix_hints: vec![FixHint {
                    documentation_link: DocumentationLink::NoLink,
                    location: CodeLocation::Nowhere,
                    summary: "pin the seed".to_string(),
                    suggestion: "env.NIX_OUTPATH_USED_AS_RANDOM_SEED".to_string(),
                }],
            },
            member_routes: vec![
                member_route("libspa-sys", "sottomano-sources"),
                member_route("pipewire-sys", "sottomano-sources"),
            ],
            intermediate_crate_count: 3,
        }
    }

    fn feature_trigger_path() -> TriggerPath {
        TriggerPath {
            rebuild_trigger: RebuildTrigger {
                trigger_location: CodeLocation::Nowhere,
                trigger_label: "feature sets".to_string(),
                trigger_change: TriggerChange::Summary {
                    change_text: "bindgen -experimental".to_string(),
                },
                trigger_sources: TriggerSources::Unknown,
                observation_label: "flips in",
                observing_crates: vec![ObservingCrate {
                    crate_name: "bindgen".to_string(),
                    watches_through_build_script: false,
                }],
                evidence: "2 crates got new unit hashes".to_string(),
                recurrence: TriggerRecurrence::Recurring,
                fix_hints: Vec::new(),
            },
            member_routes: vec![member_route("bindgen", "sottomano-sources")],
            intermediate_crate_count: 0,
        }
    }

    fn parent_count(combined_graph: &CombinedGraph, node_key: &str) -> usize {
        let child_node = combined_graph.node_ids_by_key[node_key];
        combined_graph
            .graph_edges
            .iter()
            .filter(|graph_edge| graph_edge.to_node == child_node)
            .count()
    }

    fn child_count(combined_graph: &CombinedGraph, node_key: &str) -> usize {
        let parent_node = combined_graph.node_ids_by_key[node_key];
        combined_graph
            .graph_edges
            .iter()
            .filter(|graph_edge| graph_edge.from_node == parent_node)
            .count()
    }

    #[test]
    fn groups_members_by_the_causes_that_reach_them() {
        let trigger_paths = [seed_trigger_path(), feature_trigger_path()];
        let mut combined_graph = CombinedGraph::default();
        let member_parent_nodes: Vec<usize> = trigger_paths
            .iter()
            .enumerate()
            .map(|(cause_position, trigger_path)| {
                add_cause(
                    &mut combined_graph,
                    cause_position + 1,
                    &trigger_path.rebuild_trigger,
                )
            })
            .collect();
        add_member_groups(&mut combined_graph, &trigger_paths, &member_parent_nodes);
        assert_eq!(
            parent_count(&combined_graph, "members:{0, 1}"),
            2,
            "sottomano-sources is reached from both causes"
        );
        assert_eq!(
            combined_graph.graph_nodes[combined_graph.node_ids_by_key["observers:1"]].node_label,
            "libspa-sys, pipewire-sys (build scripts)"
        );
        assert_eq!(
            child_count(&combined_graph, "environment:1:0:0"),
            2,
            "the dev shell is set by flake.nix and flake.lock"
        );
    }

    #[test]
    fn draws_groups_and_numbered_details() {
        let rendered_text = render_combined_graph(
            &[seed_trigger_path(), feature_trigger_path()],
            &TerminalStyle::plain(),
        );
        [
            "environment",
            "triggers",
            "observed by",
            "recompiles",
            "1 $BINDGEN_EXTRA_CLANG_ARGS",
            "2 feature sets",
            "dev shell: /nix/store/f020dci6a9…-n…",
            "libspa-sys, pipewire-sys (build scripts)",
            "1  $BINDGEN_EXTRA_CLANG_ARGS   (recorded 0s ago)",
            "  dev shell:     /nix/store/f020dci6a9…-nix-shell-env\n                 second line",
            "  via:           3 crates between the observers and your members",
            "2  feature sets   (2 crates got new unit hashes)",
        ]
        .into_iter()
        .for_each(|expected_text| {
            assert!(
                rendered_text.contains(expected_text),
                "{expected_text:?} missing:\n{rendered_text}"
            );
        });
    }
}
