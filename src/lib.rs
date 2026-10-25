use core::error::Error;
use std::{
    env, fmt, io,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{ChildStderr, Command, Stdio},
    time::{Duration, SystemTime},
};

use clap::{Parser, Subcommand};
use log::{debug, info};

use crate::{
    build_script_rerun_directives::RerunDirectiveMap,
    cargo_dirty_reason_log_parser::parse_log_line,
    fingerprint_unit_records::scan_fingerprint_units,
    rebuild_graph::{RebuildGraph, RebuildNode},
    recorded_env_drift::detect_recorded_env_drift,
    root_cause_cluster::{cluster, cluster_to_json},
    unit_hash_churn::detect_unit_hash_churn,
};

mod build_script_rerun_directives;
mod cargo_dirty_reason_log_parser;
mod cluster_ascii_dag_renderer;
mod env_value_token_diff;
mod fingerprint_unit_records;
mod fix_hint_library;
mod offline_report_renderer;
mod rebuild_graph;
mod rebuild_reason;
mod recorded_env_drift;
mod root_cause_cluster;
mod unit_hash_churn;

#[derive(Parser, Debug)]
#[command(author, version, about = "Analyze what causes cargo rebuilds", long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub analysis_mode: Option<AnalysisMode>,

    #[arg(
        short,
        long,
        global = true,
        help = "Path to cargo project",
        default_value = "."
    )]
    pub path: PathBuf,

    #[arg(short, long, global = true, help = "Verbose output")]
    pub verbose: bool,

    #[arg(long, global = true, help = "Output analysis as JSON")]
    pub json: bool,

    #[arg(long, help = "Cargo command to analyze", default_value = "check")]
    pub command: String,

    #[arg(help = "Additional arguments to pass to cargo", last = true)]
    pub cargo_args: Vec<String>,
}

#[derive(Subcommand, Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalysisMode {
    #[command(about = "Run cargo with fingerprint logging and explain what it rebuilds (default)")]
    Live,
    #[command(
        about = "Without building: list build scripts whose watched env vars differ from their \
                 last run"
    )]
    EnvDrift,
    #[command(
        about = "Without building: explain why crates accumulate several unit hashes in the \
                 target dir"
    )]
    UnitChurn {
        #[arg(
            long,
            default_value_t = 24,
            help = "Only explain units built in the last N hours"
        )]
        since_hours: u64,
    },
}

impl Cli {
    /// Parses argv. Tolerates the `cargo frequent ...` invocation form by
    /// dropping the `frequent` subcommand before clap sees it.
    #[must_use]
    pub fn parse_args() -> Self {
        if env::args().nth(1).as_deref() == Some("frequent") {
            Self::parse_from(env::args().take(1).chain(env::args().skip(2)))
        } else {
            Self::parse()
        }
    }

    pub fn init_logging(&self) {
        if self.verbose {
            env_logger::Builder::from_default_env()
                .filter_level(log::LevelFilter::Debug)
                .init();
        } else {
            env_logger::init();
        }
    }

    pub fn analyze(&self) -> Result<(), AnalyzerError> {
        let cargo_toml = self.path.join("Cargo.toml");
        if !cargo_toml.exists() {
            return Err(AnalyzerError::CargoTomlNotFound(cargo_toml));
        }

        match self.analysis_mode.unwrap_or(AnalysisMode::Live) {
            AnalysisMode::Live => self.analyze_live_rebuild(),
            AnalysisMode::EnvDrift => self.analyze_recorded_env_drift(),
            AnalysisMode::UnitChurn { since_hours } => self.analyze_unit_hash_churn(since_hours),
        }
    }

    fn analyze_recorded_env_drift(&self) -> Result<(), AnalyzerError> {
        let workspace_layout = read_workspace_layout(&self.path)?;
        let unit_records = scan_fingerprint_units(&workspace_layout.target_directory)?;
        let env_drifts =
            detect_recorded_env_drift(&unit_records, |variable_name| env::var(variable_name).ok());
        if self.json {
            println!("{}", serde_json::to_string_pretty(&env_drifts)?);
        } else {
            let mut report_text = String::new();
            offline_report_renderer::render_env_drift(&env_drifts, &mut report_text);
            print!("{report_text}");
        }
        Ok(())
    }

    fn analyze_unit_hash_churn(&self, since_hours: u64) -> Result<(), AnalyzerError> {
        let workspace_layout = read_workspace_layout(&self.path)?;
        let unit_records = scan_fingerprint_units(&workspace_layout.target_directory)?;
        let recent_cutoff = SystemTime::now()
            .checked_sub(Duration::from_hours(since_hours))
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let churn_report = detect_unit_hash_churn(
            &unit_records,
            workspace_layout.default_members_select_subset,
            recent_cutoff,
        );
        if self.json {
            println!("{}", serde_json::to_string_pretty(&churn_report)?);
        } else {
            let mut report_text = String::new();
            offline_report_renderer::render_unit_churn(&churn_report, &mut report_text);
            print!("{report_text}");
        }
        Ok(())
    }

