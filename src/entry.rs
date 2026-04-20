//! Entry-point discovery.
//!
//! Two sources:
//!
//! 1. `pyproject.toml` `[project.scripts]` — auto-detected via
//!    [`detect_entries`]. Script specs look like `"name" = "module.path:func"`.
//!    We resolve the module portion to a `.py` file on disk by trying, in
//!    order: `{root}/{path}.py`, `{root}/src/{path}.py`,
//!    `{root}/{path}/__init__.py`, `{root}/src/{path}/__init__.py`.
//! 2. `--entry` CLI flag — parsed via [`parse_cli_entry`]. Accepts
//!    `path/to/file.py::func`. The `::func` suffix is mandatory for v1.
//!
//! `tyreach.toml` and Dockerfile entry-point parsing are deferred to later
//! phases; the skeleton would slot in here.

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
}
