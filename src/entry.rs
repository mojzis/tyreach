//! Entry-point discovery.
//!
//! Three sources, in precedence order (highest first):
//!
//! 1. `--entry` CLI flag — parsed via [`parse_cli_entry`]. Accepts
//!    `path/to/file.py::func`. The `::func` suffix is mandatory for v1. Paths
//!    are resolved relative to the repo root argument, **not** the process
//!    CWD.
//! 2. `tyreach.toml` — parsed via [`parse_tyreach_toml`]. Top-level
//!    `entries = [{ name, entry_file, function = "main" (optional) }]`. Paths
//!    are resolved relative to the repo root.
//! 3. `pyproject.toml` `[project.scripts]` — auto-detected via
//!    [`detect_entries`]. Script specs look like `"name" = "module.path:func"`.
//!    We resolve the module portion to a `.py` file on disk by trying, in
//!    order: `{root}/{path}.py`, `{root}/src/{path}.py`,
//!    `{root}/{path}/__init__.py`, `{root}/src/{path}/__init__.py`.
//!
//! The [`resolve_entries`] helper implements the precedence: CLI wins if
//! non-empty; else `tyreach.toml` wins if present; else auto-detect
//! `pyproject.toml`.
//!
//! Dockerfile entry-point parsing is deferred to v1.1.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;

/// A single callable entry point: function `function` defined in `file`,
/// identified by `name` (typically the script name or CLI string).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryPoint {
    pub name: String,
    pub file: PathBuf,
    pub function: String,
}

#[derive(Debug, Deserialize)]
struct PyProject {
    project: Option<Project>,
}

#[derive(Debug, Deserialize)]
struct Project {
    scripts: Option<std::collections::BTreeMap<String, String>>,
}

/// Auto-detect entries from `{repo_root}/pyproject.toml [project.scripts]`.
///
/// Missing file or missing `[project.scripts]` → returns an empty vec (no
/// error). Malformed TOML or unresolvable script targets → logged as warnings
/// and skipped.
pub fn detect_entries(repo_root: &Path) -> Result<Vec<EntryPoint>> {
    let pyproject = repo_root.join("pyproject.toml");
    if !pyproject.is_file() {
        return Ok(Vec::new());
    }

    let raw = std::fs::read_to_string(&pyproject)
        .with_context(|| format!("read {}", pyproject.display()))?;
    let parsed: PyProject = match toml::from_str(&raw) {
        Ok(p) => p,
        Err(err) => {
            tracing::warn!("detect_entries: malformed pyproject.toml: {err}");
            return Ok(Vec::new());
        }
    };

    let Some(scripts) = parsed.project.and_then(|p| p.scripts) else {
        return Ok(Vec::new());
    };

    let mut out = Vec::new();
    for (name, spec) in scripts {
        if let Some((file, function)) = resolve_script_spec(repo_root, &spec) {
            out.push(EntryPoint { name, file, function });
        } else {
            tracing::warn!(
                "detect_entries: could not resolve script {name:?} = {spec:?} to a .py file"
            );
        }
    }

    Ok(out)
}

fn resolve_script_spec(repo_root: &Path, spec: &str) -> Option<(PathBuf, String)> {
    let (module, function) = spec.split_once(':')?;
    let module_path = module.replace('.', "/");

    let candidates = [
        repo_root.join(format!("{module_path}.py")),
        repo_root.join("src").join(format!("{module_path}.py")),
        repo_root.join(&module_path).join("__init__.py"),
        repo_root.join("src").join(&module_path).join("__init__.py"),
    ];

    for candidate in candidates {
        if candidate.is_file() {
            return Some((candidate, function.to_owned()));
        }
    }
    None
}

/// Parse a `--entry` CLI argument of the form `path/to/file.py::func`.
///
/// The function name is mandatory in v1. Paths are resolved relative to
/// `repo_root` when not absolute. The file must exist.
pub fn parse_cli_entry(spec: &str, repo_root: &Path) -> Result<EntryPoint> {
    let (raw_path, function) = spec.split_once("::").ok_or_else(|| {
        anyhow!("--entry {spec:?} is missing '::func'; v1 requires an explicit function")
    })?;
    if function.is_empty() {
        anyhow::bail!("--entry {spec:?} has empty function name after '::'");
    }

    let path = Path::new(raw_path);
    let resolved = if path.is_absolute() { path.to_path_buf() } else { repo_root.join(path) };

    if !resolved.is_file() {
        anyhow::bail!("--entry file does not exist: {}", resolved.display());
    }

    Ok(EntryPoint { name: spec.to_owned(), file: resolved, function: function.to_owned() })
}

