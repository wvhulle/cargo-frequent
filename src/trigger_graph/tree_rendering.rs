use std::{collections::BTreeSet, fmt::Write};

use crate::{
    trigger_detection::TriggerChange,
    trigger_graph::rebuilt_members::{TriggerPath, TriggerReport},
};

const SHOWN_NAME_LIMIT: usize = 4;
const BRANCH_LABEL_WIDTH: usize = 11;
const CHURN_WINDOW_LABEL: &str = "last 24h";

enum BranchContent {
    TextLines(Vec<String>),
    NestedBranches(Vec<TreeBranch>),
}

struct TreeBranch {
    branch_label: &'static str,
    branch_content: BranchContent,
}

const TREE_INDENT: &str = "   ";

pub fn render_trigger_tree(trigger_report: &TriggerReport) -> String {
    let mut rendered_text = render_report_header(trigger_report);
    trigger_report
        .trigger_paths
        .iter()
        .enumerate()
        .for_each(|(cause_position, trigger_path)| {
            rendered_text.push('\n');
            rendered_text.push_str(&render_trigger_branches(
                cause_position + 1,
                trigger_path,
                trigger_report.workspace_member_count,
            ));
        });
    rendered_text
}

fn render_report_header(trigger_report: &TriggerReport) -> String {
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
    format!(
        "{cause_phrase} {} of {} workspace members ({CHURN_WINDOW_LABEL})\n{hidden_note}",
        recompiled_members.len(),
        trigger_report.workspace_member_count
    )
}

fn render_trigger_branches(
    cause_number: usize,
    trigger_path: &TriggerPath,
    workspace_member_count: usize,
) -> String {
    let rebuild_trigger = &trigger_path.rebuild_trigger;
    let observers_named_in_change =
        rebuild_trigger
            .observing_crates
            .iter()
            .any(|observing_crate| {
                rebuild_trigger
                    .trigger_change
                    .to_string()
                    .contains(&observing_crate.crate_name)
            });
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
    let mut member_names: Vec<String> = trigger_path
        .member_routes
        .iter()
        .map(|member_route| member_route.member_name.clone())
        .collect();
    member_names.sort();
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

    let tree_branches = [
        Some(change_branch(&rebuild_trigger.trigger_change)),
        (!observers_named_in_change).then(|| TreeBranch {
            branch_label: rebuild_trigger.observation_label,
            branch_content: BranchContent::TextLines(vec![format!(
                "{}{build_script_suffix}",
                compact_name_list(&observer_names)
            )]),
        }),
        (!member_names.is_empty()).then(|| TreeBranch {
            branch_label: "recompiles",
            branch_content: BranchContent::TextLines(vec![format!(
                "{member_summary}{intermediate_note}"
            )]),
        }),
    ]
    .into_iter()
    .flatten()
    .chain(
        rebuild_trigger
            .fix_hints
            .iter()
            .enumerate()
            .map(|(hint_position, fix_hint)| TreeBranch {
                branch_label: if hint_position == 0 { "fix" } else { "also" },
                branch_content: BranchContent::TextLines(vec![
                    fix_hint.summary.clone(),
                    fix_hint.suggestion.clone(),
                ]),
            }),
    )
    .collect::<Vec<_>>();

    let mut rendered_text = format!(
        "{cause_number}  {}   ({})\n",
        rebuild_trigger.trigger_label, rebuild_trigger.evidence
    );
    layout_tree_branches(&tree_branches, TREE_INDENT, &mut rendered_text);
    rendered_text
}

fn change_branch(trigger_change: &TriggerChange) -> TreeBranch {
    TreeBranch {
        branch_label: "change",
        branch_content: match trigger_change {
            TriggerChange::Summary { change_text } => {
                BranchContent::TextLines(vec![change_text.clone()])
            }
            TriggerChange::Replacement {
                removed_text,
                added_text,
            } => BranchContent::NestedBranches(vec![
                TreeBranch {
                    branch_label: "removed",
                    branch_content: BranchContent::TextLines(vec![removed_text.clone()]),
                },
                TreeBranch {
                    branch_label: "added",
                    branch_content: BranchContent::TextLines(vec![added_text.clone()]),
                },
            ]),
        },
    }
}

