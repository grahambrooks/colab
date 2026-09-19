# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

```sh
cargo build --workspace                  # debug build of every crate
cargo build --release --workspace        # release build → target/release/colab
cargo test --workspace                   # run all unit + corpus tests
cargo test -p colab-dsl --test corpus    # run only the corpus harness
cargo test <name>                        # run a single test (substring match)
cargo clippy --workspace --all-targets -- -D warnings  # CI-equivalent lint gate
cargo run -p colab-cli -- refactor --script <script> [paths...]  # run a codemod
cargo run -p colab-cli -- refactor -C <dir> --script <script> .  # cd into <dir> first
cargo run -p colab-cli -- refactor --script <s> --check <paths>  # CI-friendly: exit 10 if changes pending
cargo run -p colab-cli -- refactor --script <s> --format json    # one JSON doc: counters + per-rule match counts + changed paths
cargo run -p colab-cli -- refactor --script <s> --detail diff     # add capped unified diffs to that document
cargo run -p colab-cli -- refactor --script <s> --format diff    # unified diff to stdout
cargo run -p colab-cli -- refactor --script <s> --stdin --path <hint>  # stdin → transformed source on stdout
cargo run -p colab-cli -- schema                                 # JSON capability schema
cargo run -p colab-cli -- list-languages                         # registered backends
cargo run -p colab-cli -- list-rules <lang>                      # modules + actions for one backend
cargo run -p colab-cli -- explain --script <s>                   # parsed IR as JSON, no execution
cargo run -p colab-cli -- server                                 # LSP stub on stdio
```

Exit codes from `colab refactor` (also documented in `--help`):

| Code | Meaning |
| ---- | ------- |
| 0    | Success — no changes needed, or `--write` succeeded. |
| 1    | Generic / configuration error. |
| 2    | Script parse error. |
| 3    | Unsupported namespace or operation. |
| 4    | I/O error. |
| 10   | `--check` found changes that would be made. |

Defaults: `--format human` on a TTY implies `--write`; everything else (`json` / `ndjson` / `diff`, or non-TTY stdout) defaults to `--dry-run`. `--check` overrides both.

The repo is a Cargo workspace under `crates/`:

| Crate            | Role |
| ---------------- | ---- |
| `colab-core`     | `Error`/`Result` (+ `ParseDetail`), `CodeTransformer`, walker, `LanguageBackend` + `Operation` + `BackendRegistry`, `RunReport` (per-rule attribution), `render` (the shared JSON serializer), `ScopedOperation`, `suggest`. No internal deps. |
| `colab-rewrite`  | Tree-sitter rewriting primitives every backend shares: `apply_edits` (reverse byte order), `line_span`, `delete_lines`, `visit_all`, `rename_nodes_by_text`. Depends on `tree-sitter` only — deliberately *not* on `colab-core`, so `colab-dsl`/`colab-mcp` never inherit a grammar engine. |
| `colab-backends` | The single composition root: `registry()` registers all 15 backends. `colab-cli` depends on it at runtime; `colab-dsl`/`colab-mcp` as a dev-dependency only. Owns the cross-backend contract tests. |
| `colab-dsl`      | LALRPOP grammar, AST, compiler, `Refactoring` IR. Depends only on `colab-core`; no compile-time knowledge of any backend. |
| `colab-lang-*`      | One crate per language, 15 in total: 12 source languages and 3 config formats (`json`, `markdown`, `toml`). Every source backend provides an import-equivalent (`replace`/`delete`/`ensure`), `symbol` (rename), and `call` (`replace_call`); see the table below and [`docs/features.md`](docs/features.md) for the per-backend extras and caveats. |
| `colab-mcp`         | MCP server. Wraps `preview`/`apply`/`schema`/`lint_script` as MCP tools over JSON-RPC 2.0 with Content-Length framing on stdio. Depends only on `colab-core` + `colab-dsl`; never pulls in a `colab-lang-*` crate. |
| `colab-cli`         | The `colab` binary. Builds the default `BackendRegistry` (all 15 backends), serves the LSP (`colab server`) with diagnostics + completion for `.codemod` files, and launches the MCP server (`colab mcp`). |

Language backends and the modules each adds beyond the common floor
(`<import-equivalent>` / `symbol` / `call`):

