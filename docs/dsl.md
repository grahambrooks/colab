# Codemod DSL Reference

A codemod script tells `colab` what to find and how to rewrite it. The
language is small on purpose: each script is one named refactoring
containing zero or more `match` blocks, each pairing a *namespace* (the
target language and module) with an *action* (what to do at every
match site).

```
refactor "name" {
    match <lang>::<module> "<target>" { <action> }
    match <lang>::<module> "<target>" in "<glob>" { <action> }
    ...
}
```

`colab` parses, validates, and lowers the script before touching any
source files. Use [`colab explain --script foo.codemod`](./cli.md#explain)
to inspect the parsed IR; use `--format diff` (or `--check`) to preview
edits before writing.

## Grammar

Whitespace is insignificant outside string literals.

```ebnf
program     = "refactor" string "{" match* "}"
match       = "match" namespace string [ scope ] [ where ] "{" action import* "}"
scope       = "in" string
where       = "where" constraint ( "," constraint )*
constraint  = [ "not" ] test
test        = metavar "matches" ( regex | string )
            | metavar "is" ( identifier | string )
            | metavar "count" range
            | "inside" string
            | "has" string
range       = number | number ".." number | number ".." | ".." number
metavar     = "$" NAME | "$$$" NAME
regex       = "/" ( any-char-except-slash-or-newline | "\" any-char )+ "/"
import      = "requires" "import" string
            | "drops" "import" string "when" "unused"
namespace   = identifier "::" identifier
identifier  = [a-zA-Z_][a-zA-Z0-9_]* | keyword   -- see "Reserved tokens"
string      = '"' (any-char-except-double-quote)* '"'
            | "'" (any-char-except-single-quote)* "'"
action      = "replace" string
            | "delete"
            | "ensure"
            | "replace_call" string
            | "set" string
            | "insert" string
comment     = "//" any-char-except-newline*
```

Comments (`//`) are ignored anywhere whitespace is allowed.

## Actions

### `replace "<value>"`

Rewrite the matched element to `<value>`. The exact meaning of "the
matched element" depends on the namespace — see
[features.md](./features.md) for the per-namespace contract — but the
DSL surface is identical across backends:

```
match go::import "old.module" { replace "new.module" }
match rust::use  "tokio"      { replace "async_tokio" }
```

### `delete`

Remove the matched element. The match string is the value to find;
no replacement is supplied.

```
match go::import "fmt" { delete }
match rust::crate "old_crate" { delete }
```

### `ensure`

Idempotently add the matched element if no equivalent already exists.
The match string is the value to ensure.

```
match go::import "fmt" { ensure }
match python::import "os" { ensure }
```

`ensure` rules are always safe to re-run: the corpus harness asserts
each rule is a no-op on its own output.

### `replace_call "<template>"`

Rewrite a call expression whose callee text equals the match string.
Available on the `<lang>::call` module, which every backend provides.
The template is a string with substitution placeholders:

The target is the callee **exactly as the call reads in the source**, so
`Log.write` and a bare `write` are distinct targets in every backend.
PHP includes the `::`/`->`; Java and C# include the receiver. See the
[match-string conventions](#match-string-conventions) table.

| Placeholder | Expands to |
| ----------- | ---------- |
| `$1`, `$2`, … | The 1-indexed positional argument from the matched call. Out-of-range indices expand to the empty string. |
| `$args` | The original argument list, joined with `", "`. |
| `$func` | The matched function name (verbatim). |
| `$$` | A literal `$`. |

Examples:

```
// Pure rename, args passthrough.
match go::call "pkg.Old" { replace_call "pkg.New($args)" }

// Reorder positional args and add a literal.
match go::call "pkg.Old" { replace_call "pkg.New($2, $1, nil)" }

// Wrap-style: prepend a context argument.
match go::call "logger.Info" { replace_call "logger.WithContext(ctx).Info($args)" }
```

### `set '<value>'` and `insert '<value>'`

The config-file backends (`toml::key`, `json::key`, `markdown::block`)
address a key or block and give it a value written in the file's own
syntax. `set` creates the target — and any missing parents — or
overwrites a value that differs; `insert` creates it only when it is
absent and never overwrites. Values are compared by meaning, so both are
idempotent. `delete` removes the target.

```
match toml::key "project.profile_version" in "myspec.toml" { set '2' }
match toml::key "repos[repo=local].hooks[id=cargo-fmt]" in "prek.toml" {
    insert '{ id = "cargo-fmt", entry = "cargo fmt --all --check", stages = ["pre-commit"] }'
}
match json::key "/enabledPlugins/code-intelligence@gb-agent-skills" in ".claude/settings.json" { set 'true' }
```

Because `set` and `insert` act where the target is missing, an unscoped
rule would add it to every file of that type. Scope it with `in "<glob>"`
or run it on named files. The address syntax of each backend is in
[features.md](./features.md#config-file-backends).

> **Idempotency caveat.** `replace_call` is idempotent only if the
> template *renames the function*. Templates that keep the function
> name and add args (e.g. `f → f(ctx, $args)`) loop on subsequent
> passes; apply such transforms with a single `--write` run and
> verify with `--format diff` first. The corpus harness will refuse a
> case whose second pass is not a no-op.

## `include` directive

A script can pull in match clauses from another `.codemod` file by
listing `include "<path>"` inside its `refactor` block. The path
is resolved relative to the *including* script's directory (or as
an absolute path). The included file's outer `refactor "..."`
wrapper is dropped during expansion — only its match clauses are
spliced into the parent in source order, intermixed with sibling
matches.

```
// project/scripts/main.codemod
refactor "company-migration" {
    include "shared/javax-to-jakarta.codemod"
    match go::import "internal.old" { replace "internal.new" }
}
```

Cycle detection is path-canonical: a → b → a yields a clear
`circular include` error. Includes only resolve when the
compilation entry point has a known base path (the CLI's
`compile_at_path`); compiling a bare string raises a clear error
when an `include` directive is encountered.

## Structural patterns: `<lang>::pattern`

Every source backend has a `pattern` module whose target is **code with
metavariables** rather than a name. Use it for any change the fixed
modules cannot express:

```
refactor "modernise" {
    // Go: pkg/errors → stdlib wrapping
    match go::pattern 'errors.Wrap($ERR, $MSG)' {
        replace 'fmt.Errorf("%s: %w", $MSG, $ERR)'
    }
    // Go: drop debug logging, whole statement and its line
    match go::pattern 'log.Println("DEBUG", $$$ARGS)' { delete }
}
```

| Metavariable | Matches |
| ------------ | ------- |
| `$NAME` | exactly one syntax node (an expression, identifier, literal, …), bound to `NAME` |
| `$$$NAME` | zero or more sibling nodes — e.g. the rest of an argument list — bound to `NAME` |
| `$_` | one node, not bound |

Names are upper-case letters, digits and `_`. A name used twice must bind
identical text: `$A == $A` matches `x == x` but not `x == y`. Any other
`$` — a PHP variable, a `$` inside a string — is literal.

**`replace '<template>'`** substitutes each binding's source text into the
template: `$NAME` and `$$$NAME` (the whole span, separators included).
**`delete`** removes each match that stands alone on its line(s) — apart
from indentation and a trailing `;` or `,` — taking the line with it. A
match nested inside a larger expression (`() => f(x)`, `[f(x), 2]`) is left
in place, since removing it would leave broken code; `colab find` still
lists it, and `replace` can rewrite it.

**Never writes broken code.** If a file parsed cleanly and a pattern
rewrite would leave a syntax error in it — usually a template that is not
valid where it lands — that file is left unchanged and the rule reports no
change for it. When `colab find` shows matches in a file that `refactor`
did not change, check the template.

How it matches:

- **By syntax, not text.** The pattern is parsed with the language's own
  grammar and compared node by node, so whitespace, line breaks and
  comments inside the matched code do not matter, and `f(x)` never matches
  inside a string or comment.
- **Fragments are fine.** A pattern that is not a complete file — a Go
  expression, a Java statement — is retried inside wrappers (a function
  body, a class) until it parses. If it parses in none, the rule fails to
  compile (exit code 3) and the error names every wrapper tried.
- **Nested matches in one pass.** In `errors.Wrap(errors.Wrap(err, "a"),
  "b")` both calls match; the inner one is rewritten first and its result
  fills the outer template, so one run reaches the fixed point. Text the
  template introduces is never re-matched, so a wrap-style template
  (`f($A)` → `f(g($A))`) cannot run away.
- **Prefiltered.** The longest literal leaf in the pattern (`errors` above)
  must appear in a file before it is parsed. A pattern made only of
  metavariables and punctuation (`$A + $B`) parses every file of its
  language.
- **Quoting.** The DSL has no escapes, so write patterns containing `"`
  with single quotes, as above.

- **Templates are re-indented.** Lines after a template's first are
  indented to the match's column, so write a multi-line template
  flush-left in its final shape. (Space-indented code only; in
  tab-indented Go they stay flush-left.)

When a pattern matches nothing, look at it the way colab does:

```sh
colab ast --lang go --pattern 'errors.Wrap($ERR, $MSG)'   # the pattern's tree
colab ast --lang go --stdin < main.go                      # the code's tree
colab find -e "match go::pattern 'errors.Wrap(\$E, \$M)' { delete }" .   # where it matches
```

### Constraints: `where`

A `where` clause after the pattern (and after `in "<glob>"`, if any)
keeps only the matches that satisfy every constraint:

```
match go::pattern 'errors.Wrap($E, $M)'
    where $M matches /^"/, not inside 'func $F(t *testing.T) { $$$ }'
{
    replace 'fmt.Errorf($M+": %w", $E)'
}
```

| Constraint | Keeps a match when |
| ---------- | ------------------ |
| `$X matches /re/` | the text bound to `$X` matches the regular expression (unanchored; write `\/` for `/`). `matches "re"` is the same. |
| `$X is <kind>` | the node bound to `$X` has this syntax-node kind (`identifier`, `interpreted_string_literal`, …; `colab ast` shows them) |
| `$$$X count n..m` | the sequence binds between `n` and `m` nodes, separators not counted. `count 2`, `count 1..`, `count ..3` also work. |
| `inside '<pattern>'` | some enclosing node matches the pattern |
| `has '<pattern>'` | some node within the match matches the pattern |
| `not <constraint>` | the constraint does not hold |

- Only `<lang>::pattern` takes a `where` clause; any other module rejects
  it by name.
- A constraint must name a metavariable the pattern binds, with the same
  sigil (`$M`, or `$$$ARGS` for `count`). An unknown node kind gets a "did
  you mean". A regex or nested pattern that does not compile fails the
  script. All of these are caught at compile time, not mid-run.
- The metavariables inside an `inside` / `has` pattern are its own. `$F`
  above matches any test function; it is not compared with the outer
  pattern's bindings.
- Constraints only remove matches, so the prefilter stays sound, and
  `colab find` lists exactly what `refactor` would change.

### Imports: `requires import` and `drops import … when unused`

A template that writes a new package name needs the import, but only in
the files it changed. Import clauses go after the action, inside the
braces:

```
match go::pattern 'errors.Wrap($E, $M)' where $M matches /^"/ {
    replace 'fmt.Errorf($M+": %w", $E)'
    requires import "fmt"
    drops import "github.com/pkg/errors" when unused
}
```

- **`requires import "<x>"`** runs the backend's import-equivalent
  `ensure` (`go::import`, `rust::use`, `c::include`, `ruby::require`, …)
  on each file the rule rewrote, and on no other file. Every source
  backend supports it.
- **`drops import "<x>" when unused`** deletes the import from a file the
  rule rewrote, once nothing in that file can still refer to it.
  Supported for Go: an aliased import is used while its alias appears; an
  unaliased one while any name its package might have (`errors`; `yaml`
  for `gopkg.in/yaml.v3`) appears as an identifier. Blank (`_`) and dot
  imports are never dropped. Other backends reject the clause rather than
  guess. Delete those imports with a separate rule.
- Drops run before requires, so a file whose only import is replaced ends
  up with `import "fmt"`.
- The clauses belong to the rule. Its report line shows them, and when the
  rule changes nothing, they do nothing, so the rule stays idempotent.
- **Forgetting one is caught.** In Go and Python, a template that writes a
  qualifier (`fmt.`, `pathlib.`) the file neither used before nor imports
  afterwards is a [conflict](./cli.md#conflicts): `--write` writes
  nothing and exits 11. A later rule in the same script that adds the
  import (`match go::import "fmt" { ensure }`) also clears it.

## Inline rules

The `refactor "<name>" { … }` wrapper is optional on the command line.
Each `-e` / `--rule` value is one `match` block; the CLI wraps them in
`refactor "inline" { … }` and compiles them exactly like a file:

```sh
colab refactor -e 'match go::import "old/pkg" { replace "new/pkg" }' .
colab explain  -e 'match python::import "six" { delete }'
```

Parse errors are reported against the rules as typed — rule N is line N,
with the column inside that rule. Over MCP every tool already takes the
script as text, so the same one-liner works there wrapped in a
`refactor` block.

## Multi-rule scripts

A `refactor` block may contain any number of `match` blocks. Rules
are applied to each candidate file in source order; the file is
written back once after every rule has run.

```
refactor "tokio-major-bump" {
    match rust::crate "tokio" { replace "tokio-2" }
    match rust::use   "tokio" { replace "tokio_2" }
}
```

The empty form is also legal (useful as a placeholder pack):

```
refactor "stub" { }
```

## Namespaces

Each backend owns a namespace — `c`, `cpp`, `csharp`, `go`, `java`, `js`,
`kotlin`, `php`, `python`, `ruby`, `rust`, `swift` — and exposes one or
more *modules* within it. Every backend provides an import-equivalent
(named for the language's own construct), a `symbol` rename, and a `call`
rewrite; those with a namespace or package declaration expose one too. The full list is
machine-discoverable via `colab schema` and `colab list-rules <lang>`,
and is documented in [features.md](./features.md).

Namespaces colab does not implement produce a clear
`Error::UnsupportedOperation` (CLI exit code 3) instead of silently
no-oping.

## Match-string conventions

The match string is parsed by the backend that owns the namespace.
Conventions are consistent across backends but not identical; the
table below summarises:

| Module | Match string is | Examples |
| ------ | --------------- | -------- |
| `c::include` / `cpp::include` | Bare path, no delimiters. Matches both `<angle>` and `"quoted"` forms. | `"stdio.h"`, `"old/lib.h"` |
| `cpp::namespace` | Declared name, exact — including the nested form. | `"old_ns"`, `"a::b"` |
| `csharp::using` | Dotted name. For an alias, the right-hand side. | `"System.Text"` |
| `csharp::namespace` | Exact dotted namespace, block or file-scoped. | `"Old.App"` |
| `csharp::call` | Verbatim callee text. | `"Foo.Old"`, `"Old"` |
| `go::import` | Exact import path. | `"fmt"`, `"github.com/x/y"` |
| `go::package` | Package clause identifier. | `"oldpkg"` |
| `go::symbol` | Identifier text (rewrites every matching `identifier` / `type_identifier` / `field_identifier` in the file). | `"OldType"` |
| `go::struct_tag` | `<key>:<value>` pair (no quotes around value). | `"json:old_name"` |
| `go::call` | Verbatim source text of the function being called. | `"pkg.Old"`, `"Old"` |
| `<lang>::pattern` | Code with `$NAME` / `$$$NAME` / `$_` metavariables — see [Structural patterns](#structural-patterns-langpattern). | `'errors.Wrap($E, $M)'`, `'console.log($$$A)'` |
| `rust::use` | Leading path prefix, segment-wise. | `"tokio"`, `"tokio::sync"` |
| `rust::symbol` | Identifier text. | `"OldThing"` |
| `rust::crate` | Cargo.toml dependency key. | `"old_crate"` |
| `rust::call` | Verbatim function text. Method calls (`x.foo()`) excluded. | `"old_fn"`, `"pkg::old"` |
| `java::import` | Exact dotted import name. | `"java.util.List"` |
| `java::package` | Exact dotted package. | `"com.old"` |
| `java::symbol` | Identifier text. | `"OldGreeter"` |
| `java::call` | Verbatim callee text, receiver included. | `"Old.run"`, `"this.foo"` |
| `js::call` | Verbatim callee text. | `"mod.old"`, `"oldFn"` |
| `kotlin::import` | Qualified name. For an alias, the qualified name; for `a.b.*`, write `"a.b"`. | `"com.old.Client"` |
| `kotlin::package` | Exact dotted package. | `"com.old.app"` |
| `kotlin::call` | Verbatim callee text. | `"Client.run"` |
| `php::use` | Backslash-separated name. **No escapes** — one backslash. | `"App\Old\Thing"` |
| `php::namespace` | Exact backslash-separated namespace. | `"App\Old"` |
| `php::call` | Verbatim callee, including `::` or `->`. | `"Old::run"`, `"$obj->run"` |
| `python::call` | Verbatim callee text. | `"mod.old"`, `"old_fn"` |
| `ruby::require` | Quoted path from `require`/`require_relative`. | `"old/client"` |
| `ruby::symbol` | Identifier or constant text. Covers `module`/`class` names. | `"OldClient"` |
| `ruby::call` | Verbatim callee, receiver included. Parenthesised calls only. | `"Log.write"` |
| `swift::import` | Module path. | `"OldLog"`, `"UIKit.UIView"` |
| `swift::call` | Verbatim callee text. | `"Log.write"` |
| `python::import` | Leading dotted prefix, segment-wise (covers `import` and `from … import`). | `"old_pkg"`, `"old_pkg.sub"` |
| `python::symbol` | Identifier text. | `"old_helper"` |
| `js::import` | Exact ES module specifier (the string after `from`). | `"lodash"` |
| `js::symbol` | Identifier text (covers `identifier`, `property_identifier`, `shorthand_property_identifier`). | `"oldHelper"` |

In every case matching is on tree-sitter node text, not raw substrings
— `tokio` does **not** match `my_tokio` and `another.module` does
**not** match `yet.another.module`.

## Path scope: `in "<glob>"`

An optional `in "<glob>"` clause after the match string restricts one
rule to matching paths:

```
match rust::symbol "Config" in "crates/colab-core/**" { replace "CoreConfig" }
```

This matters most for `<lang>::symbol`, which is a whole-file syntactic
rename with no scope analysis: without a scope, an identically-named type
in an unrelated crate is renamed too. Scoping is enforced through the
rule's relevance check, so a file outside the glob is never parsed by that
rule — it shows up as a lower `scanned` count, not just a suppressed edit.

| Pattern | Matches |
| ------- | ------- |
| `core/**` | everything under a top-level `core/` |
| `**/core/**` | everything under any `core/` directory, at any depth |
| `src/*.rs` | Rust files directly in `src/`, not in subdirectories |
| `**/*_test.go` | test files anywhere |

`*` stops at a path separator; `**` crosses directories. Globs are matched
against the path as the walker yields it, so the directory you invoke
`colab` from (and any `-C`) affects what a glob sees. A leading `./` is
ignored, so `colab refactor -C repo --script s .` and a glob of `core/**`
agree; an absolute path argument yields absolute paths, which a relative
glob does not match. An invalid glob is
rejected when the script compiles rather than silently matching nothing.

`in` scopes a single rule; [`--include` / `--exclude`](./cli.md) filter the
whole run. Reach for `--include` to narrow what colab looks at, and `in`
when one rule in a multi-rule script needs a tighter boundary than the
rest. See [`examples/rust/scoped_rename`](../examples/rust/scoped_rename/)
for a worked example.

## Idempotency

Every transform must satisfy: applying it twice produces the same
result as applying it once.

The corpus harness (`crates/colab-dsl/tests/corpus.rs`) re-applies
every script in the `tests/corpus/` tree and fails if the second pass
diverges from the first. Idempotency is what makes `--check` and
`--dry-run` meaningful in CI.

The standard ways to achieve idempotency in your own scripts:

- **Renames** — make sure the new name doesn't match the old. Path
  matching is segment-wise so `tokio → async_tokio` is safe; substring
  pitfalls (`io → I/O`) usually break this rule and should be avoided.
- **`ensure`** is idempotent by construction.
- **`delete`** is idempotent by construction.
- **`replace_call`** — *change the function name*, or accept that the
  rule is single-pass and apply it once.

## Conflicts

Some mistakes are visible in the file a rule just changed, and colab
checks for them before writing (see [Conflicts](./cli.md#conflicts) in
the CLI reference):

- `match <lang>::symbol "a" { replace "b" }` where `b` already occurs in
  the file: the rename would merge two names.
- A Go or Python `pattern` template that introduces an unimported
  qualifier (above).
- With `--verify-lsp`, any error the language server sees after the change
  and not before.

A `--write` run with conflicts writes nothing and exits 11;
`--allow-conflicts` writes anyway. A `pattern` rule whose template would
not parse where it lands is skipped instead, and its report says so
(`unchanged`).

## Worked examples

### Rename a Go module across imports and one call

```
refactor "rename-pkg" {
    match go::import "github.com/example/old" { replace "github.com/example/new" }
    match go::call   "old.Init"               { replace_call "new.Init($args)" }
}
```

### Migrate `tokio` → `async_tokio` end-to-end

```
refactor "tokio-rename" {
    match rust::crate "tokio" { replace "async_tokio" }
    match rust::use   "tokio" { replace "async_tokio" }
}
```

### Drop a deprecated import and add the replacement

```
refactor "swap-logger" {
    match go::import "github.com/old/logger" { delete }
    match go::import "github.com/new/logger" { ensure }
}
```

### Rewrite struct tag and the matching column name

```
refactor "rename-user-name" {
    match go::struct_tag "json:user_name" { replace "json:username" }
    match go::struct_tag "db:user_name"   { replace "db:username"   }
}
```

### Reorder positional args of a renamed function

```
refactor "swap-args" {
    match go::call "pkg.Old" {
        replace_call "pkg.New($2, $1, nil)"
    }
}
```

### Header file (pack)

```
// java-jakarta-migration.codemod
//
// Mechanical javax → jakarta swaps. Runs the full set in one pass.
refactor "javax-to-jakarta" {
    match java::import "javax.annotation.Nonnull"  { replace "jakarta.annotation.Nonnull"  }
    match java::import "javax.persistence.Entity"  { replace "jakarta.persistence.Entity"  }
    match java::import "javax.servlet.http.HttpServletRequest" {
        replace "jakarta.servlet.http.HttpServletRequest"
    }
}
```

## Reserved tokens

The grammar keywords are:

`refactor`, `match`, `in`, `include`, `replace`, `delete`, `ensure`,
`replace_call`, `set`, `insert`, and for `where` / import clauses
`where`, `not`, `matches`, `is`, `count`, `inside`, `has`, `requires`,
`import`, `drops`, `when`, `unused`.

`::` is the namespace separator. `//` starts a line comment.

**Keywords are still usable as namespace segments.** A backend's module
is named after the language's own construct, and C's is `#include` — so
`match c::include "stdio.h" { ... }` parses fine even though `include` is
also the pack directive. The two positions are never ambiguous, because a
namespace only ever follows `match`. The same holds for `delete`,
`ensure`, `replace`, `in`, `set` and `insert`, and for every `where` and
import-clause keyword: `go::import` still parses.

**String literals have no escape sequences.** A backslash is a literal
backslash, which is what makes PHP namespaces (`"App\Old\Thing"`) work
naturally. It also means a double-quoted string cannot contain `"`, so a
second spelling exists: a single-quoted string, which cannot contain `'`.
Use it for values that are quoted text themselves — a TOML inline table,
a JSON object. Either spelling may span lines.

## Future directions

The DSL is intentionally small. Capabilities being considered:

- A `wrap` action shorthand for the common
  `replace_call "f(ctx, $args)"` pattern.
- A query namespace (e.g. `go::regex`) for opt-in regex-based
  rewrites where a tree-sitter rule does not suffice.
- Raw strings, for patterns that need both `'` and `"` (a Rust lifetime
  and a string literal); see [`dogfood-log.md`](dogfood-log.md).
- Type constraints (`$X has type T`) via the language server's hover.
- A scope-aware `<lang>::symbol` mode that respects shadowing.
  (`in "<glob>"` narrows a rename by *path*; this would narrow it by
  lexical scope.)

Track these in [`development-plan.md`](development-plan.md) and the project issue tracker.