/// Parse `{repo_root}/tyreach.toml` into a list of entry points.
///
/// Schema:
///
/// ```toml
/// [[entries]]
/// name = "cli"
/// entry_file = "myapp/cli.py"
/// function = "main"   # optional; defaults to "main"
/// ```
///
/// Missing file → `Ok(Vec::new())`. Malformed TOML or an unreadable/missing
/// `entry_file` on disk → `Err` with a span-aware message.
pub fn parse_tyreach_toml(repo_root: &Path) -> Result<Vec<EntryPoint>> {
    let path = repo_root.join("tyreach.toml");
    if !path.is_file() {
        return Ok(Vec::new());
    }

    let raw = std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;

    let doc: TyreachToml = toml::from_str(&raw).map_err(|err| {
        // toml::de::Error carries a span. Surface it in the error message so
        // users see the offending line/column alongside the reason.
        let span = err.span();
        match span {
            Some(range) => {
                let (line, col) = line_col(&raw, range.start);
                anyhow!(
                    "malformed tyreach.toml at {}:{}:{}: {}",
                    path.display(),
                    line,
                    col,
                    err.message()
                )
            }
            None => anyhow!("malformed tyreach.toml {}: {}", path.display(), err.message()),
        }
    })?;

    let mut out = Vec::new();
    for raw_entry in doc.entries.unwrap_or_default() {
        let entry_file = Path::new(&raw_entry.entry_file);
        let resolved = if entry_file.is_absolute() {
            entry_file.to_path_buf()
        } else {
            repo_root.join(entry_file)
        };
        if !resolved.is_file() {
            anyhow::bail!(
                "tyreach.toml entry {:?}: entry_file does not exist: {}",
                raw_entry.name,
                resolved.display()
            );
        }
        let function = raw_entry.function.unwrap_or_else(|| "main".to_owned());
        out.push(EntryPoint { name: raw_entry.name, file: resolved, function });
    }
    Ok(out)
}

/// Resolve entry points given the repo root and optional CLI entries.
///
/// Precedence (first non-empty source wins):
///
/// 1. `cli_entries` (parsed with [`parse_cli_entry`]).
/// 2. `tyreach.toml` (parsed with [`parse_tyreach_toml`]).
/// 3. `pyproject.toml` `[project.scripts]` (parsed with [`detect_entries`]).
///
/// Errors if *all three* sources yield zero entries — callers usually want to
/// tell the user to supply `--entry` or populate one of the config files.
pub fn resolve_entries(repo_root: &Path, cli_entries: &[String]) -> Result<Vec<EntryPoint>> {
    if !cli_entries.is_empty() {
        return cli_entries.iter().map(|spec| parse_cli_entry(spec, repo_root)).collect();
    }

    let from_tyreach = parse_tyreach_toml(repo_root).context("parse tyreach.toml")?;
    if !from_tyreach.is_empty() {
        return Ok(from_tyreach);
    }

    let detected = detect_entries(repo_root).context("detect entries from pyproject.toml")?;
    if detected.is_empty() {
        anyhow::bail!(
            "no entry points found; supply --entry path/to/file.py::func, create tyreach.toml, or add [project.scripts]; run 'tyreach setup' for a diagnosis"
        );
    }
    Ok(detected)
}

#[derive(Debug, Deserialize)]
struct TyreachToml {
    entries: Option<Vec<TyreachEntry>>,
}

#[derive(Debug, Deserialize)]
struct TyreachEntry {
    name: String,
    entry_file: String,
    function: Option<String>,
}

