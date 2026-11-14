# Architecture

Colab is a Cargo workspace that compiles a small DSL (the `.codemod`
language) into a list of language-specific tree-sitter rewrites and
applies them deterministically to a filesystem tree. This document
describes the workspace layout, the runtime data flow, and the steps
to add a new backend, namespace, or action.

For language-level information see [`docs/dsl.md`](dsl.md);
for runtime CLI behaviour see [`docs/cli.md`](cli.md); for the
capability matrix see [`docs/features.md`](features.md).

## Workspace layout

```
crates/
  colab-core/         # Error/Result (+ ParseDetail), CodeTransformer,
                      # walker, LanguageBackend trait + Operation +
                      # BackendRegistry, RuleSpec, template engine,
                      # RunReport (per-rule attribution), render (the
                      # shared JSON serializer), ScopedOperation,
                      # suggest ("did you mean"), and the generic
                      # pieces every backend inherits: constraint
                      # (`where` data), fences, prose, followup (import
                      # clauses), ignore (`colab:ignore`), near_miss,
                      # find, fileops. No internal deps.
  colab-dsl/          # LALRPOP grammar, AST, compiler, Refactoring IR.
                      # Depends only on colab-core; never on a backend.
  colab-rewrite/      # Tree-sitter rewriting primitives shared by all
                      # backends (apply_edits, line_span, delete_lines,
                      # visit_all, rename_nodes_by_text, rename_conflict),
                      # `calls` (replace_call for the backends whose calls
                      # have `function` / `arguments` fields)
                      # and `pattern`, the structural matcher behind
                      # every <lang>::pattern (wraps ast-grep-core; also
                      # evaluates `where`; build_rule_where). Depends on
                      # tree-sitter, ast-grep-core, regex and
                      # colab-core; colab-core never depends on it, so
                      # colab-dsl and colab-mcp never inherit a grammar
                      # engine.
  colab-backends/     # The single composition root: registry() wires
                      # every backend. Runtime dep of colab-cli, dev-only
                      # for colab-dsl / colab-mcp. Home of the
                      # cross-backend contract tests.
  colab-lang-c/       # C backend (tree-sitter-c).
  colab-lang-cpp/     # C++ backend (tree-sitter-cpp).
  colab-lang-csharp/  # C# backend (tree-sitter-c-sharp).
  colab-lang-go/      # Go backend (tree-sitter-go).
  colab-lang-java/    # Java backend (tree-sitter-java).
  colab-lang-js/      # JS/TS backend (tree-sitter-javascript).
  colab-lang-kotlin/  # Kotlin backend (tree-sitter-kotlin-ng).
  colab-lang-php/     # PHP backend (tree-sitter-php).
  colab-lang-python/  # Python backend (tree-sitter-python).
  colab-lang-ruby/    # Ruby backend (tree-sitter-ruby).
  colab-lang-rust/    # Rust backend (tree-sitter-rust + toml_edit).
  colab-lang-swift/   # Swift backend (tree-sitter-swift).
  colab-lang-json/    # JSON config (tree-sitter-json): key by JSON Pointer.
  colab-lang-markdown/  # Markdown by structure (tree-sitter-md): section,
                      # row, item, text, link, frontmatter, block.
  colab-lang-toml/    # TOML config (toml_edit): key by dotted path.
  colab-lsp-client/   # colab as an LSP client: rename, mv, action,
                      # references, safe-delete, --tidy, --verify-lsp,
                      # and the kept server (`colab lsp-daemon`).
                      # Depends on colab-core + colab-rewrite only.
  colab-mcp/          # Model Context Protocol server: the colab.* tools
                      # (preview, apply, find, ast, schema, lint_script,
                      # test_rule, list_*, and the workflows
                      # move_import, set_config_key, edit_markdown).
                      # Tools that need other crates (outline, section,
                      # references, rename, rename_symbol, action) are
                      # injected by the binary via colab_mcp::ToolProvider.
                      # JSON-RPC 2.0 over stdio, newline-delimited
                      # (MCP stdio transport; the LSP server keeps
                      # Content-Length). Depends only on colab-core +
                      # colab-dsl; like colab-dsl, must not pull in
                      # any colab-lang-* crate at runtime.
  colab-cli/          # `colab` binary: every command; colab.toml and
                      # `colab fix` (config.rs, packs.rs); the CI
                      # formats (ci_format.rs); git file operations
                      # (apply.rs); the LSP-client commands
                      # (lsp_commands.rs); the Markdown commands and
                      # link check (markdown.rs); the LSP server for
                      # .codemod files; `colab mcp`. Depends on
                      # colab-backends, and on colab-lsp-client and
                      # colab-lang-markdown directly.
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
        │  compile_at_source(text, base, &registry)      │
        │      │   expands includes; for each Match →    │
        │      │   registry.get(lang).build_rule(…),     │
        │      │   wrapped in ScopedOperation (in),      │
        │      │   FencedOperation (fences),             │
        │      │   WithFollowups (import clauses)        │
        │      ▼                                         │
        │  Refactoring { rules: Vec<Box<dyn Operation>> }│
        │  (impls CodeTransformer)                       │
        └───────────────┬────────────────────────────────┘
                        │
        ┌───────────────▼────────────────────────────────┐
        │              colab_core::walker                │
        │  walk(transformer, root, &mut visitor)         │
        │  – sorted directory traversal                  │
        │  – calls apply_at(path, src) per file:         │
        │      gate on is_file_relevant, then prefilter, │
        │      then parse — per rule                     │
        │  – undoes changes on `colab:ignore` lines,     │
        │    collects each rule's conflicts              │
        │  – yields FileChange { path, before, after,    │
        │                rules_fired, conflicts }        │
        │  – returns WalkOutcome { files_visited,        │
        │                          skipped }             │
        └───────────────┬────────────────────────────────┘
                        │  folded into colab_core::RunReport
        ┌───────────────▼────────────────────────────────┐
        │   after the walk (colab_cli::cli / apply.rs)   │
        │  --tidy, --verify-lsp, Markdown link check →   │
        │  conflicts? refuse to write (exit 11) : write  │
        │  (git for file ops) → near_miss::probe         │
        └───────────────┬────────────────────────────────┘
                        │
        ┌───────────────▼────────────────────────────────┐
        │             colab_core::render                 │
        │  RunReport + changed files → JSON, at a        │
        │  Detail level, with diffs capped               │
        └───────────────┬────────────────────────────────┘
                        │
        ┌───────────────▼────────────────────────────────┐
        │      colab_cli::format  |  colab_mcp           │
        │  reporter chosen by --format, or the MCP       │
        │  tools/call response — both render the same    │
        │  RunReport so the two surfaces cannot drift    │
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
to skip writes.

The walker actually calls `apply_at(path, source)`, which returns an
`ApplyOutcome { output, rules_fired }`. Knowing the path lets
`Refactoring` skip rules that cannot apply to this file rather than
parsing to find out, and `rules_fired` is what makes per-rule match
counts possible. The default `apply_at` delegates to `apply` and
reports no attribution, so a single-rule implementor need not override
it.

### `LanguageBackend` and `Operation`

```rust
pub trait LanguageBackend: Send + Sync {
    fn lang(&self) -> &'static str;
    fn description(&self) -> &'static str { "" }
    fn capabilities(&self) -> &'static [Capability];
    fn build_rule(&self, module: &str, spec: RuleSpec) -> Result<Box<dyn Operation>>;
    // Abridged. Also: build_rule_where (`where` clauses; pattern only),
    // import_module (what `requires import` lowers to), drop_unused_import
    // (Go), prose_ranges / prose_locator (`and prose`), fence_names /
    // fence_locator (`in fences`), syntax_tree (`colab ast`), and
    // unsupported (the "did you mean" error for unknown modules).
}

