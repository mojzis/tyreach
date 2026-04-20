//! Budget smoke tests — synthetic Snapshot only, no ty required.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    reason = "integration-test helpers; failures should fail loudly"
)]

use std::collections::HashMap;

use tyreach::budget::fit_to_budget;
use tyreach::model::{Annotation, Edge, Kind, Node};
use tyreach::rank;
use tyreach::walker::Snapshot;

fn internal(qname: &str) -> Node {
    Node {
        qname: qname.to_owned(),
        signature: "def f() -> None".to_owned(),
        doc: "helper".to_owned(),
        file: "f.py".to_owned(),
        line: 1,
        kind: Kind::Internal,
    }
}

fn edge(from: &str, to: &str) -> Edge {
    Edge { from: from.to_owned(), to: to.to_owned(), annotation: Annotation::Resolved }
}

fn chain() -> Snapshot {
    // main -> a -> b -> c -> d chain.
    let mut snap = Snapshot {
        nodes: vec![internal("main"), internal("a"), internal("b"), internal("c"), internal("d")],
        edges: vec![edge("main", "a"), edge("a", "b"), edge("b", "c"), edge("c", "d")],
        depth_by_qname: HashMap::from([
            ("main".to_owned(), 0),
            ("a".to_owned(), 1),
            ("b".to_owned(), 2),
            ("c".to_owned(), 3),
            ("d".to_owned(), 4),
        ]),
        ..Snapshot::default()
    };
    rank::rank(&mut snap);
    snap
}

#[test]
fn smaller_budget_keeps_fewer_nodes() {
    let snap = chain();
    let tight = fit_to_budget(snap.clone(), 50);
    let loose = fit_to_budget(snap, 5000);
    assert!(
        tight.nodes.len() < loose.nodes.len(),
        "tight budget must keep fewer nodes (tight={}, loose={})",
        tight.nodes.len(),
        loose.nodes.len()
    );
    assert!(tight.truncation.is_some());
    assert!(loose.truncation.is_none());
}

#[test]
fn ultra_tight_budget_keeps_only_entry_points() {
    let snap = chain();
    let original_len = snap.nodes.len();
    let out = fit_to_budget(snap, 10);
    assert_eq!(out.nodes.len(), 1, "only the entry point should survive");
    assert_eq!(out.nodes[0].qname, "main");
    assert!(out.edges.is_empty(), "no edge may remain when all targets are dropped");
    let trunc = out.truncation.expect("truncation metadata required");
    assert_eq!(trunc.original_node_count as usize, original_len);
    assert_eq!(trunc.dropped_nodes as usize, original_len - 1);
}

#[test]
fn entry_points_always_survive() {
    // Two entry points with a shared callee. Both depth-0 nodes must remain.
    let mut snap = Snapshot {
        nodes: vec![internal("entry_a"), internal("entry_b"), internal("shared")],
        edges: vec![edge("entry_a", "shared"), edge("entry_b", "shared")],
        depth_by_qname: HashMap::from([
            ("entry_a".to_owned(), 0),
            ("entry_b".to_owned(), 0),
            ("shared".to_owned(), 1),
        ]),
        ..Snapshot::default()
    };
    rank::rank(&mut snap);
    let out = fit_to_budget(snap, 5);
    let survivors: Vec<&str> = out.nodes.iter().map(|n| n.qname.as_str()).collect();
    assert!(survivors.contains(&"entry_a"), "entry_a missing: {survivors:?}");
    assert!(survivors.contains(&"entry_b"), "entry_b missing: {survivors:?}");
}
