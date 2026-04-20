//! Node ranking.
//!
//! Simple weighted sum of BFS-depth (shallower = more important) and fan-in
//! (called from more places = more important). Entry points — nodes at depth
//! 0 — receive `f64::INFINITY` so they always top the ranking and survive
//! every budget truncation.
//!
//! `rank` writes scores into `snapshot.scores` without reordering
//! `snapshot.nodes`; downstream callers (rendering, budget fitting) pick their
//! own sort policy.
//
// TODO(v2): PageRank. A single forward pass over edges with a damping factor
// gives a principled fan-in weight instead of the flat `0.3 * count` below.

use crate::walker::Snapshot;

const FANIN_WEIGHT: f64 = 0.3;

/// Compute a score per node and store it in `snapshot.scores`.
///
/// Scoring rule:
///   - Entry-point nodes (depth 0): `f64::INFINITY`.
///   - All others: `1 / (depth + 1) + 0.3 * fan_in`.
///
/// Fan-in counts every incoming edge. Union-annotated edges already appear
/// once per target (see walker emission), so counting all edges per-target is
/// correct — no special casing needed.
pub fn rank(snapshot: &mut Snapshot) {
    snapshot.scores.clear();

    for node in &snapshot.nodes {
        let fanin = snapshot.edges.iter().filter(|e| e.to == node.qname).count();
        let depth = snapshot.depth_by_qname.get(&node.qname).copied();

        let score = match depth {
            Some(0) => f64::INFINITY,
            Some(d) => {
                let depth_term = 1.0 / f64::from(d.saturating_add(1));
                // fanin is bounded by edges.len() (≤ u32 in practice).
                #[allow(
                    clippy::cast_precision_loss,
                    reason = "fan-in counts up to millions; precision loss irrelevant for ordering"
                )]
                let fanin_term = FANIN_WEIGHT * (fanin as f64);
                depth_term + fanin_term
            }
            None => {
                // Nodes without a recorded depth (externals, unresolved leaves)
                // score on fan-in alone — they act as pure sinks.
                #[allow(
                    clippy::cast_precision_loss,
                    reason = "fan-in counts up to millions; precision loss irrelevant for ordering"
                )]
                let fanin_term = FANIN_WEIGHT * (fanin as f64);
                fanin_term
            }
        };

        snapshot.scores.insert(node.qname.clone(), score);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::model::{Annotation, Edge, Kind, Node};
    use crate::walker::Snapshot;

    fn internal(qname: &str) -> Node {
        Node {
            qname: qname.to_owned(),
            signature: String::new(),
            doc: String::new(),
            file: "x.py".to_owned(),
            line: 1,
            kind: Kind::Internal,
        }
    }

    fn edge(from: &str, to: &str) -> Edge {
        Edge { from: from.to_owned(), to: to.to_owned(), annotation: Annotation::Resolved }
    }

    #[test]
    fn entry_points_score_infinity() {
        let mut snapshot = Snapshot {
            nodes: vec![internal("main"), internal("helper")],
            edges: vec![edge("main", "helper")],
            depth_by_qname: HashMap::from([("main".to_owned(), 0), ("helper".to_owned(), 1)]),
            ..Snapshot::default()
        };
        rank(&mut snapshot);
        assert!(snapshot.scores["main"].is_infinite(), "entry must be infinity");
        assert!(snapshot.scores["helper"].is_finite(), "non-entry must be finite");
        assert!(snapshot.scores["main"] > snapshot.scores["helper"]);
    }

    #[test]
    fn higher_fanin_beats_single_caller() {
        // `popular` is reachable from three distinct callers at depth 2; `lone`
        // is at the same depth with a single caller. The ranker must prefer
        // `popular`.
        let mut snapshot = Snapshot {
            nodes: vec![
                internal("main"),
                internal("a"),
                internal("b"),
                internal("c"),
                internal("popular"),
                internal("lone"),
            ],
            edges: vec![
                edge("main", "a"),
                edge("main", "b"),
                edge("main", "c"),
                edge("a", "popular"),
                edge("b", "popular"),
                edge("c", "popular"),
                edge("a", "lone"),
            ],
            depth_by_qname: HashMap::from([
                ("main".to_owned(), 0),
                ("a".to_owned(), 1),
                ("b".to_owned(), 1),
                ("c".to_owned(), 1),
                ("popular".to_owned(), 2),
                ("lone".to_owned(), 2),
            ]),
            ..Snapshot::default()
        };
        rank(&mut snapshot);
        assert!(
            snapshot.scores["popular"] > snapshot.scores["lone"],
            "three-caller node must outrank single-caller node at same depth"
        );
    }

    #[test]
    fn shallower_beats_deeper_at_equal_fanin() {
        let mut snapshot = Snapshot {
            nodes: vec![internal("main"), internal("shallow"), internal("deep")],
            edges: vec![edge("main", "shallow"), edge("shallow", "deep")],
            depth_by_qname: HashMap::from([
                ("main".to_owned(), 0),
                ("shallow".to_owned(), 1),
                ("deep".to_owned(), 2),
            ]),
            ..Snapshot::default()
        };
        rank(&mut snapshot);
        assert!(snapshot.scores["shallow"] > snapshot.scores["deep"]);
    }

    #[test]
    fn node_without_depth_scores_on_fanin() {
        // Externals carry no depth entry; they score purely on fan-in.
        let mut snapshot = Snapshot {
            nodes: vec![
                internal("main"),
                Node {
                    qname: "ext.leaf".to_owned(),
                    signature: String::new(),
                    doc: String::new(),
                    file: String::new(),
                    line: 0,
                    kind: Kind::External,
                },
            ],
            edges: vec![Edge {
                from: "main".to_owned(),
                to: "ext.leaf".to_owned(),
                annotation: Annotation::External,
            }],
            depth_by_qname: HashMap::from([("main".to_owned(), 0)]),
            ..Snapshot::default()
        };
        rank(&mut snapshot);
        assert!((snapshot.scores["ext.leaf"] - 0.3).abs() < 1e-9);
    }
}
