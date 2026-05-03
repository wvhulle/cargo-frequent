use std::{
    fs::{self, remove_file},
    process::Command,
};

use assert_cmd::{cargo, prelude::*};
use tempfile::TempDir;

fn create_test_project(name: &str) -> TempDir {
    let temp_dir = TempDir::new().unwrap();
    let cargo_toml = temp_dir.path().join("Cargo.toml");

    fs::write(
        &cargo_toml,
        format!(
            r#"
[package]
name = "{name}"
version = "0.1.0"
edition = "2021"

[build-dependencies]
cc = "1.0"
"#
        ),
    )
    .unwrap();

    let src_dir = temp_dir.path().join("src");
    fs::create_dir(&src_dir).unwrap();

    // Create a simple main.rs that uses dependencies
    fs::write(
        src_dir.join("main.rs"),
        r#"
fn main() {
    println!("Hello, world!");
}
"#,
    )
    .unwrap();

    // Create a build script that checks environment variables
    fs::write(
        temp_dir.path().join("build.rs"),
        r#"
use std::env;

fn main() {
    println!("cargo:rerun-if-env-changed=CUSTOM_VAR");
    println!("cargo:rerun-if-env-changed=CC");

    if let Ok(val) = env::var("CUSTOM_VAR") {
        println!("cargo:warning=CUSTOM_VAR is set to: {}", val);
    }

    // Simple C compilation that's sensitive to CC
    cc::Build::new()
        .file("build_test.c")
        .compile("build_test");
}
"#,
    )
    .unwrap();

    // Create a simple C file for the build script
    fs::write(
        temp_dir.path().join("build_test.c"),
        r"
int build_test_function(void) {
    return 42;
}
",
    )
    .unwrap();

    temp_dir
}

#[test]
fn detects_rebuilds_when_environment_variables_change() {
    let project = create_test_project("env-test");

    let mut cmd1 = Command::new("cargo");
    cmd1.arg("clean").current_dir(project.path());
    cmd1.assert().success();

    let mut cmd2 = Command::new("cargo");
    cmd2.arg("build")
        .current_dir(project.path())
        .env_remove("CUSTOM_VAR");
    let _ = cmd2.assert();

    let mut cmd = Command::new(cargo::cargo_bin!("cargo-frequent"));
    cmd.arg("--path")
        .arg(project.path())
        .arg("--command")
        .arg("build")
        .env("CUSTOM_VAR", "test_value");

    let output = cmd.assert().success();
    let stdout = String::from_utf8_lossy(&output.get_output().stdout);
    assert!(
        stdout.contains("$CUSTOM_VAR"),
        "Expected stdout to contain '$CUSTOM_VAR', got: {stdout}"
    );
}

#[test]
fn detects_rebuilds_when_c_compiler_environment_changes() {
    let project = create_test_project("cc-test");

    let mut cmd1 = Command::new("cargo");
    cmd1.arg("clean").current_dir(project.path());
    let _ = cmd1.assert();

    let mut cmd2 = Command::new("cargo");
    cmd2.arg("build")
        .current_dir(project.path())
        .env("CC", "gcc");
    let _ = cmd2.assert();

    let mut cmd = Command::new(cargo::cargo_bin!("cargo-frequent"));
    cmd.arg("--path")
        .arg(project.path())
        .arg("--command")
        .arg("build")
        .env("CC", "clang");

    let output = cmd.assert().success();
    let stdout = String::from_utf8_lossy(&output.get_output().stdout);
    assert!(
        stdout.contains("$CC"),
        "Expected stdout to contain '$CC', got: {stdout}"
    );
}

#[test]
fn detects_rebuilds_when_source_files_are_modified() {
    let project = create_test_project("target-test");

    let _ = remove_file(project.path().join("build.rs"));

    let cargo_toml = project.path().join("Cargo.toml");
    fs::write(
        &cargo_toml,
        r#"
[package]
name = "target-test"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();

    let mut cmd1 = Command::new("cargo");
    cmd1.arg("build").current_dir(project.path());
    cmd1.assert().success();

    let src_file = project.path().join("src/main.rs");
    fs::write(
        &src_file,
        r#"
fn main() {
    println!("Hello, modified world!");
}
"#,
    )
    .unwrap();

    let mut cmd = Command::new(cargo::cargo_bin!("cargo-frequent"));
    cmd.arg("--path")
        .arg(project.path())
        .arg("--command")
        .arg("build");

    let output = cmd.assert().success();
    let stdout = String::from_utf8_lossy(&output.get_output().stdout);
    assert!(
        stdout.contains("main.rs"),
        "Expected stdout to contain 'main.rs', got: {stdout}"
    );
}
