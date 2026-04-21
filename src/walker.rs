//! BFS call-graph walker.
//!
//! Composes tree-sitter call-site extraction with ty LSP goto-definition to
//! build a flat `(nodes, edges)` table starting from one or more entry points.
//! Externals become leaf nodes; unresolved call sites become synthetic
//! `<unresolved>` nodes. Cycles are broken by a `visited: HashSet<qname>`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::time::timeout;

use crate::classify::Classifier;
use crate::entry::EntryPoint;
use crate::extract::{extract_call_sites, CallSite};
use crate::lsp::client::TyLspClient;
use crate::lsp::protocol::{Hover, HoverContents, Location, MarkedStringOrString};
use crate::model::{Annotation, Edge, Kind, Node};
use crate::parse::{parse_file, ParsedFile};
use crate::qname::qname_for;

const GOTO_TIMEOUT: Duration = Duration::from_secs(5);

/// Per-snapshot truncation metadata emitted by `budget::fit_to_budget`.
///
/// When `Some`, the snapshot was shrunk to fit a token budget and the renderer
/// skips optional signature/doc lines.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct Truncation {
    pub dropped_nodes: u32,
    pub original_node_count: u32,
}

/// One BFS snapshot — flat nodes + edges plus per-qname depth for ranking,
/// optional per-qname scores (populated by `rank::rank`), and optional
/// truncation metadata (populated by `budget::fit_to_budget`).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Snapshot {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub depth_by_qname: HashMap<String, u32>,
    #[serde(default)]
    pub scores: HashMap<String, f64>,
    #[serde(default)]
    pub truncation: Option<Truncation>,
}

/// Pending item in the BFS queue: an internal function we need to expand.
struct WalkItem {
    qname: String,
    file: PathBuf,
    byte_range: Range<usize>,
    depth: u32,
}

/// Walker state. One walker handles one or more entry points, sharing the
/// LSP client, classifier, visited set, and parsed-file cache.
pub struct Walker<'a> {
    client: &'a TyLspClient,
    classifier: Classifier,
    repo_root: PathBuf,
    visited: HashSet<String>,
    node_qnames: HashSet<String>,
    queue: VecDeque<WalkItem>,
    snapshot: Snapshot,
    parsed_cache: HashMap<PathBuf, ParsedFile>,
}

impl<'a> Walker<'a> {
    pub fn new(client: &'a TyLspClient, repo_root: &Path) -> Result<Self> {
        let classifier = Classifier::new(repo_root)?;
        let repo_root = classifier.repo_root().to_path_buf();
        Ok(Self {
            client,
            classifier,
            repo_root,
            visited: HashSet::new(),
            node_qnames: HashSet::new(),
            queue: VecDeque::new(),
            snapshot: Snapshot::default(),
            parsed_cache: HashMap::new(),
        })
    }

    /// Consume the walker and return the accumulated snapshot.
    pub fn into_snapshot(self) -> Snapshot {
        self.snapshot
    }

    /// Walk the call graph from `entry`. Idempotent across multiple calls —
    /// the shared `visited` set prevents duplicate expansion.
    pub async fn walk(&mut self, entry: EntryPoint) -> Result<()> {
        tracing::info!("walking entry: {}", entry.name);

        let file = entry
            .file
            .canonicalize()
            .with_context(|| format!("canonicalize entry file {}", entry.file.display()))?;
        let qname = qname_for(&file, &self.repo_root, &entry.function);
        let Some(byte_range) = self.find_function_range(&file, &entry.function)? else {
            tracing::warn!(
                "walk: could not locate function {} in {}",
                entry.function,
                file.display()
            );
            return Ok(());
        };

        self.queue.push_back(WalkItem { qname, file, byte_range, depth: 0 });

        while let Some(item) = self.queue.pop_front() {
            if !self.visited.insert(item.qname.clone()) {
                continue;
            }
            self.snapshot.depth_by_qname.entry(item.qname.clone()).or_insert(item.depth);

            if let Err(err) = self.expand(&item).await {
                tracing::warn!("walk: expand {} failed: {err:?}", item.qname);
            }
        }

        Ok(())
    }

