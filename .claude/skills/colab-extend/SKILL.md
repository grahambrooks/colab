---
name: colab-extend
description: Add a capability to the colab codebase itself — a new language backend, or a new module/action on an existing one. Use when working inside the colab repo on colab-lang-* crates, the LanguageBackend/Operation traits, the DSL grammar, or the walker. Encodes the invariants (per-rule gating, prefilter soundness, idempotency, corpus coverage) that a change here must not break.
---

# Extending colab

This is for changing colab itself, not for running a codemod (that is
`colab-codemod`). It covers the two shapes an extension takes: a new
namespace on an existing backend, and a whole new language.

## Architecture in one paragraph

`colab-dsl` parses a script into an AST and lowers each `match` block
into a `Box<dyn Operation>` by asking a `BackendRegistry` — so it has
**no compile-time knowledge of any backend**. `colab-core::walker` drives
the resulting `Refactoring` over the filesystem, calling
`CodeTransformer::apply_at(path, source)` per file. Each
`colab-lang-*` crate owns one `LanguageBackend` and its `Operation`
impls. `colab-cli` is the only place that knows all backends exist.

The hard rule: **`colab-dsl` must never depend on a `colab-lang-*` crate
at runtime** (dev-dependency for tests only). The CI matrix relies on it
so one broken backend cannot block unrelated work.

## Adding a module or action to an existing backend

Four places, in order:

1. **The `Operation` impl** — a new struct in the right module of the
   `colab-lang-*` crate (e.g. `crates/colab-lang-go/src/imports.rs`).
   Implement `is_file_relevant`, `apply`, and `prefilter`.
2. **`build_rule`** — a match arm in that crate's `LanguageBackend`
   mapping `(module, RuleSpec)` to your operation.
3. **`capabilities()`** — add the module/action so `colab schema`,
   `colab list-rules`, and LSP completion advertise it. Discovery is
   generated from this; there is no second list to update.
4. **A corpus case** under
   `tests/corpus/<lang>/<case>/{script,input/,expected/}`.

Nothing in `colab-dsl`, `walker.rs`, or `cli.rs` should need to change.
If you find yourself editing those, the design is fighting you — say so
rather than plumbing a special case through.

## Adding a new language backend

Same as above, plus:

- a new `crates/colab-lang-<name>/` crate depending on `colab-core` and
  its tree-sitter grammar;
- one `registry.register(Box::new(<Backend>))` line in
  `colab-cli/src/cli.rs::default_backends` — **the only** line the
  binary needs;
- workspace member + dependency entries in the root `Cargo.toml`;
- at least one corpus case, which is a merge gate.

Add the crate to `docs/features.md` and the tables in `CLAUDE.md` /
`docs/architecture.md` in the same change.

## The invariants your code must satisfy

### `apply` is identity when there is nothing to do

The walker uses string equality with the input to decide whether to
write. Returning a rebuilt-but-identical string is not merely wasteful —
it defeats the skip.

### `prefilter` must be a *necessary* condition, or `None`

```rust
fn prefilter(&self) -> Option<&str> { Some(&self.from) }
```

The walker skips the tree-sitter parse entirely when this literal is
absent from the source. The contract is one-directional: **its absence
must imply a no-op.** An over-broad literal costs performance; a too-
narrow one is a correctness bug.

Concretely: return a literal only if you are sure the operation cannot
change a file that does not contain it. Every current matcher is exact
equality or a prefix on tree-sitter node text, so the target appears
contiguously in the source — that is what makes returning `self.from`
sound. If you write a matcher that normalizes, reformats, or reassembles
before comparing, return `None`.

**`ensure`-style operations must return `None`.** They act precisely
when the target is *absent*; a prefilter would suppress them in exactly
the files that need the insert. There is a regression test for this in
`crates/colab-dsl/src/model.rs`.

Where the literal is not contiguous, pick the discriminating part that
is. `go::struct_tag` matches `key:"value"` in the source but its match
string is `key:value`, so it returns the value:

