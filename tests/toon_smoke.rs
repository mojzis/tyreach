//! TOON round-trip smoke test. Go/no-go for toon-format 0.4.5 in Phase 1.

use tyreach::model::{Annotation, Edge, Kind, Node};
use tyreach::toon_io::{read_snapshot_toon, write_snapshot_toon};

#[test]
fn round_trips_realistic_snapshot() {
    let nodes = vec![
        Node {
            qname: "tiny_app.main.main".to_string(),
            signature: "def main() -> str".to_string(),
            doc: "Return the environment value or a fallback.".to_string(),
            file: "tiny_app/main.py".to_string(),
            line: 6,
            kind: Kind::Internal,
        },
        Node {
            qname: "os.environ.get".to_string(),
            signature: String::new(),
            doc: String::new(),
            file: String::new(),
            line: 0,
            kind: Kind::External,
        },
    ];
    let edges = vec![
        Edge {
            from: "tiny_app.main.main".to_string(),
            to: "tiny_app.lib.foo".to_string(),
            annotation: Annotation::Resolved,
        },
        Edge {
            from: "tiny_app.main.main".to_string(),
            to: "os.environ.get".to_string(),
            annotation: Annotation::External,
        },
    ];

    let entries = vec!["tiny_app.main.main".to_string()];

    let mut buf = Vec::new();
    write_snapshot_toon(&nodes, &edges, &entries, &mut buf).expect("encode");
    let encoded = String::from_utf8(buf).expect("utf8");
    assert!(!encoded.is_empty(), "encoder produced an empty document");

    let (nodes_out, edges_out, entries_out) = read_snapshot_toon(&encoded).expect("decode");
    assert_eq!(nodes_out, nodes, "nodes must round-trip");
    assert_eq!(edges_out, edges, "edges must round-trip");
    assert_eq!(entries_out, entries, "entries must round-trip");
}
