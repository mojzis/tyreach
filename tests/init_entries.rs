//! `detect_init_entries` / `__init__.py` fallback source tests.
//!
//! Fixtures mirror the four shapes `detect_init_entries` handles:
//!
//! * `__all__` wins when present (and underscore-prefixed names are omitted
//!   regardless of origin).
//! * Absent `__all__` → top-level `def`/`class` (non-underscore).
//! * Absent `__all__` → `from .submod import X` re-exports.
//! * Empty `__init__.py` → zero entries, not an error.
//!
//! Tests also confirm precedence: `resolve_entries` only falls through to
//! `detect_init_entries` when CLI, tyreach.toml, and pyproject scripts all
//! produce nothing.

#![allow(clippy::expect_used, reason = "test fixtures want terse panics on FS errors")]

use std::fs;
use std::path::Path;

use tyreach::entry::{detect_init_entries, resolve_entries};

fn write(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("mkdir");
    }
    fs::write(path, body).expect("write");
}

#[test]
fn fixture_1_dunder_all_wins_and_excludes_underscore() {
    let dir = tempfile::tempdir().expect("tempdir");
    let pkg = dir.path().join("mylib");
    write(
        &pkg.join("__init__.py"),
        r#"__all__ = ["foo", "bar"]

def foo():
    pass

def bar():
    pass

def _hidden():
    pass
"#,
    );

    let entries = detect_init_entries(dir.path()).expect("detect");
    let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["foo", "bar"], "__all__ must drive the export list");
    for e in &entries {
        assert_eq!(e.function, e.name, "function must mirror the export name");
        assert!(
            e.file.ends_with("mylib/__init__.py") || e.file.ends_with("mylib\\__init__.py"),
            "entry file must point at the package __init__.py, got {}",
            e.file.display()
        );
    }
}

#[test]
fn fixture_2_top_level_defs_when_no_dunder_all() {
    let dir = tempfile::tempdir().expect("tempdir");
    let pkg = dir.path().join("mylib");
    write(
        &pkg.join("__init__.py"),
        r"def alpha():
    pass

def beta():
    pass

def _helper():
    pass
",
    );

    let entries = detect_init_entries(dir.path()).expect("detect");
    let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["alpha", "beta"], "underscore-prefixed names must be omitted");
}

#[test]
fn fixture_3_relative_reexport_when_no_dunder_all() {
    let dir = tempfile::tempdir().expect("tempdir");
    let pkg = dir.path().join("mylib");
    write(&pkg.join("submod.py"), "def X():\n    pass\n");
    write(
        &pkg.join("__init__.py"),
        r"from .submod import X
",
    );

    let entries = detect_init_entries(dir.path()).expect("detect");
    let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["X"], "single relative re-export must surface as one entry");
}

#[test]
fn fixture_4_empty_init_yields_zero_entries() {
    let dir = tempfile::tempdir().expect("tempdir");
    let pkg = dir.path().join("mylib");
    write(&pkg.join("__init__.py"), "");

    let entries = detect_init_entries(dir.path()).expect("detect");
    assert!(entries.is_empty(), "empty __init__.py must yield zero entries, not an error");
}

#[test]
fn aliased_reexport_uses_alias_name() {
    // `from .m import X as Y` — Y is the public name per PEP 8 conventions.
    let dir = tempfile::tempdir().expect("tempdir");
    let pkg = dir.path().join("mylib");
    write(&pkg.join("submod.py"), "def X():\n    pass\n");
    write(&pkg.join("__init__.py"), "from .submod import X as Y\n");

    let entries = detect_init_entries(dir.path()).expect("detect");
    let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["Y"], "aliased re-export must surface as the alias");
}

#[test]
fn src_layout_is_scanned() {
    let dir = tempfile::tempdir().expect("tempdir");
    let pkg = dir.path().join("src/mylib");
    write(
        &pkg.join("__init__.py"),
        r"def hello():
    pass
",
    );

    let entries = detect_init_entries(dir.path()).expect("detect");
    let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["hello"]);
}

#[test]
fn init_is_only_source_when_nothing_else_configured() {
    // resolve_entries must fall all the way through CLI -> tyreach.toml ->
    // pyproject scripts and land on detect_init_entries.
    let dir = tempfile::tempdir().expect("tempdir");
    let pkg = dir.path().join("mylib");
    write(
        &pkg.join("__init__.py"),
        r#"__all__ = ["run"]

def run():
    pass
"#,
    );

    let entries = resolve_entries(dir.path(), &[]).expect("resolve");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "run");
    assert_eq!(entries[0].function, "run");
}

#[test]
fn pyproject_scripts_take_precedence_over_init_exports() {
    // When both sources have entries, pyproject scripts must win — init.py
    // is a fallback, not a peer.
    let dir = tempfile::tempdir().expect("tempdir");
    let pkg = dir.path().join("mylib");
    write(&pkg.join("cli.py"), "def main():\n    pass\n");
    write(
        &pkg.join("__init__.py"),
        r#"__all__ = ["exported"]

def exported():
    pass
"#,
    );
    write(
        &dir.path().join("pyproject.toml"),
        r#"[project]
name = "mylib"
version = "0.1.0"

[project.scripts]
demo = "mylib.cli:main"
"#,
    );

    let entries = resolve_entries(dir.path(), &[]).expect("resolve");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "demo", "pyproject scripts must eclipse __init__ exports");
    assert_eq!(entries[0].function, "main");
}

#[test]
fn dunder_all_blocks_def_and_import_fallbacks() {
    // When __all__ is present, the def/class + re-export paths MUST NOT
    // contribute, even if the def/class name is public and the re-export
    // is present. This mirrors Python's own `from pkg import *` rule.
    let dir = tempfile::tempdir().expect("tempdir");
    let pkg = dir.path().join("mylib");
    write(&pkg.join("submod.py"), "def extra():\n    pass\n");
    write(
        &pkg.join("__init__.py"),
        r#"__all__ = ["chosen"]

def chosen():
    pass

def also_public():
    pass

from .submod import extra
"#,
    );

    let entries = detect_init_entries(dir.path()).expect("detect");
    let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["chosen"], "__all__ is the sole export source when present");
}

#[test]
fn classes_count_as_exports_without_dunder_all() {
    let dir = tempfile::tempdir().expect("tempdir");
    let pkg = dir.path().join("mylib");
    write(
        &pkg.join("__init__.py"),
        r"class Thing:
    pass

class _Internal:
    pass
",
    );

    let entries = detect_init_entries(dir.path()).expect("detect");
    let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["Thing"]);
}

#[test]
fn multiple_packages_union_with_dedup() {
    // Two packages at repo root, each exporting a name; result is the union.
    let dir = tempfile::tempdir().expect("tempdir");
    write(
        &dir.path().join("pkg_a/__init__.py"),
        r#"__all__ = ["shared", "only_a"]

def shared():
    pass

def only_a():
    pass
"#,
    );
    write(
        &dir.path().join("pkg_b/__init__.py"),
        r#"__all__ = ["shared", "only_b"]

def shared():
    pass

def only_b():
    pass
"#,
    );

    let entries = detect_init_entries(dir.path()).expect("detect");
    let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
    // Stable `read_dir` order is enforced by candidate_init_files sort:
    // pkg_a then pkg_b.
    assert_eq!(names, vec!["shared", "only_a", "only_b"]);
}