```rust
fn prefilter(&self) -> Option<&str> {
    self.from.split_once(':').map(|(_, value)| value)
}
```

### Transforms must be idempotent

Applying twice must equal applying once. The corpus harness re-applies
every script and fails if the second pass diverges — which also makes it
the guard against a too-narrow prefilter (a rule that skipped work on
pass one but not pass two shows up here).

Prefer exact equality on the relevant tree-sitter node over substring
matching. Substring matches break composition: `"another.module"`
matching `"yet.another.module"` makes two rules interfere.

`replace_call` templates that keep the function name are inherently
non-idempotent and must not appear in a corpus case.

### Tree-sitter edits are applied in reverse byte order

Collect `(start, end, replacement)` edits, sort by start, then apply
in reverse so earlier offsets stay valid. See
`crates/colab-lang-go/src/imports.rs` for the canonical shape; follow it
in any new rewriter.

### Unknown names fail loudly and name the alternatives

Use the provided helper in your `build_rule` catch-all:

```rust
(other, spec) => Err(self.unsupported(other, &spec)),
```

`LanguageBackend::unsupported` distinguishes a misspelled *module* from
a valid module asked for an unsupported *action*, lists the valid names,
and adds a "did you mean" via `colab_core::suggest`. Do not hand-roll
the message, and never format a `RuleSpec` with `{:?}` — that leaks Rust
internals and buries the useful part.

### A rule that matches nothing is a reportable event

`RunReport` carries per-rule match counts and every output surface warns
on `files_matched == 0`. If you add a new way to run rules, keep the
attribution intact — `rules_fired` in `ApplyOutcome` is what feeds it.

### Output has one serializer

`colab-core::render` is what both `colab-cli/src/format.rs` and
`colab-mcp` use. Do not add a second, surface-specific shape; the two
surfaces drifting is the problem it exists to prevent.

## Adding a DSL action or clause

Grammar changes ripple further than backend changes. All of:

1. `crates/colab-dsl/src/codemod.lalrpop` — the grammar. Editing this
   triggers a rebuild via `build.rs`; generated code lands in `OUT_DIR`.
2. `crates/colab-dsl/src/ast.rs` — the AST node.
3. `colab_core::RuleSpec` + `action_name()` — the backend-neutral form.
4. `compiler::lower_match` — AST to `RuleSpec`.
5. `colab_cli::discover::explain` — the JSON IR serializer.
6. At least one backend that implements it.
7. `docs/dsl.md`, including the reserved-token list if you added a
   keyword.

## Verifying

```sh
cargo test --workspace
cargo test -p colab-dsl --test corpus                    # corpus only
cargo clippy --workspace --all-targets -- -D warnings    # the CI gate
cargo bench -p colab-cli --bench walker                  # if you touched the hot path
```

The bench includes an `Ungated` wrapper that reproduces the pre-gating
execution model, so `5 rules, 2 languages` vs `5 rules, ungated` shows
what the gate and prefilter are buying. If your change makes those
converge, gating has regressed.

Make a new corpus case earn its place: confirm it *fails* without your
change. A case that passes either way tests nothing.

## Checklist before you call it done

- [ ] `Operation` impl with a sound `prefilter` (`None` for `ensure`)
- [ ] `build_rule` arm, catch-all delegating to `self.unsupported`
- [ ] `capabilities()` updated — drives schema, `list-rules`, completion
- [ ] `registry.register(...)` in `default_backends` (new backend only)
- [ ] Corpus case that fails without the change
- [ ] Idempotent: corpus second pass is a no-op
- [ ] A runnable example under `examples/` (required by `CLAUDE.md`)
- [ ] `docs/dsl.md` updated; `docs/features.md` if the matrix changed
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean

## Error-handling conventions

All fallible code returns `colab_core::Result<T>`. Wrap I/O errors with
`Error::io_at(path, source)` so the offending path is in the message.
Library code never panics on user input; reserve `expect`/`panic!` for
genuinely unreachable conditions (loading a built-in tree-sitter
grammar) and `#[cfg(test)]`.
