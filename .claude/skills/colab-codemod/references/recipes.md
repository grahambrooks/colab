# Namespace reference and worked recipes

Every namespace colab supports, what its match string means, and a
worked example. Match-string conventions differ per namespace — this is
the most common source of a rule that matches nothing.

## Capability matrix

| Namespace | `replace` | `delete` | `ensure` | `replace_call` | Applies to |
| --------- | :-------: | :------: | :------: | :------------: | ---------- |
| `go::import`     | ✅ | ✅ | ✅ |    | `.go` |
| `go::symbol`     | ✅ |    |    |    | `.go` |
| `go::struct_tag` | ✅ |    |    |    | `.go` |
| `go::call`       |    |    |    | ✅ | `.go` |
| `rust::use`      | ✅ | ✅ | ✅ |    | `.rs` |
| `rust::symbol`   | ✅ |    |    |    | `.rs` |
| `rust::crate`    | ✅ | ✅ |    |    | `Cargo.toml` |
| `rust::call`     |    |    |    | ✅ | `.rs` |
| `java::import`   | ✅ | ✅ | ✅ |    | `.java` |
| `java::package`  | ✅ |    |    |    | `.java` |
| `java::symbol`   | ✅ |    |    |    | `.java` |
| `python::import` | ✅ | ✅ | ✅ |    | `.py` |
| `python::symbol` | ✅ |    |    |    | `.py` |
| `js::import`     | ✅ | ✅ |    |    | `.js` `.mjs` `.cjs` `.jsx` `.ts` `.tsx` |
| `js::symbol`     | ✅ |    |    |    | same as `js::import` |

Note the gaps: `js::import` has no `ensure`, `rust::crate` has no
`ensure`, and only Go and Rust have `call`. Asking for an unsupported
pair fails at compile time (exit 3) and names the actions that *are*
supported.

Confirm at runtime rather than trusting this table:
`colab list-rules <lang>`.

## What the match string means

| Namespace | Match string is | Examples |
| --------- | --------------- | -------- |
| `go::import` | Exact import path. | `"fmt"`, `"github.com/x/y"` |
| `go::symbol` | Identifier text. | `"OldType"` |
| `go::struct_tag` | `<key>:<value>`, **no quotes around the value**. | `"json:old_name"` |
| `go::call` | Verbatim source text of the function called. | `"pkg.Old"`, `"Old"` |
| `rust::use` | Leading path prefix, segment-wise. | `"tokio"`, `"tokio::sync"` |
| `rust::symbol` | Identifier text. | `"OldThing"` |
| `rust::crate` | `Cargo.toml` dependency key. | `"old_crate"` |
| `rust::call` | Verbatim function text. Method calls (`x.foo()`) excluded. | `"old_fn"`, `"pkg::old"` |
| `java::import` | Exact dotted import name. | `"java.util.List"` |
| `java::package` | Exact dotted package. | `"com.old"` |
| `java::symbol` | Identifier text. | `"OldGreeter"` |
| `python::import` | Leading dotted prefix, segment-wise. Covers `import x` and `from x import y`. | `"old_pkg"`, `"old_pkg.sub"` |
| `python::symbol` | Identifier text. | `"old_helper"` |
| `js::import` | Exact ES module specifier (the string after `from`). | `"lodash"` |
| `js::symbol` | Identifier text. | `"oldHelper"` |

Matching is on tree-sitter node text, never raw substrings: `tokio` does
not match `my_tokio`, and `another.module` does not match
`yet.another.module`.

**Segment-prefix vs exact** is the distinction that catches people out.
`rust::use "tokio"` matches `use tokio::sync::Mutex` — it rewrites the
leading segment and leaves the rest. `go::import "old/pkg"` matches only
the exact path `old/pkg`, because a Go import is one atomic string.

---

## Recipes

### Rename a dependency end-to-end (Rust)

The manifest and the source have to move together, in one script, so
neither can be forgotten.

```
refactor "tokio-fork" {
    match rust::crate "tokio" { replace "async_tokio" }
    match rust::use   "tokio" { replace "async_tokio" }
}
```

`rust::crate` edits `Cargo.toml` via `toml_edit`-validated line scanning;
`rust::use` rewrites the leading segment of every `use tokio::…`.
Re-running is a no-op.

### Move a Go module

```
refactor "logger-move" {
    match go::import "internal/oldlog" { replace "internal/log" }
}
```

If callers also reference the package by a different name after the move,
add a `go::call` rule — the import path and the call-site qualifier are
separate edits.

