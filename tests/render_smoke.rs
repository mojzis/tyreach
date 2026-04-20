//! Rendered-view smoke tests using a synthetic `Snapshot` — no ty needed.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    reason = "integration-test helpers; failures should fail loudly"
)]

use std::collections::HashMap;

use tyreach::model::{Annotation, Edge, Kind, Node};
use tyreach::rank;
use tyreach::render::render;
use tyreach::walker::Snapshot;

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

fn render_str(snapshot: &Snapshot) -> String {
    let mut buf = Vec::new();
    render(snapshot, &mut buf).expect("render");
    String::from_utf8(buf).expect("utf8")
}

#[test]
fn topological_order_places_caller_before_callees_with_markers() {
    // main -> helper (internal), main -> os.environ.get (external),
    // main -> <unresolved>:getattr(...)@file:1 (unresolved).
    // main -> a, main -> b (union)
    let unresolved_qname = "<unresolved>:getattr(obj, n)()@/tmp/dyn.py:3";
    let mut snap = Snapshot {
        nodes: vec![
            internal("main"),
            internal("helper"),
            internal("impl_a"),
            internal("impl_b"),
            external("os.environ.get"),
            Node {
                qname: unresolved_qname.to_owned(),
                signature: String::new(),
                doc: String::new(),
                file: String::new(),
                line: 0,
                kind: Kind::Unresolved,
            },
        ],
        edges: vec![
            edge("main", "helper", Annotation::Resolved),
            edge("main", "os.environ.get", Annotation::External),
            edge("main", unresolved_qname, Annotation::Unresolved),
            edge("main", "impl_a", Annotation::Union),
            edge("main", "impl_b", Annotation::Union),
        ],
        depth_by_qname: HashMap::from([
            ("main".to_owned(), 0),
            ("helper".to_owned(), 1),
            ("impl_a".to_owned(), 1),
            ("impl_b".to_owned(), 1),
        ]),
        ..Snapshot::default()
    };
    rank::rank(&mut snap);

    let text = render_str(&snap);
    // Entry point appears first.
    assert!(text.starts_with("main"), "main must be first line: {text}");
    assert!(text.contains("(entry)"), "entry marker missing: {text}");

    // Ordering: main line is above the helper line.
    let main_pos = text.find("main").expect("main");
    let helper_pos = text.find("\nhelper").expect("helper line");
    assert!(main_pos < helper_pos, "main must precede helper: {text}");

    // Inline markers.
    assert!(text.contains("os.environ.get [ext]"), "[ext] marker missing: {text}");
    assert!(text.contains("getattr(obj, n)() [?]"), "[?] marker missing: {text}");
    assert!(text.contains("[union:"), "[union: ...] marker missing: {text}");

    // External leaf must NOT appear as its own top-level line.
    for line in text.lines() {
        assert!(line != "os.environ.get", "external leaf appeared standalone: {text}");
    }
}

#[test]
fn diamond_node_rendered_exactly_once() {
    // main -> a -> d, main -> b -> d. `d` must appear once.
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
    let text = render_str(&snap);
    let count = text.lines().filter(|l| l.trim() == "d").count();
    assert_eq!(count, 1, "diamond node d must be rendered once: {text}");
}
