# Architecture

Colab is a Cargo workspace that compiles a small DSL (the `.codemod`
language) into a list of language-specific tree-sitter rewrites and
applies them deterministically to a filesystem tree. This document
describes the workspace layout, the runtime data flow, and the steps
to add a new backend, namespace, or action.

For language-level information see [`docs/dsl.md`](docs/dsl.md);
for runtime CLI behaviour see [`docs/cli.md`](docs/cli.md); for the
capability matrix see [`docs/features.md`](docs/features.md).

## Workspace layout

```
crates/
  colab-core/         # Error/Result, CodeTransformer, walker,
                      # LanguageBackend trait + Operation +
                      # BackendRegistry, RuleSpec, template engine.
                      # No internal deps.
  colab-dsl/          # LALRPOP grammar, AST, compiler, Refactoring IR.
                      # Depends only on colab-core; never on a backend.
  colab-lang-go/      # Go backend (tree-sitter-go).
  colab-lang-java/    # Java backend (tree-sitter-java).
  colab-lang-js/      # JS/TS backend (tree-sitter-javascript).
  colab-lang-python/  # Python backend (tree-sitter-python).
  colab-lang-rust/    # Rust backend (tree-sitter-rust + toml_edit).
  colab-mcp/          # Model Context Protocol server (preview /
                      # apply / schema / lint_script as MCP tools).
                      # JSON-RPC 2.0 over stdio with Content-Length
                      # framing. Depends only on colab-core +
                      # colab-dsl; like colab-dsl, must not pull in
                      # any colab-lang-* crate at runtime.
  colab-cli/          # `colab` binary; assembles the default
                      # BackendRegistry, owns the LSP server,
                      # launches the MCP server via `colab mcp`.
tests/corpus/         # Per-language end-to-end fixtures.
examples/             # Runnable scripts and migration packs.
docs/                 # User-facing reference.
```

The crucial rule: **`colab-dsl` must not depend on any
`colab-lang-*` crate at runtime.** Backends are passed in via
`BackendRegistry` at compile time. This lets a broken backend ship
without blocking unrelated work and makes a future MCP / WASM
plugin story tractable.

## Data flow

```
        ┌────────────────────────────────────────────────┐
script  │                colab_cli::cli                  │
text ─▶ │  parse args  →  read script  →  drive run      │
        └───────────────┬────────────────────────────────┘
                        │  &BackendRegistry  (registered backends)
        ┌───────────────▼────────────────────────────────┐
        │                colab_dsl                       │
        │                                                │
        │  parse(text)                                   │
        │      │                                         │
        │      ▼                                         │
        │  ast::Command  (raw AST, n match blocks)       │
        │      │                                         │
        │      ▼                                         │
        │  compile(ast, &registry)                       │
        │      │   for each Match → registry.get(lang)   │
        │      │              .build_rule(module, spec)  │
        │      ▼                                         │
        │  Refactoring { rules: Vec<Box<dyn Operation>> }│
        │  (impls CodeTransformer)                       │
        └───────────────┬────────────────────────────────┘
                        │
        ┌───────────────▼────────────────────────────────┐
        │              colab_core::walker                │
        │  walk(transformer, root, &mut visitor)         │
        │  – sorted directory traversal                  │
        │  – yields FileChange { path, before, after }   │
        └───────────────┬────────────────────────────────┘
                        │
        ┌───────────────▼────────────────────────────────┐
        │               colab_cli::format                │
        │  reporter chosen by --format:                  │
        │    HumanReporter / JsonReporter / DiffReporter │
        │  visitor decides --write / --dry-run / --check │
        └────────────────────────────────────────────────┘
```

## Key abstractions

### `CodeTransformer`

```rust
pub trait CodeTransformer {
    fn is_file_relevant(&self, path: &Path) -> bool;
    fn apply(&self, source_code: &str) -> String;
}
```

The walker only knows about this trait. `Refactoring` (in
`colab-dsl`) implements it by composing every rule's `apply`
left-to-right.

**Invariant:** `apply` returns its input unchanged when there is
nothing to rewrite. The walker uses string equality with the input
to skip writes; rule composition relies on irrelevant rules being
identity (a Go rule sees Rust source via the multi-rule walker and
must not modify it).

### `LanguageBackend` and `Operation`

```rust
pub trait LanguageBackend: Send + Sync {
    fn lang(&self) -> &'static str;
    fn description(&self) -> &'static str { "" }
    fn capabilities(&self) -> &'static [Capability];
    fn build_rule(&self, module: &str, spec: RuleSpec) -> Result<Box<dyn Operation>>;
}

pub trait Operation: fmt::Debug + fmt::Display + Send + Sync {
    fn is_file_relevant(&self, path: &Path) -> bool;
    fn apply(&self, source_code: &str) -> String;
}
```

A backend advertises its `capabilities()` (modules + supported
actions) for `colab schema`/`list-rules` and lowers a parsed match
block into a concrete `Operation`. `colab-dsl` calls
`LanguageBackend::build_rule` via the registry; it has no
compile-time knowledge of any backend.

### `RuleSpec`

The backend-neutral lowered form of a match block:

```rust
pub enum RuleSpec {
    Replace { target: String, replacement: String },
    Delete  { target: String },
    Ensure  { target: String },
    ReplaceCall { target: String, template: String },
}
```

Adding a new action variant means: AST + grammar token, lower in
`compiler::lower_match`, JSON serializer in
`colab_cli::discover::explain`, and at least one backend that
implements it.

### `BackendRegistry`

