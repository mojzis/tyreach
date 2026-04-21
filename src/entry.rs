//! Entry-point discovery.
//!
//! Four sources, in precedence order (highest first):
//!
//! 1. `--entry` CLI flag — parsed via [`parse_cli_entry`]. Accepts
//!    `path/to/file.py::func`. The `::func` suffix is mandatory for v1. Paths
//!    are resolved relative to the repo root argument, **not** the process
//!    CWD.
//! 2. `tyreach.toml` — parsed via [`parse_tyreach_toml`]. Top-level
//!    `entries = [{ name, entry_file, function = "main" (optional) }]`. Paths
//!    are resolved relative to the repo root.
//! 3. `pyproject.toml` `[project.scripts]` — auto-detected via
//!    [`detect_entries`]. Script specs look like `"name" = "module.path:func"`.
//!    We resolve the module portion to a `.py` file on disk by trying, in
//!    order: `{root}/{path}.py`, `{root}/src/{path}.py`,
//!    `{root}/{path}/__init__.py`, `{root}/src/{path}/__init__.py`.
//! 4. `__init__.py` exports — auto-detected via [`detect_init_entries`] for
//!    libraries that publish an API without a CLI. Top-level packages (directly
//!    under `repo_root` or `repo_root/src`) are scanned; exports come from
//!    `__all__`, else top-level `def`/`class`, else `from .submod import X`
//!    re-exports. Capped at 20 per discovery.
//!
//! The [`resolve_entries`] helper implements the precedence: CLI wins if
//! non-empty; else `tyreach.toml`; else `pyproject.toml` scripts; else
//! `__init__.py` exports.
//!
//! Dockerfile entry-point parsing is deferred to v1.1.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use tree_sitter::StreamingIterator;

use crate::parse::{parse_file, ParsedFile};

/// Cap on number of entries a single `detect_init_entries` call emits.
///
/// Libraries occasionally re-export dozens of names; the walker is fine with
/// that but the downstream token budget isn't, and a ranked view across 50
/// entries is rarely more useful than across 20. Keeping the first 20 in
/// source order matches the convention authors use when they curate `__all__`
/// themselves.
const INIT_ENTRY_CAP: usize = 20;

/// Maximum number of `__init__.py` -> `__init__.py` re-export hops we will
/// follow when resolving an export to the file that defines it.
///
/// A single `.py` module is the common case (depth 0). A subpackage with its
/// own facade (`pkg/__init__.py` -> `pkg/sub/__init__.py`) is depth 1. Anything
/// beyond 3 is almost certainly a misconfigured facade chain — we warn and
/// drop so the snapshot doesn't silently chase the wrong target.
const MAX_REEXPORT_DEPTH: usize = 3;

/// A single callable entry point: function `function` defined in `file`,
/// identified by `name` (typically the script name or CLI string).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryPoint {
    pub name: String,
    pub file: PathBuf,
    pub function: String,
}

#[derive(Debug, Deserialize)]
struct PyProject {
    project: Option<Project>,
}

#[derive(Debug, Deserialize)]
struct Project {
    scripts: Option<std::collections::BTreeMap<String, String>>,
}

/// Auto-detect entries from `{repo_root}/pyproject.toml [project.scripts]`.
///
/// Missing file or missing `[project.scripts]` → returns an empty vec (no
/// error). Malformed TOML or unresolvable script targets → logged as warnings
/// and skipped.
pub fn detect_entries(repo_root: &Path) -> Result<Vec<EntryPoint>> {
    let pyproject = repo_root.join("pyproject.toml");
    if !pyproject.is_file() {
        return Ok(Vec::new());
    }

    let raw = std::fs::read_to_string(&pyproject)
        .with_context(|| format!("read {}", pyproject.display()))?;
    let parsed: PyProject = match toml::from_str(&raw) {
        Ok(p) => p,
        Err(err) => {
            tracing::warn!("detect_entries: malformed pyproject.toml: {err}");
            return Ok(Vec::new());
        }
    };

    let Some(scripts) = parsed.project.and_then(|p| p.scripts) else {
        return Ok(Vec::new());
    };

    let mut out = Vec::new();
    for (name, spec) in scripts {
        if let Some((file, function)) = resolve_script_spec(repo_root, &spec) {
            out.push(EntryPoint { name, file, function });
        } else {
            tracing::warn!(
                "detect_entries: could not resolve script {name:?} = {spec:?} to a .py file"
            );
        }
    }

    Ok(out)
}