### Drop a dead import and guarantee its replacement (Python)

```
refactor "urllib2-to-requests" {
    match python::import "urllib2" { delete }
    match python::import "requests" { ensure }
}
```

`ensure` inserts the import only where it is missing, so it is safe to
re-run. Note that `ensure` is the one action with no cheap pre-filter —
it acts precisely when the target is *absent*.

### javax → jakarta (Java)

The classic migration. One rule per moved class; put them in a shared
pack and `include` it.

```
refactor "javax-to-jakarta" {
    match java::import "javax.servlet.http.HttpServletRequest" {
        replace "jakarta.servlet.http.HttpServletRequest"
    }
    match java::import "javax.servlet.http.HttpServletResponse" {
        replace "jakarta.servlet.http.HttpServletResponse"
    }
}
```

```
// callers/main.codemod
refactor "our-migration" {
    include "../packs/javax-to-jakarta.codemod"
    match java::package "com.old" { replace "com.new" }
}
```

`include` splices the other file's match clauses in source order; its
`refactor "..."` wrapper is dropped. Paths resolve relative to the
*including* script. Over MCP, `include` needs `cwd`.

### Rename a struct tag key or value (Go)

```
refactor "snake-to-camel" {
    match go::struct_tag "json:user_name" { replace "json:username" }
    match go::struct_tag "db:user_name"   { replace "db:username" }
}
```

The match string omits the quotes that appear in the source: the tag
`` `json:"user_name"` `` is matched by `"json:user_name"`. Tag *options*
are part of the value — `json:"name,omitempty"` is matched by
`"json:name,omitempty"`, not by `"json:name"`.

You can change the key as well as the value: `"json:foo"` →
`"protobuf:foo"` rewrites the key.

### Rewrite call sites, reordering arguments (Go / Rust)

```
refactor "api-v2" {
    // Straight rename, arguments passed through.
    match go::call "pkg.Old" { replace_call "pkg.New($args)" }

    // Reorder positional args and add a literal.
    match go::call "pkg.Fetch" { replace_call "pkg.Fetch2($2, $1, nil)" }
}
```

Template placeholders:

| Placeholder | Expands to |
| ----------- | ---------- |
| `$1`, `$2`, … | 1-indexed positional argument. Out of range → empty string. |
| `$args` | The original argument list, joined with `", "`. |
| `$func` | The matched function name, verbatim. |
| `$$` | A literal `$`. |

> **Idempotency trap.** A template that does not rename the function —
> `match go::call "f" { replace_call "f(ctx, $args)" }` — re-wraps on
> every run. Apply it with exactly one `--write` and keep it out of CI.
> Templates that rename the function are safe to re-run.

Method calls (`x.foo()`) are deliberately not matched by `rust::call`.

### Scoped symbol rename

When two crates share a type name, scope the rename to the one you mean:

```
refactor "core-config" {
    match rust::symbol "Config" in "crates/core/**" { replace "CoreConfig" }
}
```

Without `in`, both are renamed. See
[`examples/rust/scoped_rename`](../../../../examples/rust/scoped_rename/)
in this repo for a runnable version.

### Migrate a JS package

```
refactor "lodash-to-es-toolkit" {
    match js::import "lodash" { replace "es-toolkit" }
    match js::import "lodash/debounce" { replace "es-toolkit/debounce" }
}
```

Specifier matching is exact, so subpath imports need their own rule.
`js::import` has no `ensure`.

### A multi-rule script across concerns

Rules compose left to right on each file. This one changes an import, a
struct tag, and a call site in a single pass:

```
refactor "logger-v2" {
    match go::import "old/logger" { replace "new/logger" }
    match go::struct_tag "json:user_name" { replace "json:username" }
    match go::call "logger.Log" { replace_call "logger.Info($args)" }
}
```

The per-rule counts tell you all three landed:

```json
"rules":[{"i":0,"files":1,...},{"i":1,"files":1,...},{"i":2,"files":1,...}]
```

## Reusable packs

Put shared migrations in their own `.codemod` file and `include` them.
`colab pack list` shows the packs colab can find. A pack with no match
clauses compiles to zero rules and does nothing — the run reports
`scanned: N, changed: 0`, which is the signal that you included a
placeholder.

## Stdin

For an editor hook or a one-file check, skip the walker entirely:

```sh
cat foo.go | colab refactor --script s.codemod --stdin --path foo.go
```

`--path` is a hint used only to decide which rules are relevant; no file
is opened. The rewritten source goes to stdout.
