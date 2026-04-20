# Build: ty-reach (working name — rename to taste)

## Goal

Rust library + Python CLI that, given one or more Python entry points in a uv-managed monorepo, produces a ranked reachability map of symbols, scoped to the repo and stopping at the site-packages boundary. Output is a snapshot sized to a token budget, intended to be read by coding agents as orientation for a codebase.

One-shot, no daemon. The snapshot is the persistence.

## Reference code

- **ty handling** (LSP calls, symbol resolution, type info): see `../ty-find/`. Copy the relevant modules; strip the daemon/server bits. The LSP-to-useful translation layer is what we want.
- **tree-sitter handling** (Python AST parsing, call-site extraction): see `../biston/`. Use as the pattern.
- **PyPI packaging** (maturin, GitHub Actions, Trusted Publishing): same pattern as both above.

Do not extract shared crates yet — copy, ship, dedupe later once the real shared surface is visible.

## Core algorithm

For each entry point:

1. Resolve entry module → entry function(s) via ty.
2. Traverse the call graph (BFS, by qualified name):
   - Parse the current function's body with tree-sitter.
   - Extract every call site (`name()`, `obj.method()`, attribute chains).
   - Resolve each call site via ty.
   - If target is **internal** (lives in the repo): enqueue and traverse.
   - If target is **external** (site-packages): record as leaf, do not traverse.
   - If target is **unresolved** (dynamic dispatch, `getattr`, decorator magic): record as `unresolved` with source location — known unknowns are more useful than missing edges.
   - If ty returns **multiple candidates** (method dispatch union): record all edges, mark as `union`.
   - Break cycles via visited set on qualified name.
3. Emit a flat node list + edge list (see data model below). The tree/DAG shape is reconstructible from edges; do not serialize recursively.

## Data model

Flat, not nested. Two tables:

**Nodes** — one row per reachable symbol:

- `qname` (qualified name, e.g. `myapp.ingest.batch.process`)
- `signature` (from ty; empty for externals and unresolved)
- `doc` (first line of docstring, if present; empty otherwise)
- `file` (source path relative to repo root; empty for externals)
- `line` (definition line; empty for externals)
- `kind` (`internal` | `external` | `unresolved`)

**Edges** — one row per call site:

- `from` (caller qname)
- `to` (callee qname)
- `annotation` (`resolved` | `external` | `union` | `unresolved`)

Rationale: a call graph is a DAG (minus recursion), and nested serialization duplicates nodes at diamond points. Flat tables avoid that, diff cleanly at row granularity, and map naturally to TOON's tabular format.

## Ranking & budget fitting

Rank nodes by: BFS depth from entry point (shallower = more important) + fan-in count (called from more places = more important). Simple weighted sum for v1. Leave PageRank as a follow-up.

Fit to token budget by truncating the ranked node list. Edges referencing truncated nodes are dropped. Default 2000 tokens, user-configurable.

## Entry-point detection

Auto-detect from:

- `pyproject.toml [project.scripts]`
- `Dockerfile` `CMD` / `ENTRYPOINT`
- A config file at repo root (`tyreach.toml` or similar) for hand-specified entries — monorepo docker commands often live in helm/yaml, and we want a simple override. Schema: list of `{ name, entry_file, function? }`.

CLI flag `--entry path/to/file.py[::func]` to add or override.

## Output format

**Canonical: TOON** (Token-Oriented Object Notation) via the `toon-format` crate on crates.io.

Rationale: this tool's only consumer is LLM agents. TOON is designed for that, is lossless, serde-integrated in Rust, and ~40% cheaper in tokens than JSON for tabular data (which is exactly the shape of our data model). Line-oriented, diffs cleanly. `grep`/`awk` work naturally on it.

**Rendered view: topologically sorted successors**, one node per line, callees inline. This is what an agent actually reads to orient itself. Forward direction: entry points at top, leaves at bottom, externals marked `[ext]` inline so terminal nodes are visually obvious.

Example shape:

```
myapp.worker.main                                       (entry)
  -> myapp.config.load, myapp.ingest.run, myapp.report.emit
myapp.config.load
  -> myapp.config.parse_toml, os.environ.get [ext]
myapp.ingest.run
  -> myapp.ingest.batch.process, myapp.ingest.batch.validate
myapp.ingest.batch.process
  -> polars.scan_parquet [ext], myapp.schema.check
```

Each node appears exactly once; DAG diamonds don't duplicate. Node metadata (signature, doc) is rendered below the qname on indented lines when the budget allows; omitted when tight.

Commands:

- `tyreach snapshot` → writes `.toon` (canonical) + `.txt` (rendered view) side by side.
- `tyreach render path/to/snapshot.toon` → rendered view only, from existing snapshot.

## Scope (v1)

**In:** snapshot generation, entry-point detection, TOON canonical + rendered-view output, faithful unresolved/union annotations, maturin packaging to PyPI.

**Out (later):** snapshot diff, daemon mode, polyglot support, PageRank, incremental caching.

## Packaging

Rust lib + Python CLI wrapper via maturin, published to PyPI through GitHub Actions Trusted Publishing. Match the biston / ty-find pattern exactly — same workflow file shape, same `pyproject.toml` conventions.

Dependencies:

- `toon-format` (crates.io) — canonical output format
- tree-sitter + tree-sitter-python (pattern from biston)
- ty LSP handling (copied from ty-find)