/// Auto-detect entries from top-level packages' `__init__.py` exports.
///
/// Fallback for library repos with no CLI and no `[project.scripts]`. Scans
/// directories directly under `repo_root` (and under `repo_root/src`) that
/// contain an `__init__.py`, then enumerates exports in priority order:
///
/// 1. `__all__ = [...]` string literals.
/// 2. Top-level `def`/`class` whose name does not start with `_`.
/// 3. `from .<submod> import <name> [as <alias>]` — the (aliased) name.
///
/// For each discovered export, the emitted `EntryPoint.file` points at the
/// file that actually defines the callable (`<pkg>/submod.py` for re-exports,
/// `<pkg>/__init__.py` when the body defines it). The walker's name-based
/// entry-point resolver then finds a matching `def`/`class` there — pointing
/// at the re-export facade would produce an empty snapshot on the common
/// pure-re-export library shape.
///
/// Deduplicated by export name, capped at 20 entries per discovery. When
/// multiple packages contribute, their exports are unioned and re-capped.
///
/// Missing files, unreadable packages, and malformed Python are never fatal —
/// they drop out of the result and the caller falls through to the
/// "no entries" error path.
#[allow(
    clippy::unnecessary_wraps,
    reason = "Result keeps the signature in lock-step with detect_entries and parse_tyreach_toml so callers can use .context() uniformly and we can surface I/O errors in a later iteration without a breaking change."
)]
pub fn detect_init_entries(repo_root: &Path) -> Result<Vec<EntryPoint>> {
    let mut out = Vec::new();

    for init_path in candidate_init_files(repo_root) {
        out.extend(resolve_package_entries(&init_path));
    }

    // Union-dedup across packages, preserving first occurrence.
    let mut seen = std::collections::HashSet::new();
    out.retain(|e| seen.insert(e.name.clone()));

    if out.len() > INIT_ENTRY_CAP {
        tracing::warn!(
            "detect_init_entries: discovered {} exports across packages; capping at {}",
            out.len(),
            INIT_ENTRY_CAP
        );
        out.truncate(INIT_ENTRY_CAP);
    }

    Ok(out)
}

/// Find `__init__.py` files of top-level packages under `repo_root`.
///
/// Looks one level deep in `repo_root` and in `repo_root/src`. Nested
/// subpackages are intentionally ignored — a library's top-level package API
/// is almost always what users import, and scanning deeper blows up the
/// entry count on monorepos without making the snapshot more useful.
fn candidate_init_files(repo_root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for base in [repo_root.to_path_buf(), repo_root.join("src")] {
        let Ok(entries) = std::fs::read_dir(&base) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let init = path.join("__init__.py");
            if init.is_file() {
                out.push(init);
            }
        }
    }
    // Stable order across platforms; `read_dir` does not guarantee one.
    out.sort();
    out
}

/// A name re-exported from `__init__.py` via `from .submod import original [as alias]`.
///
/// `alias` is what callers see (e.g. from `from pkg import X`), `original` is
/// the name to look up in `submod.py`, and `submod` is the first path segment
/// after the leading dot.
#[derive(Debug, Clone)]
struct RelativeReexport {
    alias: String,
    original: String,
    submod: String,
}

/// Parse, resolve, and emit `EntryPoint`s for a single `__init__.py`.
///
/// Drops exports that can't be resolved to a concrete `def <name>` /
/// `class <name>` file (with `tracing::warn!`) so the walker's name-based
/// entry-point resolver doesn't end up searching the facade itself.
fn resolve_package_entries(init_path: &Path) -> Vec<EntryPoint> {
    let parsed = match parse_file(init_path) {
        Ok(p) => p,
        Err(err) => {
            tracing::warn!("detect_init_entries: could not parse {}: {err:#}", init_path.display());
            return Vec::new();
        }
    };

    let local_defs = collect_local_defs(&parsed);
    let reexports = collect_relative_reexports(&parsed);
    let reexport_by_alias: std::collections::HashMap<&str, &RelativeReexport> =
        reexports.iter().map(|r| (r.alias.as_str(), r)).collect();

    let export_names = collect_candidate_export_names(&parsed, &reexports);

    let pkg_dir = init_path.parent().unwrap_or_else(|| Path::new("."));

    let mut out = Vec::new();
    for alias in export_names {
        // Rule 1: defined directly in __init__.py — keep the facade path.
        if local_defs.contains(&alias) {
            out.push(EntryPoint {
                name: alias.clone(),
                file: init_path.to_path_buf(),
                function: alias,
            });
            continue;
        }

        // Rule 2: re-exported via `from .submod import original [as alias]`.
        if let Some(reexport) = reexport_by_alias.get(alias.as_str()) {
            match resolve_reexport_file(pkg_dir, &reexport.submod, &reexport.original, 0) {
                Some(resolved_file) => {
                    out.push(EntryPoint {
                        name: reexport.alias.clone(),
                        file: resolved_file,
                        function: reexport.original.clone(),
                    });
                }
                None => {
                    tracing::warn!(
                        "detect_init_entries: {} re-exports {:?} from .{}, but submodule was not found; dropping entry",
                        init_path.display(),
                        reexport.alias,
                        reexport.submod,
                    );
                }
            }
            continue;
        }

        // Rule 3: name in __all__ but neither defined nor imported here.
        tracing::warn!(
            "detect_init_entries: {} lists {:?} in __all__ but it is neither defined nor imported in the module; dropping entry",
            init_path.display(),
            alias,
        );
    }

    if out.len() > INIT_ENTRY_CAP {
        tracing::warn!(
            "detect_init_entries: {} exports in {}; capping at {}",
            out.len(),
            init_path.display(),
            INIT_ENTRY_CAP
        );
        out.truncate(INIT_ENTRY_CAP);
    }

    out
}

