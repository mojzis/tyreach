//! Token-budget truncation.
//!
//! Drops the lowest-scored non-entry-point nodes (cascading dropped edges)
//! until an approximate TOON-token estimate fits under `budget_tokens`.
//! The estimate is a char-count / 4 approximation — good enough to drive
//! budget decisions without re-encoding on every iteration.
//!
//! Entry points (score = `f64::INFINITY` from `rank::rank`) are never
//! dropped. If the budget is so small that even the entry points alone
//! exceed it, the function returns all entry points with zero edges and
//! flags the full drop in `Snapshot::truncation`.

use crate::walker::{Snapshot, Truncation};

/// Rough chars-per-token divisor (matches GPT-style estimators).
const CHARS_PER_TOKEN: usize = 4;

/// Per-row overhead we add for the TOON column separators, key markers, and
/// trailing newline — rough but consistent across estimate iterations.
const NODE_ROW_OVERHEAD: usize = 30;
const EDGE_ROW_OVERHEAD: usize = 15;

/// Shrink `snapshot` so its estimated TOON-token count is ≤ `budget_tokens`.
///
/// Strategy:
///   1. Estimate current size. If already under budget, return as-is.
///   2. Otherwise repeatedly drop the lowest-scored non-entry-point node and
///      every edge touching it. Recompute the estimate after each drop.
///   3. Bound the loop at `nodes.len()` iterations.
///
/// Entry points (score = `f64::INFINITY`) are preserved even when budget is
/// tiny; the returned `Snapshot.truncation` reflects the drop count.
pub fn fit_to_budget(mut snapshot: Snapshot, budget_tokens: usize) -> Snapshot {
    let original_node_count = snapshot.nodes.len();

    if estimate_tokens(&snapshot) <= budget_tokens {
        return snapshot;
    }

    let max_iters = snapshot.nodes.len();
    for _ in 0..max_iters {
        let Some(drop_qname) = lowest_scored_droppable(&snapshot) else {
            break;
        };
        drop_node(&mut snapshot, &drop_qname);
        if estimate_tokens(&snapshot) <= budget_tokens {
            break;
        }
    }

    let dropped = original_node_count.saturating_sub(snapshot.nodes.len());
    if dropped > 0 {
        snapshot.truncation = Some(Truncation {
            dropped_nodes: u32::try_from(dropped).unwrap_or(u32::MAX),
            original_node_count: u32::try_from(original_node_count).unwrap_or(u32::MAX),
        });
    }
    snapshot
}

/// Approximate TOON token cost of a snapshot. Counts characters per row
/// (fields + a small fixed overhead) and divides by `CHARS_PER_TOKEN`.
fn estimate_tokens(snapshot: &Snapshot) -> usize {
    let node_chars: usize = snapshot
        .nodes
        .iter()
        .map(|n| n.qname.len() + n.signature.len() + n.doc.len() + n.file.len() + NODE_ROW_OVERHEAD)
        .sum();
    let edge_chars: usize =
        snapshot.edges.iter().map(|e| e.from.len() + e.to.len() + EDGE_ROW_OVERHEAD).sum();
    (node_chars + edge_chars) / CHARS_PER_TOKEN
}

/// Return the qname of the lowest-scored non-entry-point node, or `None` if
/// nothing is left to drop.
fn lowest_scored_droppable(snapshot: &Snapshot) -> Option<String> {
    snapshot
        .nodes
        .iter()
        .filter_map(|n| {
            let score = snapshot.scores.get(&n.qname).copied().unwrap_or(0.0);
            if score.is_infinite() {
                None
            } else {
                Some((n.qname.clone(), score))
            }
        })
        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(qname, _)| qname)
}

/// Remove a node by qname and every edge that touches it.
fn drop_node(snapshot: &mut Snapshot, qname: &str) {
    snapshot.nodes.retain(|n| n.qname != qname);
    snapshot.edges.retain(|e| e.from != qname && e.to != qname);
    snapshot.scores.remove(qname);
    snapshot.depth_by_qname.remove(qname);
}