```rust
pub struct BackendRegistry { /* … */ }
impl BackendRegistry {
    pub fn register(&mut self, backend: Box<dyn LanguageBackend>);
    pub fn get(&self, lang: &str) -> Option<&dyn LanguageBackend>;
    pub fn languages(&self) -> Vec<&'static str>;
}
```

`colab-cli::cli::default_backends` is the single place where every
backend is registered. New `colab-lang-*` crates plug in here.

### `FileChange` (walker visitor)

```rust
pub struct FileChange { pub path: PathBuf, pub before: String, pub after: String }
walker::walk(transformer, path, &mut |change| { … });
```

The visitor closure decides whether to write back, format a diff,
emit JSON, or just count for `--check`. Directory entries are
sorted before iteration so output ordering is deterministic across
filesystems.

## Adding a new backend

To add `colab-lang-<x>` (replace `<x>` with `kotlin`, `swift`, etc.):

1. **New crate.** `crates/colab-lang-<x>/` with `Cargo.toml`,
   `src/lib.rs`. Add to the workspace `members` list and the
   workspace `[workspace.dependencies]` table.
2. **Tree-sitter grammar.** Add the grammar dep
   (e.g. `tree-sitter-kotlin = "0.X"`) to the workspace deps.
3. **Backend struct + capabilities.** Implement `LanguageBackend`:
   declare the namespace name, write a `Capability` array describing
   every `(module, action)` pair, and write `build_rule` to dispatch
   to your `Operation` impls.
4. **Operations.** One module per namespace
   (`src/imports.rs`, `src/symbols.rs`, …). Each Operation impl is
   a struct that holds the rule's parameters, plus an `apply`
   method that uses tree-sitter to compute byte-range edits and
   applies them in reverse byte order.
5. **Register.** Add `registry.register(Box::new(<X>Backend));` to
   `crates/colab-cli/src/cli.rs::default_backends`. Add
   `colab-lang-<x>` to `colab-cli/Cargo.toml` `[dependencies]` and
   to `colab-dsl/Cargo.toml` `[dev-dependencies]` (for the corpus
   harness).
6. **Corpus.** At least one fixture under
   `tests/corpus/<lang>/<case>/{script,input/,expected/}`. The
   harness in `crates/colab-dsl/tests/corpus.rs` will pick it up
   automatically and run it (and assert idempotency).
7. **Docs.** Add a row to `docs/features.md` and update the
   capability table in `README.md`.

CI (`.github/workflows/ci.yml`) runs `cargo build/test/clippy
--workspace`, so a broken backend is caught immediately.

## Adding a new namespace or action to an existing backend

- **New module under an existing language:** drop a new `src/<m>.rs`,
  add `Operation` impls, add a `Capability` row to that backend's
  `CAPABILITIES`, add an arm to `build_rule`, add a corpus case.
- **New action verb (in addition to `replace`/`delete`/`ensure`/
  `replace_call`):** extend `Action` in
  `crates/colab-dsl/src/ast.rs`, add the token to
  `codemod.lalrpop`, add the matching `RuleSpec` variant in
  `colab_core::backend`, lower in
  `colab_dsl::compiler::lower_match`, serialize in
  `colab_cli::discover::explain`, then implement in whatever
  backends should support it.

## Invariants (and why)

- **`CodeTransformer::apply` returns input unchanged when nothing
  matches.** The walker uses equality to skip writes; multi-rule
  composition treats irrelevant rules as identity.
- **`colab-dsl` never depends on a backend at runtime.** Keeps the
  CI matrix per-backend isolated.
- **Tree-sitter edits are applied in reverse byte order.** Earlier
  offsets stay valid as later ones are replaced.
- **Path matching prefers exact equality on tree-sitter node text.**
  Substring matching (`"another.module"` matching
  `"yet.another.module"`) breaks idempotency under composition. The
  Go imports rewriter uses field-level exact match for this reason.
- **Every transform must be idempotent on its own output.** The
  corpus harness asserts this by re-applying every rule. Templates
  in `replace_call` that keep the function name violate this and
  must not be added to corpus cases.
- **Unknown namespaces fail loudly.** `BackendRegistry::get`
  returning `None` becomes `Error::UnsupportedOperation` (CLI exit
  3). Backends do the same for unknown modules within their
  language.

## Error handling

Every fallible operation returns `colab_core::Result<T>`. I/O
errors are wrapped with `Error::io_at(path, source)`. The CLI maps
each error variant to a documented exit code (1/2/3/4) and emits
one `error!` log line. Library code never panics on user input;
`expect`/`panic!` is reserved for genuinely unreachable conditions
(e.g. failing to load a built-in tree-sitter grammar).

## Build script invariants

- `crates/colab-dsl/build.rs` runs `lalrpop::process_root()`.
  Editing `src/codemod.lalrpop` requires a rebuild — the generated
  `codemod.rs` lives in `OUT_DIR`, not the source tree.
- `crates/colab-cli/build.rs` shells out to
  `git rev-parse --short HEAD` to embed a version suffix. Builds
  must happen inside a git checkout.

## Testing layers

```
unit tests            in each crate's src/
integration tests     crates/colab-cli/tests/cli.rs (assert_cmd)
DSL unit tests        crates/colab-dsl/src/compiler.rs (parser)
end-to-end corpus     crates/colab-dsl/tests/corpus.rs +
                      tests/corpus/<lang>/<case>/{script,input/,expected/}
```

The corpus harness is the canonical "did the new feature break
anything" test: it walks every case, applies the script, diffs
against `expected/`, then re-applies and asserts idempotency.
