use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    process::Command,
};

use serde::Deserialize;

use crate::frequent_error::FrequentError;

pub struct WorkspaceMetadata {
    pub target_directory: PathBuf,
    pub member_names: BTreeSet<String>,
    pub default_members_select_subset: bool,
}

#[derive(Deserialize)]
struct CargoMetadataJson {
    target_directory: PathBuf,
    packages: Vec<CargoPackageJson>,
    workspace_members: Vec<String>,
    #[serde(default)]
    workspace_default_members: Vec<String>,
}

#[derive(Deserialize)]
struct CargoPackageJson {
    name: String,
}

pub fn read_workspace_metadata(project_path: &Path) -> Result<WorkspaceMetadata, FrequentError> {
    let manifest_path = project_path.join("Cargo.toml");
    if !manifest_path.is_file() {
        return Err(FrequentError::CargoTomlMissing { manifest_path });
    }
    let metadata_output = Command::new("cargo")
        .args(["metadata", "--format-version=1", "--no-deps"])
        .current_dir(project_path)
        .output()
        .map_err(|io_error| FrequentError::CargoSpawnFailed { io_error })?;
    if !metadata_output.status.success() {
        return Err(FrequentError::CargoMetadataExited {
            exit_status: metadata_output.status,
        });
    }
    let metadata_json: CargoMetadataJson = serde_json::from_slice(&metadata_output.stdout)
        .map_err(|json_error| FrequentError::CargoMetadataUnparsable { json_error })?;

    Ok(WorkspaceMetadata {
        default_members_select_subset: !metadata_json.workspace_default_members.is_empty()
            && metadata_json.workspace_default_members.len()
                < metadata_json.workspace_members.len(),
        member_names: metadata_json
            .packages
            .into_iter()
            .map(|package| package.name)
            .collect(),
        target_directory: metadata_json.target_directory,
    })
}
