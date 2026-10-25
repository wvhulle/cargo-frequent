use std::env;

const CARGO_RUN_EXACT_VARIABLES: &[&str] = &[
    "CARGO_MANIFEST_DIR",
    "CARGO_MANIFEST_PATH",
    "CARGO_CRATE_NAME",
    "CARGO_BIN_NAME",
    "CARGO_PRIMARY_PACKAGE",
    "CARGO_TARGET_TMPDIR",
    "OUT_DIR",
];
const CARGO_RUN_VARIABLE_PREFIXES: &[&str] = &["CARGO_PKG_", "CARGO_BIN_EXE_"];

#[must_use]
pub fn is_cargo_run_variable(variable_name: &str) -> bool {
    CARGO_RUN_EXACT_VARIABLES.contains(&variable_name)
        || CARGO_RUN_VARIABLE_PREFIXES
            .iter()
            .any(|variable_prefix| variable_name.starts_with(variable_prefix))
}

#[must_use]
pub fn inherited_cargo_run_variables() -> Vec<String> {
    env::vars_os()
        .filter_map(|environment_entry| environment_entry.0.into_string().ok())
        .filter(|variable_name| is_cargo_run_variable(variable_name))
        .collect()
}

#[must_use]
pub fn lookup_terminal_env(variable_name: &str) -> Option<String> {
    if is_cargo_run_variable(variable_name) {
        return None;
    }
    env::var(variable_name).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_variables_that_cargo_run_sets() {
        assert!(is_cargo_run_variable("CARGO_MANIFEST_DIR"), "exact name");
        assert!(is_cargo_run_variable("CARGO_PKG_VERSION_MAJOR"), "prefix");
        assert!(!is_cargo_run_variable("CARGO_TARGET_DIR"), "user setting");
        assert!(!is_cargo_run_variable("RUSTC"), "user may set it");
        assert_eq!(lookup_terminal_env("CARGO_MANIFEST_DIR"), None);
    }
}
