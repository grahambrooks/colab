# Dogfooding: developing colab with colab

colab exists so that an agent reaches for it instead of `Edit` or `sed`
when a change is mechanical. The cheapest way to find out where it falls
short is to make that choice ourselves, on colab's own Rust code, every
time it fits, and to write down each time it did not.

This applies to anyone changing this repo, human or agent.

## 1. Pin a working binary

```sh
scripts/dogfood.sh        # builds, copies to .dogfood/colab (git-ignored)
alias colab=.dogfood/colab
```

Use the pinned copy, not `cargo run`. That way a half-finished change in
the working tree cannot break the tool you are using to finish it. Re-pin
after a commit whose tests pass, and when you want a feature you just
built.

`rust-analyzer` must be on `PATH` for `rename`, `mv`, `action`,
`references` and `safe-delete`. The first call in a session takes about
7 seconds while it indexes the workspace.

## 2. Choose the tool by the shape of the change

| The change | Use | Not |
| ---------- | --- | --- |
| The same edit at many sites (a new trait method in every backend, a renamed helper's call sites, a signature change) | `colab refactor` with `rust::pattern`, scoped with `in "crates/colab-lang-go/**"` when sites differ | 12 `Edit`s |
| Find every place shaped like X (all `build_rule` arms for `pattern`, all `impl LanguageBackend`) | `colab find` | `rg` that also matches comments and strings |
| Who uses this function or type before I change it | `colab references --symbol X --in FILE` | `rg -w X` |
| Rename a function, type, field or module across crates | `colab rename --at FILE:L:C --to NEW` | `symbol` rename or `sed` |
| Move or rename a source file | `colab mv FROM TO` (git mv plus `mod` and path updates) | `git mv` plus hand-edits |
| Delete code you believe is dead | `colab safe-delete --symbol X --in FILE` | deleting and waiting for the compiler |
| Extract a function or variable, inline, change a signature | `colab action --at FILE:L:C-L:C --list`, then `--kind` / `--title` | retyping the code |
| A `use` path moved | `match rust::use "old::path" { replace "new::path" }` | |
| A dependency or version in `Cargo.toml` | `match toml::key "workspace.dependencies.regex" { set '"1"' }` | |
| One edit in one place | `Edit` | colab, which buys nothing there |

When colab would fit and you use something else anyway, that is a
finding. Log it (section 5).

## 3. The loop

Look before you change anything. Write the rule into a heredoc so the
shell leaves `$` alone:

```sh
colab find --script - crates <<'EOF'
refactor "probe" {
  match rust::pattern 'fn syntax_tree(&self, $I: $T) -> $R { $$$B }' { delete }
}
EOF
```

`find` shows each match and its bindings. When the match count is right,
preview the change as a diff:

```sh
colab refactor --format diff --script - crates <<'EOF'
refactor "add import_module" {
  match rust::pattern 'fn description(&self) -> $RET { $$$B }' in "crates/colab-lang-go/**" {
    replace 'fn description(&self) -> $RET {
    $$$B
}

fn import_module(&self) -> Option<$RET> {
    Some("import")
}'
  }
}
EOF
```

Then apply it with a build check that undoes the change if the build
fails:

```sh
colab refactor --write --bisect 'cargo check -q --workspace --tests' --script - crates <<'EOF'
…
EOF
```

Choose the check flag by how the rules relate:

- **`--bisect CMD`** applies the whole script, then runs `CMD` once. If it
  fails, it searches for the rule that broke it and reverts everything.
  Use it for a coordinated change, where the rules only compile together
  (a new parameter, plus every caller that now passes it).
- **`--verify CMD`** runs `CMD` after *each* rule, and stops and reverts
  that rule at the first failure. Use it only when every rule stands on
  its own. On a coordinated change it always fails at the first rule.

`colab undo` reverts the last run. Run `cargo fmt` afterwards, because
`--tidy` does not cover `refactor` yet.

For the language-server commands, preview first: they are dry runs unless
you pass `--write`.

```sh
colab references --symbol build_rule --in crates/colab-core/src/backend.rs
colab rename --at crates/colab-core/src/backend.rs:164:8 --to build_operation --format diff
colab rename --at … --to build_operation --write --tidy --verify 'cargo check -q --workspace'
```

## 4. Writing Rust patterns

- **Write templates flush-left, in their final multi-line shape.** colab
  re-indents every line after the first to the column of the match. A
  template written on one line comes out on one line.
- **Lifetimes.** DSL strings have no escapes, so a single-quoted pattern
  cannot contain `'static`. Capture the type instead, and reuse the
  capture in the template: `-> $RET` binds `&'static str`, and
  `Option<$RET>` writes `Option<&'static str>`.
- **A template needing both `'` and `"`** cannot be written today. Find a
  capture that carries one of them, or fall back to `Edit` and log it.
- **`$$$B` for a body.** `{ $$$B }` matches a block of any length.
  `$_` matches one node you do not need to bind.
- **When a pattern matches nothing**, check its tree first:
  `colab ast --lang rust --pattern '…'`. Then run the same command with
  `--stdin` on the code you expected it to match, and compare the two
  trees.
- **Scope per crate.** Backends differ in small ways, such as the import
  module's name. Write one rule per crate with `in "crates/colab-lang-X/**"`,
  in a single script.

## 5. Log what got in the way

Add an entry to [`dogfood-log.md`](dogfood-log.md) whenever colab:

- could not express the change;
- produced output that needed hand-fixing;
- was slower or noisier than the alternative; or
- was skipped for `Edit` or `sed` on a change it should have handled.

Each entry records the date, the task, the command you ran, what went
wrong, and its outcome: fixed in a named commit, scheduled to a named
milestone, or "won't fix" with the reason. A small fix goes into the same
commit as the work that found it.

To see what colab saved, price the diff it produced as `Read` + `Edit`
calls and compare with the bytes of the colab commands and their output:

```sh
git diff -U0 > /tmp/change.diff
scripts/price-edits.py --rev HEAD /tmp/change.diff
```

The [token cost table](dogfood-log.md#token-cost-m27--m22) has the
results so far: about 77% saved overall, more as sites multiply.

When colab did the work in a commit, say so in the commit body, e.g.
`colab refactor (rust::pattern, 12 sites)`. These notes are the raw
material for the M23 eval suite.
