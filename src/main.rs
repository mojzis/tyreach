use std::fs;
use std::io::{self, BufWriter};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use tyreach::budget::{fit_to_budget, scale_budget, PER_ENTRY_BUDGET_FLOOR};
use tyreach::entry::{
    detect_entries, detect_init_entries, parse_tyreach_toml, resolve_entries, EntryPoint,
};
use tyreach::rank::rank;
use tyreach::render::render;
use tyreach::toon_io::{read_snapshot_toon, write_snapshot_toon};
use tyreach::walker::Snapshot;
use tyreach::workspace::WorkspaceDetector;

const LONG_ABOUT: &str = "\
tyreach walks the Python call graph from one or more entry points and emits a \
ranked, token-budgeted reachability snapshot: a flat `nodes + edges` table in \
canonical TOON plus a topologically-sorted rendered text view.

It uses tree-sitter for call-site extraction and the `ty` LSP for symbol \
resolution, scoped to the repo and stopping at site-packages. The output is \
designed to be dropped into a coding agent's context window before code \
changes so the agent has call-graph situational awareness.";

const AFTER_LONG_HELP: &str = "\
Getting started in an unfamiliar repo:

  1. Run `tyreach setup` to see which entry-point source is active.
  2. If entries are detected, run `tyreach snapshot` — writes
     <name>.toon and <name>.txt side-by-side.
  3. If no entries are detected, either pass --entry on the CLI or
     write a tyreach.toml in the repo root.

Entry-point precedence (highest first):

  1. --entry path/to/file.py::func        (CLI flag, repeatable)
  2. tyreach.toml                         (repo root)
  3. pyproject.toml [project.scripts]     (auto-detected)
  4. <pkg>/__init__.py exports            (auto-detected, library fallback)

Minimal tyreach.toml:

  [[entries]]
  name = \"cli\"
  entry_file = \"myapp/cli.py\"
  function = \"main\"   # optional; defaults to \"main\"

Run `tyreach setup` inside a repo to diagnose which source wins, see the \
resolved file paths, and get a ready-to-paste CLAUDE.md snippet that tells \
coding agents to read the snapshot before editing Python code.";

const ABOUT: &str = "\
Ranked, token-budgeted reachability snapshot of a Python project. \
Run `tyreach --help` for a setup walkthrough or `tyreach setup` to \
diagnose which entry-point source a repo uses.";

#[derive(Parser, Debug)]
#[command(
    name = "tyreach",
    version,
    about = ABOUT,
    long_about = LONG_ABOUT,
    after_long_help = AFTER_LONG_HELP,
)]
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
        /// Include noisy builtin call targets (`builtins.print`, `Path`,
        /// `mkdir`, ...) in the rendered text view. Off by default so the
        /// `.txt` is easier to scan; the `.toon` is unaffected either way.
        #[arg(long = "with-builtins", default_value_t = false)]
        with_builtins: bool,
    },
    /// Render a previously captured TOON snapshot as a topologically-sorted text view.
    Render {
        /// Path to the TOON snapshot to render. Reads stdin when omitted.
        input: Option<PathBuf>,
        /// Include noisy builtin call targets in the rendered output.
        #[arg(long = "with-builtins", default_value_t = false)]
        with_builtins: bool,
    },
    /// Inspect a repo and report which entry-point source (tyreach.toml /
    /// pyproject.toml) is active. Read-only — no snapshot is produced.
    Setup {
        /// Repository root to inspect. Defaults to the current directory.
        #[arg(value_name = "REPO", default_value = ".")]
        repo: PathBuf,
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
        Command::Snapshot { repo, entries, budget, out, no_render, to_stdout, with_builtins } => {
            run_snapshot(
                &repo,
                &entries,
                budget,
                out.as_deref(),
                no_render,
                to_stdout,
                with_builtins,
            )
            .await
        }
        Command::Render { input, with_builtins } => run_render(input.as_deref(), with_builtins),
        Command::Setup { repo } => run_setup(&repo),
    }
}

