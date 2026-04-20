//! Precedence: CLI --entry > tyreach.toml > pyproject.toml [project.scripts].
//!
//! Exercises `tyreach::entry::resolve_entries` against three combinations of
//! config sources in a temp repo. No ty needed — this is pure TOML / FS
//! plumbing.

#![allow(clippy::expect_used, reason = "test fixtures want terse panics on FS errors")]

use std::fs;
use std::path::Path;

use tempfile::TempDir;
use tyreach::entry::resolve_entries;

fn write(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("mkdir");
    }
    fs::write(path, body).expect("write");
}

/// Build a repo layout with both a pyproject `scripts` entry and, optionally,
/// a `tyreach.toml` file. The caller decides which to supply.
fn seed_repo(dir: &TempDir, with_tyreach: bool) {
    // Common package layout: app/cli.py (pyproject target) and
    // app/special.py (tyreach.toml target).
    let pkg = dir.path().join("app");
    write(&pkg.join("__init__.py"), "");
    write(&pkg.join("cli.py"), "def main():\n    pass\n");
    write(&pkg.join("special.py"), "def run():\n    pass\n");
    write(&pkg.join("other.py"), "def override():\n    pass\n");

    write(
        &dir.path().join("pyproject.toml"),
        r#"[project]
name = "demo"
version = "0.1.0"

[project.scripts]
demo = "app.cli:main"
"#,
    );

    if with_tyreach {
        write(
            &dir.path().join("tyreach.toml"),
            r#"[[entries]]
name = "special"
entry_file = "app/special.py"
function = "run"
"#,
        );
    }
}

#[test]
fn tyreach_toml_wins_when_no_cli_flag() {
    let dir = tempfile::tempdir().expect("tempdir");
    seed_repo(&dir, /* with_tyreach */ true);

    let entries = resolve_entries(dir.path(), &[]).expect("resolve");
    assert_eq!(entries.len(), 1, "exactly one entry expected");
    assert_eq!(entries[0].name, "special", "tyreach.toml must win over pyproject");
    assert_eq!(entries[0].function, "run");
}

#[test]
fn cli_flag_wins_over_tyreach_toml_and_pyproject() {
    let dir = tempfile::tempdir().expect("tempdir");
    seed_repo(&dir, /* with_tyreach */ true);

    let cli = vec!["app/other.py::override".to_owned()];
    let entries = resolve_entries(dir.path(), &cli).expect("resolve");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].function, "override", "CLI must win; got {entries:?}");
}

#[test]
fn pyproject_fallback_when_neither_cli_nor_tyreach() {
    let dir = tempfile::tempdir().expect("tempdir");
    seed_repo(&dir, /* with_tyreach */ false);

    let entries = resolve_entries(dir.path(), &[]).expect("resolve");
    assert_eq!(entries.len(), 1, "pyproject fallback produces exactly the configured script");
    assert_eq!(entries[0].name, "demo");
    assert_eq!(entries[0].function, "main");
}

#[test]
fn empty_repo_errors_with_helpful_message() {
    let dir = tempfile::tempdir().expect("tempdir");
    // No pyproject, no tyreach, no CLI.
    let err = resolve_entries(dir.path(), &[]).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("no entry points"), "unexpected message: {msg}");
}
