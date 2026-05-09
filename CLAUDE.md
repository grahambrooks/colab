# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

```sh
cargo build                              # debug build
cargo build --release                    # release build → target/release/colab
cargo test                               # run all unit tests
cargo test <name>                        # run a single test (substring match), e.g. cargo test parses_full_program
cargo clippy --all-targets -- -D warnings  # CI-equivalent lint gate; fix everything it flags
cargo run -- refactor --script <script> [paths...]  # run a codemod against paths
cargo run -- refactor -C <dir> --script <script> .  # cd into <dir> first
cargo run -- server                      # start the LSP stub on stdio
```

`build.rs` shells out to `git rev-parse --short HEAD` to embed a version suffix and runs `lalrpop::process_root()` to regenerate the parser. Builds must happen inside a git checkout, and editing `src/codemod/codemod.lalrpop` requires a rebuild for changes to take effect (the generated `codemod.rs` lives in `OUT_DIR`, not the source tree).

## Architecture

A codemod runs through a four-stage pipeline; see `ARCHITECTURE.md` for the diagram and module map.

```
script text  →  codemod::compiler::parse  →  codemod::ast (raw AST)
             →  codemod::compiler::compile →  codemod::model::Refactoring (runtime IR)
             →  walker::process_path        →  codemod::go::imports::rename (tree-sitter rewrite)
```

Key invariants when changing this code:

- **`CodeTransformer` is the only contract `walker.rs` knows about.** It takes `&str` (not `&String`) and returns the rewritten source. Returning the input unchanged is the signal to skip the write — preserve that.
- **`Rule` is the extension point.** Each variant is one language-specific operation. Adding a transform = new `Rule` variant + impl arms in `model.rs` + new module under `codemod/<lang>/` + namespace mapping in `compiler::lower_rule`. Do **not** plumb language details into `walker.rs` or `cli.rs`.
- **Unknown namespaces must fail loudly.** `compiler::lower_rule` returns `Error::UnsupportedOperation` for any `lang::module` it doesn't recognise; never silently fall back to a default transform.
- **Tree-sitter edits are applied in reverse byte order** in `codemod/go/imports.rs` so earlier offsets stay valid. Any new tree-sitter rewriter should follow the same pattern.

## Error handling

All fallible code returns `crate::error::Result<T>`. I/O errors must be wrapped with `Error::io_at(path, source)` so the offending path appears in the message. The CLI maps any error into a single `error!` log line plus a non-zero exit code; library code never panics on user input. Reserve `expect`/`panic!` for genuinely unreachable conditions (e.g. loading a built-in tree-sitter grammar) and inside `#[cfg(test)]`.

## Coding standards (from `.github/copilot-instructions.md`)

- Idiomatic Rust and idiomatic module conventions.
- Prefer `Result`-returning APIs over `panic!` / `expect` outside tests.
