use anyhow::Result;
use clap::{Parser, Subcommand};

/// `tyreach` — produce ranked, token-budgeted reachability snapshots of a Python project.
///
/// Phase 1 exposes the CLI surface but returns `unimplemented` for both subcommands.
/// The walk + render pipelines land in phases 2 and 3.
#[derive(Parser, Debug)]
#[command(name = "tyreach", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Walk the call graph from entry points and emit a `nodes + edges` TOON snapshot.
    Snapshot {
        /// Repository root to analyze. Defaults to the current working directory.
        #[arg(long)]
        repo: Option<std::path::PathBuf>,
        /// Explicit entry points (fully-qualified names or file paths).
        #[arg(long = "entry")]
        entries: Vec<String>,
    },
    /// Render a previously captured TOON snapshot as a topologically-sorted text view.
    Render {
        /// Path to the TOON snapshot to render. Reads stdin when omitted.
        #[arg(long)]
        input: Option<std::path::PathBuf>,
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
        Command::Snapshot { .. } | Command::Render { .. } => {
            anyhow::bail!("unimplemented: Phase 2/3")
        }
    }
}
