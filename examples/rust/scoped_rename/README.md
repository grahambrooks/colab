# Scope a symbol rename to one part of the tree

`<lang>::symbol` is a syntactic rename: it rewrites every identifier node
whose text matches, in every file it is given, with no scope analysis. That
is fine when the name is unique, and wrong the moment two crates happen to
share one. The `in "<glob>"` clause narrows a rule to the paths that should
be touched.

This example has two `Config` types — one in `core/`, one in `cli/` — that
are unrelated apart from the name.

```sh
# From the repo root. Start with the blast radius, not the diff:
colab refactor --script examples/rust/scoped_rename/scoped.codemod \
    --format json examples/rust/scoped_rename
```

```json
{"changed":["examples/rust/scoped_rename/core/src/lib.rs"],
 "rules":[{"files":1,"i":0,"rule":"rust::symbol \"Config\" -> \"CoreConfig\" in \"**/core/**\""}],
 "summary":{"changed":1,"scanned":1,"visited":4,"skipped":0,"elapsed_ms":5,...}}
```

Read the three counters together: 4 files were visited, 1 was scanned, 1
changed. The scope did not merely suppress an edit — `cli/src/main.rs` was
never parsed at all, because a scoped rule reports itself irrelevant for
paths outside its glob. The README and the `.codemod` script account for the
other two visited-but-not-scanned files.

`rules[0].files` is 1, so the rule is doing something. A rule reported with
`"files": 0` matched nothing and is almost certainly wrong — that is the
check worth making before `--write`.

Drop the `in "**/core/**"` clause and re-run: `scanned` and `changed` both
become 2, and `cli/src/main.rs` is renamed along with it. That is the failure
the scope prevents.

To see the hunks for the one file that matters:

```sh
colab refactor --script examples/rust/scoped_rename/scoped.codemod \
    --format diff examples/rust/scoped_rename
```

## Glob syntax

Matched against the path as the walker yields it — relative to where you
invoke `colab`, so `-C` and the target paths both affect what a glob sees.

| Pattern | Matches |
| ------- | ------- |
| `core/**` | everything under a top-level `core/` |
| `**/core/**` | everything under any `core/` directory at any depth |
| `src/*.rs` | Rust files directly in `src/`, not in subdirectories |
| `**/*_test.rs` | test files anywhere |

`*` stops at a path separator; `**` is what crosses directories. An invalid
glob fails at compile time (exit code 1) rather than silently matching
nothing.

## Scope vs `--include`

`--include` filters the whole run; `in` scopes a single rule. Use `--include`
to narrow what colab looks at, and `in` when one rule in a multi-rule script
needs a tighter boundary than the others.