/// Collect the ordered, deduped list of candidate export *alias* names from
/// `__init__.py`.
///
/// If `__all__` is set, that's the list verbatim (minus `_`-prefixed).
/// Otherwise the union of top-level `def`/`class` names and re-export aliases
/// is returned in source order. The cap is applied by the caller after
/// resolution so the warning reflects post-resolution counts.
fn collect_candidate_export_names(
    parsed: &ParsedFile,
    reexports: &[RelativeReexport],
) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();

    if let Some(all_list) = extract_all_list(parsed) {
        for name in all_list {
            if name.starts_with('_') {
                continue;
            }
            if seen.insert(name.clone()) {
                names.push(name);
            }
        }
        return names;
    }

    // No __all__: merge local defs/classes + re-export aliases in source order.
    let reexport_aliases: std::collections::HashSet<&str> =
        reexports.iter().map(|r| r.alias.as_str()).collect();

    let root = parsed.tree.root_node();
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        match child.kind() {
            "function_definition" | "class_definition" => {
                let Some(name) = top_level_def_or_class_name(&child, &parsed.source) else {
                    continue;
                };
                if !name.starts_with('_') && seen.insert(name.clone()) {
                    names.push(name);
                }
            }
            "import_from_statement" => {
                if !import_from_is_relative(&child, &parsed.source) {
                    continue;
                }
                for alias in import_from_names(&child, &parsed.source) {
                    if alias.starts_with('_') {
                        continue;
                    }
                    if reexport_aliases.contains(alias.as_str()) && seen.insert(alias.clone()) {
                        names.push(alias);
                    }
                }
            }
            _ => {}
        }
    }

    names
}

/// Collect names introduced by top-level `def <name>` / `class <name>`.
fn collect_local_defs(parsed: &ParsedFile) -> std::collections::HashSet<String> {
    let root = parsed.tree.root_node();
    let mut out = std::collections::HashSet::new();
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        if let Some(name) = top_level_def_or_class_name(&child, &parsed.source) {
            out.insert(name);
        }
    }
    out
}

/// Collect `from .<submod> import <original> [as <alias>]` re-exports.
///
/// Only relative imports with a named module (not bare `from . import x`) are
/// considered — we need the submodule name to resolve the target file. The
/// `submod` captured here is the first path segment after the leading dots,
/// so `from .a.b import x` yields `submod = "a"`. Deeper submodule paths are
/// uncommon in `__init__.py` re-export shims, so we accept the lossy shape
/// and fall back to the walker's resolver when a deeper lookup would be
/// needed.
fn collect_relative_reexports(parsed: &ParsedFile) -> Vec<RelativeReexport> {
    let root = parsed.tree.root_node();
    let mut out = Vec::new();

    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        if child.kind() != "import_from_statement" {
            continue;
        }
        if !import_from_is_relative(&child, &parsed.source) {
            continue;
        }
        let Some(submod) = relative_submod_name(&child, &parsed.source) else {
            continue;
        };
        for (original, alias) in import_from_originals_and_aliases(&child, &parsed.source) {
            out.push(RelativeReexport { alias, original, submod: submod.clone() });
        }
    }

    out
}