/// Prune edges whose endpoints no longer exist as nodes. Used after manual
/// edits to `nodes` — in this module we keep edges/nodes in sync via
/// `drop_node`, but exported for belt-and-suspenders use from tests.
#[cfg(test)]
fn prune_orphan_edges(snapshot: &mut Snapshot) {
    use std::collections::HashSet;
    let present: HashSet<&str> = snapshot.nodes.iter().map(|n| n.qname.as_str()).collect();
    snapshot.edges.retain(|e| present.contains(e.from.as_str()) && present.contains(e.to.as_str()));
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::model::{Annotation, Edge, Kind, Node};
    use crate::rank;

    fn node(qname: &str) -> Node {
        Node {
            qname: qname.to_owned(),
            signature: "def f()".to_owned(),
            doc: "doc".to_owned(),
            file: "f.py".to_owned(),
            line: 1,
            kind: Kind::Internal,
        }
    }

    fn edge(from: &str, to: &str) -> Edge {
        Edge { from: from.to_owned(), to: to.to_owned(), annotation: Annotation::Resolved }
    }

    fn chain_snapshot() -> Snapshot {
        // main -> a -> b -> c; depths 0..3.
        let mut snap = Snapshot {
            nodes: vec![node("main"), node("a"), node("b"), node("c")],
            edges: vec![edge("main", "a"), edge("a", "b"), edge("b", "c")],
            depth_by_qname: HashMap::from([
                ("main".to_owned(), 0),
                ("a".to_owned(), 1),
                ("b".to_owned(), 2),
                ("c".to_owned(), 3),
            ]),
            ..Snapshot::default()
        };
        rank::rank(&mut snap);
        snap
    }

    #[test]
    fn generous_budget_drops_nothing() {
        let snap = chain_snapshot();
        let out = fit_to_budget(snap.clone(), 10_000);
        assert_eq!(out.nodes.len(), snap.nodes.len());
        assert!(out.truncation.is_none(), "no truncation metadata when under budget");
    }

    #[test]
    fn small_budget_drops_lowest_scored_first() {
        let snap = chain_snapshot();
        // Shrink enough to force at least one drop. The deepest node `c`
        // (depth 3, no incoming fan-in beyond 1) has the lowest score.
        let out = fit_to_budget(snap, 20);
        assert!(out.nodes.len() < 4, "should have dropped at least one node");
        assert!(out.nodes.iter().any(|n| n.qname == "main"), "entry point must survive");
        assert!(out.truncation.is_some(), "truncation metadata must be emitted");

        // The very first node dropped in a chain must be `c` (deepest).
        let dropped: Vec<&str> = ["a", "b", "c"]
            .iter()
            .filter(|q| !out.nodes.iter().any(|n| n.qname == **q))
            .copied()
            .collect();
        assert!(dropped.contains(&"c"), "c (deepest) must be dropped; dropped={dropped:?}");
    }

    #[test]
    fn ultra_tiny_budget_keeps_entries_only() {
        let snap = chain_snapshot();
        let out = fit_to_budget(snap, 1);
        // All non-entry nodes dropped; entry survives.
        assert_eq!(out.nodes.len(), 1);
        assert_eq!(out.nodes[0].qname, "main");
        assert!(out.edges.is_empty(), "edges touching dropped nodes must cascade");
        let trunc = out.truncation.expect("truncation metadata required");
        assert_eq!(trunc.original_node_count, 4);
        assert_eq!(trunc.dropped_nodes, 3);
    }

    #[test]
    fn edge_cascade_cleans_up() {
        // Drop middle node — both edges that touch it must disappear.
        let mut snap = chain_snapshot();
        drop_node(&mut snap, "b");
        prune_orphan_edges(&mut snap);
        assert!(
            !snap.edges.iter().any(|e| e.from == "b" || e.to == "b"),
            "no edge may reference dropped node"
        );
        // Only main->a should remain; a->b and b->c both gone.
        assert_eq!(snap.edges.len(), 1);
        assert_eq!(snap.edges[0].from, "main");
        assert_eq!(snap.edges[0].to, "a");
    }

    #[test]
    fn monotone_node_count_in_budget() {
        // Monotonicity: a bigger budget must keep at least as many nodes.
        let snap = chain_snapshot();
        let small = fit_to_budget(snap.clone(), 15);
        let big = fit_to_budget(snap, 10_000);
        assert!(
            small.nodes.len() <= big.nodes.len(),
            "tighter budget must not keep more nodes (small={}, big={})",
            small.nodes.len(),
            big.nodes.len()
        );
    }
}
