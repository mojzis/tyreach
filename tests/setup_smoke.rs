//! `tyreach setup` smoke tests.
//!
//! Exercise the three shapes of repo `setup` cares about:
//! * `pyproject.toml [project.scripts]` only
//! * both `tyreach.toml` and `pyproject.toml` — tyreach.toml wins
//! * empty repo — prints the ready-to-copy skeleton
//!
//! All three cases must exit 0; no-entries is informational, not an error.

#![allow(clippy::expect_used, reason = "test fixtures want terse panics on FS errors")]

use std::fs;
use std::path::Path;

use assert_cmd::Command;
use tempfile::TempDir;

fn write(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("mkdir");
    }
    fs::write(path, body).expect("write");
}

#[test]
fn setup_reports_pyproject_scripts() {
    let dir = tempfile::tempdir().expect("tempdir");
    let pkg = dir.path().join("app");
    write(&pkg.join("__init__.py"), "");
    write(&pkg.join("cli.py"), "def main():\n    pass\n");

    write(
        &dir.path().join("pyproject.toml"),
        r#"[project]
name = "demo"
version = "0.1.0"

[project.scripts]
demo = "app.cli:main"
"#,
    );

    let output = Command::cargo_bin("tyreach")
        .expect("cargo bin")
        .arg("setup")
        .arg(dir.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(output).expect("utf8");

    assert!(
        stdout.contains("[x] pyproject.toml scripts"),
        "expected active pyproject row, got:\n{stdout}"
    );
    assert!(stdout.contains("demo"), "expected entry name `demo` in stdout:\n{stdout}");
    // Resolved file path must appear — the exact canonicalized prefix varies
    // across platforms/tmpdirs, so anchor on the trailing segments.
    assert!(
        stdout.contains("app/cli.py") || stdout.contains("app\\cli.py"),
        "expected resolved `app/cli.py` path in stdout:\n{stdout}"
    );
}

#[test]
fn setup_reports_tyreach_toml_winning() {
    let dir = tempfile::tempdir().expect("tempdir");
    let pkg = dir.path().join("app");
    write(&pkg.join("__init__.py"), "");
    write(&pkg.join("cli.py"), "def main():\n    pass\n");
    write(&pkg.join("special.py"), "def run():\n    pass\n");

    write(
        &dir.path().join("pyproject.toml"),
        r#"[project]
name = "demo"
version = "0.1.0"

[project.scripts]
demo = "app.cli:main"
"#,
    );
    write(
        &dir.path().join("tyreach.toml"),
        r#"[[entries]]
name = "special"
entry_file = "app/special.py"
function = "run"
"#,
    );

    let output = Command::cargo_bin("tyreach")
        .expect("cargo bin")
        .arg("setup")
        .arg(dir.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(output).expect("utf8");

    assert!(
        stdout.contains("[x] tyreach.toml"),
        "expected active tyreach.toml row, got:\n{stdout}"
    );
    assert!(stdout.contains("special"), "expected resolved entry name `special`, got:\n{stdout}");
    assert!(
        stdout.contains("app/special.py") || stdout.contains("app\\special.py"),
        "expected resolved `app/special.py` path in stdout:\n{stdout}"
    );
    // The pyproject entry name must NOT appear in the resolved-entries block.
    // It *may* appear in the sources table (e.g. as part of an "eclipsed by
    // tyreach.toml" note), but only the tyreach.toml entry should be listed
    // as resolved. Assert that `demo` is not on a line under `resolved
    // entries:` by checking that the file target doesn't show up.
    assert!(
        !stdout.contains("app/cli.py") && !stdout.contains("app\\cli.py"),
        "pyproject entry must not be in resolved list when tyreach.toml wins:\n{stdout}"
    );
}

#[test]
fn setup_prints_skeleton_when_empty() {
    let dir: TempDir = tempfile::tempdir().expect("tempdir");
    // Deliberately empty — no pyproject.toml, no tyreach.toml.

    let output = Command::cargo_bin("tyreach")
        .expect("cargo bin")
        .arg("setup")
        .arg(dir.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(output).expect("utf8");

    assert!(
        stdout.contains("no entry points discovered"),
        "expected empty-case preamble, got:\n{stdout}"
    );
    assert!(
        stdout.contains("[[entries]]"),
        "expected tyreach.toml skeleton header, got:\n{stdout}"
    );
    assert!(stdout.contains("entry_file"), "expected entry_file field in skeleton:\n{stdout}");
    assert!(stdout.contains("--entry"), "expected example `--entry` invocation, got:\n{stdout}");
}
