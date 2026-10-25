pub mod cargo_run_variables;
pub mod dirty_reason_parser;
pub mod environment_sources;
pub mod feature_requests;
pub mod fingerprint_build_log;
pub mod fingerprint_units;
pub mod workspace_metadata;

use std::{fmt, fs, path::Path};

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct FileLocation {
    pub relative_path: String,
    pub line_number: usize,
}

impl fmt::Display for FileLocation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}:{}", self.relative_path, self.line_number)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub enum CodeLocation {
    #[default]
    Nowhere,
    File {
        file_path: String,
    },
    Line(FileLocation),
}

impl fmt::Display for CodeLocation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Nowhere => Ok(()),
            Self::File { file_path } => write!(formatter, "{file_path}"),
            Self::Line(file_location) => write!(formatter, "{file_location}"),
        }
    }
}

#[must_use]
pub fn locate_toml_key(
    toml_path: &Path,
    project_path: &Path,
    key_path: &[&str],
) -> Option<FileLocation> {
    let toml_text = fs::read_to_string(toml_path).ok()?;
    let toml_document = toml_edit::Document::parse(toml_text.clone()).ok()?;
    let (last_key, parent_keys) = key_path.split_last()?;
    let parent_table = parent_keys
        .iter()
        .try_fold(toml_document.as_table(), |current_table, table_key| {
            current_table.get(table_key)?.as_table()
        })?;
    let key_span = parent_table.key(last_key)?.span()?;
    Some(FileLocation {
        relative_path: relative_display_path(toml_path, project_path),
        line_number: line_number_at(&toml_text, key_span.start),
    })
}

#[must_use]
pub fn line_number_at(file_text: &str, byte_offset: usize) -> usize {
    file_text[..byte_offset.min(file_text.len())]
        .matches('\n')
        .count()
        + 1
}

#[must_use]
pub fn relative_display_path(file_path: &Path, project_path: &Path) -> String {
    file_path
        .strip_prefix(project_path)
        .unwrap_or(file_path)
        .display()
        .to_string()
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn finds_nested_toml_key_line() {
        let project_directory = TempDir::new().unwrap();
        let manifest_path = project_directory.path().join("Cargo.toml");
        fs::write(
            &manifest_path,
            "[workspace]\nmembers = [\"a\", \"b\"]\ndefault-members = [\"a\"]\n",
        )
        .unwrap();
        assert_eq!(
            locate_toml_key(
                &manifest_path,
                project_directory.path(),
                &["workspace", "default-members"]
            ),
            Some(FileLocation {
                relative_path: "Cargo.toml".to_string(),
                line_number: 3
            })
        );
        assert_eq!(
            locate_toml_key(
                &manifest_path,
                project_directory.path(),
                &["package", "name"]
            ),
            None
        );
    }
}
