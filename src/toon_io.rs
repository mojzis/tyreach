//! TOON I/O for snapshot tables.
//!
//! The on-disk layout is two TOON tables back-to-back (nodes, then edges),
//! followed by an `[entries]` metadata list, all preceded by a version
//! header comment. Blank lines separate the sections so `grep`/`awk` remain
//! happy and the file remains diffable.
//!
//! toon-format crate 0.4.5 encodes `Vec<T>` of `serde`-derive structs as a
//! tabular block directly; we encode each section separately and assemble the
//! document by hand so the blocks stay parseable in isolation.
//!
//! The `[entries]` block is a supplementary list of qnames that were
//! walker entry points (BFS depth 0). It lets `tyreach render` reproduce
//! the `(entry)` markers emitted by `tyreach snapshot` from the canonical
//! TOON alone. A missing `[entries]` block (older TOON files) is tolerated
//! and results in no entry markers.

use std::io::Write;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::model::{Edge, Node};

const HEADER: &str = "# tyreach snapshot v1";
const NODES_MARKER: &str = "# nodes";
const EDGES_MARKER: &str = "# edges";
const ENTRIES_MARKER: &str = "# entries";

/// Wrappers so each section round-trips as a single top-level map (toon-format
/// requires a named field at the document root).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct NodesDoc {
    nodes: Vec<Node>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct EdgesDoc {
    edges: Vec<Edge>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct EntriesDoc {
    entries: Vec<String>,
}

/// Write nodes + edges + entries as three TOON sections back-to-back,
/// version-headered.
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
///
/// # entries
/// entries[K]: qname1,qname2
/// ```
///
/// `entries` is the list of walker entry-point qnames — consumed by
/// `render::render` to emit `(entry)` markers. Sorted for determinism.
pub fn write_snapshot_toon(
    nodes: &[Node],
    edges: &[Edge],
    entries: &[String],
    out: &mut impl Write,
) -> Result<()> {
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

    writeln!(out).context("blank separator")?;
    writeln!(out, "{ENTRIES_MARKER}").context("write entries marker")?;

    let mut sorted_entries = entries.to_vec();
    sorted_entries.sort();
    let entries_doc = EntriesDoc { entries: sorted_entries };
    let encoded_entries = toon_format::encode_default(&entries_doc).context("encode entries")?;
    out.write_all(encoded_entries.as_bytes()).context("write entries block")?;
    if !encoded_entries.ends_with('\n') {
        writeln!(out).context("trailing newline")?;
    }

    Ok(())
}

/// Parse a snapshot document back into `(nodes, edges, entries)`.
///
/// The `entries` block is optional: older TOON files without it parse fine
/// and yield an empty entries list (so `render` will emit no entry markers).
pub fn read_snapshot_toon(src: &str) -> Result<(Vec<Node>, Vec<Edge>, Vec<String>)> {
    let nodes_block = extract_block(src, NODES_MARKER, &[EDGES_MARKER, ENTRIES_MARKER])
        .context("nodes block missing from snapshot")?;
    let edges_block = extract_block(src, EDGES_MARKER, &[ENTRIES_MARKER])
        .context("edges block missing from snapshot")?;

    let nodes_doc: NodesDoc =
        toon_format::decode_default(&nodes_block).context("decode nodes block")?;
    let edges_doc: EdgesDoc =
        toon_format::decode_default(&edges_block).context("decode edges block")?;

    let entries = match extract_block(src, ENTRIES_MARKER, &[]) {
        Ok(block) if block.trim().is_empty() => Vec::new(),
        Ok(block) => {
            let entries_doc: EntriesDoc =
                toon_format::decode_default(&block).context("decode entries block")?;
            entries_doc.entries
        }
        // Missing entries block is allowed (backward-compat with older TOON).
        Err(_) => Vec::new(),
    };

    Ok((nodes_doc.nodes, edges_doc.edges, entries))
}

/// Cut out the substring between a `# start` marker line and the first of
/// any `end_markers` that follows, or EOF if none matches.
fn extract_block(src: &str, start_marker: &str, end_markers: &[&str]) -> Result<String> {
    // Collect once and iterate twice — single pass over `src.lines()` is
    // enough.
    let lines: Vec<&str> = src.lines().collect();
    let Some(start) = lines.iter().position(|l| l.trim() == start_marker) else {
        bail!("start marker {start_marker:?} not found");
    };
    let content_start = start + 1;
    let end = lines[content_start..]
        .iter()
        .position(|l| end_markers.contains(&l.trim()))
        .map_or(lines.len(), |off| content_start + off);

    Ok(lines[content_start..end].join("\n"))
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
    fn round_trips_three_sections() {
        let nodes = sample_nodes();
        let edges = sample_edges();
        let entries = vec!["app.main.main".to_owned()];

        let mut buf = Vec::new();
        write_snapshot_toon(&nodes, &edges, &entries, &mut buf).expect("write");
        let text = String::from_utf8(buf).expect("utf8");

        assert!(text.starts_with(HEADER), "must start with version header");
        assert!(text.contains(NODES_MARKER));
        assert!(text.contains(EDGES_MARKER));
        assert!(text.contains(ENTRIES_MARKER));
        assert!(text.contains("qname"));
        assert!(text.contains("from"));

        let (n2, e2, ents2) = read_snapshot_toon(&text).expect("read");
        assert_eq!(n2, nodes);
        assert_eq!(e2, edges);
        assert_eq!(ents2, entries);
    }

    #[test]
    fn round_trips_empty_sections() {
        let mut buf = Vec::new();
        write_snapshot_toon(&[], &[], &[], &mut buf).expect("write");
        let text = String::from_utf8(buf).expect("utf8");
        let (n2, e2, ents2) = read_snapshot_toon(&text).expect("read");
        assert!(n2.is_empty());
        assert!(e2.is_empty());
        assert!(ents2.is_empty());
    }

    #[test]
    fn reads_legacy_toon_without_entries_block() {
        // TOON written by an older tyreach (no `# entries` section) must still
        // parse — renders just won't emit any `(entry)` marker. We generate
        // the legacy shape by writing with an empty entries list, then
        // stripping the `# entries` section from the resulting text.
        let nodes = sample_nodes();
        let edges = sample_edges();
        let mut buf = Vec::new();
        write_snapshot_toon(&nodes, &edges, &[], &mut buf).expect("write");
        let full = String::from_utf8(buf).expect("utf8");
        let legacy = full
            .split_once(ENTRIES_MARKER)
            .map_or_else(|| full.clone(), |(before, _)| before.trim_end().to_owned() + "\n");
        assert!(!legacy.contains(ENTRIES_MARKER), "precondition: no entries marker");

        let (n, e, ents) = read_snapshot_toon(&legacy).expect("read legacy");
        assert_eq!(n, nodes);
        assert_eq!(e, edges);
        assert!(ents.is_empty(), "legacy TOON must yield empty entries");
    }

    #[test]
    fn read_missing_markers_errors() {
        let bad = "this is not a tyreach snapshot\n";
        assert!(read_snapshot_toon(bad).is_err());
    }

    #[test]
    fn entries_written_sorted_for_determinism() {
        let nodes = sample_nodes();
        let edges = sample_edges();
        let entries = vec!["z.z".to_owned(), "a.a".to_owned(), "m.m".to_owned()];

        let mut buf = Vec::new();
        write_snapshot_toon(&nodes, &edges, &entries, &mut buf).expect("write");
        let text = String::from_utf8(buf).expect("utf8");
        let a_pos = text.find("a.a").expect("a.a present");
        let m_pos = text.find("m.m").expect("m.m present");
        let z_pos = text.find("z.z").expect("z.z present");
        assert!(a_pos < m_pos && m_pos < z_pos, "entries must be written sorted: {text}");
    }
}
