//! Roundtrip: synthetic Snapshot -> TOON -> parsed-back Snapshot. Render both;
//! the rendered outputs must be byte-identical.

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
    render(snap, &mut buf).expect("render");
    String::from_utf8(buf).expect("utf8")
}

#[test]
fn toon_roundtrip_preserves_rendered_view() {
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
    let mut buf = Vec::new();
    write_snapshot_toon(&original.nodes, &original.edges, &mut buf).expect("write");
    let text = String::from_utf8(buf).expect("utf8");
    let (nodes, edges) = read_snapshot_toon(&text).expect("read");

    assert_eq!(nodes, original.nodes, "nodes must round-trip");
    assert_eq!(edges, original.edges, "edges must round-trip");

    // Rebuild a Snapshot from the on-disk table (scores/depths lost — that's by design).
    // Render both.
    let reloaded = Snapshot { nodes, edges, ..Snapshot::default() };
    // Strip scores/depths from the original too so we compare apples-to-apples.
    let stripped = Snapshot {
        nodes: original.nodes.clone(),
        edges: original.edges.clone(),
        ..Snapshot::default()
    };

    assert_eq!(
        render_str(&reloaded),
        render_str(&stripped),
        "rendered view must be identical after TOON round-trip"
    );
}