| Crate | Namespace | Import module | Extra modules | Files |
| ----- | --------- | ------------- | ------------- | ----- |
| `colab-lang-c`      | `c`      | `include` | — | `.c`, `.h` |
| `colab-lang-cpp`    | `cpp`    | `include` | `namespace` | `.cpp`/`.cc`/`.cxx`/`.hpp`/`.hh`/`.h` … |
| `colab-lang-csharp` | `csharp` | `using`   | `namespace` | `.cs`, `.csx` |
| `colab-lang-go`     | `go`     | `import`  | `package`, `struct_tag` | `.go` |
| `colab-lang-java`   | `java`   | `import`  | `package` | `.java` |
| `colab-lang-js`     | `js`     | `import`  | — | `.js`/`.mjs`/`.cjs`/`.jsx`/`.ts`/`.tsx` |
| `colab-lang-kotlin` | `kotlin` | `import`  | `package` | `.kt`, `.kts` |
| `colab-lang-php`    | `php`    | `use`     | `namespace` | `.php`, `.phtml` |
| `colab-lang-python` | `python` | `import`  | — | `.py` |
| `colab-lang-ruby`   | `ruby`   | `require` | — | `.rb`, `Rakefile`, … |
| `colab-lang-rust`   | `rust`   | `use`     | `crate` (no `ensure`) | `.rs`, `Cargo.toml` |
| `colab-lang-swift`  | `swift`  | `import`  | — | `.swift` |

