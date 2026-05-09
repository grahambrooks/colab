# Rename a crate end-to-end

A walk-through of renaming `old_tokio` → `tokio` across both `Cargo.toml`
and source files in one script.

```sh
# From the repo root:
colab refactor --script examples/rust/rename_crate/rename.codemod \
    examples/rust/rename_crate
```

What the script does:

1. `match rust::crate "old_tokio" { replace "tokio" }` rewrites the
   `[dependencies]` entry in `Cargo.toml`.
2. `match rust::use "old_tokio" { replace "tokio" }` rewrites every
   `use old_tokio::…` declaration in `*.rs` files under the target
   tree.

Try a `--check` first if you want CI-style read-only verification:

```sh
colab refactor --script examples/rust/rename_crate/rename.codemod \
    --check examples/rust/rename_crate
echo "exit: $?"   # 10 if changes are pending, 0 if already applied
```