/// Resolve the file where `original` is defined, following at most one hop
/// through a subpackage `__init__.py` re-export chain.
///
/// Returns the file that actually defines `original` via `def`/`class`, or
/// `None` if no resolution works out before hitting the depth cap. Warns and
/// drops on excess depth.
fn resolve_reexport_file(
    pkg_dir: &Path,
    submod: &str,
    original: &str,
    depth: usize,
) -> Option<PathBuf> {
    if depth >= MAX_REEXPORT_DEPTH {
        tracing::warn!(
            "detect_init_entries: re-export chain for {:?} under {} exceeded depth {}; dropping",
            original,
            pkg_dir.display(),
            MAX_REEXPORT_DEPTH,
        );
        return None;
    }

    // Prefer the flat `.py` file — the common shape.
    let flat = pkg_dir.join(format!("{submod}.py"));
    if flat.is_file() {
        return Some(flat);
    }

    // Subpackage: recurse into `<submod>/__init__.py` when it also re-exports
    // `original`. If that inner init defines `original` directly, return it;
    // otherwise follow the inner re-export one more hop.
    let subpkg_init = pkg_dir.join(submod).join("__init__.py");
    if !subpkg_init.is_file() {
        return None;
    }

    let parsed = match parse_file(&subpkg_init) {
        Ok(p) => p,
        Err(err) => {
            tracing::warn!(
                "detect_init_entries: could not parse {} while resolving re-export: {err:#}",
                subpkg_init.display()
            );
            return None;
        }
    };

    if collect_local_defs(&parsed).contains(original) {
        return Some(subpkg_init);
    }

    let inner_dir = subpkg_init.parent()?.to_path_buf();
    for reexport in collect_relative_reexports(&parsed) {
        if reexport.alias == original {
            return resolve_reexport_file(
                &inner_dir,
                &reexport.submod,
                &reexport.original,
                depth + 1,
            );
        }
    }

    None
}

/// Return the first path segment after the leading dot(s) in
/// `from .<submod>[.<rest>] import ...`, or `None` for bare `from . import x`.
fn relative_submod_name(node: &tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let module = node.child_by_field_name("module_name")?;
    if module.kind() != "relative_import" {
        return None;
    }

    // A `relative_import` contains one or more `import_prefix` dots followed
    // optionally by a `dotted_name`.
    let mut cursor = module.walk();
    for child in module.children(&mut cursor) {
        if child.kind() == "dotted_name" {
            let first = child.child(0)?;
            return first.utf8_text(source).ok().map(str::to_owned);
        }
    }
    None
}

/// Parse `__all__ = [...]` into the list of string literals, if present.
///
/// Returns `None` when `__all__` is absent so callers can fall through to the
/// `def`/`class` path. Non-list right-hand sides (e.g. tuples, computed
/// values) are treated as "present but empty" — we prefer that over quietly
/// falling through, since authors who set `__all__` almost always mean it.
fn extract_all_list(parsed: &ParsedFile) -> Option<Vec<String>> {
    let query = all_assignment_query(&parsed.tree);
    let name_idx = query.capture_index_for_name("name")?;
    let right_idx = query.capture_index_for_name("right")?;

    let mut cursor = tree_sitter::QueryCursor::new();
    let mut matches = cursor.matches(query, parsed.tree.root_node(), parsed.source.as_slice());

    while let Some(m) = matches.next() {
        let Some(name_node) = m.captures.iter().find(|c| c.index == name_idx).map(|c| c.node)
        else {
            continue;
        };
        let Some(right_node) = m.captures.iter().find(|c| c.index == right_idx).map(|c| c.node)
        else {
            continue;
        };
        if name_node.utf8_text(&parsed.source).ok() != Some("__all__") {
            continue;
        }

        if right_node.kind() != "list" {
            // __all__ = (...)/{...}/some_var — found but not a plain list.
            // We treat this as "explicitly defined, we can't read it" and stop.
            return Some(Vec::new());
        }

        let mut out = Vec::new();
        let mut inner = right_node.walk();
        for item in right_node.children(&mut inner) {
            if item.kind() != "string" {
                continue;
            }
            out.push(string_literal_value(&item, &parsed.source));
        }
        return Some(out);
    }

    None
}

/// Extract the textual value of a Python string literal node.
///
/// tree-sitter represents `"foo"` as a `string` with a `string_start`,
/// `string_content`, and `string_end`. We concatenate the `string_content`
/// children rather than slicing the outer node so quote characters and
/// f-string/b-string prefixes don't leak into the result. An empty string
/// `""` has no `string_content` child but returning `""` lets the caller's
/// `_`-prefix filter drop it uniformly with explicitly-named hidden exports.
fn string_literal_value(node: &tree_sitter::Node<'_>, source: &[u8]) -> String {
    let mut cursor = node.walk();
    let mut out = String::new();
    for child in node.children(&mut cursor) {
        if child.kind() == "string_content" {
            if let Ok(text) = child.utf8_text(source) {
                out.push_str(text);
            }
        }
    }
    out
}