/// 1-based (line, column) from a byte offset into `src`.
fn line_col(src: &str, byte: usize) -> (usize, usize) {
    let clamped = byte.min(src.len());
    let prefix = &src[..clamped];
    let line = prefix.bytes().filter(|b| *b == b'\n').count() + 1;
    let last_newline = prefix.rfind('\n').map_or(0, |p| p + 1);
    let col = src[last_newline..clamped].chars().count() + 1;
    (line, col)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_simple_script_entry() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("pyproject.toml"),
            r#"
[project]
name = "demo"
version = "0.1.0"

[project.scripts]
demo = "demo.main:run"
"#,
        )
        .expect("write pyproject");

        let pkg = dir.path().join("demo");
        std::fs::create_dir_all(&pkg).expect("mkdir");
        std::fs::write(pkg.join("__init__.py"), "").expect("init");
        std::fs::write(pkg.join("main.py"), "def run():\n    pass\n").expect("main");

        let entries = detect_entries(dir.path()).expect("detect");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "demo");
        assert_eq!(entries[0].function, "run");
        assert!(entries[0].file.ends_with("demo/main.py"));
    }

    #[test]
    fn detects_src_layout_entry() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("pyproject.toml"),
            r#"
[project]
name = "demo"
version = "0.1.0"

[project.scripts]
demo = "demo.main:run"
"#,
        )
        .expect("write pyproject");

        let pkg = dir.path().join("src/demo");
        std::fs::create_dir_all(&pkg).expect("mkdir");
        std::fs::write(pkg.join("__init__.py"), "").expect("init");
        std::fs::write(pkg.join("main.py"), "def run():\n    pass\n").expect("main");

        let entries = detect_entries(dir.path()).expect("detect");
        assert_eq!(entries.len(), 1);
        assert!(entries[0].file.ends_with("src/demo/main.py"));
    }

    #[test]
    fn missing_pyproject_yields_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let entries = detect_entries(dir.path()).expect("detect");
        assert!(entries.is_empty());
    }

    #[test]
    fn pyproject_without_scripts_yields_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("pyproject.toml"),
            "[project]\nname = \"demo\"\nversion = \"0.1.0\"\n",
        )
        .expect("write");
        let entries = detect_entries(dir.path()).expect("detect");
        assert!(entries.is_empty());
    }

    #[test]
    fn parse_cli_entry_requires_function() {
        let dir = tempfile::tempdir().expect("tempdir");
        let err = parse_cli_entry("some/file.py", dir.path()).unwrap_err();
        assert!(err.to_string().contains("missing '::func'"));
    }

    #[test]
    fn parse_cli_entry_resolves_relative_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pkg = dir.path().join("pkg");
        std::fs::create_dir_all(&pkg).expect("mkdir");
        std::fs::write(pkg.join("mod.py"), "def go():\n    pass\n").expect("write");

        let entry = parse_cli_entry("pkg/mod.py::go", dir.path()).expect("parse");
        assert_eq!(entry.function, "go");
        assert!(entry.file.ends_with("pkg/mod.py"));
    }

    #[test]
    fn parse_cli_entry_rejects_missing_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(parse_cli_entry("nope.py::fn", dir.path()).is_err());
    }

    #[test]
    fn malformed_pyproject_yields_empty_not_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Unterminated table header — toml parser will reject.
        std::fs::write(dir.path().join("pyproject.toml"), "[project\nname = broken\n")
            .expect("write");
        // Per contract, a malformed pyproject.toml should not propagate the
        // parse error; the function logs and returns an empty vec so the CLI
        // can still accept `--entry` arguments.
        let entries = detect_entries(dir.path()).expect("detect");
        assert!(entries.is_empty(), "malformed pyproject must not yield entries");
    }

    #[test]
    fn unresolvable_script_spec_is_skipped() {
        let dir = tempfile::tempdir().expect("tempdir");
        // script points at a module that has no .py file anywhere under the root.
        std::fs::write(
            dir.path().join("pyproject.toml"),
            r#"
[project]
name = "demo"
version = "0.1.0"

[project.scripts]
demo = "nonexistent.module:run"
"#,
        )
        .expect("write");
        let entries = detect_entries(dir.path()).expect("detect");
        assert!(entries.is_empty(), "unresolvable script spec must be skipped silently");
    }

    #[test]
    fn tyreach_toml_missing_is_empty_not_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let entries = parse_tyreach_toml(dir.path()).expect("parse");
        assert!(entries.is_empty());
    }

    #[test]
    fn tyreach_toml_parses_entries_with_default_function() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pkg = dir.path().join("myapp");
        std::fs::create_dir_all(&pkg).expect("mkdir");
        std::fs::write(pkg.join("cli.py"), "def main():\n    pass\n").expect("cli.py");
        std::fs::write(pkg.join("worker.py"), "def go():\n    pass\n").expect("worker.py");

        std::fs::write(
            dir.path().join("tyreach.toml"),
            r#"
[[entries]]
name = "cli"
entry_file = "myapp/cli.py"

[[entries]]
name = "worker"
entry_file = "myapp/worker.py"
function = "go"
"#,
        )
        .expect("write tyreach.toml");

        let entries = parse_tyreach_toml(dir.path()).expect("parse");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "cli");
        assert_eq!(entries[0].function, "main", "default function is `main`");
        assert_eq!(entries[1].name, "worker");
        assert_eq!(entries[1].function, "go");
    }

    #[test]
    fn tyreach_toml_malformed_reports_location() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Unterminated table header — toml parser will reject at a known span.
        std::fs::write(dir.path().join("tyreach.toml"), "[[entries\nname = \"x\"\n")
            .expect("write");
        let err = parse_tyreach_toml(dir.path()).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("malformed tyreach.toml"), "missing prefix: {msg}");
        assert!(msg.contains(":1:"), "malformed toml must report line 1: {msg}");
    }

    #[test]
    fn tyreach_toml_missing_entry_file_errors() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("tyreach.toml"),
            "[[entries]]\nname = \"x\"\nentry_file = \"nope.py\"\n",
        )
        .expect("write");
        let err = parse_tyreach_toml(dir.path()).unwrap_err();
        assert!(err.to_string().contains("does not exist"), "got: {err}");
    }

    #[test]
    fn resolve_entries_cli_beats_config_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pkg = dir.path().join("app");
        std::fs::create_dir_all(&pkg).expect("mkdir");
        std::fs::write(pkg.join("cli.py"), "def main():\n    pass\n").expect("cli");
        std::fs::write(pkg.join("other.py"), "def run():\n    pass\n").expect("other");
        std::fs::write(
            dir.path().join("pyproject.toml"),
            "[project]\nname=\"x\"\nversion=\"0.1.0\"\n[project.scripts]\nx = \"app.cli:main\"\n",
        )
        .expect("pyproject");
        std::fs::write(
            dir.path().join("tyreach.toml"),
            "[[entries]]\nname=\"cli\"\nentry_file=\"app/cli.py\"\n",
        )
        .expect("tyreach");

        let entries = resolve_entries(dir.path(), &["app/other.py::run".to_owned()]).expect("ok");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].function, "run", "cli must override config files");
    }

    #[test]
    fn resolve_entries_tyreach_beats_pyproject() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pkg = dir.path().join("app");
        std::fs::create_dir_all(&pkg).expect("mkdir");
        std::fs::write(pkg.join("__init__.py"), "").expect("init");
        std::fs::write(pkg.join("cli.py"), "def main():\n    pass\n").expect("cli");
        std::fs::write(pkg.join("special.py"), "def run():\n    pass\n").expect("special");
        std::fs::write(
            dir.path().join("pyproject.toml"),
            "[project]\nname=\"x\"\nversion=\"0.1.0\"\n[project.scripts]\nx = \"app.cli:main\"\n",
        )
        .expect("pyproject");
        std::fs::write(
            dir.path().join("tyreach.toml"),
            "[[entries]]\nname=\"special\"\nentry_file=\"app/special.py\"\nfunction=\"run\"\n",
        )
        .expect("tyreach");

        let entries = resolve_entries(dir.path(), &[]).expect("ok");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "special");
        assert_eq!(entries[0].function, "run");
    }

    #[test]
    fn resolve_entries_falls_back_to_pyproject() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pkg = dir.path().join("app");
        std::fs::create_dir_all(&pkg).expect("mkdir");
        std::fs::write(pkg.join("__init__.py"), "").expect("init");
        std::fs::write(pkg.join("cli.py"), "def main():\n    pass\n").expect("cli");
        std::fs::write(
            dir.path().join("pyproject.toml"),
            "[project]\nname=\"x\"\nversion=\"0.1.0\"\n[project.scripts]\nx = \"app.cli:main\"\n",
        )
        .expect("pyproject");

        let entries = resolve_entries(dir.path(), &[]).expect("ok");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].function, "main");
    }

    #[test]
    fn resolve_entries_errors_when_nothing_configured() {
        let dir = tempfile::tempdir().expect("tempdir");
        let err = resolve_entries(dir.path(), &[]).unwrap_err();
        assert!(err.to_string().contains("no entry points"), "unexpected: {err}");
    }
}
