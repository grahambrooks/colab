# Rust 2021 → 2024 (`edition-2021-to-2024.colab`)

The edition changes that are pure syntax, as a reviewable diff:

| Rule | 2024 requires |
| ---- | ------------- |
| `extern "C" { … }` → `unsafe extern "C" { … }` | extern blocks must be `unsafe` |
| `#[no_mangle]`, `#[export_name = …]`, `#[link_section = …]` → `#[unsafe(…)]` | unsafe attributes are marked |
| `std::env::set_var` / `remove_var` (and `env::…`) → wrapped in `unsafe { … }` | both are `unsafe fn` |
| identifiers named `gen` → `r#gen` | `gen` is a reserved keyword |

```sh
colab refactor --script examples/packs/rust/edition-2021-to-2024.colab --format diff .
colab refactor --script examples/packs/rust/edition-2021-to-2024.colab --write --tidy .
# then set `edition = "2024"` in Cargo.toml and build
```

On the sample in `tests/corpus/rust/edition_2024/`, `cargo check` as
edition 2024 stops at 8 errors before the pack (the `gen` parse errors hide the `set_var` ones) and builds after it; every rule fires on the sample.

## What it does not cover

These need meaning, not syntax; `cargo fix --edition` or a person handles
them:

- **`Cargo.toml`'s `edition`.** It may be a plain string, inherited
  (`edition.workspace = true`) or an older edition; bump it yourself.
- **Macro bodies.** A `macro_rules!` body is a token tree to the parser,
  so `$e:expr` → `$e:expr_2021`, and anything above inside a macro, is
  not rewritten.
- **`set_var` / `remove_var` imported by name** (`use std::env::set_var;`
  then `set_var(…)`), and calls that need a `// SAFETY:` justification:
  each new `unsafe` block should be reviewed.
- **Semantic changes**: RPIT lifetime capture, `if let` and tail
  expression temporary scopes, never-type fallback, `Box<[T]>`'s
  `IntoIterator`, `unsafe_op_in_unsafe_fn`, `static mut` references,
  prelude additions (`Future`, `IntoFuture`), and `cargo` resolver and
  lint changes.
- **`gen` in strings, comments or macro input.** Only identifiers are
  renamed.
