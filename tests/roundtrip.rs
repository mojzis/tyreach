//! Roundtrip: synthetic Snapshot -> TOON -> parsed-back Snapshot.
//!
//! Two guarantees are checked here:
//!
//! 1. **Rendered parity** — render(original) == render(reloaded). Entry-point
//!    `(entry)` markers must survive, which is why the canonical TOON carries
//!    an `[entries]` list.
//! 2. **Bit-identical TOON round-trip** — writing the reloaded snapshot back
//!    out produces exactly the bytes it was read from (modulo trailing-
//!    whitespace normalization). This is the anti-regression for the
//!    canonical-completeness bug: if `[entries]` were lost on read, the
//!    second write would silently differ.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    reason = "integration-test helpers; failures should fail loudly"
)]

use std::collections::{HashMap, VecDeque};

use tyreach::model::{Annotation, Edge, Kind, Node};
use tyreach::rank;
use tyreach::render::render;
use tyreach::toon_io::{read_snapshot_toon, write_snapshot_toon};
use tyreach::walker::Snapshot;

fn internal(qname: &str, sig: &str, doc: &str) -> Node {
    Node {
        qname: qname.to_owned(),
        signature: sig.to_owned(),
        doc: doc.to_owned(),
        file: "demo.py".to_owned(),
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

fn render_str(snap: &Snapshot) -> String {
    let mut buf = Vec::new();
    render(snap, &mut buf, false).expect("render");
    String::from_utf8(buf).expect("utf8")
}

/// Strip trailing whitespace from each line — the only safe normalization
/// for a bit-identical round-trip assertion (toon-format may emit lines
/// without trailing spaces; our writers preserve content as-is).
fn strip_trailing_ws(s: &str) -> String {
    let mut out = String::new();
    for line in s.lines() {
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

/// BFS from entries over edges to rebuild `depth_by_qname`. Mirrors the
/// reconstruction `tyreach render` performs when re-rendering a TOON file.
fn reconstruct_depths(entries: &[String], edges: &[Edge]) -> HashMap<String, u32> {
    let mut depth: HashMap<String, u32> = HashMap::new();
    let mut queue: VecDeque<String> = VecDeque::new();
    for q in entries {
        depth.insert(q.clone(), 0);
        queue.push_back(q.clone());
    }
    while let Some(q) = queue.pop_front() {
        let next = depth.get(&q).copied().unwrap_or(0).saturating_add(1);
        for e in edges.iter().filter(|e| e.from == q) {
            if !depth.contains_key(&e.to) {
                depth.insert(e.to.clone(), next);
                queue.push_back(e.to.clone());
            }
        }
    }
    depth
}

fn entry_qnames(snap: &Snapshot) -> Vec<String> {
    let mut v: Vec<String> = snap
        .depth_by_qname
        .iter()
        .filter_map(|(q, d)| if *d == 0 { Some(q.clone()) } else { None })
        .collect();
    v.sort();
    v
}

#[test]
fn toon_roundtrip_preserves_rendered_view_including_entry_markers() {
    let mut original = Snapshot {
        nodes: vec![
            internal("app.main.main", "def main() -> str", "entry"),
            internal("app.lib.foo", "def foo() -> int", ""),
            external("os.environ.get"),
        ],
        edges: vec![
            edge("app.main.main", "app.lib.foo", Annotation::Resolved),
            edge("app.main.main", "os.environ.get", Annotation::External),
        ],
        depth_by_qname: HashMap::from([
            ("app.main.main".to_owned(), 0),
            ("app.lib.foo".to_owned(), 1),
        ]),
        ..Snapshot::default()
    };
    rank::rank(&mut original);

    // Write to TOON, read back.
    let entries = entry_qnames(&original);
    let mut buf = Vec::new();
    write_snapshot_toon(&original.nodes, &original.edges, &entries, &mut buf).expect("write");
    let text_first = String::from_utf8(buf).expect("utf8");
    let (nodes, edges, entries_back) = read_snapshot_toon(&text_first).expect("read");

    assert_eq!(nodes, original.nodes, "nodes must round-trip");
    assert_eq!(edges, original.edges, "edges must round-trip");
    assert_eq!(entries_back, entries, "entries must round-trip");

    // Rebuild a Snapshot from the on-disk data. `depth_by_qname` is
    // reconstructed *only* for entry points (depth 0) — that's the minimum
    // the renderer needs to emit `(entry)` markers.
    let depth_by_qname = entries_back.iter().map(|q| (q.clone(), 0_u32)).collect();
    let reloaded = Snapshot { nodes, edges, depth_by_qname, ..Snapshot::default() };

    // Strip non-canonical state from the original too so we compare
    // apples-to-apples (scores/truncation aren't serialized, but depth-0
    // entries now are).
    let stripped_depths =
        entry_qnames(&original).into_iter().map(|q| (q, 0_u32)).collect::<HashMap<_, _>>();
    let stripped = Snapshot {
        nodes: original.nodes.clone(),
        edges: original.edges.clone(),
        depth_by_qname: stripped_depths,
        ..Snapshot::default()
    };

    let rendered_reloaded = render_str(&reloaded);
    let rendered_stripped = render_str(&stripped);
    assert_eq!(
        rendered_reloaded, rendered_stripped,
        "rendered view must be identical after TOON round-trip"
    );
    assert!(
        rendered_reloaded.contains("(entry)"),
        "entry marker must survive the round-trip: {rendered_reloaded}"
    );

    // Bit-identical TOON round-trip: re-writing the reloaded snapshot must
    // reproduce the original TOON bytes (modulo trailing-whitespace
    // normalization). This guards against silent loss of canonical fields.
    let mut buf2 = Vec::new();
    write_snapshot_toon(&reloaded.nodes, &reloaded.edges, &entries_back, &mut buf2)
        .expect("write#2");
    let text_second = String::from_utf8(buf2).expect("utf8");
    assert_eq!(
        strip_trailing_ws(&text_first),
        strip_trailing_ws(&text_second),
        "TOON must be bit-identical after a read+write round-trip"
    );
}

/// Rank-aware roundtrip: `snapshot -> rank -> render` must equal
/// `snapshot -> write -> read -> reconstruct-depths -> rank -> render`.
///
/// This covers the case the trivial roundtrip test misses: when non-entry
/// nodes have meaningful depth (and therefore meaningful rank scores), the
/// re-renderer must reconstruct depth via BFS so the topo-sort tie-breaks
/// line up with what `tyreach snapshot` wrote. Without depth reconstruction,
/// every non-entry falls to `f64::NEG_INFINITY` and the render reorders.
#[test]
fn rank_aware_roundtrip_preserves_render_order() {
    // Layout:
    //   root -> (a, b)
    //   a    -> shared      (depth 2)
    //   b    -> shared      (depth 2)
    //   root -> deep        (depth 1)
    //   deep -> deeper      (depth 2)
    //   deeper -> deepest   (depth 3)
    //
    // `shared` has fan-in 2 so it outscores its depth-2 peer `deeper`. A
    // naive re-render without depth reconstruction treats every non-entry as
    // NEG_INFINITY and sorts alphabetically, reversing the documented order.
    let nodes = vec![
        internal("app.root", "def root()", ""),
        internal("app.a", "def a()", ""),
        internal("app.b", "def b()", ""),
        internal("app.shared", "def shared()", ""),
        internal("app.deep", "def deep()", ""),
        internal("app.deeper", "def deeper()", ""),
        internal("app.deepest", "def deepest()", ""),
    ];
    let edges = vec![
        edge("app.root", "app.a", Annotation::Resolved),
        edge("app.root", "app.b", Annotation::Resolved),
        edge("app.root", "app.deep", Annotation::Resolved),
        edge("app.a", "app.shared", Annotation::Resolved),
        edge("app.b", "app.shared", Annotation::Resolved),
        edge("app.deep", "app.deeper", Annotation::Resolved),
        edge("app.deeper", "app.deepest", Annotation::Resolved),
    ];

    let depth_by_qname = HashMap::from([
        ("app.root".to_owned(), 0_u32),
        ("app.a".to_owned(), 1),
        ("app.b".to_owned(), 1),
        ("app.shared".to_owned(), 2),
        ("app.deep".to_owned(), 1),
        ("app.deeper".to_owned(), 2),
        ("app.deepest".to_owned(), 3),
    ]);
    let mut original = Snapshot { nodes, edges, depth_by_qname, ..Snapshot::default() };
    rank::rank(&mut original);
    let rendered_original = render_str(&original);

    // Serialize, read back, reconstruct depths, re-rank, render.
    let entries_written = entry_qnames(&original);
    let mut buf = Vec::new();
    write_snapshot_toon(&original.nodes, &original.edges, &entries_written, &mut buf)
        .expect("write");
    let text = String::from_utf8(buf).expect("utf8");
    let (nodes_back, edges_back, entries_back) = read_snapshot_toon(&text).expect("read");
    let depths_back = reconstruct_depths(&entries_back, &edges_back);
    let mut reloaded = Snapshot {
        nodes: nodes_back,
        edges: edges_back,
        depth_by_qname: depths_back,
        ..Snapshot::default()
    };
    rank::rank(&mut reloaded);
    let rendered_reloaded = render_str(&reloaded);

    assert_eq!(
        rendered_original, rendered_reloaded,
        "rank-aware re-render must match original render byte-for-byte\n--- original ---\n{rendered_original}\n--- reloaded ---\n{rendered_reloaded}"
    );
}
