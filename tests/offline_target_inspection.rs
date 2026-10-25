use std::{fs, path::Path, process::Command};

use assert_cmd::{cargo, prelude::*};
use tempfile::TempDir;

fn write_project_file(project_root: &Path, relative_path: &str, contents: &str) {
    let file_path = project_root.join(relative_path);
    fs::create_dir_all(file_path.parent().unwrap()).unwrap();
    fs::write(file_path, contents).unwrap();
}

fn run_cargo_frequent(
    project_root: &Path,
    subcommand_args: &[&str],
    env_overrides: &[(&str, &str)],
) -> String {
    let mut frequent_command = Command::new(cargo::cargo_bin!("cargo-frequent"));
    frequent_command
        .args(subcommand_args)
        .arg("--path")
        .arg(project_root);
    for (variable_name, value) in env_overrides {
        frequent_command.env(variable_name, value);
    }
    let assertion = frequent_command.assert().success();
    String::from_utf8_lossy(&assertion.get_output().stdout).into_owned()
}

fn run_cargo_check(project_root: &Path, check_args: &[&str], env_overrides: &[(&str, &str)]) {
    let mut check_command = Command::new("cargo");
    check_command
        .arg("check")
        .args(check_args)
        .current_dir(project_root);
    for (variable_name, value) in env_overrides {
        check_command.env(variable_name, value);
    }
    check_command.assert().success();
}

#[test]
fn env_drift_isolates_changed_random_seed() {
    let project_directory = TempDir::new().unwrap();
    let project_root = project_directory.path();
    write_project_file(
        project_root,
        "Cargo.toml",
        r#"
[package]
name = "drift-probe"
version = "0.1.0"
edition = "2021"
"#,
    );
    write_project_file(project_root, "src/lib.rs", "");
    write_project_file(
        project_root,
        "build.rs",
        r#"fn main() { println!("cargo:rerun-if-env-changed=DRIFT_PROBE"); }"#,
    );

    run_cargo_check(
        project_root,
        &[],
        &[("DRIFT_PROBE", "-O2 -frandom-seed=aaa")],
    );

    let unchanged_report = run_cargo_frequent(
        project_root,
        &["env-drift"],
        &[("DRIFT_PROBE", "-O2 -frandom-seed=aaa")],
    );
    assert!(
        unchanged_report.contains("No build script would rerun"),
        "same env should not drift: {unchanged_report}"
    );

    let drifted_report = run_cargo_frequent(
        project_root,
        &["env-drift"],
        &[("DRIFT_PROBE", "-O2 -frandom-seed=bbb")],
    );
    assert!(drifted_report.contains("$DRIFT_PROBE"), "{drifted_report}");
    assert!(drifted_report.contains("drift-probe"), "{drifted_report}");
    assert!(
        drifted_report.contains("-frandom-seed=aaa -> -frandom-seed=bbb"),
        "{drifted_report}"
    );
    assert!(
        drifted_report.contains("Nix -frandom-seed"),
        "{drifted_report}"
    );

    let live_report = run_cargo_frequent(
        project_root,
        &[],
        &[("DRIFT_PROBE", "-O2 -frandom-seed=bbb")],
    );
    assert!(live_report.contains("$DRIFT_PROBE"), "{live_report}");
    assert!(
        live_report.contains("hint: only the Nix -frandom-seed flag changed"),
        "{live_report}"
    );
}

#[test]
fn unit_churn_traces_package_selection_to_features() {
    let workspace_directory = TempDir::new().unwrap();
    let workspace_root = workspace_directory.path();
    write_project_file(
        workspace_root,
        "Cargo.toml",
        r#"
[workspace]
members = ["shared", "alpha", "beta"]
resolver = "2"
"#,
    );
    write_project_file(
        workspace_root,
        "shared/Cargo.toml",
        r#"
[package]
name = "shared"
version = "0.1.0"
edition = "2021"

[features]
extra = []
"#,
    );
    write_project_file(workspace_root, "shared/src/lib.rs", "");
    for (member_name, shared_features) in [("alpha", r#", features = ["extra"]"#), ("beta", "")] {
        write_project_file(
            workspace_root,
            &format!("{member_name}/Cargo.toml"),
            &format!(
                r#"
[package]
name = "{member_name}"
version = "0.1.0"
edition = "2021"

[dependencies]
shared = {{ path = "../shared"{shared_features} }}
"#
            ),
        );
        write_project_file(workspace_root, &format!("{member_name}/src/lib.rs"), "");
    }

    run_cargo_check(workspace_root, &["-p", "beta"], &[]);
    run_cargo_check(workspace_root, &["-p", "alpha"], &[]);

    let churn_json = run_cargo_frequent(workspace_root, &["unit-churn", "--json"], &[]);
    let churn_report: serde_json::Value = serde_json::from_str(&churn_json).expect("valid JSON");
    let leaf_causes = churn_report["leaf_causes"].as_array().expect("leaf causes");
    let shared_feature_cause = leaf_causes
        .iter()
        .find(|leaf_cause| leaf_cause["difference"]["Features"]["crate_name"] == "shared")
        .unwrap_or_else(|| panic!("shared feature churn expected: {churn_json}"));
    assert_eq!(
        shared_feature_cause["difference"]["Features"]["only_newer"],
        serde_json::json!(["extra"])
    );
    assert!(
        shared_feature_cause["fix_hints"][0]["suggestion"]
            .as_str()
            .unwrap()
            .contains("feature-unification"),
        "{churn_json}"
    );

    let churn_text = run_cargo_frequent(workspace_root, &["unit-churn"], &[]);
    assert!(
        churn_text.contains("shared: features +extra"),
        "{churn_text}"
    );
}
