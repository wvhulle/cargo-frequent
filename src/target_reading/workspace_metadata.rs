use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use cargo_metadata::{Metadata, MetadataCommand};

use crate::{
    frequent_error::FrequentError,
    target_reading::cargo_run_variables::inherited_cargo_run_variables,
};

const RUSTC_INFO_CACHE_VARIABLE: &str = "CARGO_CACHE_RUSTC_INFO";

pub struct WorkspaceMetadata {
    pub target_directory: PathBuf,
    pub member_names: BTreeSet<String>,
    pub default_member_names: BTreeSet<String>,
    pub cargo_metadata: Metadata,
}

impl WorkspaceMetadata {
    #[must_use]
    pub fn default_members_select_subset(&self) -> bool {
        !self.default_member_names.is_empty()
            && self.default_member_names.len() < self.member_names.len()
    }
}

pub fn read_workspace_metadata(project_path: &Path) -> Result<WorkspaceMetadata, FrequentError> {
    let manifest_path = project_path.join("Cargo.toml");
    if !manifest_path.is_file() {
        return Err(FrequentError::CargoTomlMissing { manifest_path });
    }
    let isolated_metadata_command = || {
        let mut metadata_command = MetadataCommand::new();
        metadata_command.env(RUSTC_INFO_CACHE_VARIABLE, "0");
        inherited_cargo_run_variables()
            .into_iter()
            .for_each(|inherited_variable| {
                metadata_command.env_remove(inherited_variable);
            });
        metadata_command
    };
    let cargo_metadata = isolated_metadata_command()
        .manifest_path(&manifest_path)
        .other_options(vec!["--locked".to_string(), "--offline".to_string()])
        .exec()
        .or_else(|_| {
            isolated_metadata_command()
                .manifest_path(&manifest_path)
                .no_deps()
                .exec()
        })
        .map_err(|metadata_error| FrequentError::CargoMetadataFailed { metadata_error })?;
    let package_names_of = |package_ids: &[cargo_metadata::PackageId]| -> BTreeSet<String> {
        cargo_metadata
            .packages
            .iter()
            .filter(|package| package_ids.contains(&package.id))
            .map(|package| package.name.to_string())
            .collect()
    };
    let member_names = package_names_of(&cargo_metadata.workspace_members);
    let default_member_names = if cargo_metadata.workspace_default_members.is_available() {
        package_names_of(&cargo_metadata.workspace_default_members)
    } else {
        BTreeSet::new()
    };
    Ok(WorkspaceMetadata {
        target_directory: cargo_metadata.target_directory.clone().into_std_path_buf(),
        member_names,
        default_member_names,
        cargo_metadata,
    })
}
