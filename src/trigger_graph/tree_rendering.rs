use std::collections::BTreeSet;

use termtree::Tree;

use crate::{
    target_reading::CodeLocation,
    trigger_detection::{RebuildTrigger, SourceStep, TriggerChange, TriggerSources},
    trigger_graph::{
        name_lists::{compact_name_list, shorten_shared_prefixes},
        rebuilt_members::{TriggerPath, TriggerReport},
        terminal_styling::{FIX_LINES, LineRoles, PLAIN_LINES, TerminalStyle, TextRole},
    },
};

const BRANCH_LABEL_WIDTH: usize = 11;
const TREE_NAME_LIMIT: usize = 4;
const CHURN_WINDOW_LABEL: &str = "last 24h";

const STEP_LABEL_WIDTH: usize = 13;

pub fn render_trigger_tree(
    trigger_report: &TriggerReport,
    terminal_style: &TerminalStyle,
) -> String {
    let mut rendered_text = render_report_header(trigger_report, terminal_style);
    trigger_report
        .trigger_paths
        .iter()
        .enumerate()
        .for_each(|(cause_position, trigger_path)| {
            rendered_text.push('\n');
            rendered_text.push_str(&render_trigger_branches(
                &CauseRendering {
                    cause_number: cause_position + 1,
                    workspace_member_count: trigger_report.workspace_member_count,
                    terminal_style,
                },
                trigger_path,
            ));
        });
    rendered_text
}

fn render_report_header(trigger_report: &TriggerReport, terminal_style: &TerminalStyle) -> String {
    let hidden_note = match trigger_report.hidden_trigger_count {
        0 => String::new(),
        1 => "1 more cause hidden (one-off or beyond the top 5); show it with --all\n".to_string(),
        hidden_count => {
            format!(
                "{hidden_count} more causes hidden (one-off or beyond the top 5); show them with \
                 --all\n"
            )
        }
    };
    if trigger_report.trigger_paths.is_empty() {
        return format!(
            "No recurring rebuild triggers in target/ ({CHURN_WINDOW_LABEL}).\n{hidden_note}Run \
             `cargo frequent --build` to watch a build.\n"
        );
    }
    let recompiled_members: BTreeSet<&str> = trigger_report
        .trigger_paths
        .iter()
        .flat_map(|trigger_path| &trigger_path.member_routes)
        .map(|member_route| member_route.member_name.as_str())
        .collect();
    let cause_count = trigger_report.trigger_paths.len();
    let cause_phrase = if cause_count == 1 {
        "1 cause recompiles".to_string()
    } else {
        format!("{cause_count} causes recompile")
    };
    let summary_line = format!(
        "{cause_phrase} {} of {} workspace members ({CHURN_WINDOW_LABEL})",
        recompiled_members.len(),
        trigger_report.workspace_member_count
    );
    format!(
        "{}\n{}",
        terminal_style.paint(&summary_line, TextRole::Heading),
        terminal_style.paint(&hidden_note, TextRole::Evidence)
    )
}

struct CauseRendering<'style> {
    cause_number: usize,
    workspace_member_count: usize,
    terminal_style: &'style TerminalStyle,
}

fn render_trigger_branches(
    cause_rendering: &CauseRendering<'_>,
    trigger_path: &TriggerPath,
) -> String {
    let terminal_style = cause_rendering.terminal_style;
    let workspace_member_count = cause_rendering.workspace_member_count;
    let rebuild_trigger = &trigger_path.rebuild_trigger;
    let cause_tree =
        Tree::new(format!(
            "{}   {}{}",
            terminal_style.paint(
                &format!(
                    "{}  {}",
                    cause_rendering.cause_number, rebuild_trigger.trigger_label
                ),
                TextRole::Heading
            ),
            terminal_style.location_prefix(&rebuild_trigger.trigger_location),
            terminal_style.paint(
                &format!("({})", rebuild_trigger.evidence),
                TextRole::Evidence
            )
        ))
        .with_leaves(
            [
                Some(change_branch(
                    terminal_style,
                    &rebuild_trigger.trigger_change,
                )),
                source_branch(terminal_style, &rebuild_trigger.trigger_sources),
                observer_leaf(terminal_style, rebuild_trigger),
                member_leaf(terminal_style, trigger_path, workspace_member_count),
            ]
            .into_iter()
            .flatten()
            .chain(rebuild_trigger.fix_hints.iter().enumerate().map(
                |(hint_position, fix_hint)| {
                    styled_leaf(
                        terminal_style,
                        &LeafLabel {
                            branch_label: if hint_position == 0 { "fix" } else { "also" },
                            label_width: BRANCH_LABEL_WIDTH,
                            leaf_location: &fix_hint.location,
                        },
                        &format!(
                            "{}\n{}{}",
                            fix_hint.summary,
                            fix_hint.suggestion,
                            terminal_style.documentation_line(&fix_hint.documentation_link)
                        ),
                        &FIX_LINES,
                    )
                },
            )),
        );
    cause_tree.to_string()
}

