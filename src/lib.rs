use core::error::Error;
use std::{
    env, fmt, io,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{ChildStderr, Command, Stdio},
};

use clap::Parser;
use log::{debug, info};

use crate::{
    build_script_rerun_directives::RerunDirectiveMap,
    cargo_dirty_reason_log_parser::parse_log_line,
    rebuild_graph::{RebuildGraph, RebuildNode},
    root_cause_cluster::{cluster, cluster_to_json},
};

mod build_script_rerun_directives;
mod cargo_dirty_reason_log_parser;
mod cluster_ascii_dag_renderer;
mod rebuild_graph;
mod rebuild_reason;
mod root_cause_cluster;

#[derive(Parser, Debug)]
#[command(author, version, about = "Analyze what causes cargo rebuilds", long_about = None)]
pub struct Cli {
    #[arg(short, long, help = "Path to cargo project", default_value = ".")]
    pub path: PathBuf,

    #[arg(short, long, help = "Verbose output")]
    pub verbose: bool,

    #[arg(long, help = "Output analysis as JSON")]
    pub json: bool,

    #[arg(long, help = "Cargo command to analyze", default_value = "check")]
    pub command: String,

    #[arg(help = "Additional arguments to pass to cargo", last = true)]
    pub cargo_args: Vec<String>,
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
        let cargo_command = build_cargo_command(&self.command, &self.cargo_args);

        let cargo_toml = self.path.join("Cargo.toml");
        if !cargo_toml.exists() {
            return Err(AnalyzerError::CargoTomlNotFound(cargo_toml));
        }

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
            .env("CARGO_LOG", "cargo::core::compiler::fingerprint=info")
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
    let target_dir = resolve_target_dir(project_path)?;
    build_script_rerun_directives::scan(&target_dir)
}

fn resolve_target_dir(project_path: &Path) -> Result<PathBuf, AnalyzerError> {
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
    let target = parsed
        .get("target_directory")
        .and_then(|v| v.as_str())
        .map(PathBuf::from)
        .ok_or_else(|| {
            AnalyzerError::CargoMetadataFailed("missing target_directory field".to_string())
        })?;
    Ok(target)
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
