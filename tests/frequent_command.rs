use std::{fs, path::Path, process::Command};

use assert_cmd::{cargo, prelude::*};
use tempfile::TempDir;

fn write_single_crate(project_root: &Path) {
    fs::write(
        project_root.join("Cargo.toml"),
        "[package]\nname = \"edited-crate\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::create_dir_all(project_root.join("src")).unwrap();
    fs::write(
        project_root.join("src/lib.rs"),
        "pub const VALUE: u8 = 1;\n",
    )
    .unwrap();
}

fn frequent_command(project_root: &Path) -> Command {
    let mut frequent_command = Command::new(cargo::cargo_bin!("cargo-frequent"));
    frequent_command
        .arg("--path")
        .arg(project_root)
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("CARGO_BUILD_TARGET_DIR");
    frequent_command
}

#[test]
fn help_lists_the_flags() {
    let help_output = Command::new(cargo::cargo_bin!("cargo-frequent"))
        .arg("--help")
        .assert()
        .success();
    let help_text = String::from_utf8_lossy(&help_output.get_output().stdout).into_owned();
    ["--build", "--graph", "--all", "--json", "--path"]
        .into_iter()
        .for_each(|flag_name| {
            assert!(
                help_text.contains(flag_name),
                "{flag_name} missing:\n{help_text}"
            );
        });
}

#[test]
fn accepts_cargo_subcommand_form() {
    Command::new(cargo::cargo_bin!("cargo-frequent"))
        .args(["frequent", "--help"])
        .assert()
        .success();
}

#[test]
fn reports_missing_manifest() {
    let empty_directory = TempDir::new().unwrap();
    let failure_output = frequent_command(empty_directory.path()).assert().failure();
    let error_text = String::from_utf8_lossy(&failure_output.get_output().stderr).into_owned();
    assert!(error_text.contains("Cargo.toml not found"), "{error_text}");
}

#[test]
fn quiet_project_has_no_triggers() {
    let project_directory = TempDir::new().unwrap();
    write_single_crate(project_directory.path());
    Command::new("cargo")
        .args(["check", "--quiet"])
        .current_dir(project_directory.path())
        .env_remove("CARGO_TARGET_DIR")
        .assert()
        .success();
    let report_output = frequent_command(project_directory.path())
        .assert()
        .success();
    let report_text = String::from_utf8_lossy(&report_output.get_output().stdout).into_owned();
    assert!(
        report_text.contains("No recurring rebuild triggers"),
        "{report_text}"
    );
}

#[test]
fn build_flag_shows_edited_file() {
    let project_directory = TempDir::new().unwrap();
    write_single_crate(project_directory.path());
    Command::new("cargo")
        .args(["check", "--quiet"])
        .current_dir(project_directory.path())
        .env_remove("CARGO_TARGET_DIR")
        .assert()
        .success();
    fs::write(
        project_directory.path().join("src/lib.rs"),
        "pub const VALUE: u8 = 2;\n",
    )
    .unwrap();

    let report_output = frequent_command(project_directory.path())
        .arg("--build")
        .assert()
        .success();
    let report_text = String::from_utf8_lossy(&report_output.get_output().stdout).into_owned();
    assert!(report_text.contains("src/lib.rs"), "{report_text}");
    assert!(report_text.contains("edited-crate"), "{report_text}");
    assert!(report_text.contains("seen in this build"), "{report_text}");
}

#[test]
fn json_output_lists_trigger_paths() {
    let project_directory = TempDir::new().unwrap();
    write_single_crate(project_directory.path());
    Command::new("cargo")
        .args(["check", "--quiet"])
        .current_dir(project_directory.path())
        .env_remove("CARGO_TARGET_DIR")
        .assert()
        .success();
    fs::write(
        project_directory.path().join("src/lib.rs"),
        "pub const VALUE: u8 = 3;\n",
    )
    .unwrap();

    let json_output = frequent_command(project_directory.path())
        .args(["--json", "--build"])
        .assert()
        .success();
    let trigger_paths: serde_json::Value =
        serde_json::from_slice(&json_output.get_output().stdout).expect("valid JSON");
    assert_eq!(
        trigger_paths["trigger_paths"][0]["rebuild_trigger"]["trigger_label"], "src/lib.rs",
        "{trigger_paths}"
    );
}
