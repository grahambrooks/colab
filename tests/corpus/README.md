# Test corpus

Each case lives at `tests/corpus/<lang>/<case>/` and contains:

- `script` — the codemod DSL script to run.
- `input/` — source tree the script is applied to (mirrors a real
  project layout).
- `expected/` — the expected source tree after one application.
- `rules.expected` — per-rule attribution after that pass: how many
  files each rule changed (`<count> <rule>`), and for a rule that changed
  nothing, the near-miss values the report would suggest
  (`    near <value> (<n> file(s))`).
- `find.expected` — what `colab find` reports over `input/`:
  `<path>:<line>:<col>: [<rule index>] <matched text>`, followed by the
  metavariable bindings of `<lang>::pattern` rules (`    $NAME = text`).

The harness in `crates/colab-dsl/tests/corpus.rs`:

1. Compiles `script` to a `Refactoring`.
2. For each file in `input/`, applies the transform and asserts the
   result matches the file at the same relative path under `expected/`.
3. Re-applies the transform to its own output and asserts the second
   pass is a no-op (idempotency).
4. Checks every `match` block with `colab_dsl::check` (what the editor
   diagnoses with) and asserts it lowers to the same rule `compile` made.
5. Compares the run's per-rule attribution and near misses with
   `rules.expected`, and `colab find`'s locations with `find.expected`.

The two `.expected` files are generated, not hand-written. After writing
`script`, `input/` and `expected/`, run

```sh
COLAB_BLESS=1 cargo test -p colab-dsl --test corpus
```

and review what it wrote. A `0` in `rules.expected` is a rule that never
fired, which is a bug unless the case is about dead rules
(`go/no_match`, `*/near_miss_targets`).

Cases worth knowing when changing the engine:

- `go/pkg_errors_migration` composes patterns with import rules. It
  covers nested matches (`errors.Wrap(errors.Wrap(…))` must be rewritten
  in one pass), `$$$ARGS`, comments and strings left alone, and the
  gofmt-shaped import edits (`ensure` joins the existing group, `delete`
  leaves no stray blank line).
- `python/pattern_metavars`: a repeated metavariable must bind equal
  text, `$_` is a wildcard, and `in "<glob>"` scopes a pattern.
- `go/near_miss_targets`, `python/near_miss_targets`: dead rules and the
  hints they produce, including a transposed typo (`mian` → `main`).

Adding a new backend is gated on landing at least one case here.
