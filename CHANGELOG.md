# Changelog

All notable changes to `tyreach` are captured here. Dates are ISO-8601; the
format follows [Keep a Changelog](https://keepachangelog.com/) loosely.

## 0.1.0 — 2026-04-20

First shippable release. What works:

- BFS call-graph walker composing tree-sitter call-site extraction with
  ty LSP goto-definition. Cycles, diamonds, recursion all terminate.
- Classification: each call site resolves to exactly one of `resolved`,
  `external`, `union`, `unresolved`. LSP errors and timeouts degrade to
  `unresolved`; the walk never panics.
- Entry points from three sources, precedence
  `--entry > tyreach.toml > pyproject.toml [project.scripts]`.
- Ranking + token budgeting: nodes dropped in ascending score order until
  the snapshot fits the `--budget`.
- TOON canonical output (`nodes`, `edges`, `entries` blocks) plus a
  topologically-sorted rendered text view. `tyreach render` round-trips
  bit-identically.
- `tyreach snapshot --stdout` for one-shot piping.
- Packaged as a maturin-built wheel; wheels matrix-built for
  Ubuntu / macOS / Windows via GitHub Actions with PyPI Trusted Publishing.