fn layout_tree_branches(
    tree_branches: &[TreeBranch],
    indent_prefix: &str,
    rendered_text: &mut String,
) {
    let branch_count = tree_branches.len();
    tree_branches
        .iter()
        .enumerate()
        .for_each(|(branch_position, tree_branch)| {
            let is_last_branch = branch_position + 1 == branch_count;
            let branch_glyph = if is_last_branch { "└─" } else { "├─" };
            let continuation_glyph = if is_last_branch { "  " } else { "│ " };
            match &tree_branch.branch_content {
                BranchContent::TextLines(branch_lines) => {
                    branch_lines
                        .iter()
                        .enumerate()
                        .for_each(|(line_position, branch_line)| {
                            let first_line = line_position == 0;
                            let line_glyph = if first_line {
                                branch_glyph
                            } else {
                                continuation_glyph
                            };
                            let line_label = if first_line {
                                tree_branch.branch_label
                            } else {
                                ""
                            };
                            let _ = writeln!(
                                rendered_text,
                                "{indent_prefix}{line_glyph} {line_label:<BRANCH_LABEL_WIDTH$} \
                                 {branch_line}"
                            );
                        });
                }
                BranchContent::NestedBranches(nested_branches) => {
                    let _ = writeln!(
                        rendered_text,
                        "{indent_prefix}{branch_glyph} {}",
                        tree_branch.branch_label
                    );
                    layout_tree_branches(
                        nested_branches,
                        &format!("{indent_prefix}{continuation_glyph} "),
                        rendered_text,
                    );
                }
            }
        });
}

fn compact_name_list(crate_names: &[String]) -> String {
    let shown_names =
        shorten_shared_prefixes(&crate_names[..crate_names.len().min(SHOWN_NAME_LIMIT)]);
    let hidden_name_count = crate_names.len().saturating_sub(SHOWN_NAME_LIMIT);
    if hidden_name_count > 0 {
        format!("{shown_names}, +{hidden_name_count} more")
    } else {
        shown_names
    }
}

fn shorten_shared_prefixes(crate_names: &[String]) -> String {
    crate_names
        .iter()
        .fold(
            ShortenedNames {
                group_prefix: String::new(),
                shortened_names: Vec::new(),
            },
            |mut shortened_names, crate_name| {
                match crate_name.strip_prefix(&shortened_names.group_prefix) {
                    Some(name_rest) if !shortened_names.group_prefix.is_empty() => {
                        shortened_names
                            .shortened_names
                            .push(format!("-{name_rest}"));
                    }
                    _ => {
                        shortened_names.group_prefix = crate_name
                            .split_once('-')
                            .map(|(name_prefix, _)| format!("{name_prefix}-"))
                            .unwrap_or_default();
                        shortened_names.shortened_names.push(crate_name.clone());
                    }
                }
                shortened_names
            },
        )
        .shortened_names
        .join(", ")
}

struct ShortenedNames {
    group_prefix: String,
    shortened_names: Vec<String>,
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

    fn seed_report(member_names: &[&str], workspace_member_count: usize) -> TriggerReport {
        TriggerReport {
            trigger_paths: vec![TriggerPath {
                rebuild_trigger: RebuildTrigger {
                    trigger_label: "$BINDGEN_EXTRA_CLANG_ARGS".to_string(),
                    trigger_change: TriggerChange::Summary {
                        change_text: "-frandom-seed=a -> -frandom-seed=b".to_string(),
                    },
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
            render_trigger_tree(&seed_report(&["sottomano-shell", "sottomano-sources"], 9));
        [
            "1 cause recompiles 2 of 9 workspace members (last 24h)",
            "2 more causes hidden",
            "1  $BINDGEN_EXTRA_CLANG_ARGS   (recorded 3h ago, reruns on the next build)",
            "├─ change      -frandom-seed=a -> -frandom-seed=b",
            "├─ watched by  bindgen, pipewire-sys (build scripts)",
            "├─ recompiles  sottomano-shell, -sources  via 11 crates",
            "└─ fix         Nix re-seeds",
            "               strip it",
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
        let rendered_text = render_trigger_tree(&store_report);
        [
            "   ├─ change\n",
            "   │  ├─ removed     /nix/store/…-wayland-1.25.0-dev/lib/pkgconfig\n",
            "   │  └─ added       /nix/store/…-wayland-1.26.0-dev/lib/pkgconfig\n",
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
        let rendered_text = render_trigger_tree(&seed_report(&["alpha", "beta"], 2));
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
        let rendered_text = render_trigger_tree(&churn_report);
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
            compact_name_list(&crate_names),
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
        let rendered_text = render_trigger_tree(&seed_report(&member_names, 9));
        assert!(
            rendered_text.contains(
                "├─ recompiles  sottomano-shell, -sources, -state, -widget-showcase, -widgets  \
                 via 11 crates"
            ),
            "{rendered_text}"
        );
    }

    #[test]
    fn explains_empty_report() {
        let rendered_text = render_trigger_tree(&TriggerReport {
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
