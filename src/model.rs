//! Core data model for tyreach snapshots.
//!
//! `Node`s and `Edge`s form the flat reachability table. Mirrors
//! `docs/plans/plan.md` exactly — downstream TOON encoding and rendering
//! depend on field names staying stable.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Internal,
    External,
    Unresolved,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Annotation {
    Resolved,
    External,
    Union,
    Unresolved,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Node {
    pub qname: String,
    /// Rendered signature (empty for externals/unresolved).
    pub signature: String,
    /// First line of the docstring, empty otherwise.
    pub doc: String,
    /// Path relative to repo root, empty for externals.
    pub file: String,
    /// 1-based line number, 0 for externals.
    pub line: u32,
    pub kind: Kind,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Edge {
    pub from: String,
    pub to: String,
    pub annotation: Annotation,
}
