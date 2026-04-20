//! `tyreach` — ranked, token-budgeted reachability snapshot of Python symbols.
//!
//! Library surface for the phase-1 scaffold. Modules here are the composition
//! points the snapshot command and later phases will build on.

pub mod extract;
pub mod lsp;
pub mod model;
pub mod parse;
pub mod toon_io;
pub mod workspace;