async fn run_snapshot(
    repo: &Path,
    cli_entries: &[String],
    budget_tokens: usize,
    out_prefix: Option<&Path>,
    no_render: bool,
    to_stdout: bool,
    with_builtins: bool,
) -> Result<()> {
    let root = WorkspaceDetector::find_workspace_root(repo).unwrap_or_else(|| repo.to_path_buf());
    tracing::info!("snapshot root: {}", root.display());

    let entries = resolve_entries(&root, cli_entries)?;
    let entry_name_for_prefix = entries.first().map(|e| e.name.clone());
    let entry_count = entries.len();
    let effective_budget = scale_budget(budget_tokens, entry_count);
    let floor = PER_ENTRY_BUDGET_FLOOR.saturating_mul(entry_count);
    tracing::info!(
        "budget: {} tokens (cli={}, entries={}, floor={})",
        effective_budget,
        budget_tokens,
        entry_count,
        floor
    );

    let mut snapshot = tyreach::snapshot(&root, entries).await?;
    rank(&mut snapshot);
    let snapshot = fit_to_budget(snapshot, effective_budget);

    if to_stdout {
        let stdout = io::stdout();
        let mut handle = stdout.lock();
        render(&snapshot, &mut handle, with_builtins).context("render to stdout")?;
        return Ok(());
    }

    let prefix = out_prefix
        .map(Path::to_path_buf)
        .or_else(|| entry_name_for_prefix.map(derive_prefix_from_name))
        .unwrap_or_else(|| PathBuf::from("tyreach-snapshot"));

    write_toon_file(&snapshot, &prefix)?;
    if !no_render {
        write_rendered_file(&snapshot, &prefix, with_builtins)?;
    }

    // Structurally-valid but empty snapshots are a silent foot-gun: an agent
    // reads the empty `.txt` and concludes "no reachable code" when the
    // actual cause is usually that the entry points at a non-function object
    // (`app = typer.Typer()`). Loud-fail to stderr, but keep exit 0 so
    // scripted callers that key on exit code don't break.
    if snapshot.nodes.is_empty() && snapshot.edges.is_empty() {
        print_empty_snapshot_warning();
    }

    Ok(())
}