/// If `node` is a top-level `function_definition` or `class_definition`,
/// return the declared name.
fn top_level_def_or_class_name(node: &tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    match node.kind() {
        "function_definition" | "class_definition" => {
            let name = node.child_by_field_name("name")?;
            name.utf8_text(source).ok().map(str::to_owned)
        }
        _ => None,
    }
}

/// Is `import_from_statement` a relative import (`from .x import y`)?
///
/// tree-sitter encodes the `from` target as the `module_name` field, which is
/// either a `dotted_name` (absolute) or a `relative_import` (contains an
/// `import_prefix` like `.` or `..`).
fn import_from_is_relative(node: &tree_sitter::Node<'_>, _source: &[u8]) -> bool {
    let Some(module) = node.child_by_field_name("module_name") else {
        // `from . import submod` has no module_name field — still relative.
        // Detect by scanning children for an `import_prefix`.
        let mut cursor = node.walk();
        return node
            .children(&mut cursor)
            .any(|c| c.kind() == "import_prefix" || c.kind() == "relative_import");
    };
    module.kind() == "relative_import"
}

/// Collect the imported (aliased) names from a `from X import a, b as c`.
///
/// Returns the visible name — the alias if one exists, otherwise the original
/// identifier. Used for candidate-export enumeration where we only care about
/// what external code can reach by name.
fn import_from_names(node: &tree_sitter::Node<'_>, source: &[u8]) -> Vec<String> {
    import_from_originals_and_aliases(node, source).into_iter().map(|(_, alias)| alias).collect()
}

/// Collect `(original, alias)` pairs from a `from X import a, b as c`.
///
/// `original` is the name as written in the module being imported from;
/// `alias` is what the name becomes in the current module (equal to `original`
/// when there's no `as` clause). The alias is what appears in `__all__`; the
/// original is what we look up in the target `.py` file.
fn import_from_originals_and_aliases(
    node: &tree_sitter::Node<'_>,
    source: &[u8],
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut cursor = node.walk();
    for child in node.children_by_field_name("name", &mut cursor) {
        match child.kind() {
            "dotted_name" => {
                if let Ok(text) = child.utf8_text(source) {
                    out.push((text.to_owned(), text.to_owned()));
                }
            }
            "aliased_import" => {
                let name_text = child
                    .child_by_field_name("name")
                    .and_then(|n| n.utf8_text(source).ok())
                    .map(str::to_owned);
                let alias_text = child
                    .child_by_field_name("alias")
                    .and_then(|n| n.utf8_text(source).ok())
                    .map(str::to_owned);
                match (name_text, alias_text) {
                    (Some(name), Some(alias)) => out.push((name, alias)),
                    (Some(name), None) => out.push((name.clone(), name)),
                    _ => {}
                }
            }
            _ => {}
        }
    }
    out
}

fn resolve_script_spec(repo_root: &Path, spec: &str) -> Option<(PathBuf, String)> {
    let (module, function) = spec.split_once(':')?;
    let module_path = module.replace('.', "/");

    let candidates = [
        repo_root.join(format!("{module_path}.py")),
        repo_root.join("src").join(format!("{module_path}.py")),
        repo_root.join(&module_path).join("__init__.py"),
        repo_root.join("src").join(&module_path).join("__init__.py"),
    ];

    for candidate in candidates {
        if candidate.is_file() {
            return Some((candidate, function.to_owned()));
        }
    }
    None
}

/// Parse a `--entry` CLI argument of the form `path/to/file.py::func`.
///
/// The function name is mandatory in v1. Paths are resolved relative to
/// `repo_root` when not absolute. The file must exist.
pub fn parse_cli_entry(spec: &str, repo_root: &Path) -> Result<EntryPoint> {
    let (raw_path, function) = spec.split_once("::").ok_or_else(|| {
        anyhow!("--entry {spec:?} is missing '::func'; v1 requires an explicit function")
    })?;
    if function.is_empty() {
        anyhow::bail!("--entry {spec:?} has empty function name after '::'");
    }

    let path = Path::new(raw_path);
    let resolved = if path.is_absolute() { path.to_path_buf() } else { repo_root.join(path) };

    if !resolved.is_file() {
        anyhow::bail!("--entry file does not exist: {}", resolved.display());
    }

    Ok(EntryPoint { name: spec.to_owned(), file: resolved, function: function.to_owned() })
}

