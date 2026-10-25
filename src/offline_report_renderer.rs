use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write,
};

use crate::{
    fix_hint_library::FixHint,
    recorded_env_drift::RecordedEnvDrift,
    unit_hash_churn::{LeafUnitDifference, UnitChurnReport},
};

const SHOWN_LEAF_CAUSE_LIMIT: usize = 10;
const SHOWN_CHURNING_UNIT_LIMIT: usize = 15;
const SHOWN_CRATE_NAME_LIMIT: usize = 6;

pub fn render_env_drift(env_drifts: &[RecordedEnvDrift], report_text: &mut String) {
    if env_drifts.is_empty() {
        report_text.push_str(
            "No build script would rerun: every recorded env value matches the current \
             environment.\n",
        );
        return;
    }
    report_text.push_str(
        "Build scripts that will rerun because the environment differs from their last run:\n\n",
    );
    for env_drift in env_drifts {
        let volatile_marker = if env_drift.volatile {
            " (volatile)"
        } else {
            ""
        };
        let _ = writeln!(report_text, "${}{volatile_marker}", env_drift.variable_name);
        let _ = writeln!(
            report_text,
            "  watched by: {}",
            abbreviate_crate_names(&env_drift.watching_crates)
        );
        let _ = writeln!(report_text, "  change:     {}", env_drift.token_diff);
        let _ = writeln!(
            report_text,
            "  recorded:   {} ago",
            describe_elapsed_seconds(env_drift.newest_recorded_run_seconds_ago)
        );
        render_fix_hints(&env_drift.fix_hints, report_text);
        report_text.push('\n');
    }
}

pub fn render_unit_churn(churn_report: &UnitChurnReport, report_text: &mut String) {
    if churn_report.churning_units.is_empty() {
        report_text.push_str("No crate has more than one unit hash in the target dir.\n");
        return;
    }

    report_text.push_str("Leaf causes of unit-hash churn (most explained crates first):\n");
    for leaf_cause in churn_report.leaf_causes.iter().take(SHOWN_LEAF_CAUSE_LIMIT) {
        let _ = writeln!(
            report_text,
            "  {}\n    explains {}: {}",
            describe_leaf_difference(&leaf_cause.difference),
            leaf_cause.explained_crates.len(),
            abbreviate_crate_names(&leaf_cause.explained_crates)
        );
    }
    if churn_report.leaf_causes.len() > SHOWN_LEAF_CAUSE_LIMIT {
        let _ = writeln!(
            report_text,
            "  … {} more (use --json)",
            churn_report.leaf_causes.len() - SHOWN_LEAF_CAUSE_LIMIT
        );
    }

    let distinct_fix_hints: BTreeSet<&FixHint> = churn_report
        .leaf_causes
        .iter()
        .take(SHOWN_LEAF_CAUSE_LIMIT)
        .flat_map(|leaf_cause| &leaf_cause.fix_hints)
        .collect();
    if !distinct_fix_hints.is_empty() {
        report_text.push('\n');
        render_fix_hints(
            &distinct_fix_hints.into_iter().cloned().collect::<Vec<_>>(),
            report_text,
        );
    }

    report_text.push_str("\nCrates with several unit hashes (built in window / total):\n");
    let merged_unit_rows = merge_rows_by_crate_and_kind(churn_report);
    for ((crate_name, unit_kind, profile_directory), (recent_unit_count, unit_count)) in
        merged_unit_rows.iter().take(SHOWN_CHURNING_UNIT_LIMIT)
    {
        let _ = writeln!(
            report_text,
            "  {crate_name:<32} {recent_unit_count:>3} / {unit_count:<3} {unit_kind} \
             [{profile_directory}]"
        );
    }
    if merged_unit_rows.len() > SHOWN_CHURNING_UNIT_LIMIT {
        let _ = writeln!(
            report_text,
            "  … {} more (use --json)",
            merged_unit_rows.len() - SHOWN_CHURNING_UNIT_LIMIT
        );
    }
}

type MergedUnitRow<'report> = ((&'report str, &'report str, &'report str), (usize, usize));

fn merge_rows_by_crate_and_kind(churn_report: &UnitChurnReport) -> Vec<MergedUnitRow<'_>> {
    let mut counts_by_row: BTreeMap<(&str, &str, &str), (usize, usize)> = BTreeMap::new();
    for churn_finding in &churn_report.churning_units {
        let (recent_unit_count, unit_count) = counts_by_row
            .entry((
                &churn_finding.crate_name,
                &churn_finding.unit_kind,
                &churn_finding.profile_directory,
            ))
            .or_default();
        *recent_unit_count += churn_finding.recent_unit_count;
        *unit_count += churn_finding.unit_count;
    }
    let mut merged_unit_rows: Vec<MergedUnitRow<'_>> = counts_by_row.into_iter().collect();
    merged_unit_rows.sort_by(|(_, left_counts), (_, right_counts)| right_counts.cmp(left_counts));
    merged_unit_rows
}

