use std::path::{Path, PathBuf};

use anstyle::{AnsiColor, Effects, Style};
use terminal_link::Link;
use url::Url;

use crate::{target_reading::CodeLocation, trigger_detection::fix_hints::DocumentationLink};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextRole {
    Plain,
    Heading,
    Evidence,
    BranchLabel,
    RemovedValue,
    AddedValue,
    FixSummary,
    FixSuggestion,
}

pub struct LineRoles {
    pub first_line_role: TextRole,
    pub continuation_role: TextRole,
}

pub const PLAIN_LINES: LineRoles = LineRoles {
    first_line_role: TextRole::Plain,
    continuation_role: TextRole::Plain,
};

pub const FIX_LINES: LineRoles = LineRoles {
    first_line_role: TextRole::FixSummary,
    continuation_role: TextRole::FixSuggestion,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalStyle {
    pub styling_enabled: bool,
    pub hyperlinks_enabled: bool,
    pub project_root: PathBuf,
}

impl TerminalStyle {
    #[must_use]
    pub fn for_terminal(project_root: &Path) -> Self {
        Self {
            styling_enabled: true,
            hyperlinks_enabled: supports_hyperlinks::on(supports_hyperlinks::Stream::Stdout),
            project_root: project_root
                .canonicalize()
                .unwrap_or_else(|_| project_root.to_path_buf()),
        }
    }

    #[cfg(test)]
    #[must_use]
    pub const fn plain() -> Self {
        Self {
            styling_enabled: false,
            hyperlinks_enabled: false,
            project_root: PathBuf::new(),
        }
    }

    #[must_use]
    pub fn paint(&self, text: &str, text_role: TextRole) -> String {
        if !self.styling_enabled || text.is_empty() || text_role == TextRole::Plain {
            return text.to_string();
        }
        let role_style = match text_role {
            TextRole::Plain => Style::new(),
            TextRole::Heading => Style::new().effects(Effects::BOLD),
            TextRole::Evidence => Style::new().effects(Effects::DIMMED),
            TextRole::BranchLabel => Style::new().fg_color(Some(AnsiColor::Cyan.into())),
            TextRole::RemovedValue => Style::new().fg_color(Some(AnsiColor::Red.into())),
            TextRole::AddedValue | TextRole::FixSuggestion => {
                Style::new().fg_color(Some(AnsiColor::Green.into()))
            }
            TextRole::FixSummary => Style::new()
                .fg_color(Some(AnsiColor::Yellow.into()))
                .effects(Effects::BOLD),
        };
        format!("{role_style}{text}{role_style:#}")
    }

    #[must_use]
    pub fn documentation_line(&self, documentation_link: &DocumentationLink) -> String {
        match documentation_link {
            DocumentationLink::NoLink => String::new(),
            DocumentationLink::Link { link_url } if self.hyperlinks_enabled => {
                format!("\n{}", Link::new("docs", link_url))
            }
            DocumentationLink::Link { link_url } => format!("\ndocs: {link_url}"),
        }
    }

    #[must_use]
    pub fn location_prefix(&self, code_location: &CodeLocation) -> String {
        let location_path = match code_location {
            CodeLocation::Nowhere => return String::new(),
            CodeLocation::File { file_path } => file_path,
            CodeLocation::Line(file_location) => &file_location.relative_path,
        };
        let location_text = code_location.to_string();
        let candidate_path = self.project_root.join(location_path);
        let linked_text = match Url::from_file_path(&candidate_path) {
            Ok(file_url) if self.hyperlinks_enabled && candidate_path.is_file() => {
                Link::new(&location_text, file_url.as_str()).to_string()
            }
            _ => location_text,
        };
        if !self.styling_enabled {
            return format!("{linked_text}  ");
        }
        let location_style = Style::new()
            .fg_color(Some(AnsiColor::Blue.into()))
            .effects(Effects::UNDERLINE);
        format!("{location_style}{linked_text}{location_style:#}  ")
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;
    use crate::target_reading::FileLocation;

    #[test]
    fn links_existing_file_locations_only() {
        let project_directory = TempDir::new().unwrap();
        fs::write(project_directory.path().join("flake.nix"), "{}").unwrap();
        let terminal_style = TerminalStyle {
            styling_enabled: false,
            hyperlinks_enabled: true,
            project_root: project_directory.path().to_path_buf(),
        };
        let file_url = Url::from_file_path(project_directory.path().join("flake.nix")).unwrap();
        assert_eq!(
            terminal_style.location_prefix(&CodeLocation::Line(FileLocation {
                relative_path: "flake.nix".to_string(),
                line_number: 237,
            })),
            format!("{}  ", Link::new("flake.nix:237", file_url.as_str()))
        );
        assert_eq!(
            terminal_style.location_prefix(&CodeLocation::File {
                file_path: "missing.nix".to_string(),
            }),
            "missing.nix  "
        );
        assert_eq!(terminal_style.location_prefix(&CodeLocation::Nowhere), "");
    }

    #[test]
    fn links_documentation_only_when_supported() {
        let documentation_link = DocumentationLink::Link {
            link_url: "https://doc.rust-lang.org/cargo/reference/config.html#env",
        };
        assert_eq!(
            TerminalStyle::plain().documentation_line(&documentation_link),
            "\ndocs: https://doc.rust-lang.org/cargo/reference/config.html#env"
        );
        let linking_style = TerminalStyle {
            hyperlinks_enabled: true,
            ..TerminalStyle::plain()
        };
        assert_eq!(
            linking_style.documentation_line(&documentation_link),
            format!(
                "\n{}",
                Link::new(
                    "docs",
                    "https://doc.rust-lang.org/cargo/reference/config.html#env"
                )
            )
        );
        assert_eq!(
            linking_style.documentation_line(&DocumentationLink::NoLink),
            ""
        );
    }

    #[test]
    fn plain_style_leaves_text_untouched() {
        assert_eq!(
            TerminalStyle::plain().paint("flake.nix:237", TextRole::FixSummary),
            "flake.nix:237"
        );
    }

    #[test]
    fn styles_by_role() {
        let terminal_style = TerminalStyle {
            styling_enabled: true,
            hyperlinks_enabled: false,
            project_root: PathBuf::new(),
        };
        let removed_style = Style::new().fg_color(Some(AnsiColor::Red.into()));
        assert_eq!(
            terminal_style.paint("-frandom-seed=aaa", TextRole::RemovedValue),
            format!("{removed_style}-frandom-seed=aaa{removed_style:#}")
        );
    }
}
