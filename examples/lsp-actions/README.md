# IDE refactorings via the language server (`colab action`, M25)

A small Rust crate for trying `colab action`, `references`,
`safe-delete`, and `--tidy` with `rust-analyzer` (it must be on `PATH`).
Run from this directory, or copy it into its own git repo first if you
want to `--write`:

```sh
cd examples/lsp-actions

# What does the server offer for the expression `p * 2 + 1` (line 11)?
colab action --at src/main.rs:11:16-11:25 --list

# Extract it into a function — a semantic refactoring colab does not compute.
colab action --at src/main.rs:11:16-11:25 --kind refactor.extract --title function --format diff

# Ambiguous on purpose: every rust-analyzer extract is `refactor.extract`,
# so this errors and lists all four.
colab action --at src/main.rs:11:16-11:25 --kind refactor.extract

# Who uses `total`? The server resolves it: one call, inside `write!`.
# `rg -w total` would also match any unrelated `total`.
colab references --symbol total --in src/main.rs

# `total` is used, so this refuses (exit 1) and lists the use.
colab safe-delete --symbol total --in src/main.rs

# `unused_helper` is not; delete it and let the server tidy the imports.
colab safe-delete --symbol unused_helper --in src/main.rs --tidy --format diff
```

Each change command is a dry run unless you pass `--write`; add
`--verify 'cargo check -q'` to undo automatically if the result does not
compile.
