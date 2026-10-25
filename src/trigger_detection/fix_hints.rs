use serde::Serialize;

use crate::trigger_detection::{env_value_change::EnvValueChange, unit_hash_churn::ChurnCause};

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct FixHint {
    pub summary: String,
    pub suggestion: String,
}

const VOLATILE_EXACT_NAMES: &[&str] = &[
    "PATH",
    "PWD",
    "OLDPWD",
    "SHLVL",
    "SHELL",
    "TERM",
    "TERM_PROGRAM",
    "TERM_PROGRAM_VERSION",
    "TERM_SESSION_ID",
    "HOME",
    "USER",
    "LOGNAME",
    "TMPDIR",
    "VIRTUAL_ENV",
    "VIRTUAL_ENV_PROMPT",
    "_",
];

const VOLATILE_NAME_PREFIXES: &[&str] = &[
    "XDG_",
    "CONDA_",
    "DIRENV_",
    "SSH_",
    "ITERM_",
    "VSCODE_",
    "TERMINFO_",
];

#[must_use]
pub fn is_volatile_variable(variable_name: &str) -> bool {
    VOLATILE_EXACT_NAMES.contains(&variable_name)
        || VOLATILE_NAME_PREFIXES
            .iter()
            .any(|name_prefix| variable_name.starts_with(name_prefix))
}

#[must_use]
pub fn is_nix_store_update(env_change: &EnvValueChange) -> bool {
    let changed_tokens = env_change.changed_tokens();
    matches!(env_change, EnvValueChange::TokensReplaced { .. })
        && !changed_tokens.is_empty()
        && changed_tokens
            .iter()
            .all(|token| token.contains("/nix/store/"))
}

#[must_use]
pub fn env_change_hints(variable_name: &str, env_change: &EnvValueChange) -> Vec<FixHint> {
    let changed_tokens = env_change.changed_tokens();
    let only_tokens_matching = |token_predicate: fn(&str) -> bool| {
        !changed_tokens.is_empty() && changed_tokens.iter().all(|token| token_predicate(token))
    };
    let shape_hint = match env_change {
        EnvValueChange::Reordered { .. } => Some(FixHint {
            summary: "same entries, different order".to_string(),
            suggestion: format!(
                "normalize {variable_name} in your shell setup (sort and dedupe the entries)"
            ),
        }),
        EnvValueChange::TokensReplaced { .. }
            if only_tokens_matching(|token| token.starts_with("-frandom-seed=")) =>
        {
            Some(FixHint {
                summary: "Nix picks a new -frandom-seed whenever the dev shell is re-evaluated"
                    .to_string(),
                suggestion: format!(
                    "strip it in shellHook: export {variable_name}=\"$(printf '%s' \
                     \"${variable_name}\" | sed -E 's| ?-frandom-seed=[^ ]+||g')\""
                ),
            })
        }
        EnvValueChange::TokensReplaced { .. }
            if only_tokens_matching(|token| token.contains("/nix/store/")) =>
        {
            Some(FixHint {
                summary: "Nix store paths changed".to_string(),
                suggestion: "dev shell inputs were updated; expect one rebuild. If it keeps \
                             flipping, two shells share this target dir"
                    .to_string(),
            })
        }
        EnvValueChange::Set { .. } | EnvValueChange::Unset { .. } => Some(FixHint {
            summary: format!("{variable_name} is set in one environment but not in the other"),
            suggestion: "run cargo from one environment only, or give the other tool (editor, \
                         agent) its own target dir"
                .to_string(),
        }),
        EnvValueChange::TokensReplaced { .. } => None,
    };
    let volatile_hint = is_volatile_variable(variable_name).then(|| FixHint {
        summary: format!("{variable_name} differs between shells by nature"),
        suggestion: "pin the tool the build script is looking for (e.g. PYO3_PYTHON) so it stops \
                     watching this variable"
            .to_string(),
    });
    shape_hint.into_iter().chain(volatile_hint).collect()
}

