# Features by Backend

A capability matrix of every namespace and action `colab` supports
today, plus the per-backend caveats that decide whether a script is
right for your codebase. The same data is queryable at runtime via
[`colab schema`](./cli.md#schema), [`colab list-languages`](./cli.md#list-languages),
and [`colab list-rules <lang>`](./cli.md#list-rules) — this page is
just the readable form.

## At a glance

| Namespace | `replace` | `delete` | `ensure` | `replace_call` | File extensions |
| --------- | :-------: | :------: | :------: | :------------: | --------------- |
| `go::import`        | ✅ | ✅ | ✅ |   | `.go` |
| `go::symbol`        | ✅ |   |   |   | `.go` |
| `go::struct_tag`    | ✅ |   |   |   | `.go` |
| `go::call`          |   |   |   | ✅ | `.go` |
| `rust::use`         | ✅ | ✅ | ✅ |   | `.rs` |
| `rust::symbol`      | ✅ |   |   |   | `.rs` |
| `rust::crate`       | ✅ | ✅ |   |   | `Cargo.toml` |
| `rust::call`        |   |   |   | ✅ | `.rs` |
| `java::import`      | ✅ | ✅ | ✅ |   | `.java` |
| `java::package`     | ✅ |   |   |   | `.java` |
| `java::symbol`      | ✅ |   |   |   | `.java` |
| `python::import`    | ✅ | ✅ | ✅ |   | `.py` |
| `python::symbol`    | ✅ |   |   |   | `.py` |
| `js::import`        | ✅ | ✅ |   |   | `.js`, `.mjs`, `.cjs`, `.jsx`, `.ts`, `.tsx` |
| `js::symbol`        | ✅ |   |   |   | `.js`, `.mjs`, `.cjs`, `.jsx`, `.ts`, `.tsx` |

The DSL is the same across backends; the table above just records
which `(module, action)` pairs are wired up. Asking for an
unsupported pair raises `Error::UnsupportedOperation` (CLI exit
code 3).

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
`DEVELOPMENT_PLAN.md` under "Non-goals" and conflict with colab's
syntactic-rewriter premise.

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
- [`ARCHITECTURE.md`](../ARCHITECTURE.md) — workspace layout and how
  to add a new backend or action.
- [`DEVELOPMENT_PLAN.md`](../DEVELOPMENT_PLAN.md) — roadmap and
  non-goals.