Config-file backends edit data, not source. Each has one module whose
target is an address, and the `set` / `insert` / `delete` actions (see
[`docs/features.md`](docs/features.md#config-file-backends)):

| Crate | Namespace | Module | Address | Files |
| ----- | --------- | ------ | ------- | ----- |
| `colab-lang-json`     | `json`     | `key`   | JSON Pointer (`/a/b`) | `.json` |
| `colab-lang-markdown` | `markdown` | `block` | managed-block name (`<!-- name:begin -->`) | `.md`, `.markdown` |
| `colab-lang-toml`     | `toml`     | `key`   | dotted path, `name[field=value]` selects an array element | `.toml` |

`.h` is claimed by both `c` and `cpp` — the extension cannot distinguish
them. Both run over `.h` files in a mixed script; every op is idempotent
so the result is the same, at the cost of one extra parse.

`crates/colab-dsl/build.rs` runs `lalrpop::process_root()` (editing `src/codemod.lalrpop` requires a rebuild — generated `codemod.rs` lives in `OUT_DIR`). `crates/colab-cli/build.rs` shells out to `git rev-parse --short HEAD` to embed a version suffix, so builds must happen inside a git checkout.

## Architecture

A codemod runs through a four-stage pipeline; see [`docs/architecture.md`](docs/architecture.md) for the diagram and module map.

```
script text  →  colab_dsl::parse        →  colab_dsl::ast (raw AST, possibly many Match blocks)
             →  colab_dsl::compile      →  colab_dsl::Refactoring (Vec<Box<dyn Operation>>)
                  uses BackendRegistry from colab_core
             →  colab_core::walker      →  Operation::apply (e.g. colab_lang_go::imports::ImportRename)
```

Key invariants when changing this code:

- **`CodeTransformer` is the only contract `walker.rs` knows about.** It takes `&str` (not `&String`) and returns the rewritten source. Returning the input unchanged is the signal to skip the write — preserve that. The walker calls `apply_at(path, source)`, which additionally reports which rules fired; `apply(source)` remains the path-blind fallback.
- **`LanguageBackend` is the extension point.** Adding a transform = new `Operation` impl in the appropriate `colab-lang-*` crate + matching arm in that crate's `LanguageBackend::build_rule` + a `registry.register(Box::new(<Backend>))` line in `colab-backends/src/lib.rs::registry` (the *only* registration site — `colab-cli`, the corpus harness, and the MCP tests all call it). Do **not** add backend dependencies to `colab-dsl` or plumb language details into `walker.rs` / `cli.rs` beyond that one registry line.
- **`colab-dsl` must not depend on any `colab-lang-*` crate at runtime.** Use it as a `dev-dependency` for tests only. The CI matrix relies on this so a broken backend cannot block unrelated work.
- **Rules are gated per file, not merely expected to no-op.** `Refactoring::apply_at` skips any rule whose `is_file_relevant` rejects the path, so a Go rule never parses a `.rs` file. `Operation::apply` must still be a no-op for irrelevant input (the path-blind `apply` composes rules unconditionally), but the engine no longer relies on it for cross-language safety.
- **`Operation::prefilter` must be a *necessary* condition, or `None`.** It returns a literal that has to appear in the source for the rule to change anything; when it is absent, the tree-sitter parse is skipped entirely. Returning a literal only promises that its absence implies a no-op — an over-broad literal costs performance, never correctness. **`ensure`-style operations must return `None`**, since they act precisely when the target is absent. A too-narrow prefilter shows up as a corpus idempotency failure.
- **DSL surface today: `match <lang>::<module> "<target>" [in "<glob>"] { <action> }`, with actions `replace "..."`, `delete`, `ensure`, `replace_call "<tmpl>"`, and — for the config-file backends — `set '<value>'` / `insert '<value>'`.** Strings are double- or single-quoted, with no escapes; single quotes exist for values that are quoted text themselves. `delete`/`ensure` use the `match_string` as the target. `replace_call` rewrites a matched call expression with a template using placeholders `$1`/`$2`/… (1-indexed positional args), `$args` (full arg list), `$func` (matched function name), `$$` (literal `$`). Templates that *do not rename the function* (e.g. `match go::call "f" { replace_call "f(ctx, $args)" }` — wrap-style) are intentionally non-idempotent and must be applied once; the corpus harness's idempotency check will fail any rule that loops, so verify with `--format diff` and apply with a single `--write`.
- **Symbol rename is single-file and syntactic.** `<lang>::symbol "X" { replace "Y" }` rewrites every identifier-kind tree-sitter node whose text equals `X` *within each file processed*. Narrow it by path with `in "<glob>"` (`colab_core::ScopedOperation`) when two crates share a name. There is no scope analysis (a shadowed local with the same name is also renamed) and no cross-file linkage (a struct moved to a new module would still need the user to update imports — the `<lang>::import` and `rust::crate` operations cover that). Cross-file move is deliberately out of scope; it requires whole-program reasoning that conflicts with colab's syntactic-rewriter premise (see "Non-goals" in [`docs/development-plan.md`](docs/development-plan.md)).
- **Transforms must be idempotent** on their own output. The corpus harness (`crates/colab-dsl/tests/corpus.rs`) re-applies every script and asserts the second pass is a no-op. When matching syntactic constructs, prefer exact-equality on the relevant tree-sitter node (e.g. an import path) over substring matching — substring matches break composition (see `colab-lang-go/src/imports.rs`).
- **Unknown namespaces must fail loudly, and name the alternatives.** `BackendRegistry::get` returning `None` is mapped to `Error::UnsupportedOperation` in `compiler::lower_match`; never silently fall back to a default transform. Backends reject unknown modules and unsupported actions through `LanguageBackend::unsupported`, which lists the valid names and adds a "did you mean" via `colab_core::suggest` — use it rather than hand-rolling a message, and never format a `RuleSpec` with `{:?}`.
- **A rule that matches nothing is a reportable event, not a silent success.** `RunReport` carries per-rule match counts (`RuleStat`); every output surface warns on `files_matched == 0`. Anything that adds a new way to run rules must keep that attribution intact.
- **Output is for a machine first.** `colab-core::render` is the single serializer both `colab-cli/src/format.rs` and `colab-mcp` use — compact JSON, unchanged files omitted, diffs capped, and the `visited`/`scanned`/`changed` counters kept distinct so an empty result is self-explaining. Do not add a second, surface-specific shape.
- **Never hand-roll a tree walk or an edit application.** `colab-rewrite` owns `apply_edits` (reverse byte order, so earlier offsets stay valid), `delete_lines`, `visit_all`, and `rename_nodes_by_text`. These were duplicated across 12 backends until they were extracted; re-introducing a private copy re-introduces the bug-fixed-in-one-of-twelve problem. `<lang>::symbol` should be a single `rename_nodes_by_text` call plus a `RENAME_KINDS` list.
- **Cross-backend contracts are tested generically** in `colab-backends/tests/contracts.rs`: `ensure` ops must have no prefilter, a prefilter must be derived from the target and its absence must imply a no-op, and every module name must be spellable as a DSL namespace segment (the `c::include` collision). A new backend inherits all of these without anyone remembering to wire them up.
- **Every new backend must add at least one corpus case** under `tests/corpus/<lang>/<case>/{script,input/,expected/}` before it merges.
- **Config-file backends compare values by meaning.** `set` must be a no-op when an equal value is present however it is formatted, and `set` / `insert` must return `None` from `prefilter` (they act when the target is absent; the contract test enforces it). A file that does not parse is returned unchanged.
- **Scope globs ignore a leading `./`** (`ScopedOperation`), so `colab refactor -C repo … .` and `in "core/**"` agree. A walk of `.` skips hidden directories such as `.claude/`; name such files explicitly.
- **provide examples** for user evaluation
- **update the dsl documentation** docs/dsl.md

## Error handling

All fallible code returns `crate::error::Result<T>`. I/O errors must be wrapped with `Error::io_at(path, source)` so the offending path appears in the message. The CLI maps any error into a single `error!` log line plus a non-zero exit code; library code never panics on user input. Reserve `expect`/`panic!` for genuinely unreachable conditions (e.g. loading a built-in tree-sitter grammar) and inside `#[cfg(test)]`.

## Coding standards (from `.github/copilot-instructions.md`)

- Idiomatic Rust and idiomatic module conventions.
- Prefer `Result`-returning APIs over `panic!` / `expect` outside tests.
