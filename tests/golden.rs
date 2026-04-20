//! Golden snapshot test.
//!
//! Runs the CLI against `tests/fixtures/medium_app` with a generous budget
//! (nothing truncates) and compares both outputs byte-for-byte to the
//! committed goldens at `tests/golden/medium_app.{toon,txt}`. Line endings
//! are normalized before comparison so the test passes on Windows too.
//!
//! Gated by `TY_AVAILABLE=1` because walking requires a working ty LSP.
//! To update the goldens after intentional changes:
//!     make update-golden

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::format_push_string,
    reason = "integration-test helpers; failures should fail loudly"
)]

use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

fn is_ty_available() -> bool {
    std::env::var_os("TY_AVAILABLE").is_some()
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn normalize(s: &str) -> String {
    s.replace("\r\n", "\n")
}

fn read(path: &Path) -> String {
    normalize(&std::fs::read_to_string(path).unwrap_or_else(|e| {
        panic!("failed to read {}: {e}", path.display());
    }))
}

fn diff_line_by_line(actual: &str, expected: &str) -> String {
    let mut out = String::new();
    let a_lines: Vec<&str> = actual.lines().collect();
    let e_lines: Vec<&str> = expected.lines().collect();
    let max = a_lines.len().max(e_lines.len());
    for i in 0..max {
        let a = a_lines.get(i).copied().unwrap_or("<missing>");
        let e = e_lines.get(i).copied().unwrap_or("<missing>");
        if a == e {
            let _ = writeln!(out, "  {a}");
        } else {
            let _ = writeln!(out, "- {e}");
            let _ = writeln!(out, "+ {a}");
        }
    }
    out
}

#[test]
fn golden_snapshot_matches() {
    if !is_ty_available() {
        eprintln!("golden_snapshot_matches: TY_AVAILABLE unset, skipping");
        return;
    }

    let root = workspace_root();
    let fixture = root.join("tests/fixtures/medium_app");
    let golden = root.join("tests/golden/medium_app");
    let tmp = tempfile::tempdir().expect("tempdir");
    let out_prefix = tmp.path().join("out");

    let status = Command::new(env!("CARGO_BIN_EXE_tyreach"))
        .arg("snapshot")
        .arg(&fixture)
        .arg("--budget")
        .arg("5000")
        .arg("--out")
        .arg(&out_prefix)
        .status()
        .expect("run tyreach");
    assert!(status.success(), "tyreach snapshot failed");

    let actual_toon = read(&out_prefix.with_extension("toon"));
    let expected_toon = read(&golden.with_extension("toon"));
    if actual_toon != expected_toon {
        let diff = diff_line_by_line(&actual_toon, &expected_toon);
        panic!("golden .toon mismatch. Diff:\n{diff}\nto update, run `make update-golden`");
    }

    let actual_txt = read(&out_prefix.with_extension("txt"));
    let expected_txt = read(&golden.with_extension("txt"));
    if actual_txt != expected_txt {
        let diff = diff_line_by_line(&actual_txt, &expected_txt);
        panic!("golden .txt mismatch. Diff:\n{diff}\nto update, run `make update-golden`");
    }
}