#[must_use]
pub fn churn_cause_hints(
    churn_cause: &ChurnCause,
    default_members_select_subset: bool,
) -> Vec<FixHint> {
    match churn_cause {
        ChurnCause::Features { .. } | ChurnCause::DependencySet { .. } => {
            let default_members_note = if default_members_select_subset {
                " This workspace sets default-members, so plain `cargo build` selects fewer \
                 packages than `--workspace`."
            } else {
                ""
            };
            vec![FixHint {
                summary: "feature unification depends on which packages a command selects"
                    .to_string(),
                suggestion: format!(
                    "unify per workspace: `[resolver] feature-unification = \"workspace\"` in \
                     .cargo/config.toml (nightly also needs `[unstable] feature-unification = \
                     true`), or enable the feature in every dependent.{default_members_note}"
                ),
            }]
        }
        ChurnCause::Profile { .. } => vec![FixHint {
            summary: "build and test profiles each keep their own copy of a unit".to_string(),
            suggestion: "after an edit, every profile you alternate between recompiles once; \
                         align the profile settings to share units"
                .to_string(),
        }],
        ChurnCause::Rustflags { .. } => vec![FixHint {
            summary: "RUSTFLAGS differ between invocations".to_string(),
            suggestion: "set them once in .cargo/config.toml, or give the other tool its own \
                         target dir"
                .to_string(),
        }],
        ChurnCause::Toolchain { .. } => vec![FixHint {
            summary: "different compilers share the target dir".to_string(),
            suggestion: "point the other toolchain (editor, CI script) at its own target dir"
                .to_string(),
        }],
        ChurnCause::CargoConfig { .. } => vec![FixHint {
            summary: "cargo configuration differs between invocations".to_string(),
            suggestion: "check [env], [target] and [build] for values that depend on the calling \
                         environment"
                .to_string(),
        }],
        ChurnCause::LockfileVersion { .. } => vec![FixHint {
            summary: "a dependency version changed".to_string(),
            suggestion: "Cargo.lock changed; one rebuild is expected".to_string(),
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_nix_random_seed() {
        let fix_hints = env_change_hints(
            "BINDGEN_EXTRA_CLANG_ARGS",
            &EnvValueChange::between("-O2 -frandom-seed=aaa", "-O2 -frandom-seed=bbb").unwrap(),
        );
        assert!(
            fix_hints[0].summary.contains("-frandom-seed"),
            "{fix_hints:?}"
        );
        assert!(
            fix_hints[0].suggestion.contains("BINDGEN_EXTRA_CLANG_ARGS"),
            "{fix_hints:?}"
        );
    }

    #[test]
    fn recognizes_reordered_paths() {
        let fix_hints = env_change_hints(
            "PKG_CONFIG_PATH",
            &EnvValueChange::between("/a:/b", "/b:/a").unwrap(),
        );
        assert_eq!(fix_hints[0].summary, "same entries, different order");
    }

    #[test]
    fn recognizes_store_path_updates() {
        let fix_hints = env_change_hints(
            "PKG_CONFIG_PATH",
            &EnvValueChange::between(
                "/nix/store/aaa-wayland-1.25/lib:/usr/lib",
                "/nix/store/bbb-wayland-1.26/lib:/usr/lib",
            )
            .unwrap(),
        );
        assert_eq!(fix_hints[0].summary, "Nix store paths changed");
    }

    #[test]
    fn recognizes_variable_missing_in_one_environment() {
        let fix_hints = env_change_hints("LIBCLANG_PATH", &EnvValueChange::unset_from("/clang"));
        assert!(
            fix_hints[0].summary.contains("not in the other"),
            "{fix_hints:?}"
        );
    }

    #[test]
    fn recognizes_volatile_variable() {
        let fix_hints = env_change_hints(
            "PATH",
            &EnvValueChange::between("/bin", "/usr/bin").unwrap(),
        );
        assert!(
            fix_hints
                .iter()
                .any(|fix_hint| fix_hint.summary.contains("by nature")),
            "{fix_hints:?}"
        );
        assert!(!is_volatile_variable("CC"), "CC is not volatile");
        assert!(is_volatile_variable("XDG_CONFIG_HOME"), "XDG prefix");
    }

    #[test]
    fn feature_churn_mentions_default_members_when_relevant() {
        let feature_cause = ChurnCause::Features {
            crate_name: "syn".to_string(),
            removed_features: vec![],
            added_features: vec!["visit".to_string()],
        };
        assert!(
            churn_cause_hints(&feature_cause, true)[0]
                .suggestion
                .contains("default-members"),
            "note expected"
        );
        assert!(
            !churn_cause_hints(&feature_cause, false)[0]
                .suggestion
                .contains("default-members"),
            "no note"
        );
    }
}