pub fn render_fix_hints(fix_hints: &[FixHint], report_text: &mut String) {
    for fix_hint in fix_hints {
        let _ = writeln!(report_text, "  hint: {}", fix_hint.summary);
        let _ = writeln!(report_text, "        {}", fix_hint.suggestion);
    }
}

fn describe_leaf_difference(leaf_difference: &LeafUnitDifference) -> String {
    match leaf_difference {
        LeafUnitDifference::Features {
            crate_name,
            only_older,
            only_newer,
        } => format!(
            "{crate_name}: features {}",
            describe_added_removed(only_older, only_newer)
        ),
        LeafUnitDifference::DependencySet {
            crate_name,
            only_older,
            only_newer,
        } => format!(
            "{crate_name}: dependencies {}",
            describe_added_removed(only_older, only_newer)
        ),
        LeafUnitDifference::Profile { crate_name } => format!("{crate_name}: profile"),
        LeafUnitDifference::Rustflags {
            crate_name,
            older_flags,
            newer_flags,
        } => format!(
            "{crate_name}: rustflags [{}] vs [{}]",
            older_flags.join(" "),
            newer_flags.join(" ")
        ),
        LeafUnitDifference::Rustc { crate_name } => format!("{crate_name}: rustc version"),
        LeafUnitDifference::TargetConfig { crate_name } => format!("{crate_name}: target config"),
        LeafUnitDifference::CargoConfig { crate_name } => format!("{crate_name}: cargo config"),
        LeafUnitDifference::SourceVersion { crate_name } => format!("{crate_name}: source version"),
        LeafUnitDifference::UnresolvedDependency {
            crate_name,
            dependency_name,
        } => format!("{crate_name}: dependency {dependency_name} (older unit no longer on disk)"),
        LeafUnitDifference::LocalInputsOnly { crate_name } => {
            format!("{crate_name}: same inputs, different hash")
        }
    }
}

fn describe_added_removed(only_older: &[String], only_newer: &[String]) -> String {
    only_newer
        .iter()
        .map(|name| format!("+{name}"))
        .chain(only_older.iter().map(|name| format!("-{name}")))
        .collect::<Vec<_>>()
        .join(" ")
}

fn describe_elapsed_seconds(elapsed_seconds: u64) -> String {
    match elapsed_seconds {
        0..60 => format!("{elapsed_seconds}s"),
        60..3_600 => format!("{}m", elapsed_seconds / 60),
        3_600..86_400 => format!("{}h", elapsed_seconds / 3_600),
        _ => format!("{}d", elapsed_seconds / 86_400),
    }
}

fn abbreviate_crate_names(crate_names: &[String]) -> String {
    let shown_names = crate_names
        .iter()
        .take(SHOWN_CRATE_NAME_LIMIT)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    if crate_names.len() > SHOWN_CRATE_NAME_LIMIT {
        format!(
            "{shown_names} (+{} more)",
            crate_names.len() - SHOWN_CRATE_NAME_LIMIT
        )
    } else {
        shown_names
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env_value_token_diff::EnvValueTokenDiff;

    #[test]
    fn renders_drift_with_token_change_and_hint() {
        let env_drift = RecordedEnvDrift {
            variable_name: "BINDGEN_EXTRA_CLANG_ARGS".to_string(),
            watching_crates: vec!["bindgen".to_string(), "pipewire-sys".to_string()],
            token_diff: EnvValueTokenDiff::between(
                Some("-frandom-seed=a"),
                Some("-frandom-seed=b"),
            )
            .unwrap(),
            volatile: false,
            newest_recorded_run_seconds_ago: 7_200,
            fix_hints: vec![FixHint {
                summary: "seed".to_string(),
                suggestion: "strip".to_string(),
            }],
        };
        let mut report_text = String::new();
        render_env_drift(&[env_drift], &mut report_text);
        assert!(
            report_text.contains("$BINDGEN_EXTRA_CLANG_ARGS"),
            "{report_text}"
        );
        assert!(
            report_text.contains("bindgen, pipewire-sys"),
            "{report_text}"
        );
        assert!(
            report_text.contains("-frandom-seed=a -> -frandom-seed=b"),
            "{report_text}"
        );
        assert!(report_text.contains("hint: seed"), "{report_text}");
        assert!(report_text.contains("recorded:   2h ago"), "{report_text}");
    }

    #[test]
    fn describes_feature_difference() {
        assert_eq!(
            describe_leaf_difference(&LeafUnitDifference::Features {
                crate_name: "syn".to_string(),
                only_older: vec!["fold".to_string()],
                only_newer: vec!["visit".to_string()],
            }),
            "syn: features +visit -fold"
        );
    }
}
