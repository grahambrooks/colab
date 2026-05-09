# Architecture

Colab is a small Rust binary that compiles a codemod script written in a
domain-specific language (DSL) and applies it to source files using
tree-sitter. This document describes how the pieces fit together so new
contributors can extend it without re-reading every file.

## Data flow

```
              ┌──────────────────────────────────────────────┐
 codemod      │                  src/cli.rs                  │
 script  ──▶  │  parse args  →  read script  →  drive run    │
              └──────────────┬───────────────────────────────┘
                             │
              ┌──────────────▼───────────────┐
              │       src/codemod/...        │
              │                              │
              │  compiler::compile(text)     │
              │      │                       │
              │      ▼                       │
              │  ast::Command  (raw AST)     │
              │      │                       │
              │      ▼                       │
              │  model::Refactoring          │
              │  (executable IR;             │
              │   impl CodeTransformer)      │
              └──────────────┬───────────────┘
                             │
              ┌──────────────▼───────────────┐
              │        src/walker.rs         │
              │  recurse paths, apply        │
              │  CodeTransformer per file    │
              └──────────────┬───────────────┘
                             │
              ┌──────────────▼───────────────┐
              │  src/codemod/go/imports.rs   │
              │  tree-sitter rewrites        │
              └──────────────────────────────┘
```

## Module map

| Module | Responsibility |
| --- | --- |
| `main.rs` | Initialise logging, dispatch to `cli::run`, map errors → exit code. |
| `cli.rs` | `clap`-driven argument parsing and per-subcommand handlers. |
| `error.rs` | Crate-wide `Error` / `Result` types. |
| `walker.rs` | Recursive directory traversal that drives any `CodeTransformer`. |
| `language_server.rs` | LSP stub served over stdio (lifecycle logging only today). |
| `codemod/ast.rs` | Raw grammar AST: `Command`, `Body`, `Namespace`, `Action`. |
| `codemod/codemod.lalrpop` | LALRPOP grammar; produces `ast::Command`. |
| `codemod/compiler.rs` | `parse(text)` → AST and `compile(text)` → IR (validates namespace). |
| `codemod/model.rs` | Runtime IR (`Refactoring`, `Rule`) and the `CodeTransformer` trait. |
| `codemod/go/imports.rs` | Tree-sitter Go import rename. |

## Key abstractions

### `CodeTransformer`

```rust
pub trait CodeTransformer {
    fn is_file_relevant(&self, path: &Path) -> bool;
    fn apply(&self, source_code: &str) -> String;
}
```

The walker only knows about this trait, so adding a new language or
operation does not require touching `walker.rs` or `cli.rs`.

### `Rule`

`Rule` is an enum: each variant is a single, language-specific operation.
The `CodeTransformer` impl on `Rule` matches on the variant and dispatches
to the right tree-sitter handler. New transforms become new variants.

### `Refactoring`

A named wrapper around a single `Rule`. The grammar permits one
`match`/action per script today; expanding to multiple rules per script
means giving `Refactoring` a `Vec<Rule>` and walking it.

## Extending colab

1. Define the operation in the grammar (`codemod/codemod.lalrpop`) if it
   needs new syntax. Otherwise reuse the existing `match ns::op "x" { … }`
   shape.
2. Add a variant to `codemod::model::Rule` and implement its
   `is_file_relevant` / `apply` arms.
3. Implement the actual transformation in a new module under
   `codemod/<language>/<op>.rs`.
4. Map the namespace to the new `Rule` variant in
   `codemod::compiler::lower_rule`.
5. Add unit tests next to the new transformation and a compiler test
   covering the new namespace.

## Error handling

Every fallible operation returns `error::Result<T>`. I/O errors are
wrapped with `Error::io_at(path, source)` so failures include the
offending path. The CLI maps any error to a single `error!` log line and
a non-zero exit code; library code never panics on user input.
