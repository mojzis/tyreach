//! Graceful-degradation smoke test on `tests/fixtures/realistic`.
//!
//! The fixture covers several realistic Python patterns that are either
//! unresolvable in v1 or only partially resolvable: `functools.partial`
//! indirection, `getattr` dispatch, a `@cached` decorator, an inheritance
//! diamond, and `super().__init__()`. The test asserts that tyreach
//! *terminates without panicking* and produces a snapshot in the expected
//! rough shape — not exact counts. The ranges below are guidance; if they
//! drift, tune them once after inspecting the new output rather than fighting
//! the test.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use tyreach::entry::EntryPoint;
use tyreach::model::{Annotation, Kind};

fn realistic_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/realistic")
}

fn ty_available() -> bool {
    std::env::var("TY_AVAILABLE").is_ok()
}

#[tokio::test(flavor = "multi_thread")]
async fn realistic_fixture_walks_gracefully() {
    if !ty_available() {
        eprintln!("skipping: set TY_AVAILABLE=1 to run");
        return;
    }

    let root = realistic_root();
    let entry = EntryPoint {
        name: "realistic".to_owned(),
        file: root.join("realistic/cli.py"),
        function: "main".to_owned(),
    };

    let deadline = Duration::from_secs(60);
    let start = Instant::now();
    let snapshot = tokio::time::timeout(deadline, tyreach::snapshot(&root, vec![entry]))
        .await
        .expect("snapshot within 60s")
        .expect("snapshot ok");
    let elapsed = start.elapsed();
    assert!(elapsed < deadline, "snapshot took {elapsed:?}, expected <60s");

    // --- Scale / shape assertions. ---
    assert!(
        snapshot.nodes.len() > 15,
        "expected >15 nodes, got {}: {:#?}",
        snapshot.nodes.len(),
        snapshot.nodes.iter().map(|n| &n.qname).collect::<Vec<_>>()
    );

    // --- Graceful-degradation counts. ---
    let unresolved =
        snapshot.edges.iter().filter(|e| e.annotation == Annotation::Unresolved).count();
    assert!(
        (1..=10).contains(&unresolved),
        "unresolved edge count {unresolved} outside [1, 10]: {:#?}",
        snapshot.edges
    );

    // Union count is tuned to the fixture's actual output (6 currently). The
    // exact ceiling is guidance; what matters is the walk terminated and did
    // not drown in unions.
    let union = snapshot.edges.iter().filter(|e| e.annotation == Annotation::Union).count();
    assert!(union <= 10, "union edge count {union} > 10: {:#?}", snapshot.edges);

    // --- Entry point present. ---
    let has_entry = snapshot.nodes.iter().any(|n| n.qname == "realistic.cli.main");
    assert!(
        has_entry,
        "entry qname realistic.cli.main missing; got {:?}",
        snapshot.nodes.iter().map(|n| &n.qname).collect::<Vec<_>>()
    );

    // --- At least one external edge (functools.partial or a builtin). ---
    let external_edges =
        snapshot.edges.iter().filter(|e| e.annotation == Annotation::External).count();
    let external_nodes = snapshot.nodes.iter().filter(|n| n.kind == Kind::External).count();
    assert!(
        external_edges >= 1 || external_nodes >= 1,
        "expected at least one external edge or node; got 0"
    );
}
