# Features by Backend

A capability matrix of every namespace and action `colab` supports
today, plus the per-backend caveats that decide whether a script is
right for your codebase. The same data is queryable at runtime via
[`colab schema`](./cli.md#schema), [`colab list-languages`](./cli.md#list-languages),
and [`colab list-rules <lang>`](./cli.md#list-rules) — this page is
just the readable form.

## At a glance

| Namespace | `replace` | `delete` | `ensure` | `replace_call` | `set` | `insert` | Applies to |
| --------- | :-------: | :------: | :------: | :------------: | :---: | :------: | ---------- |
| `c::include`        | ✅ | ✅ | ✅ |    |    |    | `.c`, `.h` |
| `c::symbol`         | ✅ |    |    |    |    |    | `.c`, `.h` |
| `c::call`           |    |    |    | ✅ |    |    | `.c`, `.h` |
| `cpp::include`      | ✅ | ✅ | ✅ |    |    |    | `.cpp`, `.cc`, `.cxx`, `.c++`, `.hpp`, `.hh`, `.hxx`, `.h++`, `.h` |
| `cpp::namespace`    | ✅ |    |    |    |    |    | as `cpp::include` |
| `cpp::symbol`       | ✅ |    |    |    |    |    | as `cpp::include` |
| `cpp::call`         |    |    |    | ✅ |    |    | as `cpp::include` |
| `csharp::using`     | ✅ | ✅ | ✅ |    |    |    | `.cs`, `.csx` |
| `csharp::namespace` | ✅ |    |    |    |    |    | `.cs`, `.csx` |
| `csharp::symbol`    | ✅ |    |    |    |    |    | `.cs`, `.csx` |
| `csharp::call`      |    |    |    | ✅ |    |    | `.cs`, `.csx` |
| `go::import`        | ✅ | ✅ | ✅ |    |    |    | `.go` |
| `go::package`       | ✅ |    |    |    |    |    | `.go` |
| `go::symbol`        | ✅ |    |    |    |    |    | `.go` |
| `go::struct_tag`    | ✅ |    |    |    |    |    | `.go` |
| `go::call`          |    |    |    | ✅ |    |    | `.go` |
| `java::import`      | ✅ | ✅ | ✅ |    |    |    | `.java` |
| `java::package`     | ✅ |    |    |    |    |    | `.java` |
| `java::symbol`      | ✅ |    |    |    |    |    | `.java` |
| `java::call`        |    |    |    | ✅ |    |    | `.java` |
| `js::import`        | ✅ | ✅ | ✅ |    |    |    | `.js`, `.mjs`, `.cjs`, `.jsx`, `.ts`, `.tsx` |
| `js::symbol`        | ✅ |    |    |    |    |    | as `js::import` |
| `js::call`          |    |    |    | ✅ |    |    | as `js::import` |
| `kotlin::import`    | ✅ | ✅ | ✅ |    |    |    | `.kt`, `.kts` |
| `kotlin::package`   | ✅ |    |    |    |    |    | `.kt`, `.kts` |
| `kotlin::symbol`    | ✅ |    |    |    |    |    | `.kt`, `.kts` |
| `kotlin::call`      |    |    |    | ✅ |    |    | `.kt`, `.kts` |
| `php::use`          | ✅ | ✅ | ✅ |    |    |    | `.php`, `.phtml` |
| `php::namespace`    | ✅ |    |    |    |    |    | `.php`, `.phtml` |
| `php::symbol`       | ✅ |    |    |    |    |    | `.php`, `.phtml` |
| `php::call`         |    |    |    | ✅ |    |    | `.php`, `.phtml` |
| `python::import`    | ✅ | ✅ | ✅ |    |    |    | `.py` |
| `python::symbol`    | ✅ |    |    |    |    |    | `.py` |
| `python::call`      |    |    |    | ✅ |    |    | `.py` |
| `ruby::require`     | ✅ | ✅ | ✅ |    |    |    | `.rb`, `.rake`, `.gemspec`, `.ru`, `Rakefile`, `Gemfile`, `Guardfile`, `Capfile` |
| `ruby::symbol`      | ✅ |    |    |    |    |    | as `ruby::require` |
| `ruby::call`        |    |    |    | ✅ |    |    | as `ruby::require` |
| `rust::use`         | ✅ | ✅ | ✅ |    |    |    | `.rs` |
| `rust::symbol`      | ✅ |    |    |    |    |    | `.rs` |
| `rust::crate`       | ✅ | ✅ |    |    |    |    | `Cargo.toml` |
| `rust::call`        |    |    |    | ✅ |    |    | `.rs` |
| `swift::import`     | ✅ | ✅ | ✅ |    |    |    | `.swift` |
| `swift::symbol`     | ✅ |    |    |    |    |    | `.swift` |
| `swift::call`       |    |    |    | ✅ |    |    | `.swift` |
| `json::key`         |    | ✅ |    |    | ✅ | ✅ | `.json` |
| `markdown::block`   |    | ✅ |    |    | ✅ | ✅ | `.md`, `.markdown` |
| `toml::key`         |    | ✅ |    |    | ✅ | ✅ | `.toml` |

**Config-file backends.** `json`, `markdown` and `toml` edit data files
rather than source, so they do not share the source floor below. Each has
one module whose target is an address — a JSON Pointer, a managed-block
name, or a dotted TOML path with `[field=value]` selectors — and the
`set` / `insert` / `delete` actions. `set` and `insert` create what is
missing, so scope them with `in "<glob>"` or run them on named files. See
[Config-file backends](#config-file-backends) below.

**The capability floor.** Every source backend provides an import-equivalent
(`replace`/`delete`/`ensure`), a `symbol` rename, and a `call` rewrite.
On top of that, languages with a namespace or package declaration expose
one, and Go additionally has `struct_tag` and Rust `crate`.

Two deliberate gaps:

- **`rust::crate` has no `ensure`.** Adding a dependency requires a
  version. Use `toml::key "dependencies.<name>" in "Cargo.toml" { insert '"1"' }`,
  or run `cargo add`.
- **Swift has no `package` module.** Swift source has no package or
  namespace declaration — module membership comes from the build system.

**`.h` is claimed by both `c` and `cpp`,** since the extension alone
cannot distinguish a C header from a C++ one. A script mixing `c::` and
`cpp::` rules runs both over `.h` files; every operation is idempotent,
so the result is the same either way, at the cost of one extra parse per
header. Narrow with `in "<glob>"` or `--include` if that matters.

The DSL is the same across backends; the table above just records
which `(module, action)` pairs are wired up. Asking for an
unsupported pair raises `Error::UnsupportedOperation` (CLI exit
code 3), naming the valid modules or actions and the closest match to
what you typed.

Any rule, in any backend, can be narrowed to a subset of the tree with
[`in "<glob>"`](./dsl.md#path-scope-in-glob). It is most useful on the
`symbol` rows, which are whole-file syntactic renames with no scope
analysis.

---

## Go (`colab-lang-go`)

Powered by `tree-sitter-go`.

### `go::import`

Import-path edits.

- **Match string:** the import path, **exactly** (no quotes around the
  path itself; no substring). `match go::import "fmt"` does not match
  `"my_fmt"` and does not match `"fmt/v2"`.
- **`replace`:** rewrite the import path. The surrounding `import (
  … )` block, alias prefix, and other imports stay put.
- **`delete`:** remove the entire `import_spec` line. Leaves
  `import (…)` block braces intact.
- **`ensure`:** insert `import "<target>"` immediately after the
  `package` clause when no existing `import_spec` references the
  target. Idempotent.

### `go::symbol`

In-file rename of identifier-like tokens.

- **Targets:** every `identifier`, `type_identifier`, and
  `field_identifier` whose text equals the match string.
- **Excludes:** package identifiers, label names, raw string
  contents, comments — all live under different tree-sitter kinds.
- **Caveat:** scope-blind. A local variable named `Foo` and a
  top-level `type Foo struct{}` are renamed together. Verify with
  `--format diff` before `--write`.

### `go::struct_tag`

Per-pair edits inside struct field tags.

- **Match string:** `<key>:<value>` (no quotes around the value).
  `match go::struct_tag "json:old_name"` finds `` `json:"old_name"` ``
  inside any field tag.
- **Scope:** only inside the `tag` field of a `field_declaration`.
  Other raw string literals (e.g. `var s = ` `` `json:"x"` ``) are
  left alone.
- **`replace`:** rewrites just the matched pair. Other pairs in the
  same backtick block (like `yaml:"…"`) survive untouched.
- **Tag values with options:** treated literally. To match
  `json:"name,omitempty"` you write
  `match go::struct_tag "json:name,omitempty"`.

### `go::call`

Templated rewrite of call expressions.

- **Match string:** the verbatim source text of the function being
  called, exactly. `pkg.Old`, `Old`, and `(*T).Old` are distinct.
- **`replace_call`:** see the [DSL reference](./dsl.md#replace_call-template)
  for placeholder syntax (`$1`, `$args`, `$func`, `$$`).
- **Idempotency:** templates that rename the function are idempotent;
  templates that keep the function name and add args are not. Apply
  once; the corpus harness will reject a non-idempotent rule.

---

## Rust (`colab-lang-rust`)

Powered by `tree-sitter-rust` and `toml_edit`.

### `rust::use`

Edits to `use` declarations.

- **Match string:** a leading **path prefix**, segment-wise. `tokio`
  matches `tokio`, `tokio::sync::Mutex`, `tokio as t`, and
  `tokio::*`, but never `my_tokio` or `foo::tokio::bar`.
- **`replace`:** rewrites the matched prefix. Multi-segment prefixes
  work: `tokio::sync` → `tokio_v2::sync` rewrites only that prefix.
- **`delete`:** removes the entire `use ...;` line.
- **`ensure`:** inserts `use <target>;` at the top of the file
  (after any leading inner attributes like `#![allow(...)]`) if no
  existing `use` already begins with the same path.

### `rust::symbol`

In-file rename of identifier-like tokens.

- **Targets:** `identifier`, `type_identifier`, `field_identifier`,
  and `shorthand_field_identifier` — so `Foo { x }` shorthand syntax
  also tracks renames of `x`.
- **Excludes:** macro names, lifetimes, label names, string
  contents.
- **Caveat:** scope-blind, like every other `<lang>::symbol`.

### `rust::crate`

Cargo.toml dependency edits.

- **Files:** any file named `Cargo.toml`.
- **Tables scanned:** `[dependencies]`, `[dev-dependencies]`,
  `[build-dependencies]`. Both inline-key and inline-table forms
  (`foo = "1"` and `foo = { version = "1" }`) are handled, plus the
  dotted-table form (`[dependencies.foo]`).
- **`replace`:** renames the matched key in place — preserves
  position, whitespace, comments, and key order. (`toml_edit`'s
  remove + insert would shuffle the renamed key to the end; we
  validate the key exists with `toml_edit::DocumentMut` then do a
  targeted line-scan rewrite.)
- **`delete`:** removes the dep entry. Inline-key removes the line;
  dotted-table form removes the section header and every line until
  the next section.
- **No `ensure`** today: adding a dep requires a version, which is
  out of scope for the syntactic rewriter.

### `rust::call`

Templated rewrite of call expressions.

- **Match string:** verbatim source text of the function. Method
  calls (`x.foo()`) are excluded by tree-sitter kind, so a rule
  targeting `foo` cannot collide with a method.
- See [`go::call`](#gocall) for template behaviour and idempotency
  rules; the implementation is shared via `colab_core::template`.

---

## Java (`colab-lang-java`)

Powered by `tree-sitter-java`.

### `java::import`

- **Match string:** the dotted import name, exactly. `import static`
  declarations parse as the same node and are matched uniformly.
- **`replace`:** rewrites the dotted name; the `import` keyword,
  `static` modifier, and trailing semicolon stay put.
- **`delete`:** removes the entire `import_declaration` line.
- **`ensure`:** inserts `import <target>;` after the
  `package_declaration` if no matching import exists.

### `java::package`

- **Match string:** exact dotted package name.
- **`replace`:** rewrites the file's `package` declaration only when
  the current package matches. Files in a different package are
  untouched.

### `java::symbol`

- **Targets:** `identifier` and `type_identifier` whose text equals
  the match string. Covers classes, methods, fields, constructors,
  parameters, and locals.
- **Excludes:** string literals, comments, javadoc.
- **Caveat:** scope-blind.

---

## Python (`colab-lang-python`)

Powered by `tree-sitter-python`.

### `python::import`

- **Match string:** a **dotted prefix**, segment-wise. `old_pkg`
  matches `import old_pkg`, `import old_pkg.sub`,
  `from old_pkg.sub import foo`, but not `old_pkgother`.
- **`replace`:** rewrites the matched prefix in `import_statement`
  and `import_from_statement` (including aliased forms
  `import foo as bar`).
- **`delete`:** removes the whole import line.
- **`ensure`:** inserts `import <target>` at the file top (after
  module docstring and `from __future__ import ...` lines) when no
  existing import segment-prefix-covers the target.

### `python::symbol`

- **Targets:** every `identifier` whose text equals the match
  string.
- **Excludes:** string literals (any kind of quoting), comments.
- **Caveat:** scope-blind. Local variables that shadow a top-level
  name are also renamed.

---

## JavaScript / TypeScript (`colab-lang-js`)

Powered by `tree-sitter-javascript`. The JavaScript grammar parses
TypeScript module syntax well enough for specifier and identifier
rewriting; type-aware operations are out of scope.

### `js::import`

- **Match string:** the **exact** ES module specifier (the string
  after `from`). `lodash` does not match `lodash-es`.
- **Targets:** `import_statement` and `export_statement` with a
  `source` field.
- **`replace`:** rewrites just the inner string (preserves quotes:
  single-quoted stays single-quoted).
- **`delete`:** removes the whole `import` / `export ... from`
  statement.

### `js::symbol`

- **Targets:** `identifier`, `property_identifier`,
  `shorthand_property_identifier`. Covers function names, variable
  names, object keys, JSX component names.
- **Excludes:** string literals, JSX text, regex literals.
- **Caveat:** scope-blind.

---

## What `colab` does *not* do

These are deliberate non-goals. They live in
[`development-plan.md`](development-plan.md) under "Non-goals" and
conflict with colab's syntactic-rewriter premise.

- **Whole-program semantic analysis.** No type inference, no
  cross-file binding resolution. If your refactor needs to "rename
  the field on the type returned by `Builder::build()`," that's a
  language-specific tool's job.
- **Cross-file move.** Moving a class/struct/type to a new module
  needs to update every importer. `colab` can do the *import*
  rewrite half via `<lang>::import` rules, but the move itself
  requires whole-program reasoning we deliberately don't do.
- **Scope-aware rename.** `<lang>::symbol` does not know which
  occurrences of `x` refer to which binding. We compensate with
  `--format diff` and the corpus idempotency check; for type-aware
  renames use IDE refactoring or a language-specific tool.
- **Conflict resolution between simultaneous edits.** Rules in one
  script are applied in source order, sequentially. There is no
  "merge" semantic.
- **Plugin marketplace.** Premature until `LanguageBackend` and the
  capability registry stabilise further.

## See also

- [`docs/dsl.md`](./dsl.md) — language reference, action semantics, examples.
- [`docs/cli.md`](./cli.md) — flags, exit codes, format selection.
- [`architecture.md`](architecture.md) — workspace layout and how
  to add a new backend or action.
- [`development-plan.md`](development-plan.md) — roadmap and
  non-goals.

---

## C (`colab-lang-c`) and C++ (`colab-lang-cpp`)

Powered by `tree-sitter-c` and `tree-sitter-cpp`.

**Include paths are matched bare.** The delimiters are part of the
source, not the target: `"old/lib.h"` matches both `#include "old/lib.h"`
and `#include <old/lib.h>`. A rename preserves whichever style the file
already used, so a codemod never silently converts a local include into a
system one.

For `ensure` there is no existing directive to copy the style from, so
wrap the target in angle brackets to ask for the system form:

```
match c::include "<stdlib.h>" { ensure }   // #include <stdlib.h>
match c::include "local.h"    { ensure }   // #include "local.h"
```

`cpp::namespace` matches the declared name exactly, including the nested
form — `"a::b"` matches `namespace a::b {` while `"a"` does not.
Anonymous namespaces never match. It rewrites the *declaration* only;
qualified uses elsewhere are `cpp::symbol` work, so a full namespace
rename is usually two rules.

C has no namespace module. Neither backend does macro expansion — colab
sees the source as written.

## C# (`colab-lang-csharp`)

Powered by `tree-sitter-c-sharp`.

`csharp::using` covers all three forms. For the alias form the target is
the **right-hand side**, since that is the thing being imported:

```
using System.Text;         // target "System.Text"
using static Foo.Bar;      // target "Foo.Bar"
using Alias = Foo.Bar;     // target "Foo.Bar", not "Alias"
```

`csharp::namespace` handles both block-scoped and file-scoped (C# 10+)
declarations identically.

## PHP (`colab-lang-php`)

Powered by `tree-sitter-php`.

**Backslashes are literal.** DSL string literals have no escape
sequences, so a PHP namespace separator is written as a single
backslash: `match php::use "App\Old\Thing"`.

`php::use` covers the `use function` and `use const` forms. **Grouped
imports are deliberately not matched** — `use App\Sub\{A, B};` has no
single node for `App\Sub\A`, and rewriting half a group would corrupt
it. Expand the group first.

`php::call` targets include the call syntax: `"helper"`, `"Old::run"`,
and `"$obj->run"` are three distinct targets.

## Ruby (`colab-lang-ruby`)

Powered by `tree-sitter-ruby`.

Ruby has no import statement — `require 'foo'` is an ordinary method
call — so `ruby::require` matches a call to `require`/`require_relative`
whose argument is a **string literal**. A computed require
(`require File.join(dir, 'x')`) never matches, because colab cannot know
what it resolves to. Rename preserves the quote style and the
require/require_relative form already in the file.

There is no separate namespace module: Ruby `module` and `class` names
are constants, which `ruby::symbol` already covers.

`ruby::call` rewrites **parenthesised calls only**. A paren-less call
(`puts x`) parses as a call too, but rewriting it with a template that
adds parentheses could change how the surrounding expression parses, so
those are skipped.

## Kotlin (`colab-lang-kotlin`)

Powered by `tree-sitter-kotlin-ng`.

For an aliased import (`import a.b.C as D`) the target is the qualified
name `a.b.C`, not the alias.

**Star imports are matched by their package prefix.** The `*` is
punctuation and not part of the name node, so `import a.b.*` is matched
by `"a.b"` — which correspondingly does *not* match `import a.b.C`.

`kotlin::call` skips trailing-lambda calls (`list.map { it }`): a
template cannot express a closure body, so rewriting one would lose code.
When a call has both parenthesised arguments and a trailing lambda, only
the parenthesised part is rewritten.

## Swift (`colab-lang-swift`)

Powered by `tree-sitter-swift`.

`swift::import` covers plain, submodule (`UIKit.UIView`), and
kind-qualified (`import class Old.Thing`) forms; a rename keeps the kind
keyword.

**Argument labels travel with their values.** `f(name: x)` exposes
`name: x` as one argument, so reordering with `$1`/`$2` keeps each label
attached to its value.

Like Kotlin, trailing-closure calls are skipped. There is no
`swift::package` — Swift source has no package declaration.

## Config-file backends (`colab-lang-json`, `colab-lang-markdown`, `colab-lang-toml`)

These edit configuration and documentation files, where the unit of change
is a key or a block rather than a syntax node. They exist so a repository
convention (a new pre-commit hook, a settings entry, a managed section of
`AGENTS.md`) can be applied across many repositories by the same scripts
and `--check` gate as a source refactoring.

All three share the same three actions:

| Action | Present, same value | Present, different | Absent |
| ------ | ------------------- | ------------------ | ------ |
| `set '<value>'`    | no change | overwritten | created, with missing parents |
| `insert '<value>'` | no change | **left alone** | created, with missing parents |
| `delete`           | removed   | removed        | no change |

Values are compared by meaning, not text, so a `set` whose value is already
present in another format is a no-op, and every rule is idempotent. A file
that does not parse (or, for Markdown, has repeated or unbalanced markers)
is left untouched. Values are usually quoted text themselves, so write them
in single quotes: `set '{ id = "gitleaks" }'`.

### `toml::key` — dotted path with selectors

```
match toml::key "project.profile_version" in "myspec.toml" { set '2' }
match toml::key "repos[repo=local].hooks[id=cargo-fmt]" in "prek.toml" {
  insert '{ id = "cargo-fmt", entry = "cargo fmt --all --check", stages = ["pre-commit"] }'
}
```

- Segments are separated by `.`. `name[field=value]` selects the element of
  an array — of tables (`[[repos]]`) or of inline tables
  (`hooks = [{…}]`) — whose `field` equals `value`. A value for a selected
  element must be an inline table carrying that field, or the element could
  never be found again; the script is rejected otherwise.
- A missing selected parent is created holding just its selector field
  (`[[repos]]` with `repo = "local"`).
- Edits go through `toml_edit` (TOML 1.1, so multi-line inline tables
  parse). An overwritten value keeps its trailing comment; an element
  appended to a one-per-line array takes the previous element's layout.
- Keys containing `.`, `[` or `]` cannot be addressed.

### `json::key` — JSON Pointer

```
match json::key "/enabledPlugins/code-intelligence@gb-agent-skills" in ".claude/settings.json" { set 'true' }
```

- RFC 6901: `~1` is `/` inside a key, `~0` is `~`.
- The edit is a single splice of the file's text, located with
  tree-sitter-json: key order, indentation and every other byte are kept. A
  new member follows the last one, one per line or inline to match the
  object; missing parent objects are written compactly on one line.
- An array step must be an existing index; `set` and `insert` never grow
  arrays, and `delete` does not remove array elements.

### `markdown::block` — managed block

```
match markdown::block "myspec" in "AGENTS.md" { set '## myspec
- run `make check` before committing' }
```

- The block is the lines between `<!-- <name>:begin … -->` (anything after
  the name is kept) and `<!-- <name>:end -->`. Text outside the markers is
  never touched.
- `set` makes the content exactly the value, with leading and trailing blank
  lines dropped. A missing block is appended at the end of the file, after a
  blank line. `delete` removes the markers too.
