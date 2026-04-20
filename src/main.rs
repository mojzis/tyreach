use std::fs;
use std::io::{self, BufWriter};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use tyreach::budget::fit_to_budget;
use tyreach::entry::resolve_entries;
use tyreach::rank::rank;
use tyreach::render::render;
use tyreach::toon_io::{read_snapshot_toon, write_snapshot_toon};
use tyreach::walker::Snapshot;
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
    /// Writes two files side-by-side: `<out>.toon` (canonical TOON) and
    /// `<out>.txt` (rendered text view). Use `--stdout` to pipe the rendered
    /// view instead of writing files.
    Snapshot {
        /// Repository root to analyze. Defaults to the current directory.
        #[arg(value_name = "REPO", default_value = ".")]
        repo: PathBuf,
        /// Explicit entry point `path/to/file.py::func` (repeatable). When
        /// omitted, entry points are read from `tyreach.toml` (if present) or
        /// auto-detected from `pyproject.toml [project.scripts]`.
        #[arg(long = "entry")]
        entries: Vec<String>,
        /// Token budget. Nodes are dropped in ascending score order until the
        /// snapshot fits. Default 2000.
        #[arg(long = "budget", default_value_t = 2000)]
        budget: usize,
        /// Output prefix — writes `<prefix>.toon` and `<prefix>.txt`. When
        /// omitted, the first entry's name (or `tyreach-snapshot`) is used.
        #[arg(long = "out")]
        out: Option<PathBuf>,
        /// Skip writing `<prefix>.txt`.
        #[arg(long = "no-render", default_value_t = false)]
        no_render: bool,
        /// Print the rendered text view to stdout instead of writing files.
        #[arg(long = "stdout", default_value_t = false)]
        to_stdout: bool,
    },
    /// Render a previously captured TOON snapshot as a topologically-sorted text view.
    Render {
        /// Path to the TOON snapshot to render. Reads stdin when omitted.
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
        Command::Snapshot { repo, entries, budget, out, no_render, to_stdout } => {
            run_snapshot(&repo, &entries, budget, out.as_deref(), no_render, to_stdout).await
        }
        Command::Render { input } => run_render(input.as_deref()),
    }
}

async fn run_snapshot(
    repo: &Path,
    cli_entries: &[String],
    budget_tokens: usize,
    out_prefix: Option<&Path>,
    no_render: bool,
    to_stdout: bool,
) -> Result<()> {
    let root = WorkspaceDetector::find_workspace_root(repo).unwrap_or_else(|| repo.to_path_buf());
    tracing::info!("snapshot root: {}", root.display());

    let entries = resolve_entries(&root, cli_entries)?;
    let entry_name_for_prefix = entries.first().map(|e| e.name.clone());

    let mut snapshot = tyreach::snapshot(&root, entries).await?;
    rank(&mut snapshot);
    let snapshot = fit_to_budget(snapshot, budget_tokens);

    if to_stdout {
        let stdout = io::stdout();
        let mut handle = stdout.lock();
        render(&snapshot, &mut handle).context("render to stdout")?;
        return Ok(());
    }

    let prefix = out_prefix
        .map(Path::to_path_buf)
        .or_else(|| entry_name_for_prefix.map(derive_prefix_from_name))
        .unwrap_or_else(|| PathBuf::from("tyreach-snapshot"));

    write_toon_file(&snapshot, &prefix)?;
    if !no_render {
        write_rendered_file(&snapshot, &prefix)?;
    }

    Ok(())
}

fn derive_prefix_from_name(name: String) -> PathBuf {
    // Scripts from pyproject.toml can carry characters awkward for filenames
    // (e.g. `my-tool`). Passing the name through as-is matches shell
    // expectations (`my-tool.toon` is a fine filename).
    PathBuf::from(name)
}

