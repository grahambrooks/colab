# Markdown by structure (M28)

Read and edit Markdown by heading, not by quoting text exactly. Nothing is
written without `--write`.

```sh
# The outline: each section's address, anchor, line span and contents.
colab outline examples/markdown/guide.md
colab outline examples/markdown/guide.md --format json

# One section, by heading path or anchor.
colab section examples/markdown/guide.md "Usage > Options"
colab section examples/markdown/guide.md "#internals" --body

# An ambiguous address names the candidates.
colab section examples/markdown/guide.md "Options"

# Edit sections and front matter. Rule 4 (a heading rename) reports the
# link to `#old-name` as a conflict, so --write would refuse (exit 11).
# Run it on guide.md alone: its `in "**/guide.md"` scope also matches
# fences/guide.md, where `set` would add front matter.
colab refactor --script examples/markdown/sections.codemod --format diff examples/markdown/guide.md
```

| Rule | Shows |
| ---- | ----- |
| `markdown::section "Usage > Options" { set r#"…"# }` | A path picks one of two `Options` sections; a raw string holds both quote characters. |
| `markdown::section "Usage > Examples" { set '…' }` | `set` on a missing section adds it under its parent. The `#` line in its shell fence stays code. |
| `markdown::section "#deprecated" { delete }` | Address by anchor. |
| `markdown::section "Old name" { replace "New name" }` | Rename a heading; in-file links to the old anchor become conflicts. |
| `markdown::frontmatter "updated" { set '…' }`, `"draft" { delete }` | Front-matter keys, compared by meaning. |

## Rows, items and links

```sh
# Rows by cell value (`^`: top), items by their text, links by destination.
colab refactor --script examples/markdown/rows.codemod --format diff examples/markdown

# Broken links (exit 10 when there are any).
colab links examples/markdown docs

# A heading rename that another file links to is refused (exit 11),
# naming the link; a markdown::link rule in the same run clears it.
```

## Code in fences

```sh
# A Go migration that also rewrites the ```go examples in the docs, and a
# Rust rule that only touches the docs (`in fences`). The indented fence
# in a list item is handled; the shell fence is left alone.
colab refactor --script examples/markdown/fences/migrate.codemod --format diff examples/markdown/fences

# Where a pattern matches in the docs' fences, at its place in the file.
colab find --script examples/markdown/fences/find.codemod examples/markdown/fences
```
