# tyreach

`tyreach` produces a ranked, token-budgeted reachability snapshot of Python
symbols in a uv-managed monorepo. Starting from entry points, it BFS-walks the
call graph using tree-sitter to extract call sites and the ty LSP to resolve
each call site to a definition, scoped to the repo and stopping at
site-packages. The output is a flat `nodes + edges` table in TOON plus a
topologically-sorted rendered text view, intended to orient coding agents in
an unfamiliar codebase.

See [docs/plans/plan.md](docs/plans/plan.md) for the full specification.
