# Test corpus

Each case lives at `tests/corpus/<lang>/<case>/` and contains:

- `script` — the codemod DSL script to run.
- `input/` — source tree the script is applied to (mirrors a real
  project layout).
- `expected/` — the expected source tree after one application.

The harness in `crates/colab-dsl/tests/corpus.rs`:

1. Compiles `script` to a `Refactoring`.
2. For each file in `input/`, applies the transform and asserts the
   result matches the file at the same relative path under `expected/`.
3. Re-applies the transform to its own output and asserts the second
   pass is a no-op (idempotency).

Adding a new backend is gated on landing at least one case here.
