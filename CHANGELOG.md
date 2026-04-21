# Changelog

All notable changes to `tyreach` are captured here. Dates are ISO-8601; the
format follows [Keep a Changelog](https://keepachangelog.com/) loosely.

## [Unreleased]

- Added: 'tyreach setup' subcommand — read-only diagnostic that reports which entry-point source (tyreach.toml / pyproject.toml) is active in a repo and prints a ready-to-copy tyreach.toml skeleton when none is found. Errors when the repo path does not exist or is not a directory (so a typo is not silently indistinguishable from an empty repo), and renders a malformed tyreach.toml as a diagnostic row rather than aborting.
- Changed: enriched 'tyreach --help' with a setup walkthrough so coding agents can bootstrap tyreach in an unfamiliar repo from the help output alone.

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
