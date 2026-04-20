//! Topologically-sorted rendered text view.
//!
//! One non-leaf node per line, `  -> callee, callee, callee` indented under
//! it. Entry-point nodes get `(entry)` marker; externals inline `[ext]`;
//! unresolved inline `[?]`; union-annotated edges show the multiple callees
//! joined by `|` and wrapped in a `[union: a | b]` suffix.
//!
//! External leaf nodes are deliberately skipped as standalone lines — they
//! only appear as inline callees. This matches the example shape in
//! `docs/plans/plan.md`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Write;

use anyhow::{Context, Result};

use crate::model::{Annotation, Edge, Kind, Node};
use crate::walker::Snapshot;

/// Render the snapshot as the topologically-sorted text view.
pub fn render(snapshot: &Snapshot, out: &mut impl Write) -> Result<()> {
    let node_by_qname: HashMap<&str, &Node> =
        snapshot.nodes.iter().map(|n| (n.qname.as_str(), n)).collect();

    let order = topo_order(snapshot, &node_by_qname);
    let callees_by_caller = group_callees(&snapshot.edges);
    let show_metadata = snapshot.truncation.is_none();

    for qname in &order {
        let Some(node) = node_by_qname.get(qname.as_str()) else {
            continue;
        };

        // Pure-leaf externals / unresolved nodes never appear as top-level
        // entries — they only surface inline on their callers.
        let groups = callees_by_caller.get(node.qname.as_str());
        let has_outgoing = groups.is_some_and(|g| !g.is_empty());
        if !has_outgoing && matches!(node.kind, Kind::External | Kind::Unresolved) {
            continue;
        }

        let entry_marker = if is_entry_point(snapshot, &node.qname) { "  (entry)" } else { "" };
        writeln!(out, "{qname}{entry_marker}").context("write node line")?;

        if show_metadata {
            let sig = node.signature.lines().next().unwrap_or("").trim();
            if !sig.is_empty() {
                writeln!(out, "    {sig}").context("write signature")?;
            }
            let doc = node.doc.lines().next().unwrap_or("").trim();
            if !doc.is_empty() {
                writeln!(out, "    {doc}").context("write doc")?;
            }
        }

        if let Some(rows) = groups {
            let line = format_callees(rows, &node_by_qname);
            if !line.is_empty() {
                writeln!(out, "  -> {line}").context("write callees")?;
            }
        }
    }

    Ok(())
}

/// A single call site's callee list. Either one resolved/external/unresolved
/// callee, or a union across N candidates.
enum CalleeGroup<'a> {
    Single(&'a Edge),
    Union(Vec<&'a Edge>),
}

/// Build a map from caller qname to an ordered list of call-site groups.
///
/// Union-annotated edges emitted consecutively by the walker for the same
/// caller are collapsed into a single `Union` group so the renderer can emit
/// them with a `[union: a | b]` suffix.
fn group_callees(edges: &[Edge]) -> HashMap<&str, Vec<CalleeGroup<'_>>> {
    let mut out: HashMap<&str, Vec<CalleeGroup<'_>>> = HashMap::new();
    // Pending union run: the from-qname for the run and the collected edges.
    let mut pending_union: Option<(String, Vec<&Edge>)> = None;

    for edge in edges {
        let is_union = edge.annotation == Annotation::Union;
        match (&mut pending_union, is_union) {
            (Some((from, bucket)), true) if *from == edge.from => {
                bucket.push(edge);
            }
            (Some((from, bucket)), _) => {
                let completed = std::mem::take(bucket);
                let from_key = from.clone();
                flush_union(&mut out, edges, &from_key, completed);
                pending_union = None;
                if is_union {
                    pending_union = Some((edge.from.clone(), vec![edge]));
                } else {
                    push_single(&mut out, edges, edge);
                }
            }
            (None, true) => {
                pending_union = Some((edge.from.clone(), vec![edge]));
            }
            (None, false) => {
                push_single(&mut out, edges, edge);
            }
        }
    }

    if let Some((from, bucket)) = pending_union {
        flush_union(&mut out, edges, &from, bucket);
    }

    out
}

