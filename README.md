# tyreach

`tyreach` produces a ranked, token-budgeted reachability snapshot of Python
symbols in a uv-managed monorepo — one file for coding agents to read before
they touch your codebase.

It walks the call graph from your entry points using
[tree-sitter](https://tree-sitter.github.io/) for call-site extraction and
the [ty](https://github.com/astral-sh/ty) language server for symbol
resolution. The output is a flat `nodes + edges` table in
[TOON](https://github.com/toon-lang/toon) plus a topologically-sorted rendered
text view, scoped to your repo and stopping at site-packages.

## Install

```sh
pip install tyreach            # stable wheel
uvx tyreach snapshot           # one-off via uv
```

Prerequisites:

- Python `>= 3.10`.
- `ty` on `PATH`. If you do not have it system-wide, `tyreach` will fall back
  to `uvx ty server`, so as long as `uvx` is available you are fine. Alternatively:

  ```sh
  uv add --dev ty
  ```

## Quickstart

Inside a repo with a `[project.scripts]` block in `pyproject.toml`:

```sh
cd myrepo
tyreach snapshot
```

Writes `<script>.toon` (canonical TOON) and `<script>.txt` (rendered view)
side-by-side, where `<script>` is the first entry's name — the key from
`[project.scripts]` (so a `myapp = "myapp.cli:main"` block produces
`myapp.toon` / `myapp.txt`). Override with `--out PREFIX` to pick an
explicit prefix. A typical rendered snippet:

```text
myapp.cli.main  (entry)
    def main() -> int
    Kick off a short workflow.
  -> myapp.core.Service, myapp.core.process, myapp.diamond.a.combined
myapp.diamond.a.combined
    def combined() -> int
  -> myapp.diamond.a.via_a, myapp.diamond.b.via_b
myapp.dynamic.dispatch
    def dispatch(target, name)
  -> getattr(target, name)() [?]
```

Pipe the rendered view to stdout instead of writing files:

```sh
tyreach snapshot --stdout
```

Re-render a previously written `.toon`:

```sh
tyreach render myapp.toon
```

## Configuration

### Entry points

`tyreach` picks entry points from three sources, highest precedence first:

1. `--entry path/to/file.py::func` on the command line (repeatable).
2. `tyreach.toml` in the repo root.
3. `[project.scripts]` in `pyproject.toml`.

#### `tyreach.toml` schema

```toml
# tyreach.toml — one entry per BFS root.

[[entries]]
name = "cli"                   # display name; used for output filenames
entry_file = "myapp/cli.py"    # resolved relative to the repo root
function = "main"              # optional; defaults to "main"

[[entries]]
name = "worker"
entry_file = "myapp/worker.py"
function = "run"
```

#### CLI entry-point paths

`--entry` paths are resolved **relative to the `repo_root` positional
argument, not the current working directory.** This matters when invoking
`tyreach` from outside the repo — for example:

```sh
# From any cwd:
tyreach snapshot --entry myapp/cli.py::main /path/to/repo
```

`myapp/cli.py` above is joined against `/path/to/repo/`, yielding
`/path/to/repo/myapp/cli.py`. Absolute paths are used as-is.

### CLI flags summary

```text
tyreach snapshot [REPO]
    --entry path/to/file.py::func   (repeatable; wins over tyreach.toml / pyproject)
    --budget N                      (token budget, default 2000)
    --out PREFIX                    (writes PREFIX.toon and PREFIX.txt)
    --no-render                     (skip PREFIX.txt)
    --stdout                        (render to stdout instead of files)

tyreach render [INPUT]              (re-render a TOON snapshot; stdin if omitted)
```

## Output format

The canonical output is TOON (tabular schema + CSV-like rows); see
[docs/output-format.md](docs/output-format.md) for the exact column order,
enum values, and rendered-view conventions. Both the TOON and rendered
outputs are stable across patch releases.

## Known limitations

- Top-level module calls (not inside a function body) are deliberately out of
  scope in v1. Wrap module-top logic in a function and add it as an entry.
- `functools.partial(f, x)` wrappers are typically unresolved — ty sees the
  wrapper, not `f`.
- Dockerfile entry-point parsing is deferred to v1.1. Use `tyreach.toml` or
  `--entry` to express non-Python entrypoints.
- Namespace packages (PEP 420) are best-effort — if you rely on them, put
  `__init__.py` files in for now.
- Un-annotated `self.x()` with no type hint may surface as unresolved. Add
  a return type hint or annotate `self`'s type if you want a resolved edge.
- Heavily decorated wrappers show whatever ty resolves them to; we do not
  second-guess the language server.

The walk always terminates — any LSP error or timeout degrades to an
`unresolved` edge plus a warning, never a panic.

## Contributing / development

See [CONTRIBUTING.md](CONTRIBUTING.md). TL;DR: `make review` before pushing.
Pre-commit hooks are declared in `prek.toml`.

## License

MIT. See [LICENSE](LICENSE).
