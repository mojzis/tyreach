# tyreach output format

`tyreach snapshot` writes two artefacts:

- `<prefix>.toon` — the **canonical** TOON snapshot. Machine-readable, stable
  across minor releases, designed to be the single source of truth.
- `<prefix>.txt` — a topologically-sorted rendered view, derived entirely from
  the `.toon` file. Intended for humans and coding agents skimming the graph.

`tyreach render` takes a `.toon` and re-emits the rendered view bit-identically
to what `snapshot` produced — the canonical form never loses information the
renderer needs.

Stability promise: the TOON column order, enum values, and file-layout
markers documented here are frozen for the `0.x` series. Additive changes
(new optional columns, new annotation variants) are possible; removals and
renames are not.

## TOON schema

A snapshot TOON file is three tabular blocks separated by blank lines and
comment markers:

```text
# tyreach snapshot v1
# nodes
nodes[N]{qname,signature,doc,file,line,kind}:
  <row>
  ...

# edges
edges[M]{from,to,annotation}:
  <row>
  ...

# entries
entries[K]: qname1,qname2,...
```

### `# nodes` — column order is load-bearing

| # | column      | type   | meaning                                                        |
|---|-------------|--------|----------------------------------------------------------------|
| 1 | `qname`     | string | qualified name, e.g. `myapp.core.Service.process`              |
| 2 | `signature` | string | first line of the ty hover result (empty for externals / unresolved) |
| 3 | `doc`       | string | first line of the docstring, empty otherwise                   |
| 4 | `file`      | string | path **relative to repo root**; empty for externals            |
| 5 | `line`      | u32    | 1-based line number of the function definition; `0` for externals |
| 6 | `kind`      | enum   | `internal` \| `external` \| `unresolved`                       |

The `kind` enum values:

- **`internal`** — the definition resides inside the repo; the walker
  descended into it.
- **`external`** — the definition resolved to a file outside the repo
  (site-packages / stdlib / editable install rooted elsewhere). Leaf node;
  not expanded.
- **`unresolved`** — ty returned zero results, an LSP error, a timeout, or
  the call site is intrinsically dynamic (`getattr(x, name)()`,
  `functools.partial(...)` tails). Synthesized `qname` of the form
  `<unresolved>:<callee_text>@<file>:<line>`.

### `# edges` — column order is load-bearing

| # | column       | type   | meaning                             |
|---|--------------|--------|-------------------------------------|
| 1 | `from`       | string | caller qname                        |
| 2 | `to`         | string | callee qname                        |
| 3 | `annotation` | enum   | `resolved` \| `external` \| `union` \| `unresolved` |

The `annotation` enum values:

- **`resolved`** — a single internal target; the walker enqueued it.
- **`external`** — a single external target (kind=external); leaf, not
  enqueued.
- **`union`** — the call site had N>1 goto-definition results. One edge per
  candidate, all annotated `union`. Consecutive `union` edges sharing a
  `from` qname are a single call-site group (the renderer collapses them
  with a `[union: a | b | ...]` suffix).
- **`unresolved`** — 0 results, error, timeout, or dynamic dispatch. The
  target is always an `<unresolved>:` synthetic node.

### `# entries` — BFS roots

`entries` is a sorted list of the qualified names that were walker entry
points (BFS depth 0). This block is consumed by the renderer to emit
`(entry)` markers; `tyreach render` alone cannot infer it from nodes /
edges. A missing `# entries` block is tolerated (older TOON files) and
yields an unmarked render.

### Row encoding

Values follow standard TOON (v0.4) CSV conventions: bare identifiers
unquoted, strings containing commas / quotes / whitespace double-quoted,
empty strings rendered as `""`. Decode with
[`toon-format`](https://crates.io/crates/toon-format) or any spec-compliant
reader.

## Rendered view (`<prefix>.txt`)

The rendered view is plain UTF-8, designed for diffing and agent prompts.
Shape:

```text
<qname>[  (entry)]
    <signature>
    <docstring first line>
  -> <callee>, <callee>, <callee> [union: a | b]
```

Rules:

- One non-leaf node per block. External and unresolved nodes appear only
  inline as callees (never as standalone blocks).
- Indentation:
  - Node header: 0 spaces.
  - Signature / doc lines: 4 spaces.
  - Callee arrow line: 2 spaces + `-> `.
- Markers:
  - `  (entry)` — appended to the header of any BFS depth-0 node.
  - ` [ext]` — inline suffix on externally-resolved callees.
  - ` [?]` — inline suffix on unresolved callees.
  - ` [union: a | b | ...]` — suffix on a group of union candidates. The
    leading callee name before the bracket is the first candidate.
- When `--budget` truncates the snapshot, signatures and docs are omitted
  (the truncation metadata is carried on-disk in the TOON `truncation`
  field).
- Node ordering: Kahn topological sort, seeded by entry points, with
  score-descending tie-breaks (rank score = BFS-depth-weighted). Cycles
  break gracefully: leftover nodes are appended in score-descending order
  with a `tracing::warn` logged once.

### Example

From [`tests/golden/medium_app.txt`](../tests/golden/medium_app.txt):

```text
medium_app.main.main  (entry)
    def main() -> str
  -> medium_app.a.run_a, medium_app.b.run_b
medium_app.a.run_a
    def run_a() -> str
  -> json.dumps [ext], medium_app.shared.util
medium_app.b.run_b
    def run_b() -> str
  -> medium_app.shared.util
medium_app.shared.util
    def util(arg: str) -> str
```

Note:

- `json.dumps` appears only inline with `[ext]` — it has no standalone block.
- `medium_app.shared.util` is reached via both `run_a` and `run_b` (a
  diamond) but rendered exactly once.
