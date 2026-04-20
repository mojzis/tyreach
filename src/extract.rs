//! Call-site extraction.
//!
//! Given a parsed Python file and the byte range of a function body, returns
//! the call sites inside it. Mirrors the tree-sitter query pattern used in
//! biston for function extraction, scoped to a subtree and deduplicated by
//! the callee token's start byte.

use std::ops::Range;
use std::sync::OnceLock;

use tree_sitter::StreamingIterator;

use crate::parse::ParsedFile;

/// A single call site inside a function body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallSite {
    /// The textual identifier of the callee (e.g. `foo` or `get` for `os.environ.get`).
    pub callee_text: String,
    /// 0-based line of the callee-name token, suitable for LSP.
    pub line: u32,
    /// 0-based UTF-16-style character column of the callee-name token.
    pub character: u32,
    /// Byte range of the callee-name token in the source.
    pub byte_range: Range<usize>,
    /// `true` when the callee is itself a call expression (`foo()()`) or
    /// another dynamic construct we cannot resolve via goto-definition. The
    /// walker emits these as `<unresolved>` without calling the LSP.
    pub dynamic: bool,
}

/// Extract call sites inside a function body.
///
/// The query runs against the subtree rooted at the node whose byte range
/// matches `within`. If no such node exists the function returns an empty vec.
/// Call sites are deduplicated by the start byte of the callee-name token.
#[allow(
    clippy::expect_used,
    reason = "static tree-sitter query is hard-coded; capture names must exist"
)]
pub fn extract_call_sites(parsed: &ParsedFile, within: Range<usize>) -> Vec<CallSite> {
    let Some(scope) = parsed.tree.root_node().descendant_for_byte_range(within.start, within.end)
    else {
        return Vec::new();
    };

    let query = call_site_query(&parsed.tree);
    let mut cursor = tree_sitter::QueryCursor::new();
    let callee_idx =
        query.capture_index_for_name("callee_name").expect("query must have @callee_name capture");
    let call_idx = query.capture_index_for_name("call").expect("query must have @call capture");
    let dynamic_idx = query.capture_index_for_name("dynamic_call");

    let mut seen_starts = std::collections::HashSet::new();
    let mut sites = Vec::new();

    let mut matches = cursor.matches(query, scope, parsed.source.as_slice());
    while let Some(m) = matches.next() {
        // Dynamic-call branch: `(call function: (call))` — the callee is
        // another call expression, so goto-definition on it is meaningless.
        if let Some(d_idx) = dynamic_idx {
            if let Some(dyn_node) = m.captures.iter().find(|c| c.index == d_idx).map(|c| c.node) {
                let start_byte = dyn_node.start_byte();
                if !seen_starts.insert(start_byte) {
                    continue;
                }
                let text =
                    dyn_node.utf8_text(&parsed.source).unwrap_or("<invalid-utf8>").to_owned();
                let position = dyn_node.start_position();
                sites.push(CallSite {
                    callee_text: text,
                    line: u32::try_from(position.row).unwrap_or(u32::MAX),
                    character: u32::try_from(position.column).unwrap_or(u32::MAX),
                    byte_range: dyn_node.byte_range(),
                    dynamic: true,
                });
                continue;
            }
        }

        let Some(callee_node) = m.captures.iter().find(|c| c.index == callee_idx).map(|c| c.node)
        else {
            continue;
        };

        // Skip matches where the outer call was emitted but we don't have a
        // usable callee (shouldn't happen with this query, but belt-and-suspenders).
        if m.captures.iter().all(|c| c.index != call_idx) {
            continue;
        }

        let start_byte = callee_node.start_byte();
        if !seen_starts.insert(start_byte) {
            continue;
        }

        let text = callee_node.utf8_text(&parsed.source).unwrap_or("<invalid-utf8>").to_owned();
        let position = callee_node.start_position();
        sites.push(CallSite {
            callee_text: text,
            line: u32::try_from(position.row).unwrap_or(u32::MAX),
            character: u32::try_from(position.column).unwrap_or(u32::MAX),
            byte_range: callee_node.byte_range(),
            dynamic: false,
        });
    }

    sites
}

/// Compiled tree-sitter query matching direct and attribute-style calls.
#[allow(clippy::expect_used, reason = "hard-coded tree-sitter query is known-valid")]
fn call_site_query(tree: &tree_sitter::Tree) -> &'static tree_sitter::Query {
    static QUERY: OnceLock<tree_sitter::Query> = OnceLock::new();
    QUERY.get_or_init(|| {
        let language = tree.language();
        tree_sitter::Query::new(
            &language,
            r"
            (call function: [
              (identifier) @callee_name
              (attribute attribute: (identifier) @callee_name)
            ]) @call

            (call function: (call)) @dynamic_call
            ",
        )
        .expect("call site extraction query must compile")
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::parse::parse_bytes;

    fn parse(source: &str) -> ParsedFile {
        parse_bytes(source.as_bytes().to_vec(), PathBuf::from("test.py")).expect("parse")
    }

    fn function_body_range(parsed: &ParsedFile) -> Range<usize> {
        let root = parsed.tree.root_node();
        let mut cursor = root.walk();
        for child in root.children(&mut cursor) {
            if child.kind() == "function_definition" {
                return child.byte_range();
            }
        }
        panic!("no function_definition in source");
    }

    #[test]
    fn extracts_plain_identifier_call() {
        let source = "def main():\n    foo()\n";
        let parsed = parse(source);
        let range = function_body_range(&parsed);
        let sites = extract_call_sites(&parsed, range);
        assert_eq!(sites.len(), 1, "one call site expected");
        assert_eq!(sites[0].callee_text, "foo");
    }

    #[test]
    fn extracts_attribute_call() {
        let source = "def main():\n    os.environ.get(\"X\")\n";
        let parsed = parse(source);
        let range = function_body_range(&parsed);
        let sites = extract_call_sites(&parsed, range);
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].callee_text, "get");
    }

    #[test]
    fn extracts_multiple_calls() {
        let source = "def main():\n    foo()\n    bar()\n    baz.qux()\n";
        let parsed = parse(source);
        let range = function_body_range(&parsed);
        let sites = extract_call_sites(&parsed, range);
        let names: Vec<_> = sites.iter().map(|s| s.callee_text.as_str()).collect();
        assert_eq!(names, vec!["foo", "bar", "qux"]);
    }

    #[test]
    fn empty_function_has_no_call_sites() {
        let source = "def main():\n    pass\n";
        let parsed = parse(source);
        let range = function_body_range(&parsed);
        let sites = extract_call_sites(&parsed, range);
        assert!(sites.is_empty(), "empty body must yield no sites");
    }

    #[test]
    fn scoped_range_excludes_outer_calls() {
        // Outer function `other` calls `baz` outside the inner `main` scope.
        // Extracting within `main`'s range must only yield calls inside main.
        let source = "def other():\n    baz()\n\ndef main():\n    foo()\n";
        let parsed = parse(source);

        let root = parsed.tree.root_node();
        let mut cursor = root.walk();
        let main_fn = root
            .children(&mut cursor)
            .find(|n| {
                n.kind() == "function_definition"
                    && n.child_by_field_name("name")
                        .and_then(|name| name.utf8_text(&parsed.source).ok())
                        == Some("main")
            })
            .expect("main function_definition");

        let sites = extract_call_sites(&parsed, main_fn.byte_range());
        let callees: Vec<_> = sites.iter().map(|s| s.callee_text.as_str()).collect();
        assert_eq!(callees, vec!["foo"], "should not include baz from outer function");
    }
}
