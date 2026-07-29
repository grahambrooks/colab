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
match       = "match" namespace string [ scope ] "{" action "}"
scope       = "in" string
namespace   = identifier "::" identifier
identifier  = [a-zA-Z_][a-zA-Z0-9_]* | keyword   -- see "Reserved tokens"
string      = '"' (any-char-except-double-quote)* '"'
action      = "replace" string
            | "delete"
            | "ensure"
            | "replace_call" string
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
`colab` from (and any `-C`) affects what a glob sees. An invalid glob is
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
`replace_call`.

`::` is the namespace separator. `//` starts a line comment.

**Keywords are still usable as namespace segments.** A backend's module
is named after the language's own construct, and C's is `#include` — so
`match c::include "stdio.h" { ... }` parses fine even though `include` is
also the pack directive. The two positions are never ambiguous, because a
namespace only ever follows `match`. The same holds for `delete`,
`ensure`, `replace`, and `in` should a backend ever want them as module
names.

**String literals have no escape sequences.** A backslash is a literal
backslash, which is what makes PHP namespaces (`"App\Old\Thing"`) work
naturally. It also means a target cannot contain a double quote.

## Future directions

The DSL is intentionally small. Capabilities being considered:

- A `wrap` action shorthand for the common
  `replace_call "f(ctx, $args)"` pattern.
- A query namespace (e.g. `go::regex`) for opt-in regex-based
  rewrites where a tree-sitter rule does not suffice.
- An `include "other.codemod"` directive so library packs compose.
- A scope-aware `<lang>::symbol` mode that respects shadowing.
  (`in "<glob>"` narrows a rename by *path*; this would narrow it by
  lexical scope.)

Track these in [`development-plan.md`](development-plan.md) and the project issue tracker.
