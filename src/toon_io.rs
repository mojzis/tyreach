//! TOON I/O for snapshot tables.
//!
//! Thin wrapper over `toon-format` 0.4.5. Phase 1 uses this only for the
//! round-trip smoke test that decides go/no-go on the crate. If the smoke
//! test fails we keep the same signatures and hand-roll the writer in
//! Phase 3.

use std::io::Write;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::model::{Edge, Node};

/// Wrapper so the whole snapshot round-trips as a single document.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct Snapshot {
    nodes: Vec<Node>,
    edges: Vec<Edge>,
}

/// Encode `nodes + edges` to TOON and write the result to `out`.
pub fn write_snapshot_toon(nodes: &[Node], edges: &[Edge], out: &mut impl Write) -> Result<()> {
    let snapshot = Snapshot { nodes: nodes.to_vec(), edges: edges.to_vec() };
    let encoded =
        toon_format::encode_default(&snapshot).context("encode snapshot to TOON failed")?;
    out.write_all(encoded.as_bytes()).context("write TOON snapshot failed")?;
    Ok(())
}

/// Decode a TOON snapshot back into `(nodes, edges)`.
pub fn read_snapshot_toon(src: &str) -> Result<(Vec<Node>, Vec<Edge>)> {
    let snapshot: Snapshot =
        toon_format::decode_default(src).context("decode TOON snapshot failed")?;
    Ok((snapshot.nodes, snapshot.edges))
}
