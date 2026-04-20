//! Qualified-name resolution for Python symbols.
//!
//! A qname is the dotted path one would use to import the symbol:
//! `pkg.subpkg.module.symbol`. The v1 resolver is path-based and deliberately
//! simple:
//!
//! - For files under `repo_root`: take the relative path, strip `.py`, replace
//!   `/` with `.`, append `.{symbol}`. `__init__` components are dropped so
//!   `pkg/__init__.py::foo` becomes `pkg.foo`.
//! - If the first component of the relative path is `src` and a `src/`
//!   directory exists directly under `repo_root`, strip that leading `src.`
//!   segment. This covers the common `src/pkg/mod.py` layout.
//! - For files outside `repo_root`: walk upward from the file until the first
//!   directory that lacks an `__init__.py`. That directory's parent defines
//!   the package root; the qname is derived from the remainder.
//!
//! Known-lossy cases (documented as deviations from the spec):
//!
//! - Namespace packages without `__init__.py` are heuristic — externals stop
//!   at the first non-package ancestor, which is usually correct but not
//!   always (PEP 420 implicit namespaces).
//! - Custom `[tool.hatch.build.targets.wheel] packages` layouts that do not
//!   live under a literal `src/` directory are not considered; they'd require
//!   parsing `pyproject.toml`. Deferred.
//! - Re-exports via `from .inner import X` are not re-resolved; the qname
//!   reflects the file where the definition lives.

use std::path::{Component, Path};

/// Build a qname for `symbol` defined in `file`, relative to `repo_root`.
pub fn qname_for(file: &Path, repo_root: &Path, symbol: &str) -> String {
    if let Some(rel) = relative_within(file, repo_root) {
        let has_src_dir = repo_root.join("src").is_dir();
        return internal_qname(&rel, symbol, has_src_dir);
    }
    external_qname(file, symbol)
}

fn relative_within(file: &Path, repo_root: &Path) -> Option<std::path::PathBuf> {
    let file = file.canonicalize().ok().unwrap_or_else(|| file.to_path_buf());
    let root = repo_root.canonicalize().ok().unwrap_or_else(|| repo_root.to_path_buf());
    file.strip_prefix(&root).ok().map(std::path::PathBuf::from)
}

fn internal_qname(rel: &Path, symbol: &str, has_src_dir: bool) -> String {
    let mut parts: Vec<String> = rel
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();

    // Strip a leading `src.` segment if the repo uses a `src/` layout.
    if has_src_dir && parts.first().map(String::as_str) == Some("src") {
        parts.remove(0);
    }

    // Strip the `.py` from the final component.
    if let Some(last) = parts.last_mut() {
        if let Some(stem) = last.strip_suffix(".py") {
            *last = stem.to_owned();
        }
    }

    // Drop a trailing `__init__` segment (package-level module).
    if parts.last().map(String::as_str) == Some("__init__") {
        parts.pop();
    }

    let module = parts.join(".");
    if module.is_empty() {
        symbol.to_owned()
    } else {
        format!("{module}.{symbol}")
    }
}

fn external_qname(file: &Path, symbol: &str) -> String {
    // Walk upward from `file`'s parent until we hit a directory without
    // `__init__.py`. The ancestors of the last `__init__.py`-bearing dir
    // form the package prefix.
    let mut segments: Vec<String> = Vec::new();
    let mut current = file.parent();

    while let Some(dir) = current {
        if dir.join("__init__.py").is_file() {
            if let Some(name) = dir.file_name() {
                segments.push(name.to_string_lossy().into_owned());
            }
            current = dir.parent();
        } else {
            break;
        }
    }

    segments.reverse();

    // Add the file's stem unless it's __init__.
    if let Some(stem) = file.file_stem().and_then(|s| s.to_str()) {
        if stem != "__init__" {
            segments.push(stem.to_owned());
        }
    }

    let module = segments.join(".");
    if module.is_empty() {
        symbol.to_owned()
    } else {
        format!("{module}.{symbol}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_module_qname() {
        let root = tempfile::tempdir().expect("tempdir");
        let pkg = root.path().join("pkg");
        std::fs::create_dir_all(&pkg).expect("mkdir");
        std::fs::write(pkg.join("__init__.py"), "").expect("init");
        let file = pkg.join("mod.py");
        std::fs::write(&file, "def foo(): pass\n").expect("file");

        assert_eq!(qname_for(&file, root.path(), "foo"), "pkg.mod.foo");
    }

    #[test]
    fn init_module_drops_suffix() {
        let root = tempfile::tempdir().expect("tempdir");
        let pkg = root.path().join("pkg");
        std::fs::create_dir_all(&pkg).expect("mkdir");
        let file = pkg.join("__init__.py");
        std::fs::write(&file, "").expect("file");

        assert_eq!(qname_for(&file, root.path(), "foo"), "pkg.foo");
    }

    #[test]
    fn src_layout_strips_leading_src() {
        let root = tempfile::tempdir().expect("tempdir");
        let pkg = root.path().join("src/pkg");
        std::fs::create_dir_all(&pkg).expect("mkdir");
        std::fs::write(pkg.join("__init__.py"), "").expect("init");
        let file = pkg.join("mod.py");
        std::fs::write(&file, "").expect("file");

        assert_eq!(qname_for(&file, root.path(), "bar"), "pkg.mod.bar");
    }

    #[test]
    fn nested_package_qname() {
        let root = tempfile::tempdir().expect("tempdir");
        let inner = root.path().join("a/b/c");
        std::fs::create_dir_all(&inner).expect("mkdir");
        let file = inner.join("mod.py");
        std::fs::write(&file, "").expect("file");

        assert_eq!(qname_for(&file, root.path(), "x"), "a.b.c.mod.x");
    }

    #[test]
    fn external_file_uses_init_walk() {
        // Simulate a site-packages layout: `/tmp/xyz/pkg/mod.py` with
        // `__init__.py` in pkg/ but not in `/tmp/xyz/`.
        let site = tempfile::tempdir().expect("tempdir");
        let pkg = site.path().join("pkg");
        std::fs::create_dir_all(&pkg).expect("mkdir");
        std::fs::write(pkg.join("__init__.py"), "").expect("init");
        let file = pkg.join("mod.py");
        std::fs::write(&file, "").expect("file");

        // Pass a *different* root so the file appears external.
        let other_root = tempfile::tempdir().expect("other");
        assert_eq!(qname_for(&file, other_root.path(), "dumps"), "pkg.mod.dumps");
    }

    #[test]
    fn external_init_file_walks_once() {
        let site = tempfile::tempdir().expect("tempdir");
        let pkg = site.path().join("pkg");
        std::fs::create_dir_all(&pkg).expect("mkdir");
        let file = pkg.join("__init__.py");
        std::fs::write(&file, "").expect("file");

        let other_root = tempfile::tempdir().expect("other");
        assert_eq!(qname_for(&file, other_root.path(), "foo"), "pkg.foo");
    }
}