    async fn expand(&mut self, item: &WalkItem) -> Result<()> {
        let file_str = item.file.to_string_lossy().into_owned();
        let _ = self.client.open_document(&file_str).await;

        // Extract everything we need from the parsed tree in one scope so the
        // immutable borrow of parsed_cache is released before we mutate self.
        let (name_line, name_char, doc, fn_row, call_sites, has_fn) = {
            let parsed = self.get_parsed(&item.file)?;
            parsed
                .tree
                .root_node()
                .descendant_for_byte_range(item.byte_range.start, item.byte_range.end)
                .map_or_else(
                    || (0_u32, 0_u32, String::new(), 0_u32, Vec::new(), false),
                    |fn_node| {
                        let (nl, nc) = function_name_position(&fn_node).unwrap_or((0, 0));
                        let doc = first_docstring_line(&fn_node, &parsed.source);
                        let row = u32::try_from(fn_node.start_position().row).unwrap_or(0);
                        // Scope call-site extraction to the function body so
                        // parameter defaults (e.g. `typer.Option(...)`) are
                        // not picked up as outgoing edges.
                        let body_range = fn_node
                            .child_by_field_name("body")
                            .map_or_else(|| item.byte_range.clone(), |b| b.byte_range());
                        let call_sites = extract_call_sites(parsed, body_range);
                        (nl, nc, doc, row, call_sites, true)
                    },
                )
        };

        if !has_fn {
            tracing::warn!(
                "walk: byte range {:?} has no descendant in {}",
                item.byte_range,
                item.file.display()
            );
            return Ok(());
        }

        let signature = fetch_hover_first_line(self.client, &file_str, name_line, name_char).await;
        let rel_file = relative_to_root(&item.file, &self.repo_root);
        let line = fn_row.saturating_add(1);

        self.ensure_node(Node {
            qname: item.qname.clone(),
            signature,
            doc,
            file: rel_file,
            line,
            kind: Kind::Internal,
        });

        for cs in call_sites {
            if let Err(err) = self.resolve_call_site(item, &file_str, &cs).await {
                tracing::warn!(
                    "walk: resolving call site {} at {}:{} failed: {err:?}",
                    cs.callee_text,
                    item.file.display(),
                    cs.line
                );
            }
        }

        Ok(())
    }

    async fn resolve_call_site(
        &mut self,
        item: &WalkItem,
        file_str: &str,
        cs: &CallSite,
    ) -> Result<()> {
        if cs.dynamic {
            self.emit_unresolved(item, cs);
            return Ok(());
        }

        let locations = match timeout(
            GOTO_TIMEOUT,
            self.client.goto_definition(file_str, cs.line, cs.character),
        )
        .await
        {
            Ok(Ok(locs)) => locs,
            Ok(Err(err)) => {
                tracing::warn!(
                    "goto_definition error at {}:{}: {err}",
                    item.file.display(),
                    cs.line
                );
                self.emit_unresolved(item, cs);
                return Ok(());
            }
            Err(_) => {
                tracing::warn!("goto_definition timeout at {}:{}", item.file.display(), cs.line);
                self.emit_unresolved(item, cs);
                return Ok(());
            }
        };

        if locations.is_empty() {
            self.emit_unresolved(item, cs);
            return Ok(());
        }

        // N>1 → union; single resolved/external → resolved/external.
        let union = locations.len() > 1;
        for loc in locations {
            self.handle_location(item, cs, &loc, union);
        }

        Ok(())
    }

