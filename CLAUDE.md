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
cargo run -p colab-cli -- refactor --script <s> --format json    # machine-readable per-file events
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
| `colab-core`     | `Error`/`Result`, `CodeTransformer`, walker, `LanguageBackend` + `Operation` + `BackendRegistry`. No internal deps. |
| `colab-dsl`      | LALRPOP grammar, AST, compiler, `Refactoring` IR. Depends only on `colab-core`; no compile-time knowledge of any backend. |
| `colab-lang-go`     | Go backend: `go::import` (rename / delete / ensure), `go::symbol` (rename), `go::struct_tag` (rename `<key>:"<value>"` pairs in field tags), `go::call` (`replace_call` template). |
| `colab-lang-java`   | Java backend: `java::import` (rename / delete / ensure), `java::package` (rename), `java::symbol` (rename) via tree-sitter-java. |
| `colab-lang-js`     | JS/TS backend: `js::import` (rename / delete) for ES module specifiers and `js::symbol` (rename) via tree-sitter-javascript. Handles `.js`, `.mjs`, `.cjs`, `.jsx`, `.ts`, `.tsx`. |
| `colab-lang-python` | Python backend: `python::import` (rename / delete / ensure) and `python::symbol` (rename) via tree-sitter-python. Import matching is segment-prefix like `rust::use`. |
| `colab-lang-rust`   | Rust backend: `rust::use` (rename / delete / ensure), `rust::symbol` (rename), `rust::crate` (rename / delete via toml_edit-validated line scan), `rust::call` (`replace_call` template). |
| `colab-cli`         | The `colab` binary. Builds the default `BackendRegistry` (`go`, `java`, `js`, `python`, `rust`) and wires the LSP stub. |

`crates/colab-dsl/build.rs` runs `lalrpop::process_root()` (editing `src/codemod.lalrpop` requires a rebuild — generated `codemod.rs` lives in `OUT_DIR`). `crates/colab-cli/build.rs` shells out to `git rev-parse --short HEAD` to embed a version suffix, so builds must happen inside a git checkout.

## Architecture

A codemod runs through a four-stage pipeline; see `ARCHITECTURE.md` for the diagram and module map.

```
script text  →  colab_dsl::parse        →  colab_dsl::ast (raw AST, possibly many Match blocks)
             →  colab_dsl::compile      →  colab_dsl::Refactoring (Vec<Box<dyn Operation>>)
                  uses BackendRegistry from colab_core
             →  colab_core::walker      →  Operation::apply (e.g. colab_lang_go::imports::ImportRename)
```

Key invariants when changing this code:

- **`CodeTransformer` is the only contract `walker.rs` knows about.** It takes `&str` (not `&String`) and returns the rewritten source. Returning the input unchanged is the signal to skip the write — preserve that.
- **`LanguageBackend` is the extension point.** Adding a transform = new `Operation` impl in the appropriate `colab-lang-*` crate + matching arm in that crate's `LanguageBackend::build_rule` + a `registry.register(Box::new(<Backend>))` line in `colab-cli/src/cli.rs::default_backends`. Do **not** add backend dependencies to `colab-dsl` or plumb language details into `walker.rs` / `cli.rs` beyond that one registry line.
- **`colab-dsl` must not depend on any `colab-lang-*` crate at runtime.** Use it as a `dev-dependency` for tests only. The CI matrix relies on this so a broken backend cannot block unrelated work.
- **`Operation::apply` must be a no-op for irrelevant input.** Multi-rule scripts compose operations left-to-right; a Go rule will be invoked on Rust files (and vice versa) and must return its input unchanged in that case. The walker's `is_file_relevant` check is OR'd across rules and only gates the read/write — it does not filter per-rule.
- **DSL actions today: `replace "..."`, `delete`, `ensure`, `replace_call "<tmpl>"`.** `delete`/`ensure` use the `match_string` as the target. `replace_call` rewrites a matched call expression with a template using placeholders `$1`/`$2`/… (1-indexed positional args), `$args` (full arg list), `$func` (matched function name), `$$` (literal `$`). Templates that *do not rename the function* (e.g. `match go::call "f" { replace_call "f(ctx, $args)" }` — wrap-style) are intentionally non-idempotent and must be applied once; the corpus harness's idempotency check will fail any rule that loops, so verify with `--format diff` and apply with a single `--write`.
- **Symbol rename is single-file and syntactic.** `<lang>::symbol "X" { replace "Y" }` rewrites every identifier-kind tree-sitter node whose text equals `X` *within each file processed*. There is no scope analysis (a shadowed local with the same name is also renamed) and no cross-file linkage (a struct moved to a new module would still need the user to update imports — the `<lang>::import` and `rust::crate` operations cover that). Cross-file move is deliberately out of scope; it requires whole-program reasoning that conflicts with colab's syntactic-rewriter premise (see "Non-goals" in `DEVELOPMENT_PLAN.md`).
- **Transforms must be idempotent** on their own output. The corpus harness (`crates/colab-dsl/tests/corpus.rs`) re-applies every script and asserts the second pass is a no-op. When matching syntactic constructs, prefer exact-equality on the relevant tree-sitter node (e.g. an import path) over substring matching — substring matches break composition (see `colab-lang-go/src/imports.rs`).
- **Unknown namespaces must fail loudly.** `BackendRegistry::get` returning `None` is mapped to `Error::UnsupportedOperation` in `compiler::lower_match`; never silently fall back to a default transform. Backends raise the same error for unknown modules within their `lang`.
- **Tree-sitter edits are applied in reverse byte order** in `colab-lang-go/src/imports.rs` so earlier offsets stay valid. Any new tree-sitter rewriter should follow the same pattern.
- **Every new backend must add at least one corpus case** under `tests/corpus/<lang>/<case>/{script,input/,expected/}` before it merges.
- **provide examples** for user evaluation
- **update the dsl documentation** docs/dsl.md

## Error handling

All fallible code returns `crate::error::Result<T>`. I/O errors must be wrapped with `Error::io_at(path, source)` so the offending path appears in the message. The CLI maps any error into a single `error!` log line plus a non-zero exit code; library code never panics on user input. Reserve `expect`/`panic!` for genuinely unreachable conditions (e.g. loading a built-in tree-sitter grammar) and inside `#[cfg(test)]`.

## Coding standards (from `.github/copilot-instructions.md`)

- Idiomatic Rust and idiomatic module conventions.
- Prefer `Result`-returning APIs over `panic!` / `expect` outside tests.