fn flush_union<'a>(
    out: &mut HashMap<&'a str, Vec<CalleeGroup<'a>>>,
    all_edges: &'a [Edge],
    from: &str,
    edges: Vec<&'a Edge>,
) {
    if edges.is_empty() {
        return;
    }
    out.entry(key_str(all_edges, from)).or_default().push(CalleeGroup::Union(edges));
}

fn push_single<'a>(
    out: &mut HashMap<&'a str, Vec<CalleeGroup<'a>>>,
    all_edges: &'a [Edge],
    edge: &'a Edge,
) {
    out.entry(key_str(all_edges, &edge.from)).or_default().push(CalleeGroup::Single(edge));
}

/// Look up the `&str` slice for `from` in `edges`. Callers pass a `from` that
/// was cloned from some edge, so the match always succeeds in practice.
fn key_str<'a>(edges: &'a [Edge], from: &str) -> &'a str {
    for edge in edges {
        if edge.from == from {
            return edge.from.as_str();
        }
    }
    ""
}

fn format_callees(groups: &[CalleeGroup<'_>], nodes: &HashMap<&str, &Node>) -> String {
    let mut parts: Vec<String> = Vec::new();
    for group in groups {
        match group {
            CalleeGroup::Single(edge) => {
                let display = unresolved_display_for(edge.to.as_str());
                let suffix = match edge.annotation {
                    Annotation::External => " [ext]",
                    Annotation::Unresolved => " [?]",
                    Annotation::Resolved | Annotation::Union => "",
                };
                parts.push(format!("{display}{suffix}"));
            }
            CalleeGroup::Union(edges) => {
                let names: Vec<String> = edges
                    .iter()
                    .map(|e| {
                        let display = unresolved_display_for(e.to.as_str());
                        let mark =
                            nodes.get(e.to.as_str()).map(|n| n.kind).map_or("", |k| match k {
                                Kind::External => " [ext]",
                                Kind::Unresolved => " [?]",
                                Kind::Internal => "",
                            });
                        format!("{display}{mark}")
                    })
                    .collect();
                if names.is_empty() {
                    continue;
                }
                let heading = &names[0];
                parts.push(format!("{heading} [union: {}]", names.join(" | ")));
            }
        }
    }
    parts.join(", ")
}

/// Unresolved qnames embed provenance that's noisy in a rendered view
/// (`<unresolved>:getattr(obj, name)()@/tmp/.../dyn.py:1`). Collapse to the
/// callee token only when the prefix matches; otherwise pass through.
fn unresolved_display_for(qname: &str) -> String {
    if let Some(rest) = qname.strip_prefix("<unresolved>:") {
        if let Some((callee, _)) = rest.rsplit_once('@') {
            return callee.to_owned();
        }
        return rest.to_owned();
    }
    qname.to_owned()
}

/// Kahn topological sort. On cycles, leftover nodes are emitted in
/// score-descending order and a warning is logged with the count.
fn topo_order(snapshot: &Snapshot, nodes: &HashMap<&str, &Node>) -> Vec<String> {
    let mut indegree: HashMap<&str, usize> = nodes.keys().map(|k| (*k, 0_usize)).collect();
    let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();

    for edge in &snapshot.edges {
        let (Some(from), Some(to)) = (nodes.get(edge.from.as_str()), nodes.get(edge.to.as_str()))
        else {
            continue;
        };
        if from.qname == to.qname {
            // Self-recursion is its own trivial cycle — ignore for topo.
            continue;
        }
        adj.entry(edge.from.as_str()).or_default().push(edge.to.as_str());
        *indegree.entry(edge.to.as_str()).or_insert(0) += 1;
    }

    let mut seed: Vec<&str> =
        indegree.iter().filter_map(|(k, d)| if *d == 0 { Some(*k) } else { None }).collect();
    seed.sort_by(|a, b| qname_cmp(snapshot, a, b));
    let mut queue: VecDeque<&str> = seed.into_iter().collect();

    let mut order: Vec<String> = Vec::with_capacity(nodes.len());
    let mut emitted: HashSet<String> = HashSet::new();
    while let Some(q) = queue.pop_front() {
        if !emitted.insert(q.to_owned()) {
            continue;
        }
        order.push(q.to_owned());
        if let Some(next) = adj.get(q) {
            let mut newly_ready: Vec<&str> = Vec::new();
            for &target in next {
                let Some(deg) = indegree.get_mut(target) else {
                    continue;
                };
                *deg = deg.saturating_sub(1);
                if *deg == 0 && !emitted.contains(target) {
                    newly_ready.push(target);
                }
            }
            newly_ready.sort_by(|a, b| qname_cmp(snapshot, a, b));
            for next_q in newly_ready {
                queue.push_back(next_q);
            }
        }
    }

    let mut leftover: Vec<&str> = nodes.keys().copied().filter(|k| !emitted.contains(*k)).collect();
    if !leftover.is_empty() {
        tracing::warn!(
            "render: {} node(s) left after topo sort (cycle); emitting by score",
            leftover.len()
        );
        leftover.sort_by(|a, b| qname_cmp(snapshot, a, b));
        for q in leftover {
            if emitted.insert(q.to_owned()) {
                order.push(q.to_owned());
            }
        }
    }

    order
}

/// Comparator: entry points first (infinite score), then higher score first,
/// ties by qname for deterministic output.
fn qname_cmp(snapshot: &Snapshot, a: &str, b: &str) -> std::cmp::Ordering {
    let score_a = snapshot.scores.get(a).copied().unwrap_or(f64::NEG_INFINITY);
    let score_b = snapshot.scores.get(b).copied().unwrap_or(f64::NEG_INFINITY);
    score_b.partial_cmp(&score_a).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.cmp(b))
}