fn print_empty_snapshot_warning() {
    eprintln!("tyreach: wrote an EMPTY snapshot (0 nodes, 0 edges).");
    eprintln!("  likely causes:");
    eprintln!("    - the entry points at a non-function object (e.g. `app = typer.Typer()`)");
    eprintln!("    - the entry file exists but the named function was not found");
    eprintln!("    - the entry function has no body, or only calls dynamic/unresolvable code");
    eprintln!("  check the entries with `tyreach setup` and try `--entry <file>::<func>`");
    eprintln!("  pointing at a real `def`.");
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

fn write_rendered_file(snapshot: &Snapshot, prefix: &Path, with_builtins: bool) -> Result<()> {
    let path = with_extension(prefix, "txt");
    let file = fs::File::create(&path).with_context(|| format!("create {}", path.display()))?;
    let mut writer = BufWriter::new(file);
    render(snapshot, &mut writer, with_builtins)
        .with_context(|| format!("render {}", path.display()))?;
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

fn tyreach_source_row(
    tyreach_result: &Result<Vec<EntryPoint>>,
    tyreach_toml_exists: bool,
) -> String {
    match tyreach_result {
        Err(err) => format!("[!] tyreach.toml            malformed: {err:#}"),
        Ok(entries) if entries.is_empty() && tyreach_toml_exists => {
            "[ ] tyreach.toml            no entries defined".to_owned()
        }
        Ok(entries) if entries.is_empty() => "[ ] tyreach.toml            not found".to_owned(),
        Ok(entries) => format!(
            "[x] tyreach.toml            {} {}",
            entries.len(),
            pluralize_entries(entries.len())
        ),
    }
}

fn pyproject_source_row(
    from_pyproject: &[EntryPoint],
    pyproject_exists: bool,
    tyreach_wins: bool,
) -> String {
    if !pyproject_exists {
        "[ ] pyproject.toml scripts  file not found".to_owned()
    } else if from_pyproject.is_empty() {
        "[ ] pyproject.toml scripts  no [project.scripts] block".to_owned()
    } else if tyreach_wins {
        format!(
            "[ ] pyproject.toml scripts  {} {} (eclipsed by tyreach.toml)",
            from_pyproject.len(),
            pluralize_entries(from_pyproject.len())
        )
    } else {
        format!(
            "[x] pyproject.toml scripts  {} {}",
            from_pyproject.len(),
            pluralize_entries(from_pyproject.len())
        )
    }
}

fn init_source_row(from_init: &[EntryPoint], init_wins: bool, tyreach_wins: bool) -> String {
    if from_init.is_empty() {
        "[ ] __init__.py exports     no top-level packages with exports".to_owned()
    } else if init_wins {
        format!(
            "[x] __init__.py exports     {} {}",
            from_init.len(),
            pluralize_entries(from_init.len())
        )
    } else {
        let eclipser = if tyreach_wins { "tyreach.toml" } else { "pyproject.toml" };
        format!(
            "[ ] __init__.py exports     {} {} (eclipsed by {eclipser})",
            from_init.len(),
            pluralize_entries(from_init.len())
        )
    }
}

fn run_setup(repo: &Path) -> Result<()> {
    // Validate the repo path up front. Without this, `setup /typo/path` would
    // silently produce the empty-case output — indistinguishable from a real
    // empty repo, which is actively misleading for an agent.
    if !repo.exists() {
        anyhow::bail!("repo path does not exist: {}", repo.display());
    }
    if !repo.is_dir() {
        anyhow::bail!("repo path is not a directory: {}", repo.display());
    }

    let (root, detected_root) = match WorkspaceDetector::find_workspace_root(repo) {
        Some(r) => (r, true),
        None => (repo.to_path_buf(), false),
    };

    println!("repo: {}", repo.display());
    if detected_root {
        println!("workspace root: {}", root.display());
    } else {
        println!("workspace root: {} (no marker found; using repo path as-is)", root.display());
    }
    println!();

    // `setup` deliberately avoids `resolve_entries` because that helper
    // collapses the "which source won" signal we need to surface here.
    // A malformed tyreach.toml is reported as a row, not a fatal error —
    // `setup` exists precisely to diagnose broken configs, so it must still
    // render the rest of the table when one source is misconfigured.
    let tyreach_result = parse_tyreach_toml(&root);
    let from_pyproject =
        detect_entries(&root).context("inspect pyproject.toml during setup diagnosis")?;
    let from_init =
        detect_init_entries(&root).context("inspect __init__.py exports during setup diagnosis")?;
    let pyproject_exists = root.join("pyproject.toml").is_file();
    let tyreach_toml_exists = root.join("tyreach.toml").is_file();

    // Precedence: --entry > tyreach.toml > pyproject.toml > __init__.py.
    // `setup` has no notion of --entry (it's a runtime-only flag on `snapshot`),
    // so we always render that row as "snapshot only".
    let tyreach_wins = tyreach_result.as_ref().is_ok_and(|v| !v.is_empty());
    let pyproject_wins = !tyreach_wins && !from_pyproject.is_empty();
    let init_wins = !tyreach_wins && !pyproject_wins && !from_init.is_empty();

    let tyreach_row = tyreach_source_row(&tyreach_result, tyreach_toml_exists);
    let pyproject_row = pyproject_source_row(&from_pyproject, pyproject_exists, tyreach_wins);
    let init_row = init_source_row(&from_init, init_wins, tyreach_wins);

    println!("entry sources (highest precedence first):");
    println!("  [ ] --entry CLI flag        snapshot only");
    println!("  {tyreach_row}");
    println!("  {pyproject_row}");
    println!("  {init_row}");
    println!();

    let from_tyreach_slice: &[EntryPoint] = tyreach_result.as_deref().unwrap_or(&[]);
    let active: &[EntryPoint] = if tyreach_wins {
        from_tyreach_slice
    } else if pyproject_wins {
        &from_pyproject
    } else if init_wins {
        &from_init
    } else {
        &[]
    };

    if active.is_empty() {
        print_empty_skeleton();
    } else {
        println!("resolved entries:");
        for entry in active {
            println!("  {}", entry.name);
            println!("    file:     {}", entry.file.display());
            println!("    function: {}", entry.function);
        }
        println!();
        println!("next: run `tyreach snapshot` from {}", root.display());
        println!();
        print_claude_md_snippet(active);
    }

    Ok(())
}

/// The prefix `snapshot` will derive — matches `derive_prefix_from_name`
/// exactly so the snippet names the files `snapshot` actually writes.
fn snapshot_prefix_for(active: &[EntryPoint]) -> &str {
    active.first().map_or("tyreach-snapshot", |e| e.name.as_str())
}

fn print_claude_md_snippet(active: &[EntryPoint]) {
    let prefix = snapshot_prefix_for(active);
    println!(
        "To wire this into a coding agent, add a section to your CLAUDE.md \
         (or AGENTS.md) so the agent reads the snapshot before editing \
         Python code:"
    );
    println!();
    println!("  ## Call-graph context (tyreach)");
    println!();
    println!("  Before changing Python code, read `{prefix}.txt` — a ranked");
    println!("  reachability snapshot of the call graph from this repo's");
    println!("  entry points, with function signatures and call edges. The");
    println!("  canonical machine-readable form is `{prefix}.toon`.");
    println!();
    println!("  Regenerate after non-trivial changes: `tyreach snapshot`.");
}

fn pluralize_entries(count: usize) -> &'static str {
    if count == 1 {
        "entry"
    } else {
        "entries"
    }
}

fn print_empty_skeleton() {
    println!("no entry points discovered. choose one:");
    println!();
    println!("  1) create tyreach.toml:");
    println!();
    println!("     [[entries]]");
    println!("     name = \"cli\"");
    println!("     entry_file = \"path/to/cli.py\"");
    println!("     function = \"main\"");
    println!();
    println!("  2) pass --entry on the CLI:");
    println!();
    println!("     tyreach snapshot --entry path/to/cli.py::main");
    println!();
    println!("  3) add [project.scripts] to pyproject.toml:");
    println!();
    println!("     [project.scripts]");
    println!("     mytool = \"mypkg.cli:main\"");
    println!();
    println!("  4) export names from <pkg>/__init__.py (library fallback):");
    println!();
    println!("     # <pkg>/__init__.py");
    println!("     __all__ = [\"run\", \"Thing\"]");
    println!();
    println!(
        "Once entries are configured, run `tyreach setup` again — it will \
         print a ready-to-paste CLAUDE.md snippet tailored to your entry \
         filename."
    );
    println!();
    print_agent_guidance();
}

/// Agent-oriented entry-picking procedure. Printed after the human-facing
/// three options when no entries are discovered. A coding agent invoking
/// `tyreach setup` needs a prescriptive procedure rather than a menu.
fn print_agent_guidance() {
    println!("If you are a coding agent running this: the repo has no discoverable");
    println!("entry points. Pick one by:");
    println!();
    println!("  1) Looking for a `def main` (or any function called from a shell script");
    println!("     or Makefile) in the obvious files: `<pkg>/__main__.py`,");
    println!("     `<pkg>/cli.py`, `src/<pkg>/main.py`, top-level scripts.");
    println!("  2) For libraries with no CLI, picking the top-level public function");
    println!("     most users would call (often in `<pkg>/__init__.py`).");
    println!("  3) Running `tyreach snapshot --entry <file>::<func>`. If the resulting");
    println!("     `.txt` has fewer than ~5 intra-repo edges, try a different entry.");
    println!();
    println!("Do not modify the repo without asking the user. If you find good entries,");
    println!("propose a `tyreach.toml` in your response rather than writing one.");
}

fn run_render(input: Option<&Path>, with_builtins: bool) -> Result<()> {
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
    render(&snapshot, &mut handle, with_builtins).context("render")?;
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

#[cfg(test)]
mod tests {
    use super::pluralize_entries;

    #[test]
    fn pluralize_entries_uses_singular_only_for_one() {
        assert_eq!(pluralize_entries(0), "entries");
        assert_eq!(pluralize_entries(1), "entry");
        assert_eq!(pluralize_entries(2), "entries");
        assert_eq!(pluralize_entries(42), "entries");
    }
}