    fn analyze_live_rebuild(&self) -> Result<(), AnalyzerError> {
        let cargo_command = build_cargo_command(&self.command, &self.cargo_args);

        info!(
            "Analyzing output of `cargo {}` on project {}",
            cargo_command,
            self.path.display()
        );

        let args: Vec<&str> = cargo_command.split_whitespace().collect();
        let (cmd, cmd_args) = args.split_first().ok_or(AnalyzerError::EmptyCommand)?;

        let output = Command::new("cargo")
            .arg(cmd)
            .args(cmd_args)
            .current_dir(&self.path)
            .env(
                "CARGO_LOG",
                "cargo::core::compiler::fingerprint=info,cargo::compiler::fingerprint=info",
            )
            .env("RUST_LOG", "debug")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        if let Some(stderr) = output.stderr {
            let reader = BufReader::new(stderr);
            self.analyze_logs(reader)?;
        }

        Ok(())
    }

    fn analyze_logs(&self, reader: BufReader<ChildStderr>) -> Result<(), AnalyzerError> {
        let mut graph = RebuildGraph::new();

        for line in reader.lines() {
            let line = line?;
            debug!("Cargo log: {line}");
            if let Some(entry) = parse_log_line(&line) {
                graph.add_node(RebuildNode::new(entry.package, entry.reason));
            }
        }

        let directives = scan_directives(&self.path).unwrap_or_else(|e| {
            debug!("build-script rerun-directive scan skipped: {e}");
            RerunDirectiveMap::new()
        });

        if self.json {
            println!("{}", cluster_to_json(&graph, &directives)?);
        } else {
            let clusters = cluster(&graph, &directives);
            let mut out = String::new();
            cluster_ascii_dag_renderer::render(&clusters, &mut out);
            print!("{out}");
        }

        Ok(())
    }
}

fn build_cargo_command(command: &str, cargo_args: &[String]) -> String {
    if cargo_args.is_empty() {
        command.to_string()
    } else {
        format!("{} {}", command, cargo_args.join(" "))
    }
}

fn scan_directives(project_path: &Path) -> Result<RerunDirectiveMap, AnalyzerError> {
    let workspace_layout = read_workspace_layout(project_path)?;
    build_script_rerun_directives::scan(&workspace_layout.target_directory)
}

struct WorkspaceLayout {
    target_directory: PathBuf,
    default_members_select_subset: bool,
}

fn read_workspace_layout(project_path: &Path) -> Result<WorkspaceLayout, AnalyzerError> {
    let output = Command::new("cargo")
        .arg("metadata")
        .arg("--format-version=1")
        .arg("--no-deps")
        .current_dir(project_path)
        .output()?;
    if !output.status.success() {
        return Err(AnalyzerError::CargoMetadataFailed(
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ));
    }
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let target_directory = parsed
        .get("target_directory")
        .and_then(|v| v.as_str())
        .map(PathBuf::from)
        .ok_or_else(|| {
            AnalyzerError::CargoMetadataFailed("missing target_directory field".to_string())
        })?;
    let member_count = |field_name: &str| {
        parsed
            .get(field_name)
            .and_then(serde_json::Value::as_array)
            .map(Vec::len)
    };
    let default_members_select_subset = matches!(
        (member_count("workspace_default_members"), member_count("workspace_members")),
        (Some(default_member_count), Some(all_member_count)) if default_member_count < all_member_count
    );
    Ok(WorkspaceLayout {
        target_directory,
        default_members_select_subset,
    })
}

#[derive(Debug)]
pub enum AnalyzerError {
    CargoTomlNotFound(PathBuf),
    EmptyCommand,
    Io(io::Error),
    Json(serde_json::Error),
    BuildScriptOutputUnreadable(PathBuf, io::Error),
    CargoMetadataFailed(String),
}

impl fmt::Display for AnalyzerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CargoTomlNotFound(path) => {
                write!(f, "Cargo.toml not found at {}", path.display())
            }
            Self::EmptyCommand => write!(f, "empty cargo command"),
            Self::Io(e) => write!(f, "IO error: {e}"),
            Self::Json(e) => write!(f, "JSON error: {e}"),
            Self::BuildScriptOutputUnreadable(path, e) => {
                write!(
                    f,
                    "could not read build-script output at {}: {e}",
                    path.display()
                )
            }
            Self::CargoMetadataFailed(msg) => write!(f, "cargo metadata failed: {msg}"),
        }
    }
}

impl Error for AnalyzerError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(e) | Self::BuildScriptOutputUnreadable(_, e) => Some(e),
            Self::Json(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for AnalyzerError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<serde_json::Error> for AnalyzerError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}
