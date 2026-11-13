# File operations through git (`colab mv`, M24)

A two-file Rust crate for trying `colab mv`. It needs `rust-analyzer` on
`PATH`. Copy it somewhere outside this repo, so it becomes its own git
work tree:

```sh
cp -R examples/file-ops /tmp/file-ops && cd /tmp/file-ops
git init -q && git add -A && git commit -qm init

colab mv src/util.rs src/helpers.rs --format diff      # the move + what follows it
colab mv src/util.rs src/helpers.rs --write --backup /tmp/file-ops.bak \
  --verify 'cargo check -q'
git status --short        # R  src/util.rs -> src/helpers.rs   (staged move)
                          #  M src/main.rs                     (unstaged edits)
colab undo --from /tmp/file-ops.bak && git status --short   # clean again
```

Things to try:

- **Make `src/main.rs` dirty first.** The move is refused until you commit,
  stash, or pass `--allow-dirty`.
- **Run it outside git** (`rm -rf .git`). It is refused: colab never moves
  files without git.
- **`--commit-moves`.** The move is committed on its own
  (`colab: move src/util.rs -> src/helpers.rs`), so `git log --follow
  src/helpers.rs` reaches `init` however much you edit the file afterwards.
- **`--no-lsp`.** Only `git mv`, with a warning that no references were
  updated.