    fn handle_location(&mut self, item: &WalkItem, cs: &CallSite, loc: &Location, union: bool) {
        let Some(target_path) = uri_to_path(&loc.uri) else {
            tracing::warn!("cannot parse target uri {}", loc.uri);
            self.emit_unresolved(item, cs);
            return;
        };

        let kind = self.classifier.classify(&target_path);

        match kind {
            Kind::Internal => {
                self.handle_internal_target(item, cs, &target_path, loc, union);
            }
            Kind::External => {
                // Deliberately skip hover on External targets. That hover is
                // the only LSP call that reaches *into* site-packages and on
                // heavyweight deps (ty's own internals, FastAPI, etc.) it
                // tarpits with 5 s timeouts per call, blowing the walk
                // budget. External nodes carry an empty signature.
                let qname = qname_for(&target_path, &self.repo_root, &cs.callee_text);
                self.ensure_node(Node {
                    qname: qname.clone(),
                    signature: String::new(),
                    doc: String::new(),
                    file: String::new(),
                    line: 0,
                    kind: Kind::External,
                });
                let annotation = if union { Annotation::Union } else { Annotation::External };
                self.snapshot.edges.push(Edge { from: item.qname.clone(), to: qname, annotation });
            }
            Kind::Unresolved => {
                self.emit_unresolved(item, cs);
            }
        }
    }

    fn handle_internal_target(
        &mut self,
        item: &WalkItem,
        cs: &CallSite,
        target_path: &Path,
        loc: &Location,
        union: bool,
    ) {
        // Resolve to a canonical target path for downstream parsing / qname.
        let canonical = target_path.canonicalize().unwrap_or_else(|_| target_path.to_path_buf());
        let target_qname = qname_for(&canonical, &self.repo_root, &cs.callee_text);

        let annotation = if union { Annotation::Union } else { Annotation::Resolved };
        self.snapshot.edges.push(Edge {
            from: item.qname.clone(),
            to: target_qname.clone(),
            annotation,
        });

        if self.visited.contains(&target_qname) {
            return;
        }
        if self.queue.iter().any(|q| q.qname == target_qname) {
            return;
        }

        // Need the exact function_definition byte range for the BFS queue.
        let target_range =
            match self.locate_function_at(&canonical, &cs.callee_text, loc.range.start.line) {
                Ok(Some(range)) => range,
                Ok(None) => {
                    tracing::warn!(
                        "internal target {} ({}:{}) — no function_definition found",
                        target_qname,
                        canonical.display(),
                        loc.range.start.line
                    );
                    return;
                }
                Err(err) => {
                    tracing::warn!("internal target {} parse failed: {err:?}", target_qname);
                    return;
                }
            };

        self.queue.push_back(WalkItem {
            qname: target_qname,
            file: canonical,
            byte_range: target_range,
            depth: item.depth.saturating_add(1),
        });
    }

    fn emit_unresolved(&mut self, item: &WalkItem, cs: &CallSite) {
        let qname = format!("<unresolved>:{}@{}:{}", cs.callee_text, item.file.display(), cs.line);
        self.ensure_node(Node {
            qname: qname.clone(),
            signature: String::new(),
            doc: String::new(),
            file: String::new(),
            line: 0,
            kind: Kind::Unresolved,
        });
        self.snapshot.edges.push(Edge {
            from: item.qname.clone(),
            to: qname,
            annotation: Annotation::Unresolved,
        });
    }

    fn ensure_node(&mut self, node: Node) {
        if self.node_qnames.insert(node.qname.clone()) {
            self.snapshot.nodes.push(node);
        }
    }

    fn get_parsed(&mut self, file: &Path) -> Result<&ParsedFile> {
        if !self.parsed_cache.contains_key(file) {
            let parsed = parse_file(file)?;
            self.parsed_cache.insert(file.to_path_buf(), parsed);
        }
        self.parsed_cache.get(file).ok_or_else(|| anyhow::anyhow!("parsed cache miss after insert"))
    }

