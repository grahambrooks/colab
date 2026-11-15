# Structural patterns (M18)

`<lang>::pattern` matches **code with holes**: `$NAME` (one node),
`$$$NAME` (zero or more), `$_` (one, unbound). Each directory has an input
file and a script. Nothing is written without `--write`.

```sh
# Where would it act? Read-only, grep-shaped.
colab find --script examples/patterns/go/migrate.colab --detail detailed examples/patterns/go

# What would change?
colab refactor --script examples/patterns/go/migrate.colab --format diff examples/patterns/go
colab refactor --script examples/patterns/python/modernise.colab --format diff examples/patterns/python
colab refactor --script examples/patterns/js/cleanup.colab --format diff examples/patterns/js

# M26: `where` constraints and import clauses. Only literal messages outside
# tests are rewritten; fmt is added and pkg/errors dropped per file, only
# where the rule changed something (other/ is untouched).
colab find --script examples/patterns/go-where/migrate.colab --detail detailed examples/patterns/go-where
colab refactor --script examples/patterns/go-where/migrate.colab --format diff examples/patterns/go-where

# M30: `let` derives a name from a capture. Getters lose their `Get`
# (`Getaway` is left: not a getter); a many-line body keeps its layout.
colab refactor --script examples/patterns/go-let/getters.colab --format diff examples/patterns/go-let

# M30: where a match sits. `any` of two goroutine shapes, `nth 1` (a
# function's first statement), `follows … directly` (the check right after
# its call; the one further down is left).
colab find --script examples/patterns/go-relations/hygiene.colab --detail detailed examples/patterns/go-relations
colab refactor --script examples/patterns/go-relations/hygiene.colab --format diff examples/patterns/go-relations

# M30: `delete` takes the comma beside an argument, dict entry or object
# property; a multi-line call loses the line.
colab refactor --script examples/patterns/delete-separators/drop-debug.colab --format diff examples/patterns/delete-separators

# M30: named patterns, one shared through `include`.
colab explain --script examples/patterns/named/hygiene.colab
colab refactor --script examples/patterns/named/hygiene.colab --format diff examples/patterns/named

# M30: one rule renames a call; rewriters rename each option inside it.
colab refactor --script examples/patterns/rewriters/client-v2.colab --format diff examples/patterns/rewriters

# M32: constraints rust-analyzer answers (needs rust-analyzer on PATH;
# run from the example, a crate of its own).
cd examples/patterns/rust-semantic && colab refactor --script semantic.colab --format diff .

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
| `go-let/` | `let $NAME = $F \| strip_prefix("Get")` builds a new name from a whole-identifier capture, for a method and its call sites; `where $F matches /^Get[A-Z]/` picks the getters. |
| `go-relations/` | `any(inside '…', inside '…')` for two shapes of goroutine; `nth 1` for a function's first statement; `follows '$V, err := os.Stat($P)' directly` for the error check right after its call. |
| `delete-separators/` | `delete` of a keyword argument, a dict entry (Python) and an object property (JavaScript) with the comma beside it, first, middle or last, and of a line in a multi-line call. |
| `named/` | `pattern goroutine = '…'` in `shared.colab`, brought in by `include`, and a local `pattern cleanup`; `where inside test_func, not any(inside goroutine, inside cleanup)`. |
| `rewriters/` | `rewriter timeout = '…' -> '…'` and `let $NEW = $$$OPTS \| rewrite(timeout, retries)`: options renamed in place on one line and on many; `not has 'legacy: true'` (a property, found in every form the pattern compiles to) skips a call. |
| `rust-semantic/` | `where $X type "Option"` changes the `Option` unwrap and not the `Result` one; `where $F resolves "std::fs::read"` changes `std::fs::read` and not the local `mine::read`. |