/// Parse `{repo_root}/tyreach.toml` into a list of entry points.
///
/// Schema:
///
/// ```toml
/// [[entries]]
/// name = "cli"
/// entry_file = "myapp/cli.py"
/// function = "main"   # optional; defaults to "main"
/// ```
///
/// Missing file → `Ok(Vec::new())`. Malformed TOML or an unreadable/missing
/// `entry_file` on disk → `Err` with a span-aware message.
pub fn parse_tyreach_toml(repo_root: &Path) -> Result<Vec<EntryPoint>> {
    let path = repo_root.join("tyreach.toml");
    if !path.is_file() {
        return Ok(Vec::new());
    }

    let raw = std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;

    let doc: TyreachToml = toml::from_str(&raw).map_err(|err| {
        // toml::de::Error carries a span. Surface it in the error message so
        // users see the offending line/column alongside the reason.
        let span = err.span();
        match span {
            Some(range) => {
                let (line, col) = line_col(&raw, range.start);
                anyhow!(
                    "malformed tyreach.toml at {}:{}:{}: {}",
                    path.display(),
                    line,
                    col,
                    err.message()
                )
            }
            None => anyhow!("malformed tyreach.toml {}: {}", path.display(), err.message()),
        }
    })?;

    let mut out = Vec::new();
    for raw_entry in doc.entries.unwrap_or_default() {
        let entry_file = Path::new(&raw_entry.entry_file);
        let resolved = if entry_file.is_absolute() {
            entry_file.to_path_buf()
        } else {
            repo_root.join(entry_file)
        };
        if !resolved.is_file() {
            anyhow::bail!(
                "tyreach.toml entry {:?}: entry_file does not exist: {}",
                raw_entry.name,
                resolved.display()
            );
        }
        let function = raw_entry.function.unwrap_or_else(|| "main".to_owned());
        out.push(EntryPoint { name: raw_entry.name, file: resolved, function });
    }
    Ok(out)
}

/// Resolve entry points given the repo root and optional CLI entries.
///
/// Precedence (first non-empty source wins):
///
/// 1. `cli_entries` (parsed with [`parse_cli_entry`]).
/// 2. `tyreach.toml` (parsed with [`parse_tyreach_toml`]).
/// 3. `pyproject.toml` `[project.scripts]` (parsed with [`detect_entries`]).
/// 4. `__init__.py` exports (parsed with [`detect_init_entries`]).
///
/// Errors if *all four* sources yield zero entries — callers usually want to
/// tell the user to supply `--entry` or populate one of the config files.
pub fn resolve_entries(repo_root: &Path, cli_entries: &[String]) -> Result<Vec<EntryPoint>> {
    if !cli_entries.is_empty() {
        return cli_entries.iter().map(|spec| parse_cli_entry(spec, repo_root)).collect();
    }

    let from_tyreach = parse_tyreach_toml(repo_root).context("parse tyreach.toml")?;
    if !from_tyreach.is_empty() {
        return Ok(from_tyreach);
    }

    let detected = detect_entries(repo_root).context("detect entries from pyproject.toml")?;
    if !detected.is_empty() {
        return Ok(detected);
    }

    let init_entries =
        detect_init_entries(repo_root).context("detect entries from __init__.py exports")?;
    if init_entries.is_empty() {
        anyhow::bail!(
            "no entry points found; supply --entry path/to/file.py::func, create tyreach.toml, add [project.scripts], or export names from <pkg>/__init__.py; run 'tyreach setup' for a diagnosis"
        );
    }
    Ok(init_entries)
}

#[derive(Debug, Deserialize)]
struct TyreachToml {
    entries: Option<Vec<TyreachEntry>>,
}

#[derive(Debug, Deserialize)]
struct TyreachEntry {
    name: String,
    entry_file: String,
    function: Option<String>,
}

/// Tree-sitter query matching `<name> = <right>` module-level assignments.
///
/// Callers filter by `@name`'s text to pick out `__all__`. We reuse the same
/// compiled query across calls via `OnceLock`, mirroring `src/extract.rs`.
#[allow(clippy::expect_used, reason = "hard-coded tree-sitter query is known-valid")]
fn all_assignment_query(tree: &tree_sitter::Tree) -> &'static tree_sitter::Query {
    static QUERY: OnceLock<tree_sitter::Query> = OnceLock::new();
    QUERY.get_or_init(|| {
        let language = tree.language();
        tree_sitter::Query::new(
            &language,
            r"
            (module
              (expression_statement
                (assignment
                  left: (identifier) @name
                  right: (_) @right)))
            ",
        )
        .expect("__all__ assignment query must compile")
    })
}

