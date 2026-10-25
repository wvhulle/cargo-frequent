use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    iter,
};

use cargo_metadata::{Metadata, Package, PackageId};
use serde::Serialize;

use crate::target_reading::{FileLocation, locate_toml_key};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FeaturePresence {
    pub enabling_members: Vec<String>,
    pub lacking_members: Vec<String>,
    pub default_build_is_lacking: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
pub enum DefaultMembersSetting {
    #[default]
    NotSet,
    Set {
        location: FileLocation,
    },
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct FeatureKey {
    crate_name: String,
    feature_name: String,
}

#[derive(Default)]
pub struct FeatureRequestIndex {
    requesters_by_feature: BTreeMap<FeatureKey, BTreeSet<PackageId>>,
    dependents_by_package: HashMap<PackageId, BTreeSet<PackageId>>,
    member_names_by_id: HashMap<PackageId, String>,
    package_ids_by_name: HashMap<String, Vec<PackageId>>,
    default_member_names: BTreeSet<String>,
    pub default_members_setting: DefaultMembersSetting,
}

impl FeatureRequestIndex {
    #[must_use]
    pub fn build(cargo_metadata: &Metadata, default_member_names: &BTreeSet<String>) -> Self {
        let requesters_by_feature = cargo_metadata
            .packages
            .iter()
            .flat_map(feature_requests_of_package)
            .fold(
                BTreeMap::new(),
                |mut requesters_by_feature: BTreeMap<FeatureKey, BTreeSet<PackageId>>,
                 feature_request| {
                    requesters_by_feature
                        .entry(feature_request.requested_feature)
                        .or_default()
                        .insert(feature_request.requesting_package);
                    requesters_by_feature
                },
            );
        let dependents_by_package = cargo_metadata
            .resolve
            .iter()
            .flat_map(|resolve_graph| &resolve_graph.nodes)
            .flat_map(|resolve_node| {
                resolve_node
                    .deps
                    .iter()
                    .map(|node_dependency| DependencyLink {
                        dependency_id: node_dependency.pkg.clone(),
                        dependent_id: resolve_node.id.clone(),
                    })
            })
            .fold(
                HashMap::new(),
                |mut dependents_by_package: HashMap<PackageId, BTreeSet<PackageId>>,
                 dependency_link| {
                    dependents_by_package
                        .entry(dependency_link.dependency_id)
                        .or_default()
                        .insert(dependency_link.dependent_id);
                    dependents_by_package
                },
            );
        let member_names_by_id = cargo_metadata
            .packages
            .iter()
            .filter(|package| cargo_metadata.workspace_members.contains(&package.id))
            .fold(HashMap::new(), |mut member_names_by_id, member_package| {
                member_names_by_id
                    .insert(member_package.id.clone(), member_package.name.to_string());
                member_names_by_id
            });
        let package_ids_by_name = cargo_metadata.packages.iter().fold(
            HashMap::new(),
            |mut package_ids_by_name: HashMap<String, Vec<PackageId>>, package| {
                package_ids_by_name
                    .entry(package.name.replace('-', "_"))
                    .or_default()
                    .push(package.id.clone());
                package_ids_by_name
            },
        );
        let workspace_manifest = cargo_metadata.workspace_root.join("Cargo.toml");
        let default_members_setting = locate_toml_key(
            workspace_manifest.as_std_path(),
            cargo_metadata.workspace_root.as_std_path(),
            &["workspace", "default-members"],
        )
        .map_or(DefaultMembersSetting::NotSet, |location| {
            DefaultMembersSetting::Set { location }
        });
        Self {
            requesters_by_feature,
            dependents_by_package,
            member_names_by_id,
            package_ids_by_name,
            default_member_names: default_member_names.clone(),
            default_members_setting,
        }
    }

    #[must_use]
    pub fn feature_presence(&self, crate_name: &str, feature_name: &str) -> FeaturePresence {
        let normalized_crate_name = crate_name.replace('-', "_");
        let requesting_packages: Vec<&PackageId> = self
            .requesters_by_feature
            .get(&FeatureKey {
                crate_name: normalized_crate_name.clone(),
                feature_name: feature_name.to_string(),
            })
            .into_iter()
            .flatten()
            .collect();
        let crate_packages: Vec<&PackageId> = self
            .package_ids_by_name
            .get(&normalized_crate_name)
            .into_iter()
            .flatten()
            .collect();
        let enabling_members = self.members_reaching(requesting_packages);
        let members_using_crate = self.members_reaching(crate_packages);
        let default_build_is_lacking = !self.default_member_names.is_empty()
            && self.default_member_names.is_disjoint(&enabling_members)
            && !self.default_member_names.is_disjoint(&members_using_crate);
        FeaturePresence {
            lacking_members: members_using_crate
                .difference(&enabling_members)
                .cloned()
                .collect(),
            enabling_members: enabling_members.into_iter().collect(),
            default_build_is_lacking,
        }
    }

    fn members_reaching(&self, starting_packages: Vec<&PackageId>) -> BTreeSet<String> {
        let mut visited_packages: BTreeSet<&PackageId> =
            starting_packages.iter().copied().collect();
        iter::successors(Some(starting_packages), |frontier_packages| {
            let next_frontier: Vec<&PackageId> = frontier_packages
                .iter()
                .filter_map(|frontier_package| self.dependents_by_package.get(*frontier_package))
                .flatten()
                .filter(|dependent_package| visited_packages.insert(dependent_package))
                .collect();
            (!next_frontier.is_empty()).then_some(next_frontier)
        })
        .flatten()
        .filter_map(|reached_package| self.member_names_by_id.get(reached_package))
        .cloned()
        .collect()
    }
}

struct DependencyLink {
    dependency_id: PackageId,
    dependent_id: PackageId,
}

struct FeatureRequest {
    requested_feature: FeatureKey,
    requesting_package: PackageId,
}

fn feature_requests_of_package(package: &Package) -> Vec<FeatureRequest> {
    let requested_by_package = |crate_name: &str, feature_name: &str| FeatureRequest {
        requested_feature: FeatureKey {
            crate_name: crate_name.replace('-', "_"),
            feature_name: feature_name.to_string(),
        },
        requesting_package: package.id.clone(),
    };
    let declared_requests = package.dependencies.iter().flat_map(|dependency| {
        dependency
            .features
            .iter()
            .map(|feature_name| requested_by_package(&dependency.name, feature_name))
    });
    let forwarded_requests = package
        .features
        .values()
        .flatten()
        .filter_map(|feature_entry| feature_entry.split_once('/'))
        .filter_map(|feature_entry| {
            let (dependency_alias, feature_name) = feature_entry;
            let dependency_alias = dependency_alias.trim_end_matches('?');
            let dependency = package.dependencies.iter().find(|dependency| {
                dependency.rename.as_deref().unwrap_or(&dependency.name) == dependency_alias
            })?;
            Some(requested_by_package(&dependency.name, feature_name))
        });
    declared_requests.chain(forwarded_requests).collect()
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use cargo_metadata::MetadataCommand;
    use tempfile::TempDir;

    use super::*;

    struct WorkspaceFile {
        relative_path: &'static str,
        file_contents: &'static str,
    }

    fn workspace_file(relative_path: &'static str, file_contents: &'static str) -> WorkspaceFile {
        WorkspaceFile {
            relative_path,
            file_contents,
        }
    }

    fn write_workspace(workspace_root: &Path) {
        let workspace_files = [
            workspace_file(
                "Cargo.toml",
                "[workspace]\nmembers = [\"shared\", \"alpha\", \"beta\", \
                 \"gamma\"]\ndefault-members = [\"beta\"]\nresolver = \"2\"\n",
            ),
            workspace_file(
                "shared/Cargo.toml",
                "[package]\nname = \"shared\"\nversion = \"0.1.0\"\nedition = \
                 \"2021\"\n\n[features]\nextra = []\n",
            ),
            workspace_file("shared/src/lib.rs", ""),
            workspace_file(
                "alpha/Cargo.toml",
                "[package]\nname = \"alpha\"\nversion = \"0.1.0\"\nedition = \
                 \"2021\"\n\n[dependencies]\nshared = { path = \"../shared\", features = \
                 [\"extra\"] }\n",
            ),
            workspace_file("alpha/src/lib.rs", ""),
            workspace_file(
                "beta/Cargo.toml",
                "[package]\nname = \"beta\"\nversion = \"0.1.0\"\nedition = \
                 \"2021\"\n\n[dependencies]\nshared = { path = \"../shared\" }\n",
            ),
            workspace_file("beta/src/lib.rs", ""),
            workspace_file(
                "gamma/Cargo.toml",
                "[package]\nname = \"gamma\"\nversion = \"0.1.0\"\nedition = \
                 \"2021\"\n\n[dependencies]\nlinked = { package = \"shared\", path = \
                 \"../shared\", optional = true }\n\n[features]\nloud = [\"linked?/extra\"]\n",
            ),
            workspace_file("gamma/src/lib.rs", ""),
        ];
        workspace_files.iter().for_each(|workspace_file| {
            let file_path = workspace_root.join(workspace_file.relative_path);
            fs::create_dir_all(file_path.parent().unwrap()).unwrap();
            fs::write(file_path, workspace_file.file_contents).unwrap();
        });
    }

    #[test]
    fn finds_members_requesting_a_feature_directly_and_through_forwarding() {
        let workspace_directory = TempDir::new().unwrap();
        write_workspace(workspace_directory.path());
        let cargo_metadata = MetadataCommand::new()
            .manifest_path(workspace_directory.path().join("Cargo.toml"))
            .exec()
            .unwrap();
        let feature_request_index =
            FeatureRequestIndex::build(&cargo_metadata, &BTreeSet::from(["beta".to_string()]));

        assert_eq!(
            feature_request_index.feature_presence("shared", "extra"),
            FeaturePresence {
                enabling_members: vec!["alpha".to_string(), "gamma".to_string()],
                lacking_members: vec!["beta".to_string(), "shared".to_string()],
                default_build_is_lacking: true,
            }
        );
        assert!(
            feature_request_index
                .feature_presence("shared", "missing")
                .enabling_members
                .is_empty(),
            "nobody requests it"
        );
        assert!(
            matches!(
                &feature_request_index.default_members_setting,
                DefaultMembersSetting::Set { location } if location.line_number == 3
            ),
            "default-members on line 3"
        );
    }
}
