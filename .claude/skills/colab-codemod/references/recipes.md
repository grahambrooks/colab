# Namespace reference and worked recipes

Every namespace colab supports, what its match string means, and a
worked example. Match-string conventions differ per namespace — this is
the most common source of a rule that matches nothing.

## Capability matrix

Every backend has the same floor: an import-equivalent (named for the
language's own construct) taking `replace`/`delete`/`ensure`, a `symbol`
rename, and a `call` rewrite. Languages with a namespace or package
declaration expose one too.

| Namespace | Import-equivalent | Namespace/package | Extra | Applies to |
| --------- | ----------------- | ----------------- | ----- | ---------- |
| `c`      | `include` | — | — | `.c` `.h` |
| `cpp`    | `include` | `namespace` | — | `.cpp` `.cc` `.cxx` `.c++` `.hpp` `.hh` `.hxx` `.h++` `.h` |
| `csharp` | `using`   | `namespace` | — | `.cs` `.csx` |
| `go`     | `import`  | `package`   | `struct_tag` | `.go` |
| `java`   | `import`  | `package`   | — | `.java` |
| `js`     | `import`  | — | — | `.js` `.mjs` `.cjs` `.jsx` `.ts` `.tsx` |
| `kotlin` | `import`  | `package`   | — | `.kt` `.kts` |
| `php`    | `use`     | `namespace` | — | `.php` `.phtml` |
| `python` | `import`  | — | — | `.py` |
| `ruby`   | `require` | — | — | `.rb` `.rake` `.gemspec` `.ru` `Rakefile` `Gemfile` … |
| `rust`   | `use`     | — | `crate` (`.toml`, no `ensure`) | `.rs` `Cargo.toml` |
| `swift`  | `import`  | — | — | `.swift` |

So: `<lang>::symbol` and `<lang>::call` exist for all twelve. The
import-equivalent module always takes all three of
`replace`/`delete`/`ensure`, with two exceptions worth knowing:

- **`rust::crate` has no `ensure`** — adding a dependency needs a version
  the DSL cannot express. Use `cargo add`.
- **`.h` is claimed by both `c` and `cpp`.** The extension cannot tell
  them apart. A mixed script runs both over `.h`; every op is idempotent
  so the result is the same, at the cost of an extra parse.

Confirm at runtime rather than trusting this table:
`colab list-rules <lang>`.

## What the match string means

| Namespace | Match string is | Examples |
| --------- | --------------- | -------- |
| `c::include` / `cpp::include` | Bare path, no delimiters; matches `<angle>` and `"quoted"` alike. | `"stdio.h"`, `"old/lib.h"` |
| `cpp::namespace` | Declared name, exact — nested form included. | `"old_ns"`, `"a::b"` |
| `csharp::using` | Dotted name. For an alias, the right-hand side. | `"System.Text"` |
| `csharp::namespace` | Exact dotted namespace, block or file-scoped. | `"Old.App"` |
| `go::import` | Exact import path. | `"fmt"`, `"github.com/x/y"` |
| `go::package` | Package clause identifier. | `"oldpkg"` |
| `kotlin::import` | Qualified name; for `a.b.*` write `"a.b"`. | `"com.old.Client"` |
| `kotlin::package` | Exact dotted package. | `"com.old.app"` |
| `php::use` | Backslash-separated name, **single backslash**. | `"App\Old\Thing"` |
| `php::namespace` | Exact backslash-separated namespace. | `"App\Old"` |
| `ruby::require` | Quoted path from `require`/`require_relative`. | `"old/client"` |
| `swift::import` | Module path. | `"OldLog"`, `"UIKit.UIView"` |
| `<lang>::call` | Callee text **exactly as the call reads**, receiver included. | `"Log.write"`, `"Old::run"` (PHP), `"$obj->run"` (PHP) |
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

---

## Recipes for the newer backends

### Swap a header and its call sites (C / C++)

```
refactor "logger-v2" {
    match c::include "oldlog/log.h" { replace "newlog/log.h" }
    match c::include "<stdlib.h>" { ensure }
    match c::call "log_write" { replace_call "newlog_write($args)" }
}
```

The match string is the **bare path**, so `"oldlog/log.h"` matches both
`#include "oldlog/log.h"` and `#include <oldlog/log.h>`; a rename keeps
whichever style the file used. For `ensure` there is no existing
directive to copy, so wrap the target in `<>` to get the system form.

### Move a namespace (C++ / C# / PHP)

`namespace` rules rewrite the *declaration* only — qualified uses
elsewhere are `symbol` work, so a full move is usually two rules:

```
refactor "ns-move" {
    match cpp::namespace "old_ui" { replace "new_ui" }
    match cpp::symbol "old_ui" { replace "new_ui" }   // qualified uses
}
```

C# handles block-scoped and file-scoped declarations identically.

### C# usings, including the alias form

```
refactor "csharp-move" {
    match csharp::using "Old.Lib.Helpers" { replace "New.Lib.Helpers" }
    match csharp::using "System.Linq" { ensure }
}
```

For `using Alias = Foo.Bar;` the target is `Foo.Bar` — the thing being
imported, not the alias. `using static Foo.Bar;` matches the same way.

### PHP: mind the backslashes

DSL string literals have **no escape sequences**, so a namespace
separator is a single backslash:

```
refactor "php-move" {
    match php::namespace "App\Old" { replace "App\New" }
    match php::use "App\Old\Thing" { replace "App\New\Thing" }
    match php::call "Thing::oldRun" { replace_call "Thing::newRun($args)" }
}
```

`php::use` covers `use function` and `use const`. **Grouped imports
(`use App\Sub\{A, B};`) are deliberately not matched** — there is no
single node for one member, and rewriting half a group would corrupt it.
Expand the group first.

PHP call targets carry the call syntax: `"helper"`, `"Old::run"`, and
`"$obj->run"` are three distinct targets.

### Ruby requires

```
refactor "ruby-migrate" {
    match ruby::require "old/client" { replace "new/client" }
    match ruby::require "json" { ensure }
    match ruby::symbol "OldClient" { replace "NewClient" }
}
```

`require` is an ordinary method call, so matching is on the string
argument — a computed `require File.join(dir, 'x')` never matches. Rename
preserves the quote style and the `require`/`require_relative` form.
`ensure` places the new require below any magic comment.

Ruby has no namespace module: `module` and `class` names are constants,
which `ruby::symbol` already covers.

`ruby::call` rewrites **parenthesised calls only** — rewriting `puts x`
could change how the surrounding expression parses.

### Kotlin package move

```
refactor "kotlin-move" {
    match kotlin::package "com.old.app" { replace "com.new.app" }
    match kotlin::import "com.old.lib.Client" { replace "com.new.lib.Client" }
}
```

For `import a.b.C as D` the target is `a.b.C`, not the alias. A star
import `import a.b.*` is matched by `"a.b"` — the `*` is punctuation, not
part of the name — which correspondingly means `"a.b"` does *not* match
`import a.b.C`.

Trailing-lambda calls (`list.map { it }`) are skipped by `kotlin::call`:
a template cannot express a closure body.

### Swift module rename

```
refactor "swift-migrate" {
    match swift::import "OldNetworking" { replace "NewNetworking" }
    match swift::import "Foundation" { ensure }
    match swift::call "Client.fetchOld" { replace_call "Client.fetch($args)" }
}
```

Submodule (`UIKit.UIView`) and kind-qualified (`import class Old.Thing`)
forms both match; a rename keeps the kind keyword.

**Argument labels travel with their values**, so reordering with
positional placeholders keeps each label attached:

```
match swift::call "Old.run" { replace_call "New.run($2, $1)" }
// Old.run(first: a, second: b)  →  New.run(second: b, first: a)
```

### One script, many languages

Rules for a language colab is not currently looking at are skipped before
parsing, not run and discarded — so a polyglot script costs about what a
single-language one does. See
[`examples/polyglot`](../../../../examples/polyglot/) for a runnable
twelve-rule, six-language version.

```
refactor "logger-v2" {
    match c::include "oldlog/log.h" { replace "newlog/log.h" }
    match csharp::using "OldLog.Client" { replace "NewLog.Client" }
    match php::use "OldLog\Client" { replace "NewLog\Client" }
    match ruby::require "oldlog/client" { replace "newlog/client" }
    match kotlin::import "com.oldlog.Client" { replace "com.newlog.Client" }
    match swift::import "OldLog" { replace "NewLog" }
}
```
