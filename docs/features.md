# Features by Backend

A capability matrix of every namespace and action `colab` supports
today, plus the per-backend caveats that decide whether a script is
right for your codebase. The same data is queryable at runtime via
[`colab schema`](./cli.md#colab-schema), [`colab list-languages`](./cli.md#colab-list-languages),
and [`colab list-rules <lang>`](./cli.md#colab-list-rules-lang) — this page is
just the readable form.

This page lists what each backend offers. Whether a feature *works* the
same in every language (a lone `$$$B` body, modifiers left out of a
pattern, a deleted item taking its docs, `colab:ignore`, generalizing an
edit, …) is in [`conformance.md`](./conformance.md): a feature ×
language matrix that a test generates by running a case in each of the
13 source languages. Each known gap there comes with a one-line
diagnosis.

## At a glance

| Namespace | `replace` | `delete` | `ensure` | `replace_call` | `set` | `insert` | `create` | Applies to |
| --------- | :-------: | :------: | :------: | :------------: | :---: | :------: | :------: | ---------- |
| `c::include`        | ✅ | ✅ | ✅ |    |    |    |    | `.c`, `.h` |
| `c::symbol`         | ✅ |    |    |    |    |    |    | `.c`, `.h` |
| `c::call`           |    |    |    | ✅ |    |    |    | `.c`, `.h` |
| `c::pattern`        | ✅ | ✅ |    |    |    |    | ✅ | `.c`, `.h` |
| `cpp::include`      | ✅ | ✅ | ✅ |    |    |    |    | `.cpp`, `.cc`, `.cxx`, `.c++`, `.hpp`, `.hh`, `.hxx`, `.h++`, `.h` |
| `cpp::namespace`    | ✅ |    |    |    |    |    |    | as `cpp::include` |
| `cpp::symbol`       | ✅ |    |    |    |    |    |    | as `cpp::include` |
| `cpp::call`         |    |    |    | ✅ |    |    |    | as `cpp::include` |
| `cpp::pattern`      | ✅ | ✅ |    |    |    |    | ✅ | as `cpp::include` |
| `csharp::using`     | ✅ | ✅ | ✅ |    |    |    |    | `.cs`, `.csx` |
| `csharp::namespace` | ✅ |    |    |    |    |    |    | `.cs`, `.csx` |
| `csharp::symbol`    | ✅ |    |    |    |    |    |    | `.cs`, `.csx` |
| `csharp::call`      |    |    |    | ✅ |    |    |    | `.cs`, `.csx` |
| `csharp::pattern`   | ✅ | ✅ |    |    |    |    | ✅ | `.cs`, `.csx` |
| `go::import`        | ✅ | ✅ | ✅ |    |    |    |    | `.go` |
| `go::package`       | ✅ |    |    |    |    |    |    | `.go` |
| `go::symbol`        | ✅ |    |    |    |    |    |    | `.go` |
| `go::struct_tag`    | ✅ |    |    |    |    |    |    | `.go` |
| `go::call`          |    |    |    | ✅ |    |    |    | `.go` |
| `go::pattern`       | ✅ | ✅ |    |    |    |    | ✅ | `.go` |
| `java::import`      | ✅ | ✅ | ✅ |    |    |    |    | `.java` |
| `java::package`     | ✅ |    |    |    |    |    |    | `.java` |
| `java::symbol`      | ✅ |    |    |    |    |    |    | `.java` |
| `java::call`        |    |    |    | ✅ |    |    |    | `.java` |
| `java::pattern`     | ✅ | ✅ |    |    |    |    | ✅ | `.java` |
| `js::import`        | ✅ | ✅ | ✅ |    |    |    |    | `.js`, `.mjs`, `.cjs`, `.jsx`, `.ts`, `.tsx` |
| `js::symbol`        | ✅ |    |    |    |    |    |    | as `js::import` |
| `js::call`          |    |    |    | ✅ |    |    |    | as `js::import` |
| `js::pattern`       | ✅ | ✅ |    |    |    |    | ✅ | as `js::import` |
| `kotlin::import`    | ✅ | ✅ | ✅ |    |    |    |    | `.kt`, `.kts` |
| `kotlin::package`   | ✅ |    |    |    |    |    |    | `.kt`, `.kts` |
| `kotlin::symbol`    | ✅ |    |    |    |    |    |    | `.kt`, `.kts` |
| `kotlin::call`      |    |    |    | ✅ |    |    |    | `.kt`, `.kts` |
| `kotlin::pattern`   | ✅ | ✅ |    |    |    |    | ✅ | `.kt`, `.kts` |
| `php::use`          | ✅ | ✅ | ✅ |    |    |    |    | `.php`, `.phtml` |
| `php::namespace`    | ✅ |    |    |    |    |    |    | `.php`, `.phtml` |
| `php::symbol`       | ✅ |    |    |    |    |    |    | `.php`, `.phtml` |
| `php::call`         |    |    |    | ✅ |    |    |    | `.php`, `.phtml` |
| `php::pattern`      | ✅ | ✅ |    |    |    |    | ✅ | `.php`, `.phtml` |
| `python::import`    | ✅ | ✅ | ✅ |    |    |    |    | `.py` |
| `python::symbol`    | ✅ |    |    |    |    |    |    | `.py` |
| `python::call`      |    |    |    | ✅ |    |    |    | `.py` |
| `python::pattern`   | ✅ | ✅ |    |    |    |    | ✅ | `.py` |
| `rust::use`         | ✅ | ✅ | ✅ |    |    |    |    | `.rs` |
| `rust::symbol`      | ✅ |    |    |    |    |    |    | `.rs` |
| `rust::crate`       | ✅ | ✅ |    |    |    |    |    | `Cargo.toml` |
| `rust::call`        |    |    |    | ✅ |    |    |    | `.rs` |
| `rust::pattern`     | ✅ | ✅ |    |    |    |    | ✅ | `.rs` |
| `swift::import`     | ✅ | ✅ | ✅ |    |    |    |    | `.swift` |
| `swift::symbol`     | ✅ |    |    |    |    |    |    | `.swift` |
| `swift::call`       |    |    |    | ✅ |    |    |    | `.swift` |
| `swift::pattern`    | ✅ | ✅ |    |    |    |    | ✅ | `.swift` |
| `ts::import` | ✅ | ✅ | ✅ |  |  |  |  | `.ts`, `.mts`, `.cts` |
| `ts::symbol` | ✅ |  |  |  |  |  |  | as `ts::import` |
| `ts::call` |  |  |  | ✅ |  |  |  | as `ts::import` |
| `ts::pattern` | ✅ | ✅ |  |  |  |  | ✅ | as `ts::import` |
| `tsx::import` | ✅ | ✅ | ✅ |  |  |  |  | `.tsx` |
| `tsx::symbol` | ✅ |  |  |  |  |  |  | as `tsx::import` |
| `tsx::call` |  |  |  | ✅ |  |  |  | as `tsx::import` |
| `tsx::pattern` | ✅ | ✅ |  |  |  |  | ✅ | as `tsx::import` |
| `json::key`         |    | ✅ |    |    | ✅ | ✅ |    | `.json` |
| `markdown::section` | ✅ | ✅ |    |    | ✅ | ✅ |    | `.md`, `.markdown` |
| `markdown::frontmatter` |  | ✅ |    |    | ✅ | ✅ |    | `.md`, `.markdown` |
| `markdown::text` | ✅ | ✅ |  |  |  |  |    | `.md`, `.markdown` |
| `markdown::block`   |    | ✅ |    |    | ✅ | ✅ |    | `.md`, `.markdown` |
| `toml::key`         |    | ✅ |    |    | ✅ | ✅ |    | `.toml` |
| `text::literal` | ✅ | ✅ |  |  |  |  |    | any text file |
| `text::word` | ✅ | ✅ |  |  |  |  |    | any text file |
| `markdown::row` |  | ✅ |  |  | ✅ | ✅ |    | `.md`, `.markdown` |
| `markdown::column` | ✅ | ✅ |  |  |  | ✅ |  | `.md`, `.markdown` |
| `markdown::item` | ✅ | ✅ | ✅ |  |  |  |    | `.md`, `.markdown` |
| `markdown::link` | ✅ | ✅ |  |  |  |  |    | `.md`, `.markdown` |

**Config-file backends.** `json`, `markdown` and `toml` edit data files
rather than source, so they do not share the source floor below. Their
modules take an address — a JSON Pointer, a dotted TOML path with
`[field=value]` selectors, a Markdown heading path or anchor (`section`),
a table-row or list-item selector (`row`, `item`), a phrase (`text`), a
link destination (`link`), a front-matter key, or a managed-block name.
Most take `set` / `insert` / `delete`; `item` takes `ensure` / `replace` /
`delete`, `text` and `link` take `replace` / `delete`, and `section` also
renames a heading with `replace`. `set` and `insert` create what is
missing, so scope them with `in "<glob>"` or run them on named files. See
[Config-file backends](#config-file-backends-colab-lang-json-colab-lang-markdown-colab-lang-toml) below.

**Text.** `text::literal` and `text::word` read a file as plain text,
not through a grammar, so one rule reaches every file type: a name in Go
strings, YAML, shell and Markdown at once. They take `replace` and
`delete`, and run on every text file the walk reaches, so scope them. See
[Text](#text-colab-lang-text) below.

**The capability floor.** Every source backend provides an import-equivalent
(`replace`/`delete`/`ensure`), a `symbol` rename, a `call` rewrite, and a
`pattern` module.
On top of that, languages with a namespace or package declaration expose
one, and Go additionally has `struct_tag` and Rust `crate`.

**`<lang>::pattern` is the escape hatch.** Its target is code with
`$NAME` / `$$$NAME` / `$_` metavariables, matched structurally with the
language's own grammar — see [Structural patterns](./dsl.md#structural-patterns-langpattern).
Use it for any shape the named modules do not cover. Every
`<lang>::pattern` also takes a [`where` clause](./dsl.md#constraints-where)
(`matches`, `is`, `count`, `inside`, `has`, `not`), and every source
backend accepts [`requires import "<x>"`](./dsl.md#imports-requires-import-and-drops-import--when-unused)
on any rule. `drops import "<x>" when unused` is Go-only: the other
backends cannot tell whether an import is still used, so they reject the
clause. Go, Python and JS templates are also checked for a qualifier
(`fmt.`, `pathlib.`, `lodash.`) that nothing imports (a JS global such as
`console` or `process` needs none), and every `symbol` rename for
a new name the file already uses; both are [conflicts](./cli.md#conflicts),
which stop a `--write` before it writes. Per-language notes:

- **Go** matches a bare identifier (`x`) only where it is used; a
  declared name (`var x`, `func x`) is not matched. Rename declarations
  with `go::symbol`.
- **Python** patterns are also compiled as type annotations, so
  `Optional[$T]` matches both `cast(Optional[str], x)` and
  `def f(a: Optional[str])`, which parse as different nodes.
- **PHP** patterns are parsed as pure PHP (no surrounding HTML), and a
  metavariable cannot stand where only a `$variable` may — `$X = $Y;`
  does not compile, `$y = $X;` does.
- **JavaScript** patterns are parsed with the JavaScript grammar, so they
  cannot contain TypeScript-only syntax (type annotations), though they
  match inside `.ts` files.
- **Rust** patterns may be struct fields (`pub conflicts: $T`) and struct-literal fields (`conflicts: $C`), so they work in `where has` / `not has`. A field pattern that could be either (`conflicts: $C`) matches both. `field: $X` also matches the shorthand `field` (see [Field shorthand](dsl.md#structural-patterns-langpattern)). One shape still does not match: `$$$` cannot stand for a struct's field list (spell the fields out).

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

### Rules inside Markdown code fences

Every source and config backend's rules can also run inside Markdown
code fences of their language, with `and fences` (both) or `in fences`
(only the fences), so a migration updates README and docs examples in the
same run. See [Code fences](./dsl.md#code-fences-in-fences-and-and-fences).

### Suppressions and findings

Any rule, in any backend, skips a line marked with a `colab:ignore`
comment (optionally naming the script or the `<lang>::<module>`), and
any rule can carry a `message` and a `severity` for reports and
`--check`. See [Findings](./dsl.md#findings-message-severity-id-and-url) and
[Suppressing a site](./dsl.md#suppressing-a-site-colabignore).

## Go (`colab-lang-go`)

Powered by colab's own Go parser (`colab-parse-go`): pure Rust, lossless,
reading around syntax errors. Its trees have tree-sitter-go's kinds and
fields, so rules written for them read as before; over 22,537 real Go
files tree-sitter reads cleanly, 99.2% of the trees are the same node for
node. Where they differ, mostly colab's tree is the one Go means: a
parameter group keeps all its names (`func f(a0, …, a9 int)`, which
tree-sitter splits after nine), and `*(*T)(p)` is a dereference of a
conversion. `colab ast` reads a bare call or statement as it is, with no
context.

### `go::import`

Import-path edits.

- **Match string:** the import path, **exactly** (no quotes around the
  path itself; no substring). `match go::import "fmt"` does not match
  `"my_fmt"` and does not match `"fmt/v2"`.
- **`replace`:** rewrite the import path. The surrounding `import (
  … )` block, alias prefix, and other imports stay put.
- **`delete`:** remove the `import_spec` line and leave what `gofmt`
  would: no blank line right after `(` or before `)` and none doubled.
  A declaration left with no imports (`import ( )`, or a lone
  `import "x"`) is removed entirely.
- **`ensure`:** when no existing `import_spec` references the target,
  add it where `goimports` would. It goes into the existing
  `import ( … )` group, in sorted position within the first block of its
  kind (standard library, i.e. no dot in the first path element, or
  third-party), or as a new block if there is none: first for the
  standard library, last otherwise. A lone `import "x"` becomes a group.
  With no imports at all, `import "<target>"` follows the `package`
  clause. Idempotent.

### `go::symbol`

In-file rename of identifier-like tokens.

- **Targets:** every `identifier`, `type_identifier`, and
  `field_identifier` whose text equals the match string.
- **Excludes:** the import alias and package qualifiers in type
  position (`package_identifier`), label names, raw string contents,
  comments — all different tree-sitter kinds. A package qualifier in an
  expression (`fmt.Sprint(…)`) is an `identifier` and *is* renamed when it
  matches.
- **Caveat:** scope-blind. A local variable named `Foo` and a
  top-level `type Foo struct{}` are renamed together. Verify with
  `--format diff` before `--write`. Narrow it to a function, `impl` or class with `where inside '<pattern>'` ([code scope](dsl.md#code-scope-where-inside-on-symbol-and-call)), or use `colab rename`.

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

Powered by rust-analyzer's parser (`ra_ap_syntax`, through
`colab-parse-rust`) and `toml_edit`: pure Rust, no tree-sitter. Its trees
recover from syntax errors and read real code, macros especially, more
fully than tree-sitter's did.

- **Kind names** are rust-analyzer's (`call_expr`, `method_call_expr`,
  `fn`, `extern_block`; `colab ast --lang rust` shows them). The names
  tree-sitter gave the same constructs still work in `where $X is …` as
  aliases (`call_expression` is both calls and method calls,
  `foreign_mod_item` is `extern_block`); a name with no counterpart
  (`type_identifier`) gets a "did you mean".
- **Fields** are tree-sitter's names (`function`, `arguments`, `name`,
  `body`, `value`, `pattern`, `type`, …), put on rust-analyzer's nodes, so
  `where $X field …` reads as before.
- **Method calls:** `$F($$$A)` matches `x.m(a)` too, `$F` binding `x.m`.
  For plain calls only, `where $F is path_expr`; a pattern that spells a
  method call (`$R.m($$$A)`) matches only method calls. A field-access
  pattern (`$X.field`) matches field accesses, not a method's callee.
- **Attributes and doc comments** are an item's own children here. A
  pattern leaves attributes and visibility out as before, a rewrite keeps
  them where they are, and a delete takes the item's docs and attributes
  with it (a plain comment above them stays).

### `rust::use`

Edits to `use` declarations.

- **Match string:** a leading **path prefix**, segment-wise. `tokio`
  matches `tokio`, `tokio::sync::Mutex`, `tokio as t`, and
  `tokio::*`, but never `my_tokio` or `foo::tokio::bar`. It is matched
  against every path a declaration names, so a brace group's members
  count: `use tree_sitter::{Parser, Tree};` names `tree_sitter::Parser`.
- **`replace`:** rewrites the matched prefix. Multi-segment prefixes
  work: `tokio::sync` → `tokio_v2::sync` rewrites only that prefix. A
  member of a brace group is renamed in place when its new path is still
  under the group (`a::b` → `a::x` in `use a::{b, c}` gives
  `use a::{x, c}`), and otherwise moves to a `use` line of its own after
  the declaration, keeping its alias.
- **`delete`:** removes the entire `use ...;` line when everything it
  names goes; otherwise only the matching members, with their commas.
  A group left with one member loses its braces
  (`use tree_sitter::Tree;`), and one left with only `self` becomes the
  group's own path.
- **`ensure`:** inserts `use <target>;` if no existing `use` already
  begins with the same path: just above the file's first top-level `use`
  (one with no attribute on it), so it joins that group, or, with no
  `use` yet, after any leading inner attributes like `#![allow(...)]` and
  inner doc comments `//!` / `/*! */`, set off by a blank line on each
  side. Run `cargo fmt` to sort it into the group.

### `rust::symbol`

In-file rename of identifier-like tokens.

- **Targets:** `identifier`, `type_identifier`, `field_identifier`,
  and `shorthand_field_identifier` — so `Foo { x }` shorthand syntax
  also tracks renames of `x`.
- **Excludes:** macro names, lifetimes, label names, string
  contents.
- **Caveat:** scope-blind, like every other `<lang>::symbol`. Narrow it to a function, `impl` or class with `where inside '<pattern>'` ([code scope](dsl.md#code-scope-where-inside-on-symbol-and-call)), or use `colab rename`.

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

Read by colab's own parser (`colab-parse-java`), whose trees hold
tree-sitter-java's kinds and fields: over 14,827 Java files (Spring
Framework, Guava, JUnit 5 and the workspace's own) it reads all 14,738
that tree-sitter reads cleanly exactly as tree-sitter did.

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
- **Caveat:** scope-blind. Narrow it to a function, `impl` or class with `where inside '<pattern>'` ([code scope](dsl.md#code-scope-where-inside-on-symbol-and-call)), or use `colab rename`.

---

## Python (`colab-lang-python`)

Read by colab's own parser (`colab-parse-python`), whose trees hold
tree-sitter-python's kinds and fields: over 8,763 Python files it reads
99.6% exactly as tree-sitter did, and the rest are where tree-sitter
misreads Python and colab does not copy it. `type(x).name = v` is an
assignment, not a type alias; `*f(x)` stars the call (`list_splat` of a
`call`), not a call of `*f`; `a := b if c else d` binds `a` to the whole
conditional.

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
  name are also renamed. Narrow it to a function, `impl` or class with `where inside '<pattern>'` ([code scope](dsl.md#code-scope-where-inside-on-symbol-and-call)), or use `colab rename`.

---

## JavaScript / TypeScript (`colab-lang-js`)

Read by colab's own parser (`colab-parse-js`, JSX included), whose
trees hold tree-sitter-javascript's kinds and fields: over 11,058
JavaScript files it reads 99.8% exactly as tree-sitter did; the rest are
`await (f)(x)`, which tree-sitter reads as a call of `await (f)` and
colab as awaiting the call. It reads TypeScript module syntax well
enough for specifier and identifier rewriting (a type annotation after
a name is an error it reads past), and `js` still runs over `.ts` and
`.tsx` files. For
TypeScript code (types, interfaces, generics, member modifiers) use
[`ts::`](#typescript-colab-lang-ts) (and [`tsx::`](#tsx-colab-lang-tsx)
for `.tsx`, JSX included), which parse them.

A method's modifiers (`static`, `async`, `get`, `*`) are anonymous
tokens: `js::pattern 'f() {}'` matches `static async f() {}`, and a
rewrite that leaves them out of its template keeps them
(`f2() {}` gives `static async f2() {}`). A pattern that spells one is
matched as written.

### `js::import`

- **Match string:** the **exact** ES module specifier (the string
  after `from`). `lodash` does not match `lodash-es`.
- **Targets:** `import_statement` and `export_statement` with a
  `source` field.
- **`replace`:** rewrites just the inner string (preserves quotes:
  single-quoted stays single-quoted).
- **`delete`:** removes the whole `import` / `export ... from`
  statement.
- **`ensure`:** adds a side-effect import `import '<target>';` after the
  last import when the module is not imported in any form.

### `js::symbol`

- **Targets:** `identifier`, `property_identifier`,
  `shorthand_property_identifier`. Covers function names, variable
  names, object keys, JSX component names.
- **Excludes:** string literals, JSX text, regex literals.
- **Caveat:** scope-blind. Narrow it to a function, `impl` or class with `where inside '<pattern>'` ([code scope](dsl.md#code-scope-where-inside-on-symbol-and-call)), or use `colab rename`.

---

## TypeScript (`colab-lang-ts`)

Powered by colab's own TypeScript parser (`colab-parse-js`'s TypeScript
dialect, with tree-sitter-typescript's node kinds), on `.ts`, `.mts` and
`.cts` files. Type annotations, interfaces, generics, enums and member
modifiers parse, so a pattern can name them: `function $F($$$A):
Promise<$T> { $$$B }`, `interface $I { $$$M }`, `Promise<$T>`, `port:
number`. `.tsx` files are [`tsx::`](#tsx-colab-lang-tsx)'s: one backend
has one grammar, and TSX's is its own.

`js` also claims `.ts`, as `c` and `cpp` both claim `.h`, so existing
`js::` scripts keep working; a bare `colab find '<pattern>'` over `.ts`
files picks `ts`.

- **`ts::import`:** as [`js::import`](#jsimport), on this grammar
  (`import type` included).
- **`ts::symbol`:** as `js::symbol`, plus `type_identifier`, so a type's
  name and its uses are renamed together.
- **`ts::call`:** as `js::call`.
- **`ts::pattern`:** a member pattern written without modifiers matches
  the member with any: `load(): void {}` and `$M(): void {}` find
  `private static async load(): void {}`. A rewrite whose template leaves
  them out keeps them, in their order; a template that writes modifiers
  of its own is used as written, and a pattern that spells one (`private
  load()`) matches only members that have it. A decorator is the
  member's sibling in the grammar: a deleted member takes it along, and a
  rewrite keeps it above the member (as a Rust rewrite keeps
  attributes). A field pattern may end in its `;`
  (`count: number = $V;`). An untyped one (`name = $V;`) reads as an
  assignment statement too, so it finds both the field and the
  statement; write `name = $V` without the `;` for fields alone.

---

## TSX (`colab-lang-tsx`)

Powered by colab's own TSX parser (`colab-parse-js`'s TSX dialect, with
tree-sitter-typescript's TSX node kinds), on `.tsx` files:
TypeScript with JSX elements. Everything [`ts::`](#typescript-colab-lang-ts)
reads parses here too, with the same pattern contexts, modifiers and
rename kinds (this crate takes them from `colab-lang-ts`), and JSX parses
as well: `<Button onClick={$H} $$$A />`, `<Button onClick={$H} $$$A>$$$C</Button>`.

`js` also claims `.tsx`, as it does `.ts`, so existing `js::` scripts keep
working; a bare `colab find '<pattern>'` over `.tsx` files picks `tsx`.

- **`tsx::import`:** as [`js::import`](#jsimport), on this grammar.
- **`tsx::symbol`:** as `ts::symbol`; a component's name in its JSX tags
  (`<Button>`, `</Button>`) is an identifier, so it is renamed with its
  declaration.
- **`tsx::call`:** as `js::call`.
- **`tsx::pattern`:** as `ts::pattern`, plus JSX: an element pattern
  matches by its tag and the attributes it names, with `$$$A` for the
  rest; a rewrite keeps the attributes and children its template passes
  through. Self-closing (`<X />`) and open-and-close (`<X>…</X>`) elements
  are different shapes, so a rule that means both is written once for each.

---

## Delegated refactorings (language servers)

`colab rename`, `mv`, `action`, `references`, `safe-delete`, and `--tidy`
(unless `colab.toml`'s `[formatters]` names a command for the file's
extension) ask the language's own server and report its answer (see
[cli.md](cli.md#colab-action)). What you get depends on the server:

| Server | Verified with colab | Expected from the server (unverified here) |
| ------ | ------------------- | ------------------------------------------ |
| rust-analyzer | rename, `mv` (`willRenameFiles` rewrites `mod` + paths), extract variable / constant / static / function, references, safe-delete, organize imports | inline, change signature, move item, many more assists |
| clangd | rename | few code actions |
| gopls | — | rename, extract function / variable, inline call, parameter refactorings |
| jdtls | — | rename (class *with* its file — needs a git work tree, see `colab mv`), extract, inline, move, change signature |
| typescript-language-server | — | rename, file moves that update imports, extract, move to file, organize imports |
| pyright | — | rename, references; few code actions (pylsp with pylsp-rope adds extract / inline) |
| sourcekit-lsp | — | rename; few code actions |

`colab action --list` shows exactly what a server offers at a position,
disabled actions with its reason. A missing capability is an error or a
warning naming it (`no codeActionProvider`, `no
workspace.fileOperations.willRename`), never a silent no-op.


**Text occurrences.** A server renames code; `colab rename
--include-text` and the `colab.rename_symbol` MCP tool also offer the old
name where it appears as prose: comments and string contents, which each
source backend recognises with its own grammar (`prose_ranges`), and
config and docs files (TOML, JSON, YAML, Markdown, text) anywhere in the
git work tree. They are listed by default and applied only on request.

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
- **Scope-aware rename — itself.** `<lang>::symbol` does not know which
  occurrences of `x` refer to which binding. For that, use
  [`colab rename`](./cli.md#colab-rename): it *delegates* to the
  language's own server (rust-analyzer, gopls, clangd, …) and reports,
  checks, backs up, and verifies the answer like any codemod. colab
  still does no name resolution of its own.
- **Merging rules' edits.** Rules in one script are applied in source
  order, sequentially, so they never need merging; colab *detects* the
  conflicts their changes can raise (see [Conflicts](cli.md#conflicts))
  and refuses to write. What it does merge is a file that changed on
  disk while the run was in progress: three ways, before writing, and
  only when the two sides' edits do not overlap.
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

Both are powered by colab's own parser (`colab-parse-c`), C as its C
dialect, with tree-sitter-c's and tree-sitter-cpp's node kinds.

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

Powered by colab's own C# parser (`colab-parse-csharp`, with
tree-sitter-c-sharp's node kinds).

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

Powered by colab's own parser (`colab-parse-php`), with tree-sitter-php's
`php_only` node kinds.

**Backslashes are literal.** DSL string literals have no escape
sequences, so a PHP namespace separator is written as a single
backslash: `match php::use "App\Old\Thing"`.

`php::use` covers the `use function` and `use const` forms. **Grouped
imports are deliberately not matched** — `use App\Sub\{A, B};` has no
single node for `App\Sub\A`, and rewriting half a group would corrupt
it. Expand the group first.

`php::call` targets include the call syntax: `"helper"`, `"Old::run"`,
and `"$obj->run"` are three distinct targets.

## Kotlin (`colab-lang-kotlin`)

Powered by colab's own parser (`colab-parse-kotlin`), with
tree-sitter-kotlin-ng's node kinds. One difference from tree-sitter-kotlin:
annotations before a declaration are always its `modifiers`, where
tree-sitter's parse sometimes read them (and the declaration) as an
`annotated_expression`.

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

Powered by colab's own parser (`colab-parse-swift`), with
tree-sitter-swift's node kinds. Expressions follow Swift's own precedence
groups: `a + b.f()` adds a call, `a == b && c` is `(a == b) && c`,
`!a.b()` negates the call, `x ?? y ?? z` groups to the right, `!=`, `<=`
and `>=` are `equality_expression` / `comparison_expression`, and `try`,
`await` and `unsafe` before an expression cover all of it (`try f().g + 1`
is one `try_expression`). tree-sitter-swift read several of these
otherwise (a call binding loosest), so `swift::pattern '$X.f()'` now finds
`b.f()` in `a + b.f()`. Where tree-sitter's parse chose between readings
by how many it held at the time (some attribute arguments after a space),
the parser picks one rule.

`swift::import` covers plain, submodule (`UIKit.UIView`), and
kind-qualified (`import class Old.Thing`) forms; a rename keeps the kind
keyword.

**Argument labels travel with their values.** `f(name: x)` exposes
`name: x` as one argument, so reordering with `$1`/`$2` keeps each label
attached to its value.

Like Kotlin, trailing-closure calls are skipped. There is no
`swift::package` — Swift source has no package declaration.

## Text (`colab-lang-text`)

Language-blind edits: a string replaced or deleted wherever it is
written, whatever the file's language. For the change no grammar can
express, such as a file extension, a service name or a URL that runs
through source, config, scripts and docs alike.

```
match text::literal "https://old.example.test" { replace "https://new.example.test" }
match text::word ".codemod" { replace ".colab" }
match text::word "billing" in "deploy/**" { replace "invoicing" }
match text::literal " (generated by codemod-tool)" { delete }
```

- `text::literal` matches every occurrence, exactly: case, whitespace and
  punctuation included, with no escapes or wildcards.
- `text::word` matches only an occurrence that is not part of a longer
  word. Where the target starts (ends) with a letter, digit or `_`, the
  character before (after) it must not be one, so `billing` matches in
  `deployment/billing` and `"billing"` but not in `billingctl`, and
  `.codemod` matches in `x.codemod` but not in `x.codemods`.
- `replace` writes the text verbatim; `delete` removes the occurrence,
  and its line too when the occurrence is all that is on it besides
  whitespace (as a `pattern` delete does), so deleting a line's text
  leaves no blank line.
- Every text file is relevant: the rule reads whatever the walk reaches,
  skipping hidden directories, `.gitignore`d paths and common binary
  formats (`.png`, `.zip`, `.woff2`, …). A binary file with another
  extension is listed as skipped. Scope a rule with `in "<glob>"` or by the
  paths given, and preview it with `colab find` or `--format diff`.
- There is no structure: a comment, a string and code look the same, and
  so do two unrelated uses of one word. Prefer a language's `symbol`,
  `import` or `pattern` rule when the change lives in one language.
- A replacement that contains its target again is warned about, since
  each run would rewrite its own output once more.
- A dead rule's near misses are the target in another case and, for
  `word`, the longer words it occurs inside.
- No `where` clause, `in fences` or `and prose`: those need a grammar.

See [`examples/text`](../examples/text/).

## Config-file backends (`colab-lang-json`, `colab-lang-markdown`, `colab-lang-toml`)

These edit configuration and documentation files, where the unit of change
is a key or a block rather than a syntax node. They exist so a repository
convention (a new pre-commit hook, a settings entry, a managed section of
`AGENTS.md`) can be applied across many repositories by the same scripts
and `--check` gate as a source refactoring.

The key-like modules (`toml::key`, `json::key`, and `markdown::section`,
`row`, `frontmatter`, `block`) share these three actions:

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
- The edit is a single splice of the file's text, located by colab's own
  JSON reader: key order, indentation and every other byte are kept. A
  new member follows the last one, one per line or inline to match the
  object; missing parent objects are written compactly on one line.
- `//` and `/* */` comments are allowed between tokens (`tsconfig.json`,
  editor settings). A file that is not JSON otherwise, a trailing comma
  included, is left alone.
- An array step must be an existing index; `set` and `insert` never grow
  arrays, and `delete` does not remove array elements.

Markdown is read by colab's own parser (`colab-parse-markdown`, CommonMark
with GFM tables and task markers), whose tree holds what these modules
address and nothing for rendering. Over 5,816 Markdown files it reads what
tree-sitter-md read in 97.6%; where they differ it is mostly CommonMark
that tree-sitter-md misread: links whose destination holds a `<`
(`…/<VERSION>/…`) or whose text holds brackets, links after a code span
holding a triple backtick, and an ordered list that is not one (`3.`
cannot start a list in the middle of a paragraph). A definition whose
destination is on the next line is read.

### `markdown::section` — a section by heading

```
match markdown::section "Order of work" in "docs/development-plan.md" { set '…new body…' }
match markdown::section "M28 — Markdown structure > Phase 1" { insert '- outline and section' }
match markdown::section "#risks" { replace "Risks and mitigations" }
match markdown::section "Unscheduled" { delete }
```

- A section is a heading and everything under it, up to the next heading of
  the same or a higher level, so it includes its subsections. ATX (`##`)
  and setext (underlined) headings both count; a `#` line inside a code
  fence does not.
- The address is a heading path, `"Parent > Child"`, compared with the
  text as a reader sees it (no `*`, backticks or link syntax). The outer
  segments must enclose the heading in that order but need not be its
  direct parents. `"#slug"` addresses a GitHub-style anchor instead, with
  duplicates numbered `-1`, `-2` as GitHub does.
- An address that matches two headings changes nothing, and the report's
  `unchanged` reason lists them with their lines. A dead rule's near misses
  are the file's heading paths.
- `set` replaces everything under the heading and keeps one blank line after it; it is a no-op when the body is already that text, ignoring trailing spaces and surrounding blank lines. On a missing section, `set` and `insert` add it at the end of its parent (the path minus its last segment), one level below, or, given `"New < Sibling"` / `"New before Sibling"`, just before that sibling at its level, or, given `"New after Sibling"`, just after it (its subsections included) at its level. The words are a placement only when a section has the sibling's name; otherwise they are part of the heading (`"Clean up after release"`); a single-segment path is added at the end of the file, under a lone `#` title if there is one.
- `replace` renames the heading in place, keeping its level. Links in the
  same file to the old anchor become [conflicts](./cli.md#conflicts), so
  `--write` stops until they are fixed.
- To read a section without the whole file, use
  [`colab outline` and `colab section`](./cli.md#colab-outline-and-colab-section).

### `markdown::frontmatter` — a front-matter key

```
match markdown::frontmatter "updated" in "wiki/**" { set '2026-11-13' }
match markdown::frontmatter "tags" { set '
  - colab
  - markdown' }
```

- Top-level keys of the YAML block between `---` lines at the top of the
  file. A key's entry is its line plus the indented or `- ` lines under it,
  so a list or block scalar is replaced whole; a value that starts with a
  newline is written on the lines under the key.
- Values compare by meaning: `"x"`, `'x'` and `x` are equal, and so are
  values that differ only in spacing.
- `set` and `insert` add a missing key before the closing `---`, and a
  missing front matter at the top of the file. Nested keys are not
  addressed yet.

### `markdown::text` — a phrase in a section

```
match markdown::text "Order of work[M28 has shipped.]" { replace "M28 shipped on 2026-11-14." }
match markdown::text "Status[(draft)]" in "docs/**" { delete }
```

- For edits smaller than a section, row or item: a phrase in the
  section's own text (before its first subsection), outside code fences.
  Add `in fences` to change it only inside the section's fences (a line
  of a shell example), or `and fences` for prose and fences alike:
  `match markdown::text "Output formats[--check .]" in fences { replace "--check --format github ." }`.
- Matched word for word, with any run of whitespace in the target
  matching any run in the file, so a phrase the file wraps across lines
  still matches. Every occurrence changes.
- `delete` removes the phrase and one space beside it.

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

### `markdown::row` — a table row

```
match markdown::row "Entries^[Date=2026-11-14, Task=Fix links]" in "docs/log.md" { insert r#"Command=`colab links`
Outcome=Done"# }
match markdown::row "Order of work[Rank=4]" { set "Why now=After the gate." }
match markdown::row "Risks@2[Risk=Old]" { delete }
```

- The target is a section address, then a selector in brackets. Rows
  match when every `Col=value` pair matches the cell in that column,
  compared as a reader sees them (no backticks or emphasis). `@N` picks
  the section's Nth table; only tables in the section's own text (before
  its first subsection) count.
- The value is a whole `| a | b |` row, or `Col=value` lines. `set`
  changes only the named cells of the one matching row (more than one is
  an error), and adds the row when none matches; `insert` adds it only
  when none matches. A new row's unnamed cells are empty, apart from the
  selector's.
- New rows go at the end, at the top with `^`, or before a given row with `< [Col=value]`, as in `"Log[Date=2026-11-14] < [Date=2026-11-12]"`. Several `^` inserts in one script each go above the previous.
- Cells get one space either side, and are padded to the column when
  every line of the table is aligned. `|` in a value is escaped.
  Other rows are never touched.

### `markdown::column` — a table column

```
match markdown::column "At a glance[`create`] < [Applies to]" { insert '' }
match markdown::row "At a glance[Namespace=rust::pattern]" { set '`create`=✅' }
match markdown::column "Log[Owner]" { delete }
match markdown::column "Log[Files]" { replace 'Applies to' }
```

- The target is a section address, then a column's header in brackets,
  compared as a reader sees it. `@N` picks the section's Nth table.
- `insert '<cell>'` adds the column, with that cell in every row, when
  no column has the header: last, first with `^`, or before another
  column with `< [Header]`. Give single rows their own value with a
  `markdown::row` `set`, as above. `delete` removes the column from
  every line; `replace '<header>'` renames it.
- Each line is spliced at its pipes, so the other cells keep their
  spacing; in an aligned table the new cells share one width. Only tables
  whose lines start with `|` are edited, and a column already present
  (or one already renamed) leaves the file unchanged.

### `markdown::item` — a list item

```
match markdown::item "Unscheduled[Call and type hierarchy]" { delete }
match markdown::item "Follow-ups[Raw strings]" { replace "Raw strings (done)" }
match markdown::item "Conventions[Run `make check` before committing]" in "AGENTS.md" { ensure }
match markdown::item "Follow-ups[Kept servers] < [Tab re-indent]" { ensure }
match markdown::item "Steps^[Install colab]" { ensure }
```

- An item is named by the start of its text, as a reader sees it; `@N`
  picks the section's Nth list. Only top-level items of a list in the
  section's own text are addressed; nested lists go with their item.
- `ensure` adds `- <text>` (continuing the list's marker, `-`, `*` or the next number) when no item starts with the text, and starts a list when the section has none. When the list's items wrap, a new one is wrapped to the list's width (backtick spans are never broken); otherwise it stays on one line, like its neighbours. `delete` removes the item with its nested lines. `replace` changes the item's text and keeps its marker and nested list.
- `< [start of an item]` puts an `ensure`'s new item just before that item (which must match one), and `^` puts it first; the new item takes its neighbour's marker and indentation. Without either it goes after the last item. A prefix that matches two items changes nothing, and says so.
- Deleting from a numbered list does not renumber the items after it, since that would touch lines the rule did not name; Markdown renders them in order anyway. `ensure` continues from the last number.

### `markdown::link` — link destinations

```
match markdown::link "guide.md" { replace "manual.md" }
match markdown::link "guide.md#usage" { replace "guide.md#how-to-use" }
match markdown::link "#old" in "docs/plan.md" { replace "#new" }
match markdown::link "draft.md" { delete }
```

| Target | Matches | `replace` rewrites |
| ------ | ------- | ------------------ |
| `#old` | the same-file anchor `#old` | the whole destination |
| `guide.md` | `guide.md`, `../docs/guide.md`, `guide.md#x` (whole trailing path segments) | that path suffix, keeping any prefix and anchor |
| `guide.md#old` | the path as above, with exactly that anchor | the suffix and the anchor |

- Inline links, images and reference definitions are links, including
  in table cells and headings; code spans and fences are not.
- Destinations are matched as written, relative to their own file, so a
  target names the trailing segments the files share. Narrow with `in`
  when two files of the same name exist.
- `delete` unwraps an inline link to its text, and removes an image or
  a reference definition's line.
- **Links are checked by every run.** When a run changes a Markdown file,
  every Markdown file under its paths is checked as it is and as the run
  would leave it, and each link the run would break (a missing file, or
  an anchor no heading has) is a [conflict](./cli.md#conflicts) of that
  file. So renaming a heading names every file linking to it, and a
  `markdown::link` rule in the same script clears them. `--no-link-check`
  skips it; `colab links` lists the links already broken.
