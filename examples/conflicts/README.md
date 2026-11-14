# Conflicts (M27)

Before writing, colab checks each change it made. A `--write` run that
finds a conflict writes nothing and exits 11.

```sh
# Two conflicts in a.go, none in b.go. Nothing is written; exit 11.
colab refactor --script examples/conflicts/check.codemod --write examples/conflicts
echo $?

# The same, as JSON: see `conflicts`.
colab refactor --script examples/conflicts/check.codemod --format json examples/conflicts

# Fixed rules: no conflicts (preview only).
colab refactor --script examples/conflicts/fixed.codemod --format diff examples/conflicts

# Ask the language server too (needs gopls on PATH, and a go.mod for
# real type checking).
colab refactor --script examples/conflicts/fixed.codemod --verify-lsp --dry-run examples/conflicts
```

| Rule | Conflict |
| ---- | -------- |
| `go::symbol "load" { replace "parse" }` | a.go already declares `parse`; the rename would merge them. |
| `fmt.Errorf(…)` template, no `requires import "fmt"` | a.go does not import fmt. b.go does, so it is fine. |

`--allow-conflicts` writes anyway, once you have read them.
