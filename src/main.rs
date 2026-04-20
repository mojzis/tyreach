use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use tyreach::entry::{detect_entries, parse_cli_entry, EntryPoint};
use tyreach::workspace::WorkspaceDetector;

/// `tyreach` — produce ranked, token-budgeted reachability snapshots of a Python project.
#[derive(Parser, Debug)]
#[command(name = "tyreach", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Walk the call graph from entry points and emit a `nodes + edges` snapshot.
    ///
    /// Phase 2 emits pretty JSON on stdout for verification; Phase 3 switches
    /// the default output to TOON.
    Snapshot {
        /// Repository root to analyze. Defaults to the current directory.
        #[arg(value_name = "REPO", default_value = ".")]
        repo: PathBuf,
        /// Explicit entry point `path/to/file.py::func` (repeatable). When
        /// omitted, entry points are auto-detected from
        /// `pyproject.toml [project.scripts]`.
        #[arg(long = "entry")]
        entries: Vec<String>,
        /// Unused in Phase 2. Reserved for Phase 3 file output.
        #[arg(long = "out")]
        out: Option<PathBuf>,
    },
    /// Render a previously captured TOON snapshot as a topologically-sorted text view.
    Render {
        /// Path to the TOON snapshot to render. Reads stdin when omitted.
        #[arg(long)]
        input: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Command::Snapshot { repo, entries, out: _ } => run_snapshot(&repo, &entries).await,
        Command::Render { .. } => anyhow::bail!("unimplemented: Phase 3"),
    }
}

async fn run_snapshot(repo: &std::path::Path, cli_entries: &[String]) -> Result<()> {
    let root = WorkspaceDetector::find_workspace_root(repo).unwrap_or_else(|| repo.to_path_buf());
    tracing::info!("snapshot root: {}", root.display());

    let entries: Vec<EntryPoint> = if cli_entries.is_empty() {
        let detected = detect_entries(&root).context("detect entries from pyproject.toml")?;
        if detected.is_empty() {
            anyhow::bail!(
                "no entry points found; supply --entry path/to/file.py::func or add [project.scripts]"
            );
        }
        detected
    } else {
        cli_entries.iter().map(|spec| parse_cli_entry(spec, &root)).collect::<Result<Vec<_>>>()?
    };

    let snapshot = tyreach::snapshot(&root, entries).await?;
    let rendered = serde_json::to_string_pretty(&snapshot).context("serialize snapshot to JSON")?;
    println!("{rendered}");
    Ok(())
}