/// 1-based (line, column) from a byte offset into `src`.
fn line_col(src: &str, byte: usize) -> (usize, usize) {
    let clamped = byte.min(src.len());
    let prefix = &src[..clamped];
    let line = prefix.bytes().filter(|b| *b == b'\n').count() + 1;
    let last_newline = prefix.rfind('\n').map_or(0, |p| p + 1);
    let col = src[last_newline..clamped].chars().count() + 1;
    (line, col)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_simple_script_entry() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("pyproject.toml"),
            r#"
[project]
name = "demo"
version = "0.1.0"

[project.scripts]
demo = "demo.main:run"
"#,
        )
        .expect("write pyproject");

        let pkg = dir.path().join("demo");
        std::fs::create_dir_all(&pkg).expect("mkdir");
        std::fs::write(pkg.join("__init__.py"), "").expect("init");
        std::fs::write(pkg.join("main.py"), "def run():\n    pass\n").expect("main");

        let entries = detect_entries(dir.path()).expect("detect");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "demo");
        assert_eq!(entries[0].function, "run");
        assert!(entries[0].file.ends_with("demo/main.py"));
    }

    #[test]
    fn detects_src_layout_entry() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("pyproject.toml"),
            r#"
[project]
name = "demo"
version = "0.1.0"

[project.scripts]
demo = "demo.main:run"
"#,
        )
        .expect("write pyproject");

        let pkg = dir.path().join("src/demo");
        std::fs::create_dir_all(&pkg).expect("mkdir");
        std::fs::write(pkg.join("__init__.py"), "").expect("init");
        std::fs::write(pkg.join("main.py"), "def run():\n    pass\n").expect("main");

        let entries = detect_entries(dir.path()).expect("detect");
        assert_eq!(entries.len(), 1);
        assert!(entries[0].file.ends_with("src/demo/main.py"));
    }

    #[test]
    fn missing_pyproject_yields_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let entries = detect_entries(dir.path()).expect("detect");
        assert!(entries.is_empty());
    }

    #[test]
    fn pyproject_without_scripts_yields_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("pyproject.toml"),
            "[project]\nname = \"demo\"\nversion = \"0.1.0\"\n",
        )
        .expect("write");
        let entries = detect_entries(dir.path()).expect("detect");
        assert!(entries.is_empty());
    }

    #[test]
    fn parse_cli_entry_requires_function() {
        let dir = tempfile::tempdir().expect("tempdir");
        let err = parse_cli_entry("some/file.py", dir.path()).unwrap_err();
        assert!(err.to_string().contains("missing '::func'"));
    }

    #[test]
    fn parse_cli_entry_resolves_relative_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pkg = dir.path().join("pkg");
        std::fs::create_dir_all(&pkg).expect("mkdir");
        std::fs::write(pkg.join("mod.py"), "def go():\n    pass\n").expect("write");

        let entry = parse_cli_entry("pkg/mod.py::go", dir.path()).expect("parse");
        assert_eq!(entry.function, "go");
        assert!(entry.file.ends_with("pkg/mod.py"));
    }

    #[test]
    fn parse_cli_entry_rejects_missing_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(parse_cli_entry("nope.py::fn", dir.path()).is_err());
    }

    #[test]
    fn malformed_pyproject_yields_empty_not_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Unterminated table header — toml parser will reject.
        std::fs::write(dir.path().join("pyproject.toml"), "[project\nname = broken\n")
            .expect("write");
        // Per contract, a malformed pyproject.toml should not propagate the
        // parse error; the function logs and returns an empty vec so the CLI
        // can still accept `--entry` arguments.
        let entries = detect_entries(dir.path()).expect("detect");
        assert!(entries.is_empty(), "malformed pyproject must not yield entries");
    }

    #[test]
    fn unresolvable_script_spec_is_skipped() {
        let dir = tempfile::tempdir().expect("tempdir");
        // script points at a module that has no .py file anywhere under the root.
        std::fs::write(
            dir.path().join("pyproject.toml"),
            r#"
[project]
name = "demo"
version = "0.1.0"

[project.scripts]
demo = "nonexistent.module:run"
"#,
        )
        .expect("write");
        let entries = detect_entries(dir.path()).expect("detect");
        assert!(entries.is_empty(), "unresolvable script spec must be skipped silently");
    }

    #[test]
    fn tyreach_toml_missing_is_empty_not_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let entries = parse_tyreach_toml(dir.path()).expect("parse");
        assert!(entries.is_empty());
    }

    #[test]
    fn tyreach_toml_parses_entries_with_default_function() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pkg = dir.path().join("myapp");
        std::fs::create_dir_all(&pkg).expect("mkdir");
        std::fs::write(pkg.join("cli.py"), "def main():\n    pass\n").expect("cli.py");
        std::fs::write(pkg.join("worker.py"), "def go():\n    pass\n").expect("worker.py");

        std::fs::write(
            dir.path().join("tyreach.toml"),
            r#"
[[entries]]
name = "cli"
entry_file = "myapp/cli.py"

[[entries]]
name = "worker"
entry_file = "myapp/worker.py"
function = "go"
"#,
        )
        .expect("write tyreach.toml");

        let entries = parse_tyreach_toml(dir.path()).expect("parse");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "cli");
        assert_eq!(entries[0].function, "main", "default function is `main`");
        assert_eq!(entries[1].name, "worker");
        assert_eq!(entries[1].function, "go");
    }

    #[test]
    fn tyreach_toml_malformed_reports_location() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Unterminated table header — toml parser will reject at a known span.
        std::fs::write(dir.path().join("tyreach.toml"), "[[entries\nname = \"x\"\n")
            .expect("write");
        let err = parse_tyreach_toml(dir.path()).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("malformed tyreach.toml"), "missing prefix: {msg}");
        assert!(msg.contains(":1:"), "malformed toml must report line 1: {msg}");
    }

    #[test]
    fn tyreach_toml_missing_entry_file_errors() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("tyreach.toml"),
            "[[entries]]\nname = \"x\"\nentry_file = \"nope.py\"\n",
        )
        .expect("write");
        let err = parse_tyreach_toml(dir.path()).unwrap_err();
        assert!(err.to_string().contains("does not exist"), "got: {err}");
    }

    #[test]
    fn resolve_entries_cli_beats_config_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pkg = dir.path().join("app");
        std::fs::create_dir_all(&pkg).expect("mkdir");
        std::fs::write(pkg.join("cli.py"), "def main():\n    pass\n").expect("cli");
        std::fs::write(pkg.join("other.py"), "def run():\n    pass\n").expect("other");
        std::fs::write(
            dir.path().join("pyproject.toml"),
            "[project]\nname=\"x\"\nversion=\"0.1.0\"\n[project.scripts]\nx = \"app.cli:main\"\n",
        )
        .expect("pyproject");
        std::fs::write(
            dir.path().join("tyreach.toml"),
            "[[entries]]\nname=\"cli\"\nentry_file=\"app/cli.py\"\n",
        )
        .expect("tyreach");

        let entries = resolve_entries(dir.path(), &["app/other.py::run".to_owned()]).expect("ok");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].function, "run", "cli must override config files");
    }

    #[test]
    fn resolve_entries_tyreach_beats_pyproject() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pkg = dir.path().join("app");
        std::fs::create_dir_all(&pkg).expect("mkdir");
        std::fs::write(pkg.join("__init__.py"), "").expect("init");
        std::fs::write(pkg.join("cli.py"), "def main():\n    pass\n").expect("cli");
        std::fs::write(pkg.join("special.py"), "def run():\n    pass\n").expect("special");
        std::fs::write(
            dir.path().join("pyproject.toml"),
            "[project]\nname=\"x\"\nversion=\"0.1.0\"\n[project.scripts]\nx = \"app.cli:main\"\n",
        )
        .expect("pyproject");
        std::fs::write(
            dir.path().join("tyreach.toml"),
            "[[entries]]\nname=\"special\"\nentry_file=\"app/special.py\"\nfunction=\"run\"\n",
        )
        .expect("tyreach");

        let entries = resolve_entries(dir.path(), &[]).expect("ok");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "special");
        assert_eq!(entries[0].function, "run");
    }

    #[test]
    fn resolve_entries_falls_back_to_pyproject() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pkg = dir.path().join("app");
        std::fs::create_dir_all(&pkg).expect("mkdir");
        std::fs::write(pkg.join("__init__.py"), "").expect("init");
        std::fs::write(pkg.join("cli.py"), "def main():\n    pass\n").expect("cli");
        std::fs::write(
            dir.path().join("pyproject.toml"),
            "[project]\nname=\"x\"\nversion=\"0.1.0\"\n[project.scripts]\nx = \"app.cli:main\"\n",
        )
        .expect("pyproject");

        let entries = resolve_entries(dir.path(), &[]).expect("ok");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].function, "main");
    }

    #[test]
    fn resolve_entries_errors_when_nothing_configured() {
        let dir = tempfile::tempdir().expect("tempdir");
        let err = resolve_entries(dir.path(), &[]).unwrap_err();
        assert!(err.to_string().contains("no entry points"), "unexpected: {err}");
    }
}
