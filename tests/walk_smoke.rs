//! End-to-end smoke tests for the BFS walker on `medium_app`.
//!
//! Gate on `TY_AVAILABLE` since spawning ty is slow and not available in
//! every CI job.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use tyreach::entry::EntryPoint;
use tyreach::model::{Annotation, Kind};

fn medium_app_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/medium_app")
}

fn ty_available() -> bool {
    std::env::var("TY_AVAILABLE").is_ok()
}

#[tokio::test(flavor = "multi_thread")]
async fn walk_main_entry_produces_diamond_and_external() {
    if !ty_available() {
        eprintln!("skipping: set TY_AVAILABLE=1 to run");
        return;
    }

    let root = medium_app_root();
    let main_file = root.join("medium_app/main.py");
    let entry = EntryPoint {
        name: "main".to_owned(),
        file: main_file.clone(),
        function: "main".to_owned(),
    };

    let deadline = Duration::from_secs(30);
    let start = Instant::now();
    let snapshot = tokio::time::timeout(deadline, tyreach::snapshot(&root, vec![entry]))
        .await
        .expect("snapshot within 30s")
        .expect("snapshot ok");
    let elapsed = start.elapsed();
    assert!(elapsed < deadline, "snapshot took {elapsed:?}, expected <30s");

    // --- Diamond assertion: util appears exactly once. ---
    let util_nodes: Vec<_> =
        snapshot.nodes.iter().filter(|n| n.qname == "medium_app.shared.util").collect();
    assert_eq!(
        util_nodes.len(),
        1,
        "util node must appear exactly once, got {}: {:#?}",
        util_nodes.len(),
        snapshot.nodes.iter().map(|n| &n.qname).collect::<Vec<_>>()
    );

    // --- Two edges point to util (from run_a and from run_b). ---
    let util_edges: Vec<_> =
        snapshot.edges.iter().filter(|e| e.to == "medium_app.shared.util").collect();
    assert_eq!(
        util_edges.len(),
        2,
        "expected 2 edges to util, got {}: {util_edges:#?}",
        util_edges.len()
    );
    let froms: Vec<&str> = util_edges.iter().map(|e| e.from.as_str()).collect();
    assert!(froms.contains(&"medium_app.a.run_a"), "missing edge run_a -> util, got {froms:?}");
    assert!(froms.contains(&"medium_app.b.run_b"), "missing edge run_b -> util, got {froms:?}");

    // --- json.dumps is External. ---
    let dumps_node = snapshot
        .nodes
        .iter()
        .find(|n| {
            let q = n.qname.as_str();
            q == "dumps" || q.rsplit('.').next() == Some("dumps")
        })
        .unwrap_or_else(|| {
            panic!(
                "no dumps node found; nodes: {:#?}",
                snapshot.nodes.iter().map(|n| &n.qname).collect::<Vec<_>>()
            )
        });
    assert_eq!(
        dumps_node.kind,
        Kind::External,
        "json.dumps must be External, got {:?}",
        dumps_node.kind
    );
    // External nodes carry an empty signature: tyreach deliberately skips
    // the hover LSP call into site-packages to avoid 5 s-per-call tarpits
    // on heavyweight deps. If this assertion fails, the hover has been
    // re-enabled — don't "fix" the test, re-read `handle_location`.
    assert_eq!(
        dumps_node.signature, "",
        "External node `{}` must have empty signature; got {:?}",
        dumps_node.qname, dumps_node.signature
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn walk_recursive_entry_terminates() {
    if !ty_available() {
        eprintln!("skipping: set TY_AVAILABLE=1 to run");
        return;
    }

    let root = medium_app_root();
    let entry = EntryPoint {
        name: "recur".to_owned(),
        file: root.join("medium_app/main.py"),
        function: "recur".to_owned(),
    };

    let deadline = Duration::from_secs(10);
    let snapshot = tokio::time::timeout(deadline, tyreach::snapshot(&root, vec![entry]))
        .await
        .expect("recur snapshot must finish within 10s")
        .expect("recur snapshot ok");

    // recur exists exactly once, regardless of the self-call.
    let recur_nodes: Vec<_> =
        snapshot.nodes.iter().filter(|n| n.qname.rsplit('.').next() == Some("recur")).collect();
    assert_eq!(
        recur_nodes.len(),
        1,
        "recur appears once, got {}: {:#?}",
        recur_nodes.len(),
        snapshot.nodes
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn walk_dyn_getattr_is_unresolved() {
    if !ty_available() {
        eprintln!("skipping: set TY_AVAILABLE=1 to run");
        return;
    }

    let root = medium_app_root();
    let entry = EntryPoint {
        name: "caller".to_owned(),
        file: root.join("medium_app/dyn.py"),
        function: "caller".to_owned(),
    };

    let deadline = Duration::from_secs(15);
    let snapshot = tokio::time::timeout(deadline, tyreach::snapshot(&root, vec![entry]))
        .await
        .expect("dyn snapshot must finish within 15s")
        .expect("dyn snapshot ok");

    // The inner getattr(obj, name)() is resolvable (getattr is builtin),
    // but the *result* call — the `()` on `getattr(...)` — is dynamic and
    // must either be unresolved or an edge to something clearly unresolvable.
    let has_unresolved_edge = snapshot.edges.iter().any(|e| e.annotation == Annotation::Unresolved);
    assert!(
        has_unresolved_edge,
        "expected at least one unresolved edge; got {:#?}",
        snapshot.edges
    );
}
