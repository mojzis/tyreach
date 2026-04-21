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
    render_str_with(snapshot, false)
}

fn render_str_with(snapshot: &Snapshot, with_builtins: bool) -> String {
    let mut buf = Vec::new();
    render(snapshot, &mut buf, with_builtins).expect("render");
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
fn union_dedups_duplicate_targets_order_preserving() {
    // `main` has one union call site whose edges name `impl_a` three times
    // plus `impl_b` once. Rendered union must list each target exactly once,
    // first-occurrence-wins, so: `impl_a | impl_b`.
    let mut snap = Snapshot {
        nodes: vec![internal("main"), internal("impl_a"), internal("impl_b")],
        edges: vec![
            edge("main", "impl_a", Annotation::Union),
            edge("main", "impl_a", Annotation::Union),
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
    let text = render_str(&snap);

    // Find the union suffix and count occurrences of `impl_a` inside it.
    let union_start = text.find("[union:").expect("union marker present");
    let union_end = text[union_start..].find(']').expect("union marker closed");
    let union_segment = &text[union_start..union_start + union_end];
    let impl_a_count = union_segment.matches("impl_a").count();
    assert_eq!(impl_a_count, 1, "impl_a should appear once inside union: {union_segment}");
    assert!(union_segment.contains("impl_b"), "impl_b must remain in union: {union_segment}");
    // Order-preserving: impl_a before impl_b.
    let a_pos = union_segment.find("impl_a").unwrap();
    let b_pos = union_segment.find("impl_b").unwrap();
    assert!(a_pos < b_pos, "first-occurrence-wins ordering broken: {union_segment}");
}

#[test]
fn noisy_builtin_filtered_by_default() {
    // `builtins.print` must not leak into rendered output when with_builtins=false.
    let mut snap = Snapshot {
        nodes: vec![internal("main"), external("builtins.print"), internal("helper")],
        edges: vec![
            edge("main", "builtins.print", Annotation::External),
            edge("main", "helper", Annotation::Resolved),
        ],
        depth_by_qname: HashMap::from([("main".to_owned(), 0), ("helper".to_owned(), 1)]),
        ..Snapshot::default()
    };
    rank::rank(&mut snap);
    let text = render_str(&snap);
    assert!(!text.contains("print"), "builtins.print must be filtered by default: {text}");
    assert!(text.contains("helper"), "non-builtin edges must still render: {text}");
}

#[test]
fn noisy_builtin_surfaced_with_opt_in() {
    // Same snapshot as above but with_builtins=true — print must appear.
    let mut snap = Snapshot {
        nodes: vec![internal("main"), external("builtins.print"), internal("helper")],
        edges: vec![
            edge("main", "builtins.print", Annotation::External),
            edge("main", "helper", Annotation::Resolved),
        ],
        depth_by_qname: HashMap::from([("main".to_owned(), 0), ("helper".to_owned(), 1)]),
        ..Snapshot::default()
    };
    rank::rank(&mut snap);
    let text = render_str_with(&snap, true);
    assert!(text.contains("print"), "--with-builtins must surface builtins.print: {text}");
}

#[test]
fn function_with_only_filtered_builtin_renders_header_no_arrow() {
    // `main` calls only `builtins.print`. With default filtering the
    // function header must still appear but no `->` line.
    let mut snap = Snapshot {
        nodes: vec![internal("main"), external("builtins.print")],
        edges: vec![edge("main", "builtins.print", Annotation::External)],
        depth_by_qname: HashMap::from([("main".to_owned(), 0)]),
        ..Snapshot::default()
    };
    rank::rank(&mut snap);
    let text = render_str(&snap);
    assert!(text.contains("main"), "main header must be present: {text}");
    assert!(!text.contains("->"), "no arrow line when all edges filtered: {text}");
    assert!(!text.contains("print"), "builtins.print must be filtered: {text}");
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
