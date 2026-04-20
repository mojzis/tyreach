# Contributing

Short version:

- We write failing tests first when we can. The existing test suite leans on
  integration-style tests over heavy mocking.
- `make review` is the single pre-push gate. It chains `cargo fmt --check`,
  `cargo clippy --all-targets --all-features -- -D warnings`,
  `cargo test --all-features`, `cargo audit`, and `cargo deny check`.
  Run it before pushing anything.
- `make review-quick` skips the network-dependent `audit` and `deny` steps
  when iterating locally.
- Pre-commit hooks live in [`prek.toml`](prek.toml). Install them with
  [`prek`](https://github.com/j178/prek) and let them run on every commit —
  do not bypass them.
- Tests touching ty LSP are gated on `TY_AVAILABLE=1`. `ty` must be on
  `PATH` or installable via `uvx ty server`.

More targets live in the [`Makefile`](Makefile) (`coverage`, `mutants`,
`update-golden`). They are intentional opt-ins, not part of the default
review cycle.

## Code conventions

- Edition 2021, `rustfmt.toml` controls formatting (max width 100,
  `use_small_heuristics = "Max"`).
- No `.unwrap()` in non-test code; use `anyhow::Context`.
- No `MutexGuard` held across `.await`.
- Prefer borrowed types in function signatures (`&str`, `&[T]`, `&Path`).
- Tests assert on values, not on "doesn't panic".

## Reporting issues

Bug reports are easier to triage when they include the minimal reproducer
(even a single Python file) plus the emitted TOON snapshot if there is one.