fn is_entry_point(snapshot: &Snapshot, qname: &str) -> bool {
    snapshot.depth_by_qname.get(qname).copied() == Some(0)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::model::{Annotation, Edge, Kind, Node};
    use crate::rank;
    use crate::walker::Snapshot;

    fn render_to_string(snapshot: &Snapshot) -> String {
        let mut buf = Vec::new();
        render(snapshot, &mut buf).expect("render");
        String::from_utf8(buf).expect("utf8")
    }

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

    fn external(qname: &str) -> Node {
        Node {
            qname: qname.to_owned(),
            signature: String::new(),
            doc: String::new(),
            file: String::new(),
            line: 0,
            kind: Kind::External,
        }
    }

    fn edge(from: &str, to: &str, ann: Annotation) -> Edge {
        Edge { from: from.to_owned(), to: to.to_owned(), annotation: ann }
    }

    #[test]
    fn topological_order_main_before_callees() {
        let mut snap = Snapshot {
            nodes: vec![internal("main"), internal("helper")],
            edges: vec![edge("main", "helper", Annotation::Resolved)],
            depth_by_qname: HashMap::from([("main".to_owned(), 0), ("helper".to_owned(), 1)]),
            ..Snapshot::default()
        };
        rank::rank(&mut snap);

        let text = render_to_string(&snap);
        let main_pos = text.find("main").expect("main present");
        let helper_pos = text.find("helper").expect("helper present");
        assert!(main_pos < helper_pos, "main must come before helper: {text}");
        assert!(text.contains("(entry)"), "main must carry (entry) marker");
    }

    #[test]
    fn diamond_emitted_once() {
        let mut snap = Snapshot {
            nodes: vec![internal("main"), internal("a"), internal("b"), internal("d")],
            edges: vec![
                edge("main", "a", Annotation::Resolved),
                edge("main", "b", Annotation::Resolved),
                edge("a", "d", Annotation::Resolved),
                edge("b", "d", Annotation::Resolved),
            ],
            depth_by_qname: HashMap::from([
                ("main".to_owned(), 0),
                ("a".to_owned(), 1),
                ("b".to_owned(), 1),
                ("d".to_owned(), 2),
            ]),
            ..Snapshot::default()
        };
        rank::rank(&mut snap);
        let text = render_to_string(&snap);
        let count = text.lines().filter(|l| l.trim() == "d").count();
        assert_eq!(count, 1, "diamond node d must be rendered exactly once: {text}");
    }

    #[test]
    fn external_callee_inline_ext_marker() {
        let mut snap = Snapshot {
            nodes: vec![internal("main"), external("os.environ.get")],
            edges: vec![edge("main", "os.environ.get", Annotation::External)],
            depth_by_qname: HashMap::from([("main".to_owned(), 0)]),
            ..Snapshot::default()
        };
        rank::rank(&mut snap);
        let text = render_to_string(&snap);
        assert!(text.contains("os.environ.get [ext]"), "ext marker missing: {text}");
        for line in text.lines() {
            assert!(line != "os.environ.get", "external leaf appeared standalone: {text}");
        }
    }

    #[test]
    fn unresolved_callee_inline_qmark() {
        let unresolved_qname = "<unresolved>:getattr(obj, name)()@/tmp/dyn.py:1";
        let mut snap = Snapshot {
            nodes: vec![
                internal("main"),
                Node {
                    qname: unresolved_qname.to_owned(),
                    signature: String::new(),
                    doc: String::new(),
                    file: String::new(),
                    line: 0,
                    kind: Kind::Unresolved,
                },
            ],
            edges: vec![edge("main", unresolved_qname, Annotation::Unresolved)],
            depth_by_qname: HashMap::from([("main".to_owned(), 0)]),
            ..Snapshot::default()
        };
        rank::rank(&mut snap);
        let text = render_to_string(&snap);
        assert!(text.contains("getattr(obj, name)() [?]"), "unresolved [?] marker missing: {text}");
    }

    #[test]
    fn union_edges_inline_pipe_bracket() {
        let mut snap = Snapshot {
            nodes: vec![internal("main"), internal("impl_a"), internal("impl_b")],
            edges: vec![
                edge("main", "impl_a", Annotation::Union),
                edge("main", "impl_b", Annotation::Union),
            ],
            depth_by_qname: HashMap::from([
                ("main".to_owned(), 0),
                ("impl_a".to_owned(), 1),
                ("impl_b".to_owned(), 1),
            ]),
            ..Snapshot::default()
        };
        rank::rank(&mut snap);
        let text = render_to_string(&snap);
        assert!(text.contains("[union:"), "union label missing: {text}");
        assert!(
            text.contains("impl_a | impl_b") || text.contains("impl_b | impl_a"),
            "union candidates missing: {text}"
        );
    }

    #[test]
    fn cycle_breaks_gracefully() {
        let mut snap = Snapshot {
            nodes: vec![internal("a"), internal("b")],
            edges: vec![edge("a", "b", Annotation::Resolved), edge("b", "a", Annotation::Resolved)],
            depth_by_qname: HashMap::from([("a".to_owned(), 0), ("b".to_owned(), 1)]),
            ..Snapshot::default()
        };
        rank::rank(&mut snap);
        let text = render_to_string(&snap);
        assert!(text.contains("\na\n") || text.starts_with('a'), "a missing in: {text}");
        assert!(text.contains("\nb"), "b missing in: {text}");
    }

    #[test]
    fn truncation_hides_metadata() {
        let mut snap = Snapshot {
            nodes: vec![Node {
                qname: "main".to_owned(),
                signature: "def main() -> str".to_owned(),
                doc: "entry doc".to_owned(),
                file: "m.py".to_owned(),
                line: 1,
                kind: Kind::Internal,
            }],
            edges: vec![],
            depth_by_qname: HashMap::from([("main".to_owned(), 0)]),
            truncation: Some(crate::walker::Truncation {
                dropped_nodes: 3,
                original_node_count: 4,
            }),
            ..Snapshot::default()
        };
        rank::rank(&mut snap);
        let text = render_to_string(&snap);
        assert!(!text.contains("def main"), "signature must be hidden under truncation: {text}");
        assert!(!text.contains("entry doc"), "doc must be hidden under truncation: {text}");
    }
}
