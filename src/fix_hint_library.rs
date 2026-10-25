use serde::Serialize;

use crate::{
    build_script_rerun_directives::is_volatile, env_value_token_diff::EnvValueTokenDiff,
    unit_hash_churn::LeafUnitDifference,
};

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct FixHint {
    pub summary: String,
    pub suggestion: String,
}

impl FixHint {
    fn new(summary: impl Into<String>, suggestion: impl Into<String>) -> Self {
        Self {
            summary: summary.into(),
            suggestion: suggestion.into(),
        }
    }
}

#[must_use]
pub fn hints_for_env_change(variable_name: &str, token_diff: &EnvValueTokenDiff) -> Vec<FixHint> {
    let mut fix_hints = Vec::new();
    let changed_tokens: Vec<&str> = token_diff.changed_tokens().collect();

    if !token_diff.reordered_only
        && !changed_tokens.is_empty()
        && changed_tokens
            .iter()
            .all(|token| token.starts_with("-frandom-seed="))
    {
        fix_hints.push(FixHint::new(
            "only the Nix -frandom-seed flag changed",
            format!(
                "nixpkgs' reproducible-builds hook derives a new seed whenever the dev shell is \
                 re-evaluated. Strip it in the shellHook: export {variable_name}=\"$(printf '%s' \
                 \"${variable_name}\" | sed -E 's| ?-frandom-seed=[^ ]+||g')\""
            ),
        ));
    } else if token_diff.reordered_only {
        fix_hints.push(FixHint::new(
            "same entries, different order",
            format!(
                "Shells assemble {variable_name} in different orders. Normalize it in the shell \
                 setup (sort and dedupe the entries) or pin it explicitly."
            ),
        ));
    } else if !changed_tokens.is_empty()
        && changed_tokens
            .iter()
            .all(|token| token.contains("/nix/store/"))
    {
        fix_hints.push(FixHint::new(
            "Nix store paths changed",
            "The dev shell inputs were updated (flake update, nixpkgs bump). Expect one rebuild; \
             if it keeps flipping, two different shells (e.g. editor vs terminal) are sharing the \
             target dir.",
        ));
    }

    if token_diff.became_set() || token_diff.became_unset() {
        fix_hints.push(FixHint::new(
            format!("{variable_name} is set in one environment but not the other"),
            "Cargo was likely run both inside and outside the dev shell (editor, another \
             terminal, an AI agent). Run every cargo invocation from the same environment or give \
             the other one its own target dir.",
        ));
    }

    if is_volatile(variable_name) {
        fix_hints.push(FixHint::new(
            format!("{variable_name} varies between shells by nature"),
            "Pin the tool location the build script is really looking for (for example \
             PYO3_PYTHON for pyo3-build-config) so it stops watching this variable.",
        ));
    }

    fix_hints
}