pub trait Operation: fmt::Debug + fmt::Display + Send + Sync {
    fn is_file_relevant(&self, path: &Path) -> bool;
    fn apply(&self, source_code: &str) -> String;
    /// A literal that must be present for this op to change anything.
    /// `None` disables the fast path — required for `ensure`.
    fn prefilter(&self) -> Option<&str> { None }
    // Abridged. Also: near_miss_target / observed_targets (the `near`
    // hints for a dead rule), locate (`colab find`), conflicts (checked
    // before writing), why_unchanged (a match the rule could not apply).
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
    Set { target: String, value: String },
    Insert { target: String, value: String },
}
```

Adding a new action variant means: AST + grammar token, lower in
`compiler::lower_match`, extend `RuleSpec::action_name` / `target`,
the JSON serializer in `explain_command` (`colab-cli/src/discover.rs`),
and at least one backend that implements it.

### `BackendRegistry`

```rust
pub struct BackendRegistry { /* … */ }
impl BackendRegistry {
    pub fn register(&mut self, backend: Box<dyn LanguageBackend>);
    pub fn get(&self, lang: &str) -> Option<&dyn LanguageBackend>;
    pub fn languages(&self) -> Vec<&'static str>;
}
```

`colab_backends::registry()` (`crates/colab-backends/src/lib.rs`) is the
single place where every backend is registered; the CLI, the corpus
harness and the MCP tests all call it. New `colab-lang-*` crates plug in
there.

### `FileChange` (walker visitor)

```rust
pub struct FileChange {
    pub path: PathBuf,
    pub before: String,
    pub after: String,
    pub rules_fired: Vec<usize>,
    pub conflicts: Vec<RuleConflict>,
}
let outcome: WalkOutcome = walker::walk(transformer, path, &mut |change| { … })?;
```

A `FileChange` is built from the transformer's `ApplyOutcome` (`output`,
`rules_fired`, `conflicts`) by `FileChange::from_outcome`. The visitor
closure decides whether to write back, format a diff,
emit JSON, or just count for `--check`. Directory entries are
sorted before iteration so output ordering is deterministic across
filesystems.

`walk` returns a `WalkOutcome` carrying what the per-file events cannot:
`files_visited` (entries yielded *before* the relevance check) and
`skipped` (files that could not be read or decoded — recorded rather
than fatal, so one binary blob cannot sink a whole-repo run).

### `RunReport` and `render`

`colab_core::report::RunReport` is the aggregate both output surfaces
serialize — three distinct file counters plus one `RuleStat` per rule:

```rust
pub struct RunReport {
    pub files_visited: u64,   // walker yielded
    pub files_scanned: u64,   // relevant, and read
    pub files_changed: u64,   // actually rewritten
    pub rules: Vec<RuleStat>, // { index, rule, files_matched, near_misses,
                              //   unchanged, message, severity }
    pub skipped: Vec<SkippedFile>,
    pub conflicts: Vec<Conflict>, // checked before anything is written
    pub file_ops: Vec<FileOp>,    // moves / creates / deletes, through git
    pub warnings: Vec<String>,
    pub spans: BTreeMap<PathBuf, Vec<RuleSpan>>, // per-line attribution, CI formats
    /* bytes_before / bytes_after, elapsed, scanned_sample */
}
```

Keeping the counters distinct is what makes an empty result
self-explaining: `visited == 0` is a path/glob problem, `scanned == 0`
is a language problem, `changed == 0` with zeroed `RuleStat`s is a
script problem. `RuleStat::files_matched == 0` marks a dead rule.

`colab_core::render` turns that into JSON at one of three `Detail`
levels (`summary` / `counts` / `diff`), bounded by `max_files` and
`max_diff_bytes`. `colab-cli/src/format.rs` and `colab-mcp` both render
through it, so the CLI and the MCP server cannot drift.

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
   a struct that holds the rule's parameters, plus an `apply` built on
   the shared helpers — `colab_rewrite::apply_edits`, `delete_lines`,
   `rename_nodes_by_text` — never a hand-rolled edit application. Ops
   that target a value implement `near_miss_target` / `observed_targets`.
   Add `src/patterns.rs`: a `PatternLanguage` (grammar, expando char,
   context wrappers, `also` contexts) for `<x>::pattern`; matching itself
   lives in `colab_rewrite::pattern`. Implement `import_module`,
   `prose_ranges` and `fence_names` so the generic clauses work.
5. **Register.** Add `registry.register(Box::new(<X>Backend));` to
   `crates/colab-backends/src/lib.rs::registry`, add `colab-lang-<x>` to
   `crates/colab-backends/Cargo.toml`, and bump `BACKEND_COUNT`. The test
   `every_colab_lang_crate_is_registered` fails if you forget.
6. **Corpus.** At least one fixture under
   `tests/corpus/<lang>/<case>/{script,input/,expected/}`, with generated
   `rules.expected` and `find.expected` (write them with
   `COLAB_BLESS=1 cargo test -p colab-dsl --test corpus` and review the
   diff). The harness picks it up and asserts idempotency.
7. **Docs.** Add the backend's rows to the `docs/features.md` matrix
   (`docs_match_capabilities.rs` fails the build otherwise), name it in
   the skill (`skill_matches_capabilities.rs`), and update the capability
   table in `README.md`.

CI (`.github/workflows/ci.yml`) runs `cargo build/test/clippy
--workspace`, so a broken backend is caught immediately.

## Adding a new namespace or action to an existing backend

- **New module under an existing language:** drop a new `src/<m>.rs`,
  add `Operation` impls, add a `Capability` row to that backend's
  `CAPABILITIES`, add an arm to `build_rule`, add a corpus case.
- **New action verb (in addition to `replace`/`delete`/`ensure`/
  `replace_call`/`set`/`insert`):** extend `Action` in
  `crates/colab-dsl/src/ast.rs`, add the token to
  `codemod.lalrpop`, add the matching `RuleSpec` variant in
  `colab_core::backend`, lower in
  `colab_dsl::compiler::lower_match`, extend `RuleSpec::action_name` /
  `target`, serialize in `explain_command` (`colab-cli/src/discover.rs`),
  then implement in whatever
  backends should support it.

## Invariants (and why)

- **`CodeTransformer::apply` returns input unchanged when nothing
  matches.** The walker uses equality to skip writes; the path-blind
  `apply` composes rules unconditionally and so treats irrelevant
  rules as identity.
- **Rules are gated per file before they are parsed.**
  `Refactoring::apply_at` skips any rule whose `is_file_relevant`
  rejects the path, then any rule whose `prefilter` literal is absent
  from the source. Cross-language safety is enforced, not assumed.
- **`Operation::prefilter` is a necessary condition or `None`.**
  Its absence must imply a no-op; an over-broad literal costs speed,
  never correctness. `ensure` operations must return `None` — they act
  precisely when their target is missing.
- **A rule that matched nothing is reported, not silently tolerated.**
  `RuleStat::files_matched == 0` is surfaced as a warning by every
  output format.
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
- **`where` narrows, never widens.** Constraints filter the match list,
  so the prefilter stays sound and `find` agrees with `refactor`.
- **A pattern match covers the whole node.** A match must account for
  every named child unless a `$$$` absorbs them, so a rewrite never drops
  what the pattern did not mention.
- **Import clauses are follow-ups.** `requires import` / `drops import`
  run only in files the rule changed (`colab_core::WithFollowups`).
- **Conflicts are checked before anything is written.** Every write path
  buffers its changes and writes nothing while a conflict stands (exit 11)
  unless `--allow-conflicts`.
- **File operations go through git or not at all**, and **semantic edits
  are delegated** to the language's own server; a kept server must answer
  exactly as a fresh one would.
- **`colab:ignore` is generic.** It is applied in `Refactoring::apply_at`;
  backends need nothing for it.

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
agent eval suite      crates/colab-cli/tests/evals.rs + evals/tasks/
DSL unit tests        crates/colab-dsl/src/compiler.rs (parser),
                      crates/colab-dsl/tests/namespace_spelling.rs
end-to-end corpus     crates/colab-dsl/tests/corpus.rs +
                      tests/corpus/<lang>/<case>/{script,input/,expected/,
                      rules.expected,find.expected}
cross-backend         crates/colab-backends/tests/contracts.rs,
                      docs_match_capabilities.rs, skill_matches_capabilities.rs
LSP client            crates/colab-lsp-client/tests/ (fake_server.rs,
                      kept_server.rs), driven by examples/fake_lsp.rs
```

The corpus harness is the canonical "did the new feature break
anything" test: it walks every case, applies the script, diffs
against `expected/`, then re-applies and asserts idempotency.