    fn find_function_range(&mut self, file: &Path, function: &str) -> Result<Option<Range<usize>>> {
        let parsed = self.get_parsed(file)?;
        Ok(find_function_definition(parsed, function, None).map(|n| n.byte_range()))
    }

    fn locate_function_at(
        &mut self,
        file: &Path,
        function: &str,
        line_hint: u32,
    ) -> Result<Option<Range<usize>>> {
        let parsed = self.get_parsed(file)?;
        Ok(find_function_definition(parsed, function, Some(line_hint)).map(|n| n.byte_range()))
    }
}

/// Find a `function_definition` whose name matches, optionally preferring the
/// one closest to `line_hint`. Searches recursively so methods inside classes
/// are discovered.
fn find_function_definition<'t>(
    parsed: &'t ParsedFile,
    name: &str,
    line_hint: Option<u32>,
) -> Option<tree_sitter::Node<'t>> {
    let mut best: Option<tree_sitter::Node<'t>> = None;
    let mut best_diff: Option<u32> = None;

    let mut stack = vec![parsed.tree.root_node()];
    while let Some(node) = stack.pop() {
        if node.kind() == "function_definition" {
            if let Some(ident) = node.child_by_field_name("name") {
                if ident.utf8_text(&parsed.source).ok() == Some(name) {
                    match line_hint {
                        None => return Some(node),
                        Some(hint) => {
                            let row = u32::try_from(node.start_position().row).unwrap_or(u32::MAX);
                            let diff = row.abs_diff(hint);
                            if best_diff.is_none_or(|bd| diff < bd) {
                                best = Some(node);
                                best_diff = Some(diff);
                            }
                        }
                    }
                }
            }
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            stack.push(child);
        }
    }

    best
}

fn function_name_position(fn_node: &tree_sitter::Node<'_>) -> Option<(u32, u32)> {
    let ident = fn_node.child_by_field_name("name")?;
    let pos = ident.start_position();
    Some((u32::try_from(pos.row).unwrap_or(0), u32::try_from(pos.column).unwrap_or(0)))
}

fn first_docstring_line(fn_node: &tree_sitter::Node<'_>, source: &[u8]) -> String {
    let Some(body) = fn_node.child_by_field_name("body") else {
        return String::new();
    };
    let mut cursor = body.walk();
    // Docstrings must be the very first statement in the body. Grab the first
    // child (if any) and check whether it's an expression_statement wrapping a
    // string literal.
    let Some(first) = body.children(&mut cursor).next() else {
        return String::new();
    };
    if first.kind() != "expression_statement" {
        return String::new();
    }
    let mut inner = first.walk();
    for expr in first.children(&mut inner) {
        if expr.kind() == "string" {
            let text = expr.utf8_text(source).unwrap_or("");
            return extract_docstring_first_line(text);
        }
    }
    String::new()
}

fn extract_docstring_first_line(raw: &str) -> String {
    // Strip surrounding quotes (triple or single) and string prefixes.
    let trimmed = raw.trim_start_matches(['r', 'R', 'b', 'B', 'u', 'U', 'f', 'F']);
    let inner = trimmed
        .strip_prefix("\"\"\"")
        .and_then(|s| s.strip_suffix("\"\"\""))
        .or_else(|| trimmed.strip_prefix("'''").and_then(|s| s.strip_suffix("'''")))
        .or_else(|| trimmed.strip_prefix('"').and_then(|s| s.strip_suffix('"')))
        .or_else(|| trimmed.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
        .unwrap_or(trimmed);
    inner.lines().next().unwrap_or("").trim().to_owned()
}

/// Fetch a hover and return the first non-fence line, or an empty string.
///
/// Timeouts and LSP errors are logged at `warn` and collapse to `""` so the
/// walker can continue. `None` hover contents collapse silently (ty returned
/// nothing for the position — normal for unresolvable identifiers).
async fn fetch_hover_first_line(
    client: &TyLspClient,
    file: &str,
    line: u32,
    character: u32,
) -> String {
    match timeout(GOTO_TIMEOUT, client.hover(file, line, character)).await {
        Ok(Ok(Some(h))) => hover_first_line(&h),
        Ok(Ok(None)) => String::new(),
        Ok(Err(err)) => {
            tracing::warn!("hover error at {file}:{line}: {err}");
            String::new()
        }
        Err(_) => {
            tracing::warn!("hover timeout at {file}:{line}");
            String::new()
        }
    }
}

fn hover_first_line(hover: &Hover) -> String {
    let raw = match &hover.contents {
        HoverContents::Markup(m) => m.value.clone(),
        HoverContents::MarkedString(ms) => ms.value.clone(),
        HoverContents::Scalar(s) => s.clone(),
        HoverContents::Array(items) => items
            .iter()
            .map(|item| match item {
                MarkedStringOrString::MarkedString(ms) => ms.value.clone(),
                MarkedStringOrString::String(s) => s.clone(),
            })
            .collect::<Vec<_>>()
            .join("\n"),
    };
    // Prefer the first non-empty, non-fence line.
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("```") {
            continue;
        }
        return trimmed.to_owned();
    }
    String::new()
}

fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let stripped = uri.strip_prefix("file://")?;
    Some(PathBuf::from(stripped))
}

fn relative_to_root(file: &Path, root: &Path) -> String {
    match file.strip_prefix(root) {
        Ok(rel) => rel.to_string_lossy().into_owned(),
        Err(_) => file.to_string_lossy().into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn docstring_triple_quote() {
        assert_eq!(extract_docstring_first_line(r#""""hello world""""#), "hello world");
    }

    #[test]
    fn docstring_multiline_first_line() {
        let s = "\"\"\"first line\nsecond line\n\"\"\"";
        assert_eq!(extract_docstring_first_line(s), "first line");
    }

    #[test]
    fn docstring_raw_prefix() {
        assert_eq!(extract_docstring_first_line(r#"r"raw""#), "raw");
    }

    #[test]
    fn hover_markup_first_line() {
        let h = Hover {
            contents: HoverContents::Markup(crate::lsp::protocol::MarkupContent {
                kind: crate::lsp::protocol::MarkupKind::Markdown,
                value: "```python\ndef foo() -> str\n```\nmore".to_owned(),
            }),
            range: None,
        };
        assert_eq!(hover_first_line(&h), "def foo() -> str");
    }

    #[test]
    fn uri_roundtrip() {
        assert_eq!(uri_to_path("file:///tmp/x.py"), Some(PathBuf::from("/tmp/x.py")));
        assert_eq!(uri_to_path("http://example"), None);
    }

    #[test]
    fn find_function_line_hint_picks_closest() {
        use crate::parse::parse_bytes;
        // Two functions share the name `foo`; the hint must disambiguate.
        let src = "class A:\n    def foo(self):\n        pass\n\nclass B:\n    def foo(self):\n        return 1\n";
        let parsed = parse_bytes(src.as_bytes().to_vec(), PathBuf::from("t.py")).expect("parse");

        // Hint points at the second `foo` (row 5). Best-diff selection must
        // return the closer definition.
        let node = find_function_definition(&parsed, "foo", Some(5)).expect("some match");
        assert_eq!(node.start_position().row, 5);

        // Hint at row 1 returns the first `foo`.
        let node = find_function_definition(&parsed, "foo", Some(1)).expect("some match");
        assert_eq!(node.start_position().row, 1);
    }

    #[test]
    fn find_function_no_hint_returns_some_match() {
        use crate::parse::parse_bytes;
        let src = "def alpha():\n    pass\n\ndef beta():\n    pass\n";
        let parsed = parse_bytes(src.as_bytes().to_vec(), PathBuf::from("t.py")).expect("parse");
        assert!(find_function_definition(&parsed, "alpha", None).is_some());
        assert!(find_function_definition(&parsed, "missing", None).is_none());
    }
}
