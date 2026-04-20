//! TOON I/O for snapshot tables.
//!
//! The on-disk layout is two TOON tables back-to-back (nodes, then edges),
//! preceded by a version header comment. A blank line separates the tables
//! so `grep`/`awk` remain happy and the file remains diffable.
//!
//! toon-format crate 0.4.5 encodes `Vec<T>` of `serde`-derive structs as a
//! tabular block directly; we encode each table separately and assemble the
//! document by hand so the two blocks stay parseable in isolation.

use std::io::Write;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::model::{Edge, Node};

const HEADER: &str = "# tyreach snapshot v1";
const NODES_MARKER: &str = "# nodes";
const EDGES_MARKER: &str = "# edges";

/// Wrappers so each table round-trips as a single top-level map (toon-format
/// requires a named field at the document root).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct NodesDoc {
    nodes: Vec<Node>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct EdgesDoc {
    edges: Vec<Edge>,
}

/// Write nodes + edges as two TOON tables back-to-back, version-headered.
///
/// Output shape:
///
/// ```text
/// # tyreach snapshot v1
/// # nodes
/// nodes[N]{qname,signature,doc,file,line,kind}:
///   ...
///
/// # edges
/// edges[M]{from,to,annotation}:
///   ...
/// ```
pub fn write_snapshot_toon(nodes: &[Node], edges: &[Edge], out: &mut impl Write) -> Result<()> {
    writeln!(out, "{HEADER}").context("write TOON header")?;
    writeln!(out, "{NODES_MARKER}").context("write nodes marker")?;

    let nodes_doc = NodesDoc { nodes: nodes.to_vec() };
    let encoded_nodes = toon_format::encode_default(&nodes_doc).context("encode nodes")?;
    out.write_all(encoded_nodes.as_bytes()).context("write nodes block")?;
    if !encoded_nodes.ends_with('\n') {
        writeln!(out).context("trailing newline")?;
    }

    writeln!(out).context("blank separator")?;
    writeln!(out, "{EDGES_MARKER}").context("write edges marker")?;

    let edges_doc = EdgesDoc { edges: edges.to_vec() };
    let encoded_edges = toon_format::encode_default(&edges_doc).context("encode edges")?;
    out.write_all(encoded_edges.as_bytes()).context("write edges block")?;
    if !encoded_edges.ends_with('\n') {
        writeln!(out).context("trailing newline")?;
    }

    Ok(())
}

/// Parse a two-table snapshot document back into `(nodes, edges)`.
pub fn read_snapshot_toon(src: &str) -> Result<(Vec<Node>, Vec<Edge>)> {
    let nodes_block = extract_block(src, NODES_MARKER, Some(EDGES_MARKER))
        .context("nodes block missing from snapshot")?;
    let edges_block =
        extract_block(src, EDGES_MARKER, None).context("edges block missing from snapshot")?;

    let nodes_doc: NodesDoc =
        toon_format::decode_default(&nodes_block).context("decode nodes block")?;
    let edges_doc: EdgesDoc =
        toon_format::decode_default(&edges_block).context("decode edges block")?;

    Ok((nodes_doc.nodes, edges_doc.edges))
}

/// Cut out the substring between a `# start` marker line and either the `end`
/// marker or EOF.
fn extract_block(src: &str, start_marker: &str, end_marker: Option<&str>) -> Result<String> {
    let mut lines = src.lines().enumerate();
    let start = loop {
        let Some((idx, line)) = lines.next() else {
            bail!("start marker {start_marker:?} not found");
        };
        if line.trim() == start_marker {
            break idx + 1; // first line of content
        }
    };
    let end = if let Some(marker) = end_marker {
        let mut end_idx = src.lines().count();
        for (idx, line) in src.lines().enumerate().skip(start) {
            if line.trim() == marker {
                end_idx = idx;
                break;
            }
        }
        end_idx
    } else {
        src.lines().count()
    };

    let block: Vec<&str> = src.lines().skip(start).take(end.saturating_sub(start)).collect();
    Ok(block.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Annotation, Kind};

    fn sample_nodes() -> Vec<Node> {
        vec![
            Node {
                qname: "app.main.main".to_owned(),
                signature: "def main() -> str".to_owned(),
                doc: "entry".to_owned(),
                file: "app/main.py".to_owned(),
                line: 10,
                kind: Kind::Internal,
            },
            Node {
                qname: "os.environ.get".to_owned(),
                signature: String::new(),
                doc: String::new(),
                file: String::new(),
                line: 0,
                kind: Kind::External,
            },
        ]
    }

    fn sample_edges() -> Vec<Edge> {
        vec![Edge {
            from: "app.main.main".to_owned(),
            to: "os.environ.get".to_owned(),
            annotation: Annotation::External,
        }]
    }

    #[test]
    fn round_trips_two_tables() {
        let nodes = sample_nodes();
        let edges = sample_edges();

        let mut buf = Vec::new();
        write_snapshot_toon(&nodes, &edges, &mut buf).expect("write");
        let text = String::from_utf8(buf).expect("utf8");

        assert!(text.starts_with(HEADER), "must start with version header");
        assert!(text.contains(NODES_MARKER));
        assert!(text.contains(EDGES_MARKER));
        assert!(text.contains("qname"));
        assert!(text.contains("from"));

        let (n2, e2) = read_snapshot_toon(&text).expect("read");
        assert_eq!(n2, nodes);
        assert_eq!(e2, edges);
    }

    #[test]
    fn round_trips_empty_tables() {
        let mut buf = Vec::new();
        write_snapshot_toon(&[], &[], &mut buf).expect("write");
        let text = String::from_utf8(buf).expect("utf8");
        let (n2, e2) = read_snapshot_toon(&text).expect("read");
        assert!(n2.is_empty());
        assert!(e2.is_empty());
    }

    #[test]
    fn read_missing_markers_errors() {
        let bad = "this is not a tyreach snapshot\n";
        assert!(read_snapshot_toon(bad).is_err());
    }
}
