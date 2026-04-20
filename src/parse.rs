use std::path::{Path, PathBuf};

use anyhow::Context;

/// A parsed Python source file.
pub struct ParsedFile {
    pub path: PathBuf,
    pub source: Vec<u8>,
    pub tree: tree_sitter::Tree,
}

/// Parse a Python file from disk.
pub fn parse_file(path: &Path) -> anyhow::Result<ParsedFile> {
    let source =
        std::fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    parse_bytes(source, path.to_path_buf())
}

/// Parse Python source bytes into a syntax tree.
pub fn parse_bytes(source: Vec<u8>, path: PathBuf) -> anyhow::Result<ParsedFile> {
    let mut parser = tree_sitter::Parser::new();
    let language = tree_sitter_python::LANGUAGE;
    parser.set_language(&language.into()).context("failed to set Python language")?;

    let tree =
        parser.parse(&source, None).context("tree-sitter parse returned None (cancelled?)")?;

    Ok(ParsedFile { path, source, tree })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_python() {
        let source = b"def foo():\n    pass\n".to_vec();
        let parsed = parse_bytes(source, PathBuf::from("test.py")).expect("parse");
        let root = parsed.tree.root_node();
        assert_eq!(root.kind(), "module");
        assert!(!root.has_error());
    }

    #[test]
    fn parse_syntax_error_still_produces_tree() {
        let source = b"def foo(\n".to_vec();
        let parsed = parse_bytes(source, PathBuf::from("test.py")).expect("parse");
        let root = parsed.tree.root_node();
        assert_eq!(root.kind(), "module");
        assert!(root.has_error());
    }

    #[test]
    fn parse_empty_file() {
        let source = b"".to_vec();
        let parsed = parse_bytes(source, PathBuf::from("test.py")).expect("parse");
        let root = parsed.tree.root_node();
        assert_eq!(root.kind(), "module");
        assert_eq!(root.child_count(), 0);
    }

    #[test]
    fn parse_binary_garbage() {
        let source = vec![0xFF, 0xFE, 0x00, 0x01, 0x80, 0x90];
        let parsed = parse_bytes(source, PathBuf::from("test.py")).expect("parse");
        // tree-sitter should still produce a tree, possibly with errors
        assert_eq!(parsed.tree.root_node().kind(), "module");
    }

    #[test]
    fn parse_file_from_disk() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("test.py");
        std::fs::write(&path, "x = 1\n").expect("write");

        let parsed = parse_file(&path).expect("parse");
        assert_eq!(parsed.path, path);
        assert!(!parsed.tree.root_node().has_error());
    }
}
