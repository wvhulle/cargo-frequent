//! Integration test: spin up a real cargo workspace, modify a leaf crate,
//! and verify the JSON cluster output is well-formed.

use std::{
    fs,
    path::Path,
    process::{Command, Stdio},
};

use assert_cmd::prelude::*;
use tempfile::TempDir;

fn create_workspace_with_dependencies() -> TempDir {
    let temp_dir = TempDir::new().unwrap();

    fs::write(
        temp_dir.path().join("Cargo.toml"),
        r#"
[workspace]
members = ["lib-a", "lib-b", "app"]
resolver = "2"
"#,
    )
    .unwrap();

    let lib_a_dir = temp_dir.path().join("lib-a");
    fs::create_dir_all(lib_a_dir.join("src")).unwrap();
    fs::write(
        lib_a_dir.join("Cargo.toml"),
        r#"
[package]
name = "lib-a"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();
    fs::write(
        lib_a_dir.join("src/lib.rs"),
        r#"
pub fn greet() -> &'static str {
    "Hello from lib-a"
}
"#,
    )
    .unwrap();

    let middle_lib_dir = temp_dir.path().join("lib-b");
    fs::create_dir_all(middle_lib_dir.join("src")).unwrap();
    fs::write(
        middle_lib_dir.join("Cargo.toml"),
        r#"
[package]
name = "lib-b"
version = "0.1.0"
edition = "2021"

[dependencies]
lib-a = { path = "../lib-a" }
"#,
    )
    .unwrap();
    fs::write(
        middle_lib_dir.join("src/lib.rs"),
        r#"
pub fn message() -> String {
    format!("lib-b says: {}", lib_a::greet())
}
"#,
    )
    .unwrap();

    let app_dir = temp_dir.path().join("app");
    fs::create_dir_all(app_dir.join("src")).unwrap();
    fs::write(
        app_dir.join("Cargo.toml"),
        r#"
[package]
name = "app"
version = "0.1.0"
edition = "2021"

[dependencies]
lib-b = { path = "../lib-b" }
"#,
    )
    .unwrap();
    fs::write(
        app_dir.join("src/main.rs"),
        r#"
fn main() {
    println!("{}", lib_b::message());
}
"#,
    )
    .unwrap();

    temp_dir
}

fn run_cargo_frequent_json(project_path: &Path) -> String {
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("cargo-frequent"));
    cmd.arg("--path")
        .arg(project_path)
        .arg("--command")
        .arg("build")
        .arg("--json")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let assertion = cmd.assert().success();
    String::from_utf8_lossy(&assertion.get_output().stdout).to_string()
}

#[test]
fn json_structure_is_valid_for_workspace_rebuild() {
    let workspace = create_workspace_with_dependencies();

    let mut build_cmd = Command::new("cargo");
    build_cmd.arg("build").current_dir(workspace.path());
    build_cmd.assert().success();

    let lib_a_src = workspace.path().join("lib-a/src/lib.rs");
    fs::write(
        &lib_a_src,
        r#"
pub fn greet() -> &'static str {
    "Hello from modified lib-a!"
}
"#,
    )
    .unwrap();

    let json = run_cargo_frequent_json(workspace.path());
    let parsed: serde_json::Value =
        serde_json::from_str(&json).expect("JSON should be valid and parseable");

    let cluster_array = parsed.as_array().expect("JSON should be an array");
    assert!(
        !cluster_array.is_empty(),
        "should have at least one cluster"
    );

    for cluster in cluster_array {
        assert!(cluster.get("trigger").is_some(), "trigger present");
        assert!(
            cluster.get("affected_packages").is_some(),
            "affected_packages present"
        );
    }
}
