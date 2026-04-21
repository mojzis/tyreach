//! `tyreach snapshot` must loud-fail on structurally-valid but empty
//! snapshots: the 0-node/0-edge case was a silent foot-gun because agents
//! read an empty rendered view and conclude "no reachable code" when the
//! real cause is usually an entry pointing at a non-function object.
//!
//! This test exercises the "function not found" path, which does NOT
//! require ty — the walker bails before any LSP call when
//! `find_function_range` returns None. Exit code must stay 0.

#![allow(clippy::expect_used, reason = "test fixtures want terse panics on FS errors")]

use std::fs;
use std::path::Path;

use assert_cmd::Command;

fn write(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("mkdir");
    }
    fs::write(path, body).expect("write");
}

#[test]
fn snapshot_warns_on_empty_output_when_function_missing() {
    let dir = tempfile::tempdir().expect("tempdir");

    // Entry file exists, but the named function doesn't — walker bails
    // early, no nodes, no edges. No `ty` traffic; this test runs without
    // TY_AVAILABLE.
    let pkg = dir.path().join("demo");
    write(&pkg.join("__init__.py"), "");
    write(&pkg.join("cli.py"), "def other():\n    pass\n");

    let out_prefix = dir.path().join("empty-snap");

    let assert = Command::cargo_bin("tyreach")
        .expect("cargo bin")
        .current_dir(dir.path())
        .arg("snapshot")
        .arg(dir.path())
        .arg("--entry")
        .arg("demo/cli.py::does_not_exist")
        .arg("--out")
        .arg(&out_prefix)
        .assert()
        // Exit code stays 0 even when the snapshot is empty: changing it
        // would break scripted callers that key on exit status.
        .success();

    let stderr = String::from_utf8(assert.get_output().stderr.clone()).expect("utf8 stderr");

    assert!(
        stderr.contains("EMPTY snapshot"),
        "expected EMPTY snapshot warning on stderr, got:\n{stderr}"
    );
    assert!(
        stderr.contains("non-function object"),
        "expected likely-causes hint on stderr, got:\n{stderr}"
    );
    assert!(
        stderr.contains("tyreach setup"),
        "expected `tyreach setup` pointer on stderr, got:\n{stderr}"
    );
}
