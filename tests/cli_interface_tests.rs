use std::{fs, process::Command};

use assert_cmd::{cargo, prelude::*};
use tempfile::TempDir;

#[test]
fn cli_reports_error_for_invalid_project_path() {
    let mut cmd = Command::new(cargo::cargo_bin!("cargo-frequent"));
    cmd.arg("--path").arg("/nonexistent/path");

    let output = cmd.assert().failure();
    let stderr = String::from_utf8_lossy(&output.get_output().stderr);
    assert!(
        stderr.contains("Cargo.toml not found"),
        "Expected stderr to contain 'Cargo.toml not found', got: {stderr}"
    );
}

#[test]
fn cli_supports_different_cargo_commands() {
    let temp_dir = TempDir::new().unwrap();
    let cargo_toml = temp_dir.path().join("Cargo.toml");

    fs::write(
        &cargo_toml,
        r#"
[package]
name = "test-project"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();

    let src_dir = temp_dir.path().join("src");
    fs::create_dir(&src_dir).unwrap();
    fs::write(src_dir.join("main.rs"), "fn main() {}").unwrap();

    for command in &["check", "build", "test"] {
        let mut cmd = Command::new(cargo::cargo_bin!("cargo-frequent"));
        cmd.arg("--path").arg(temp_dir.path());
        cmd.arg("--command").arg(command);

        let output = cmd.assert().success();
        let stdout = String::from_utf8_lossy(&output.get_output().stdout);
        assert!(
            !stdout.is_empty(),
            "expected some report output for `cargo {command}`"
        );
    }
}