#[must_use]
pub fn hints_for_leaf_difference(
    leaf_difference: &LeafUnitDifference,
    default_members_select_subset: bool,
) -> Vec<FixHint> {
    let feature_unification_hint = || {
        let default_members_note = if default_members_select_subset {
            " The workspace sets default-members, so plain `cargo build` selects fewer packages \
             than `--workspace`."
        } else {
            ""
        };
        FixHint::new(
            "feature unification depends on which packages are selected",
            format!(
                "`-p <pkg>`, default-members and `--workspace` each unify a different feature \
                 set, so shared dependencies get a new hash when you switch \
                 commands.{default_members_note} Unify across the whole workspace with \
                 `[resolver] feature-unification = \"workspace\"` in .cargo/config.toml (nightly \
                 also needs `[unstable] feature-unification = true`), or add the missing features \
                 to the dependency declarations directly."
            ),
        )
    };
    match leaf_difference {
        LeafUnitDifference::Features { .. } | LeafUnitDifference::DependencySet { .. } => {
            vec![feature_unification_hint()]
        }
        LeafUnitDifference::Profile { .. } => vec![FixHint::new(
            "different profiles",
            "dev, test and custom profiles (and their build-override settings for build scripts \
             and proc macros) each keep their own copy of every unit, so after an edit each \
             profile you alternate between recompiles it once. Align the profiles' settings, or \
             accept it as the cost of switching between `cargo build` and `cargo test`.",
        )],
        LeafUnitDifference::Rustflags { .. } => vec![FixHint::new(
            "RUSTFLAGS differ between invocations",
            "Different shells or tools pass different RUSTFLAGS. Set them once in \
             .cargo/config.toml or give the other tool its own target dir.",
        )],
        LeafUnitDifference::Rustc { .. } => vec![FixHint::new(
            "different compilers share the target dir",
            "Another toolchain (for example an editor using a different rustc) builds into the \
             same target dir; point it at its own target dir.",
        )],
        LeafUnitDifference::TargetConfig { .. } | LeafUnitDifference::CargoConfig { .. } => {
            vec![FixHint::new(
                "cargo configuration differs between invocations",
                "Check [env], [target] and [build] sections for values that depend on the \
                 invoking environment.",
            )]
        }
        LeafUnitDifference::SourceVersion { .. } => vec![FixHint::new(
            "dependency version changed",
            "Cargo.lock changed; a one-off rebuild is expected.",
        )],
        LeafUnitDifference::UnresolvedDependency { .. } => vec![FixHint::new(
            "older unit of the dependency was overwritten",
            "Run the analysis again right after the next unexpected rebuild to trace further.",
        )],
        LeafUnitDifference::LocalInputsOnly { .. } => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diff_between(
        previous_value: Option<&str>,
        current_value: Option<&str>,
    ) -> EnvValueTokenDiff {
        EnvValueTokenDiff::between(previous_value, current_value).expect("values differ")
    }

    #[test]
    fn recognizes_nix_random_seed() {
        let fix_hints = hints_for_env_change(
            "BINDGEN_EXTRA_CLANG_ARGS",
            &diff_between(Some("-O2 -frandom-seed=aaa"), Some("-O2 -frandom-seed=bbb")),
        );
        assert!(
            fix_hints[0].summary.contains("frandom-seed"),
            "{fix_hints:?}"
        );
        assert!(
            fix_hints[0].suggestion.contains("BINDGEN_EXTRA_CLANG_ARGS"),
            "{fix_hints:?}"
        );
    }

    #[test]
    fn recognizes_reordered_paths() {
        let fix_hints = hints_for_env_change(
            "PKG_CONFIG_PATH",
            &diff_between(Some("/a:/b"), Some("/b:/a")),
        );
        assert_eq!(fix_hints[0].summary, "same entries, different order");
    }

    #[test]
    fn recognizes_store_path_updates() {
        let fix_hints = hints_for_env_change(
            "PKG_CONFIG_PATH",
            &diff_between(
                Some("/nix/store/aaa-wayland-1.25/lib:/usr/lib"),
                Some("/nix/store/bbb-wayland-1.26/lib:/usr/lib"),
            ),
        );
        assert_eq!(fix_hints[0].summary, "Nix store paths changed");
    }

    #[test]
    fn recognizes_variable_missing_outside_shell() {
        let fix_hints = hints_for_env_change("LIBCLANG_PATH", &diff_between(Some("/clang"), None));
        assert!(
            fix_hints
                .iter()
                .any(|fix_hint| fix_hint.summary.contains("not the other")),
            "{fix_hints:?}"
        );
    }

    #[test]
    fn recognizes_volatile_variable() {
        let fix_hints = hints_for_env_change("PATH", &diff_between(Some("/bin"), Some("/usr/bin")));
        assert!(
            fix_hints
                .iter()
                .any(|fix_hint| fix_hint.summary.contains("varies")),
            "{fix_hints:?}"
        );
    }

    #[test]
    fn feature_difference_mentions_default_members() {
        let leaf_difference = LeafUnitDifference::Features {
            crate_name: "syn".to_string(),
            only_older: vec![],
            only_newer: vec!["visit".to_string()],
        };
        let with_default_members = hints_for_leaf_difference(&leaf_difference, true);
        assert!(
            with_default_members[0]
                .suggestion
                .contains("default-members"),
            "note expected"
        );
        assert!(
            with_default_members[0]
                .suggestion
                .contains("feature-unification"),
            "config expected"
        );
        let without_default_members = hints_for_leaf_difference(&leaf_difference, false);
        assert!(
            !without_default_members[0]
                .suggestion
                .contains("sets default-members"),
            "no note"
        );
    }

    #[test]
    fn profile_difference_is_expected() {
        let fix_hints = hints_for_leaf_difference(
            &LeafUnitDifference::Profile {
                crate_name: "serde".to_string(),
            },
            false,
        );
        assert_eq!(fix_hints[0].summary, "different profiles");
    }
}
