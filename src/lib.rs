//! `tyreach` — ranked, token-budgeted reachability snapshot of Python symbols.
//!
//! Phase 2 exposes the BFS `snapshot` pipeline: call-site extraction
//! (tree-sitter) composed with symbol resolution (ty LSP) produces a flat
//! `(nodes, edges)` table starting from one or more entry points.

use std::path::Path;

use anyhow::{Context, Result};

pub mod budget;
pub mod classify;
pub mod entry;
pub mod extract;
pub mod lsp;
pub mod model;
pub mod parse;
pub mod qname;
pub mod rank;
pub mod render;
pub mod toon_io;
pub mod walker;
pub mod workspace;

/// Walk the call graph from each of `entries`, sharing a single LSP client and
/// walker state (visited set, parse cache).
pub async fn snapshot(
    repo_root: &Path,
    entries: Vec<entry::EntryPoint>,
) -> Result<walker::Snapshot> {
    let canonical = repo_root
        .canonicalize()
        .with_context(|| format!("canonicalize repo_root {}", repo_root.display()))?;
    let workspace = canonical.to_string_lossy().into_owned();

    let client = lsp::client::TyLspClient::new(&workspace).await.context("start ty LSP client")?;

    let mut walker = walker::Walker::new(&client, &canonical)?;
    for entry in entries {
        walker.walk(entry).await?;
    }

    Ok(walker.into_snapshot())
}
