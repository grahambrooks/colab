# Structural patterns (M18)

`<lang>::pattern` matches **code with holes**: `$NAME` (one node),
`$$$NAME` (zero or more), `$_` (one, unbound). Each directory has an input
file and a script. Nothing is written without `--write`.

```sh
# Where would it act? Read-only, grep-shaped.
colab find --script examples/patterns/go/migrate.codemod --detail detailed examples/patterns/go

# What would change?
colab refactor --script examples/patterns/go/migrate.codemod --format diff examples/patterns/go
colab refactor --script examples/patterns/python/modernise.codemod --format diff examples/patterns/python
colab refactor --script examples/patterns/js/cleanup.codemod --format diff examples/patterns/js

# M26: `where` constraints and import clauses. Only literal messages outside
# tests are rewritten; fmt is added and pkg/errors dropped per file, only
# where the rule changed something (other/ is untouched).
colab find --script examples/patterns/go-where/migrate.codemod --detail detailed examples/patterns/go-where
colab refactor --script examples/patterns/go-where/migrate.codemod --format diff examples/patterns/go-where

# Why does (or doesn't) a pattern match? Compare the trees.
colab ast --lang go --pattern 'errors.Wrap($ERR, $MSG)'
colab ast --lang go --stdin < examples/patterns/go/main.go
```

| Example | Shows |
| ------- | ----- |
| `go/` | Two captures reordered into a template; `$$$ARGS` with a literal first argument; whole-line `delete`; a pattern rule beside an ordinary `go::import` rule. |
| `python/` | A method-call rename, and a *structural* rewrite (`assertTrue(a == b)` → `assertEqual(a, b)`) that no fixed module expresses. |
| `go-where/` | `where $M matches /^"/` and `not inside 'func $F(t *testing.T) { $$$ }'` narrow the matches; `requires import "fmt"` and `drops import … when unused` edit imports only in the files the rule rewrote. |
| `js/` | `delete` removes standalone statements only: the `console.log` inside an arrow body is left (deleting it would leave `() => )`), and `colab find` still lists it. |