fn observer_leaf(
    terminal_style: &TerminalStyle,
    rebuild_trigger: &RebuildTrigger,
) -> Option<Tree<String>> {
    let change_text = rebuild_trigger.trigger_change.to_string();
    let observers_named_in_change = rebuild_trigger
        .observing_crates
        .iter()
        .any(|observing_crate| change_text.contains(&observing_crate.crate_name));
    if observers_named_in_change || rebuild_trigger.observing_crates.is_empty() {
        return None;
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
    Some(labelled_leaf(
        terminal_style,
        rebuild_trigger.observation_label,
        &format!(
            "{}{build_script_suffix}",
            compact_name_list(&observer_names, TREE_NAME_LIMIT)
        ),
    ))
}

fn member_leaf(
    terminal_style: &TerminalStyle,
    trigger_path: &TriggerPath,
    workspace_member_count: usize,
) -> Option<Tree<String>> {
    let member_names: Vec<String> = trigger_path
        .member_routes
        .iter()
        .map(|member_route| member_route.member_name.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    if member_names.is_empty() {
        return None;
    }
    let routing_observers: Vec<String> = trigger_path
        .member_routes
        .iter()
        .map(|member_route| member_route.observing_crate_name.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let routing_note = if routing_observers.len() > 1 {
        format!(
            "\nreached through {}",
            compact_name_list(&routing_observers, TREE_NAME_LIMIT)
        )
    } else {
        String::new()
    };
    let member_summary =
        if member_names.len() == workspace_member_count && workspace_member_count > 1 {
            format!("all {workspace_member_count} workspace members")
        } else {
            shorten_shared_prefixes(&member_names)
        };
    let intermediate_note = match trigger_path.intermediate_crate_count {
        0 => String::new(),
        1 => "  via 1 crate".to_string(),
        intermediate_count => format!("  via {intermediate_count} crates"),
    };
    Some(labelled_leaf(
        terminal_style,
        "recompiles",
        &format!("{member_summary}{intermediate_note}{routing_note}"),
    ))
}

struct LeafLabel<'label> {
    branch_label: &'label str,
    label_width: usize,
    leaf_location: &'label CodeLocation,
}

fn labelled_leaf(
    terminal_style: &TerminalStyle,
    branch_label: &str,
    branch_text: &str,
) -> Tree<String> {
    styled_leaf(
        terminal_style,
        &LeafLabel {
            branch_label,
            label_width: BRANCH_LABEL_WIDTH,
            leaf_location: &CodeLocation::Nowhere,
        },
        branch_text,
        &PLAIN_LINES,
    )
}

fn styled_leaf(
    terminal_style: &TerminalStyle,
    leaf_label: &LeafLabel<'_>,
    branch_text: &str,
    line_roles: &LineRoles,
) -> Tree<String> {
    let label_width = leaf_label.label_width;
    let continuation_indent = " ".repeat(label_width + 1);
    let padded_label = format!("{:<label_width$}", leaf_label.branch_label);
    let aligned_text = branch_text
        .lines()
        .enumerate()
        .map(|(line_position, text_line)| {
            if line_position == 0 {
                format!(
                    "{} {}{}",
                    terminal_style.paint(&padded_label, TextRole::BranchLabel),
                    terminal_style.location_prefix(leaf_label.leaf_location),
                    terminal_style.paint(text_line, line_roles.first_line_role)
                )
            } else {
                format!(
                    "{continuation_indent}{}",
                    terminal_style.paint(text_line, line_roles.continuation_role)
                )
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    Tree::new(aligned_text).with_multiline(true)
}

fn change_branch(terminal_style: &TerminalStyle, trigger_change: &TriggerChange) -> Tree<String> {
    let value_leaf = |branch_label, branch_text: &str, text_role| {
        styled_leaf(
            terminal_style,
            &LeafLabel {
                branch_label,
                label_width: BRANCH_LABEL_WIDTH,
                leaf_location: &CodeLocation::Nowhere,
            },
            branch_text,
            &LineRoles {
                first_line_role: text_role,
                continuation_role: text_role,
            },
        )
    };
    match trigger_change {
        TriggerChange::Summary { change_text } => {
            labelled_leaf(terminal_style, "change", change_text)
        }
        TriggerChange::Replacement {
            removed_text,
            added_text,
        } => Tree::new(terminal_style.paint("change", TextRole::BranchLabel)).with_leaves([
            value_leaf("removed", removed_text, TextRole::RemovedValue),
            value_leaf("added", added_text, TextRole::AddedValue),
        ]),
    }
}

fn source_branch(
    terminal_style: &TerminalStyle,
    trigger_sources: &TriggerSources,
) -> Option<Tree<String>> {
    match trigger_sources {
        TriggerSources::Unknown => None,
        TriggerSources::CausalChain { chain_levels } => Some(titled_step_branch(
            terminal_style,
            "set by",
            &chain_levels
                .iter()
                .flat_map(|chain_level| chain_level.level_steps.clone())
                .collect::<Vec<_>>(),
        )),
        TriggerSources::IndependentFacts { source_steps } => {
            Some(titled_step_branch(terminal_style, "why", source_steps))
        }
    }
}

fn titled_step_branch(
    terminal_style: &TerminalStyle,
    branch_title: &str,
    source_steps: &[SourceStep],
) -> Tree<String> {
    Tree::new(terminal_style.paint(branch_title, TextRole::BranchLabel)).with_leaves(
        source_steps.iter().map(|source_step| {
            styled_leaf(
                terminal_style,
                &LeafLabel {
                    branch_label: source_step.step_label,
                    label_width: STEP_LABEL_WIDTH,
                    leaf_location: &source_step.location,
                },
                &source_step.step_text,
                &PLAIN_LINES,
            )
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render_trigger_tree_plain(trigger_report: &TriggerReport) -> String {
        render_trigger_tree(trigger_report, &TerminalStyle::plain())
    }
    use crate::{
        trigger_detection::{
            ChainLevel, ObservingCrate, RebuildTrigger, TriggerRecurrence,
            fix_hints::{DocumentationLink, FixHint},
        },
        trigger_graph::rebuilt_members::MemberRoute,
    };

    fn seed_report(member_names: &[&str], workspace_member_count: usize) -> TriggerReport {
        TriggerReport {
            trigger_paths: vec![TriggerPath {
                rebuild_trigger: RebuildTrigger {
                    trigger_location: CodeLocation::Nowhere,
                    trigger_label: "$BINDGEN_EXTRA_CLANG_ARGS".to_string(),
                    trigger_change: TriggerChange::Summary {
                        change_text: "-frandom-seed=a -> -frandom-seed=b".to_string(),
                    },
                    trigger_sources: TriggerSources::Unknown,
                    observation_label: "watched by",
                    observing_crates: ["bindgen", "pipewire-sys"]
                        .into_iter()
                        .map(|crate_name| ObservingCrate {
                            crate_name: crate_name.to_string(),
                            watches_through_build_script: true,
                        })
                        .collect(),
                    evidence: "recorded 3h ago, reruns on the next build".to_string(),
                    recurrence: TriggerRecurrence::Recurring,
                    fix_hints: vec![FixHint {
                        documentation_link: DocumentationLink::NoLink,
                        location: CodeLocation::Nowhere,
                        summary: "Nix re-seeds".to_string(),
                        suggestion: "strip it".to_string(),
                    }],
                },
                member_routes: member_names
                    .iter()
                    .map(|member_name| MemberRoute {
                        observing_crate_name: "pipewire-sys".to_string(),
                        member_name: (*member_name).to_string(),
                    })
                    .collect(),
                intermediate_crate_count: 11,
            }],
            hidden_trigger_count: 2,
            workspace_member_count,
        }
    }

    #[test]
    fn renders_header_and_branches() {
        let rendered_text =
            render_trigger_tree_plain(&seed_report(&["sottomano-shell", "sottomano-sources"], 9));
        [
            "1 cause recompiles 2 of 9 workspace members (last 24h)",
            "2 more causes hidden",
            "1  $BINDGEN_EXTRA_CLANG_ARGS   (recorded 3h ago, reruns on the next build)",
            "├── change      -frandom-seed=a -> -frandom-seed=b",
            "├── watched by  bindgen, pipewire-sys (build scripts)",
            "├── recompiles  sottomano-shell, -sources  via 11 crates",
            "└── fix         Nix re-seeds",
            "                strip it",
        ]
        .into_iter()
        .for_each(|expected_text| {
            assert!(
                rendered_text.contains(expected_text),
                "{expected_text:?} missing:\n{rendered_text}"
            );
        });
    }

    #[test]
    fn nests_long_replacements_under_change() {
        let mut store_report = seed_report(&["alpha"], 3);
        store_report.trigger_paths[0].rebuild_trigger.trigger_change = TriggerChange::Replacement {
            removed_text: "/nix/store/…-wayland-1.25.0-dev/lib/pkgconfig".to_string(),
            added_text: "/nix/store/…-wayland-1.26.0-dev/lib/pkgconfig".to_string(),
        };
        let rendered_text = render_trigger_tree_plain(&store_report);
        [
            "├── change\n",
            "│   ├── removed     /nix/store/…-wayland-1.25.0-dev/lib/pkgconfig\n",
            "│   └── added       /nix/store/…-wayland-1.26.0-dev/lib/pkgconfig\n",
        ]
        .into_iter()
        .for_each(|expected_text| {
            assert!(
                rendered_text.contains(expected_text),
                "{expected_text:?} missing:\n{rendered_text}"
            );
        });
    }

    #[test]
    fn nests_source_steps_under_set_by() {
        let mut nix_report = seed_report(&["alpha"], 3);
        nix_report.trigger_paths[0].rebuild_trigger.trigger_sources = TriggerSources::CausalChain {
            chain_levels: vec![ChainLevel {
                level_steps: vec![
                    SourceStep {
                        location: CodeLocation::Nowhere,
                        step_label: "direnv",
                        step_text: ".envrc:2 use flake".to_string(),
                    },
                    SourceStep {
                        location: CodeLocation::Nowhere,
                        step_label: "changed",
                        step_text: "flake.nix modified after the build scripts last ran"
                            .to_string(),
                    },
                ],
            }],
        };
        let rendered_text = render_trigger_tree_plain(&nix_report);
        [
            "├── set by\n",
            "│   ├── direnv        .envrc:2 use flake\n",
            "│   └── changed       flake.nix modified after the build scripts last ran\n",
        ]
        .into_iter()
        .for_each(|expected_text| {
            assert!(
                rendered_text.contains(expected_text),
                "{expected_text:?} missing:\n{rendered_text}"
            );
        });
    }

    #[test]
    fn summarizes_every_member() {
        let rendered_text = render_trigger_tree_plain(&seed_report(&["alpha", "beta"], 2));
        assert!(
            rendered_text.contains("all 2 workspace members"),
            "{rendered_text}"
        );
    }

    #[test]
    fn skips_observers_already_named_in_change() {
        let mut churn_report = seed_report(&["alpha"], 3);
        let churn_trigger = &mut churn_report.trigger_paths[0].rebuild_trigger;
        churn_trigger.trigger_change = TriggerChange::Summary {
            change_text: "bindgen +x, pipewire-sys ±y".to_string(),
        };
        let rendered_text = render_trigger_tree_plain(&churn_report);
        assert!(!rendered_text.contains("watched by"), "{rendered_text}");
    }

    #[test]
    fn groups_names_by_their_own_prefix() {
        let crate_names: Vec<String> = [
            "iced_controller_diagram",
            "sottomano-shell",
            "sottomano-sources",
            "xtask",
            "zbus-names",
            "zbus-xml",
        ]
        .into_iter()
        .map(ToString::to_string)
        .collect();
        assert_eq!(
            shorten_shared_prefixes(&crate_names),
            "iced_controller_diagram, sottomano-shell, -sources, xtask, zbus-names, -xml"
        );
        assert_eq!(
            compact_name_list(&crate_names, TREE_NAME_LIMIT),
            "iced_controller_diagram, sottomano-shell, -sources, xtask, +2 more"
        );
    }

    #[test]
    fn lists_every_recompiled_member() {
        let member_names = [
            "sottomano-shell",
            "sottomano-sources",
            "sottomano-state",
            "sottomano-widgets",
            "sottomano-widget-showcase",
        ];
        let rendered_text = render_trigger_tree_plain(&seed_report(&member_names, 9));
        assert!(
            rendered_text.contains(
                "├── recompiles  sottomano-shell, -sources, -state, -widget-showcase, -widgets  \
                 via 11 crates"
            ),
            "{rendered_text}"
        );
    }

    #[test]
    fn explains_empty_report() {
        let rendered_text = render_trigger_tree_plain(&TriggerReport {
            trigger_paths: Vec::new(),
            hidden_trigger_count: 1,
            workspace_member_count: 3,
        });
        assert!(
            rendered_text.starts_with("No recurring rebuild triggers"),
            "{rendered_text}"
        );
        assert!(
            rendered_text.contains("1 more cause hidden"),
            "{rendered_text}"
        );
    }
}