fn write_toon_file(snapshot: &Snapshot, prefix: &Path) -> Result<()> {
    let path = with_extension(prefix, "toon");
    let file = fs::File::create(&path).with_context(|| format!("create {}", path.display()))?;
    let mut writer = BufWriter::new(file);
    let entries = entry_qnames(snapshot);
    write_snapshot_toon(&snapshot.nodes, &snapshot.edges, &entries, &mut writer)
        .with_context(|| format!("write {}", path.display()))?;
    tracing::info!("wrote {}", path.display());
    println!("wrote {}", path.display());
    Ok(())
}

/// Entry-point qnames = nodes at BFS depth 0. Sorted so callers don't have
/// to care about ordering; `write_snapshot_toon` also sorts defensively.
fn entry_qnames(snapshot: &Snapshot) -> Vec<String> {
    let mut v: Vec<String> = snapshot
        .depth_by_qname
        .iter()
        .filter_map(|(q, d)| if *d == 0 { Some(q.clone()) } else { None })
        .collect();
    v.sort();
    v
}

fn write_rendered_file(snapshot: &Snapshot, prefix: &Path) -> Result<()> {
    let path = with_extension(prefix, "txt");
    let file = fs::File::create(&path).with_context(|| format!("create {}", path.display()))?;
    let mut writer = BufWriter::new(file);
    render(snapshot, &mut writer).with_context(|| format!("render {}", path.display()))?;
    tracing::info!("wrote {}", path.display());
    println!("wrote {}", path.display());
    Ok(())
}

/// Append `.{ext}` to a prefix path. We can't use `Path::with_extension`
/// because a prefix like `my-tool` has no stem/dot pattern we want to
/// overwrite.
fn with_extension(prefix: &Path, ext: &str) -> PathBuf {
    let mut out = prefix.as_os_str().to_owned();
    out.push(".");
    out.push(ext);
    PathBuf::from(out)
}

fn run_render(input: Option<&Path>) -> Result<()> {
    let text = if let Some(path) = input {
        fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?
    } else {
        use std::io::Read;
        let mut buf = String::new();
        io::stdin().read_to_string(&mut buf).context("read stdin")?;
        buf
    };

    let (nodes, edges, entries) = read_snapshot_toon(&text).context("parse TOON snapshot")?;
    // Re-render reconstructs scoring from the canonical TOON so the output is
    // bit-identical to `tyreach snapshot`'s rendered view (modulo ties that
    // depend on walk order — edges here are already in canonical order).
    //
    //   1. BFS from the `entries` over `edges` to recover `depth_by_qname`.
    //   2. Re-run `rank` — it's a pure function of (depth_by_qname, edges).
    //
    // Truncation metadata is not carried on-disk (walker-state only); we pass
    // the default `None`.
    let depth_by_qname = reconstruct_depths(&entries, &edges);
    let mut snapshot = Snapshot { nodes, edges, depth_by_qname, ..Snapshot::default() };
    rank(&mut snapshot);
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    render(&snapshot, &mut handle).context("render")?;
    Ok(())
}

/// BFS over `edges` starting from `entries` (depth 0). Unreachable qnames get
/// no entry in the returned map; `rank::rank` already handles `None` depths by
/// scoring on fan-in alone.
fn reconstruct_depths(
    entries: &[String],
    edges: &[tyreach::model::Edge],
) -> std::collections::HashMap<String, u32> {
    use std::collections::{HashMap, VecDeque};

    let mut depth: HashMap<String, u32> = HashMap::new();
    let mut queue: VecDeque<String> = VecDeque::new();
    for entry in entries {
        depth.insert(entry.clone(), 0);
        queue.push_back(entry.clone());
    }
    while let Some(qname) = queue.pop_front() {
        let next_depth = depth.get(&qname).copied().unwrap_or(0).saturating_add(1);
        for edge in edges.iter().filter(|e| e.from == qname) {
            if !depth.contains_key(&edge.to) {
                depth.insert(edge.to.clone(), next_depth);
                queue.push_back(edge.to.clone());
            }
        }
    }
    depth
}
