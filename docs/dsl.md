# Codemod DSL Reference

A codemod script tells `colab` what to find and how to rewrite it. The
language is small on purpose: each script is one named refactoring
containing zero or more `match` blocks (or `include` directives), each pairing a *namespace* (the
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
source files. Use [`colab explain --script foo.colab`](./cli.md#colab-explain---script-path---e-rule)
to inspect the parsed IR; use `--format diff` (or `--check`) to preview
edits before writing.

## Grammar

Whitespace is insignificant outside string literals.

```ebnf
program     = "refactor" string "{" item* "}"
            | item+                             -- bare items: see "Bare rules"
item        = match | "include" string
            | "pattern" identifier "=" string   -- a pattern constraints name
            | "rewriter" identifier "=" string "->" string   -- what `rewrite(…)` does
            | "scan" namespace string location* [ where ] "as" identifier   -- a read-only phase
            | "then" "{" item* "}"               -- runs after everything before it
match       = "match" namespace string location* [ where location* ] "{" let* action clause* "}"
                                         -- locations before the `where`, after it, or both
location    = "in" string                -- path scope, at most once
            | "in" "fences"              -- only inside Markdown code fences
            | "and" "fences"             -- the rule's files, and fences
            | "and" "prose"              -- a symbol rename, in comments and strings too
where       = "where" constraint ( "," constraint )*
constraint  = [ "not" ] test
test        = metavar "matches" ( regex | string )
            | metavar "is" ( identifier | string )
            | metavar "count" range
            | metavar "in" identifier [ "." metavar ]  -- a scan's bindings
            | metavar "type" string     -- answered by the language server
            | metavar "resolves" string -- answered by the language server
            | metavar "implements" string -- the server's type hierarchy
            | metavar "calls" string      -- the server's call hierarchy
            | metavar "called_by" string  -- the server's call hierarchy
            | "inside" pattern [ stop ]  -- without a stop, the only test on symbol / call: a code scope
            | "has" pattern [ stop ]
            | "follows" pattern [ stop ]
            | "precedes" pattern [ stop ]
            | "any" "(" constraint ( "," constraint )* ")"
            | "all" "(" constraint ( "," constraint )* ")"
            | "nth" [ "-" ] number
            | "field" identifier
            | "top"
stop        = "directly" | "until" pattern
pattern     = string | identifier            -- code, or the name of a `pattern` item
range       = number | number ".." number | number ".." | ".." number
let         = "let" metavar "=" metavar ( "|" step )+   -- before the action; pattern only
step        = identifier [ "(" [ arg ( "," arg )* ] ")" ]
arg         = string | regex | [ "-" ] number | identifier   -- a rewriter's name
metavar     = "$" NAME | "$$$" NAME
regex       = "/" ( any-char-except-slash-or-newline | "\" any-char )+ "/"
clause      = import | "message" string | "severity" ( "error" | "warning" | "info" )
            | "id" string | "url" string
import      = "requires" "import" string
            | "drops" "import" string "when" "unused"
namespace   = identifier "::" identifier
identifier  = [a-zA-Z_][a-zA-Z0-9_]* | keyword   -- see "Reserved tokens"
string      = '"' (any-char-except-double-quote)* '"'
            | "'" (any-char-except-single-quote)* "'"
            | 'r#"' (any-text-without '"#')* '"#'        -- raw string
            | 'r##"' (any-text-without '"##')* '"##'
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
Available on the `<lang>::call` module, which every source backend provides.
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

The config-file backends (`toml::key`, `json::key`, and the Markdown
modules `section`, `row`, `frontmatter` and `block`) address a key or block and give it a value written in the file's own
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
[features.md](./features.md#config-file-backends-colab-lang-json-colab-lang-markdown-colab-lang-toml).

### `create "<path>" '<template>'`

A `<lang>::pattern` rule can make a new file from each match instead of
changing the one it read. The path is relative to the matched file's
directory, and both it and the template are filled from the match's
bindings (`$NAME`, `$$$NAME`; `$$` is a `$`). A capture spanning several
lines is moved to the left margin first, so a body taken from inside a
block is not left indented. The new file ends in one newline.

```
// Move each inline module to its own file: the first rule makes the
// file, the second (which sees the same text) leaves `mod name;`.
match rust::pattern 'mod $M { $$$B }' { create "$M.rs" '$$$B' }
match rust::pattern 'mod $M { $$$B }' { replace 'mod $M;' }
```

- The matched file is left as it is, so `create` pairs with a `replace`
  or `delete` of the same pattern when the code should move.
- A file already at the path with the same text is left alone, so a
  second run is a no-op; one with other text is a conflict, never
  overwritten. Two matches that would create one path with different
  text conflict too: put a binding in the path.
- The diff shows the file as new (`--- /dev/null`), `--check` counts it
  as a pending change, and writing makes its directories. The file is
  written untracked, as edits are written unstaged; `--commit-per-rule`
  commits it with its rule. `--backup` records
  it, so `colab undo` removes it, and `--verify` / `--bisect` remove it
  when they revert.
- The path must be relative, and must use only the pattern's bindings.
  A `let` derives values for `replace` only, so `create` takes none. A
  match on a line `colab:ignore` covers creates nothing.

> **Idempotency caveat.** `replace_call` is idempotent only if the
> template *renames the function*. Templates that keep the function
> name and add args (e.g. `f → f(ctx, $args)`) loop on subsequent
> passes; apply such transforms with a single `--write` run and
> verify with `--format diff` first. The corpus harness will refuse a
> case whose second pass is not a no-op.

## Rules as JSON

An agent that has made a change once by hand need not write the rule:
`colab.generalize {lang, before, after, paths, cwd}` infers it from that
edit. A subtree `after` carries over, and an argument the edit changed or
dropped, becomes a metavariable; callees, fields, types and keys stay
literal. `assert.Equal(t, 1, got)` → `require.Equal(t, got, 1)` gives
`assert.Equal($A, $B, $C)` → `require.Equal($A, $C, $B)`, with its other
matches under `paths`. A value `after` holds only inside a longer token
(`"read config"` in `"read config: %w"`) is a warning: the template writes
it as it is. The reply's `rules` go to `colab.apply` as they are.

The edit need not be one node: lines `before` and `after` share at either
end are context and left out, and the two trees are compared so that each
place the edit changed gives a rule of its own (two calls changed in one
block give two rules). A changed callee stands for its whole call, and a
change inside an import for the import.

You can also write rules as JSON directly.

Every MCP tool that takes `script` (`colab.preview`, `colab.apply`,
`colab.find`, `colab.test_rule`, `colab.lint_script`) takes `rules`
instead, and the CLI's `refactor` / `find` take the same JSON with
`--rules-json <file|->`: one object per `match`, so an agent writes JSON
it knows rather than the DSL's syntax and quoting.

```json
{"rules": [
  {"lang": "rust", "match": "old($A)", "replace": "new($A)",
   "where": [{"not_inside": "mod tests { $$$B }"}, {"var": "$A", "matches": "^cfg"}],
   "in": "src/**"},
  {"lang": "toml", "module": "key", "match": "package.edition", "set": "\"2024\""},
  {"lang": "go", "match": "fmt.Println($$$A)", "delete": true,
   "message": "no prints", "severity": "warning"}
]}
```

| Key | DSL |
| --- | --- |
| `lang`, `module` (default `pattern`), `match` | `match <lang>::<module> "<match>"` |
| one of `replace`, `replace_call`, `set`, `insert` (a string), `delete` / `ensure` (`true`), `create` (`{"path", "template"}`) | the action |
| `in` | `in "<glob>"` |
| `and_fences`, `and_prose` (`true`) | `and fences`, `and prose` |
| `where` | `where …, …` (below) |
| `let` (strings, `"$N = $F \| snake"`) | `let …` |
| `requires_import`, `message`, `severity`, `id`, `url` | the same clauses |

A `where` entry is an object or, for anything else, a DSL constraint
string: `{"inside"|"has"|"follows"|"precedes": "<pattern>"}` (or
`not_inside`, …; `"directly": true`, `"until": "<pattern>"`),
`{"var": "$X", "matches": "<regex>"}` (or `is`, `in`, `type`,
`resolves`, `implements`, `calls`, `called_by`, `count`; `"not": true`),
`{"any"|"all": [...]}`, `{"top": true}`, `{"nth": n}`, `{"field": "<name>"}`.

Each rule is written as DSL, every string escaped so it reads back as
itself, and compiled by the one compiler, so the two forms do the same.
An unknown key or a missing action is an error naming the rule
(`rules[1]: give exactly one action …`). The MCP reply carries the
equivalent `script`, to save as a `.colab` file once the rules work.

## `include` directive

Scripts are `.colab` files. The older `.codemod` extension is still
accepted everywhere a script is looked for (`--script`, `include`,
`colab.toml` packs, `colab pack list`, `colab test`), with
a warning; rename such scripts to `.colab`.

A script can pull in match clauses from another `.colab` file by
listing `include "<path>"` inside its `refactor` block. The path
is resolved relative to the *including* script's directory (or as
an absolute path). The included file's outer `refactor "..."`
wrapper is dropped during expansion — only its match clauses are
spliced into the parent in source order, intermixed with sibling
matches.

```
// project/scripts/main.colab
refactor "company-migration" {
    include "shared/javax-to-jakarta.colab"
    match go::import "internal.old" { replace "internal.new" }
}
```

Cycle detection is path-canonical: a → b → a yields a clear
`circular include` error. A script file's includes resolve against its
directory; `--script -` and `-e` resolve against the working directory
(after `-C`); MCP resolves against its root. Only the library's
`colab_dsl::compile`, which has no base path, rejects `include`.
`include` takes a path, not a built-in pack name: run a built-in pack
with `--script builtin:go/1.18-modern`, or name it in `colab.toml`
(`pack = "go/1.18-modern"`) for `colab fix`.

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

**Qualified calls (Rust, C++, PHP).** A pattern that starts with a bare
call also matches the same call through any path: `to_svg($A, $B)`
finds `to_svg(a, b)`, `svg::to_svg(a, b)`,
`diagram_scene::svg::to_svg(a, b)` and the rooted `::svg::to_svg(a, b)`,
but not the method call `x.to_svg(a, b)`. C++ paths are `ns::f(…)` and
`::ns::f(…)`, PHP's `M\f(…)` and `\M\f(…)`. In the other languages a module
qualifier reads like a method call (`m.f(…)`), so spell it to match it. A `replace` template that starts with a bare call
keeps the path it matched when it calls the same function or a rename
of it (`render($A, &$B)` turns `svg::to_svg(c, d)` into
`svg::render(c, &d)`). A template that wraps the call
(`Ok(to_svg($A, $B))`) or is not a call is used as written, so the path
is dropped. A pattern whose callee is already a path
(`svg::to_svg($A, $B)`) matches only that path. `$COLAB_PATH` is
reserved for the matched path. A struct literal of a type named bare
(upper-case, `FileChange { $$$F }`) does the same: it also matches
`crate::walker::FileChange { … }`, and a template that starts with the
same type keeps the path.

**Field shorthand (Rust, JS, TS, Ruby).** A field written `name: $VAR` in a
struct or object literal also matches the shorthand `name` (a field set
from a variable of its name): `Write { path: $P, text: $T }` finds
`Write { path, text }` and `Write { path: p, text }` too. In Ruby the
shorthand is 3.1's `name:`, in a hash or a call's arguments:
`connect(host: $H)` finds `connect(host:)`. A rewrite of a shorthand
match fills `$VAR` with the name and keeps the shorthand. Only a field
after `{` (Ruby: also `(`) or `,` and before `,` or the closing bracket
counts, so `let x: $T`, parameters and a Ruby `def`'s required keywords
are left alone, and a `where` on `$VAR` matches only the fields written
out. C#'s `new { Name }` is not matched: its grammar cannot tell a
shorthand member from a value.

**Types (Rust).** A type on its own is a pattern too:
`HashMap<PathBuf, $V>` matches it in a signature, a `let` annotation or
a field, and `replace 'BTreeMap<PathBuf, $V>'` rewrites each.

Names are upper-case letters, digits and `_`. A name used twice must bind
identical text: `$A == $A` matches `x == x` but not `x == y`. Any other
`$` — a PHP variable, a `$` inside a string — is literal.

**`replace '<template>'`** substitutes each binding's source text into the
template: `$NAME` and `$$$NAME` (the whole span, separators included).
**`delete`** removes each match that stands alone on its line(s) — apart
from indentation and a trailing `;` or `,` — taking the line with it. A
match inside a line goes with the `,` beside it, so a list stays well
formed: an argument, element, dict entry or object property takes the
comma after it (`f(a, b, c)` → `f(a, c)`), and the last one the comma
before it (`f(a, b)` → `f(a)`). A match with no comma beside it
(`() => f(x)`, `return f(x)`, a sole argument) is left in place, since
removing it would leave broken code; `colab find` still lists it, and
`replace` can rewrite it. A deleted declaration takes what belongs to it
from just above: its doc comments (`///` and `/** */` in Rust, C#, Swift;
`/** */` in JavaScript, Java, Kotlin, PHP; `/** */` and `///` in C), Rust's
`#[…]` attributes and Python's decorators, up to the first blank line. A
plain `//` or `#` comment stays, since it may be about the code around it,
except in Go and Ruby: their documentation tools read any comment directly
above a declaration as its doc (go doc, RDoc / YARD), so a deleted Go `func`
takes the `//` lines above it, and a Ruby `def` the `#` lines. Deleting lines
between two blank lines also removes one of the blanks, so deleting a run of
items leaves a single blank line, not one per item.

A `replace` whose template is only metavariables unwraps the match, and
takes those doc comments and attributes too, since nothing is left for
them to belong to: `match rust::pattern 'mod tests { $$$B }' { replace
'$$$B' }` removes the module's `#[cfg(test)]` with it. Any other template
writes new code where the match was and keeps what is above it, so a
rename-style rewrite of an attributed function (`fn old() {}` →
`fn new() {}`) keeps its `///` docs and `#[inline]`.

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
  compile (exit code 3) and the error names every wrapper tried. Where
  one text parses as different nodes in different places, both are
  matched: in Python, `Optional[$T]` finds the subscript in
  `cast(Optional[str], x)` and the annotation in `def f(a: Optional[str])`.
  Text that is code only in such a place compiles there: Python's
  `debug=True` is also a keyword argument and `"debug": True` a dict
  entry, and JavaScript's `debug: true` is also an object property. A
  fragment that parses in more than one wrapper means each: Rust's
  `legacy: $T` matches the field of a struct declaration and the field of
  a struct literal. The prefilter is then a literal every reading needs.
  A Rust match arm is a pattern too: `Some($X) => $E,` matches the arm
  however it ends, with a `,` or with a block (`Some(x) => { … }`), as the
  last arm or not. A trailing `,` that changes no syntax node is treated
  as optional in every language; where it means something (Python's `x,`
  is a tuple), the pattern keeps it. A trailing `;` is dropped the same
  way when the pattern reads the same without it, or is no single node
  with it (a TypeScript class field's `;` is its sibling), and a
  template's own trailing `;` then goes too, so it is not doubled; a
  statement's `;` (`f(x);`) is kept. Switch cases are patterns as well:
  Go, Java, JavaScript and PHP `case $X: $$$B` (a delete takes the case
  and its body), a Kotlin `when` entry `$X -> $E`, and a PHP `match` arm
  `Status::Draft => $V`. So are the other fragments that only parse
  inside something: a C, C++ or Go parameter (`const ring_t *$R`,
  `$W ...string`), a C or C++ struct field, the values of a Go `return`
  (`nil, $E`) or a PHP `echo` (`$A, "\n"`), a Swift protocol requirement,
  and a TSX `{…}` child or attribute value (`{$NAME}`). A bare name stays
  a name: it is not also read as a parameter's type or as JSX text. A
  pattern that also parses on its own still matches in every wrapper it
  fits: C#'s `void $F() {}` is a top-level local function bare, and
  matches class methods too, with a body that is a lone `$$$B` as well
  (one C# and PHP only accept with the placeholder kept in an error
  node). C++ reads `void $F() { $$$B }` as a declaration with a brace
  initializer, so colab reads it as `{ $$$B; }`, a function body. A
  bare `$$$` is an anonymous sequence wherever `$$$B` is accepted, so
  `where inside 'mod tests { $$$ }'` scopes a Rust rule to a test module.
- **Code that does not parse is matched only through captures.** A match
  holding a syntax error outside every capture is no match:
  `go::pattern 'old($A)'` leaves `old(x :: y)` alone, where the rewrite
  would have kept `x` and lost `:: y`, while `old($$$A)` copies the whole
  argument list, error and all. The same keeps a `js::` rule from
  rewriting TypeScript-only syntax in a `.ts` file; use `ts::` there.
- **Whitespace-only rewrites are not made.** A result that differs from
  what it matched only in whitespace outside quotes — `f($$$A)` →
  `f($$$A)` over a call whose arguments span lines — changes no token, so
  the file is left as it is and the rule explains why it changed nothing.
- **Captures keep their own layout.** A rewrite keeps the matched code's
  line breaks around what it does not change, and each capture is put
  back exactly as it was written, its later lines indented by as much as
  the line it now sits on is. So `swap($A, $B)` → `swapped($B, $A)` moves
  a one-line `$B` and a many-line `$A` without reflowing either, and text
  a template appends after a one-per-line `$$$F` or `$$$B` (a field, an
  item) starts a line of its own. A `let`'s value is laid out the same
  way, as one token placed by the template, its later lines moving with
  the line it lands on as the capture it is derived from would.
- **Inside Rust macros whose arguments are expressions.** A macro call's
  arguments are an unparsed token tree to the grammar, so `rust::pattern`
  reads the contents of the standard and common logging and error macros
  (`assert!`, `assert_eq!`, `debug_assert*!`, `vec!`, `format!`,
  `print*!`, `write*!`, `panic!`, `todo!`, `unreachable!`, `dbg!`,
  `anyhow!`, `bail!`, `ensure!`, `trace!` … `error!`, and `json!` when
  its contents are expressions, not an object literal) as call arguments,
  nested macros included: `is_relevant($P)` finds
  `assert!(is_relevant(p))` and `vec![f(x), vec![is_relevant(q)]]`, and
  `replace` and `delete` edit there (a deleted argument takes its comma).
  Contents that do not parse as arguments (`vec![x; n]`, `info!(target:
  "a", …)`) are left alone, and so is every other macro: `matches!`'s
  argument is a pattern, `html!` and `sqlx::query!` are their own
  languages. A project adds its own expression macros in `colab.toml`
  (`[patterns] macros = ["my_assert", "check"]`, names without `!`),
  read by `refactor`, `find`, `fix` and the MCP tools (from the
  `colab.toml` at or above `cwd`).
  `where` tests judge a match in a macro as one in your file: `inside`
  sees the contents' own nodes, then the macro call and the code around
  it (`where inside 'fn t($$$A) { $$$B }'`, `where directly inside
  'assert!($$$A)'`), never the parser's wrapper; `follows`, `precedes`
  and `nth` count the macro's other arguments (which are in no `field`);
  and a semantic test (`type`, `resolves`, …) asks the language server
  about your file at the argument's position.
- **A skipped rewrite names its site.** When the rewritten file would not
  parse, the rule changes nothing in it, and its explanation names the
  first match whose rewrite alone breaks the parse (line and column): in a
  file of many matches, the one shaped differently.
- **The whole node, nothing left over.** A match must account for every
  named child of the code it matches, unless a `$$$` absorbs them. So
  `if $C { $$$B }` does not match an `if` that has an `else`,
  `Union[$A, $B]` does not match `Union[int, str, None]`, and
  `new $C<$T>()` does not match an anonymous class `new C<T>() { … }`: a
  rewrite of those would drop the part the pattern never mentions. Add a
  metavariable for it (`if $C { $$$B } else { $$$E }`) to match it.
- **Modifiers are the exception.** An item pattern written without
  modifiers matches the item with any, in Rust, Java, Kotlin, Swift, C#,
  PHP, JavaScript, TypeScript, C and C++ (`static`, `inline`, `const`,
  `constexpr`, attributes): `fn $F($$$A) -> $R { $$$B }` finds `pub(super) fn run(…)`,
  `void $F() {}` finds `@Override public static void f() {}`, and
  `function $F() {}` finds `public static function f() {}`. Modifiers
  are visibility, `static`, `final`, `async` and the like, and the
  annotations or attributes written with them (Java's `@Override`, C#'s
  `[Obsolete]`). A rewrite whose template is an item that spells none
  (`void $F() { run(); }`) keeps the ones it matched. A template that
  spells its own (`private void $F() {}`), or one that unwraps the item
  (`$$$B`), is used as written.
  A pattern that spells modifiers matches those exactly: `public void
  $F() {}` does not match `public static void f()`, and `@Test void $F()`
  does not match `@Test public void f()`. In C# and PHP each modifier is
  its own node; when a metavariable follows them (`$T $F() {}`), an item
  with up to four is matched. In JavaScript and TypeScript a member's
  modifiers (`private`, `static`, `async`, `readonly`, `get`, `*`) are
  keywords, matched the same way, up to four: `$M(): void {}` (`ts::`)
  finds `private static async load(): void {}`, and a rewrite keeps them
  in their order. A decorator is the member's sibling there, so a
  rewrite leaves it in place. `export` already works, since a pattern
  matches the declaration inside it; a `delete` leaves an `export
  function` in place.
- **Nested matches in one pass.** In `errors.Wrap(errors.Wrap(err, "a"),
  "b")` both calls match; the inner one is rewritten first and its result
  fills the outer template, so one run reaches the fixed point. Text the
  template introduces is never re-matched, so a wrap-style template
  (`f($A)` → `f(g($A))`) cannot run away.
- **Prefiltered.** The longest literal leaf in the pattern (`errors` above)
  must appear in a file before it is parsed. A pattern made only of
  metavariables and punctuation (`$A + $B`) parses every file of its
  language.
- **Quoting.** `"…"` takes JSON's escapes: `\"`, `\\`, `\n` (a line break)
  and `\t`; any other backslash is kept as written (`"\d"` stays `\d`).
  `'…'` and raw strings (`r#"…"#`, `r##"…"##`) are literal, which is what
  a code pattern or template wants: its own `\n` and `\\` reach the file
  untouched. So write code in single quotes, and text that holds both
  quotes in double quotes with `\"`; see
  [Reserved tokens](#reserved-tokens).

- **The layout around a change is kept.** A template replaces the whole
  match, but every token it leaves unchanged keeps the whitespace it had,
  so `func ($R $T) $F() $OUT { $$$B }` → `… $NAME() $OUT { $$$B }` renames
  the function and leaves a many-line body as it was. Next to a changed
  token, and wherever the template breaks a line, the template's own
  spacing is used, so a template that re-indents a block still does. A
  token the template joins to the one before stays joined, so a path the
  template introduces comes out as `a::b::f(…)`.
- **Templates are re-indented.** Lines after a template's first are
  indented to the match's column, so write a multi-line template
  flush-left in its final shape. Tab-indented code (Go) is indented with tabs.

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
| `follows '<pattern>'` | some earlier sibling matches the pattern. The match counts as the largest node spanning the same text, so a matched call is its statement and its siblings are the statements around it. |
| `precedes '<pattern>'` | some later sibling matches the pattern |
| `… directly` | after `inside` / `has` / `follows` / `precedes`: look at the nearest one only (the enclosing node, a direct child, the adjacent sibling; comments are skipped) |
| `… until '<pattern>'` | after the same four: look no further than a node matching this pattern (`inside 'for $$$ { $$$ }' until 'func() { $$$ }'`: in a loop, not across a closure) |
| `nth n` | the match is its parent's `n`th named child, from 1; `nth -1` is the last |
| `field <name>` | the match is the child in that field of its parent (`function` of a call, `body` of a loop; an unknown field gets a "did you mean") |
| `top` | the match is one of the file's own items or statements, a direct child of its root, judged as `nth` is (a call alone on a line is its statement). `not top` is everything nested in something: `match rust::pattern 'use $$$X;' where top { delete }` removes the file's imports and leaves a function's own. Never true in a macro's contents. |
| `any(c, …)` | at least one of the constraints holds: one rule for two shapes. `all(c, …)` groups constraints that must all hold, under `not` or `any` (at the top level a comma does the same). |
| `$X type "<type>"` | the language server says `$X` has this type (see [Asking the language server](#asking-the-language-server-type-resolves-implements-calls-called_by)) |
| `$F resolves "<path>.<name>"` | the language server says `$F` refers to that declaration |
| `$T implements "<path>.<name>"` | the server's type hierarchy gives `$T` a supertype (interface, trait, base class) that resolves to that declaration, at any depth |
| `$F calls "<path>.<name>"` | the server's call hierarchy says the function `$F` calls that declaration |
| `$F called_by "<path>.<name>"` | the server's call hierarchy says that declaration calls the function `$F` |
| `not <constraint>` | the constraint does not hold |

- `<lang>::pattern` takes every constraint. `symbol` and `call` take only
  `inside` / `not inside`, as a [code scope](#code-scope-where-inside-on-symbol-and-call);
  any other module rejects a `where` clause by name.
- A constraint must name a metavariable the pattern binds, with the same
  sigil (`$M`, or `$$$ARGS` for `count`). An unknown node kind gets a "did
  you mean". A regex or nested pattern that does not compile fails the
  script. All of these are caught at compile time, not mid-run.
- A nested pattern (`inside`, `has`, `follows`, `precedes`, `until`)
  matches in every form its text compiles to, so `has 'legacy: true'`
  finds a JavaScript property, not only a label. Its metavariables are its
  own. `$F`
  above matches any test function; it is not compared with the outer
  pattern's bindings, and a template cannot use them: a template metavariable
  the pattern does not bind (nor a `let`) fails the script, rather than
  being filled with nothing.
- Constraints only remove matches, so the prefilter stays sound, and
  `colab find` lists exactly what `refactor` would change.

### Named patterns: `pattern`

A pack that checks `inside 'func $F(t *testing.T) { $$$B }'` in five
rules says it five times. Name it once, at the top level of the script,
and use the name wherever a constraint takes a pattern:

```
refactor "test-hygiene" {
    include "shared.colab"    // defines `goroutine`
    pattern test_func = 'func $F(t *testing.T) { $$$B }'

    match go::pattern '$T.Fatal($$$A)' where inside goroutine, inside test_func {
        replace '$T.Error($$$A)'
    }
    match go::pattern 'log.Println($$$A)' where inside test_func, not inside goroutine { delete }
}
```

- `pattern <name> = '<code>'` is an item, beside `match` and `include`.
  The name stands for the code after `inside`, `has`, `follows`,
  `precedes` and `until`, in `any` / `all` groups too.
- Names are script-wide: a definition may come after its use, and an
  included file's definitions are the including script's. A name defined
  twice, or used and never defined, fails the script, the latter with a
  "did you mean". The editor checks one file at a time, so in a file with
  an `include` it takes a name it does not define to come from there.
- The code is compiled for each rule's language, so one name can serve a
  Go rule and, if it is valid there, a rule in another language.
- A named pattern's metavariables are its own, as in any nested pattern.
- `colab explain` lists the definitions; a rule's description shows the
  code the name stands for.

### Asking the language server: `type`, `resolves`, `implements`, `calls`, `called_by`

Two libraries can both have a `read` or a `Wrap`, and a variable named
`r` can be any type. A syntax tree cannot tell them apart; the language's
own server can, and colab asks it:

```
match go::pattern '$F($E, $M)' where $F resolves "github.com/pkg/errors.Wrap" {
    replace 'fmt.Errorf("%s: %w", $M, $E)'
}
match rust::pattern '$X.unwrap()' where $X type "Option" {
    replace '$X.expect("checked above")'
}
// Every function that calls the client's Dial, in whatever file: it takes
// a context now.
match go::pattern 'func $F($$$P) error { $$$B }' where $F calls "github.com/acme/client.Dial" {
    replace 'func $F(ctx context.Context, $$$P) error { $$$B }'
    requires import "context"
    message "calls client.Dial, which takes a context from v2"
}
// Handlers only: methods of the types that implement http.Handler.
match go::pattern 'func ($R *$T) ServeHTTP($W, $Q) { $$$B }' where $T implements "net/http.Handler" {
    replace 'func ($R *$T) ServeHTTP($W, $Q) { defer metrics.Track()(); $$$B }'
}
```

- **Only a filter.** The pattern and its other constraints find the
  matches first; the server is asked only about those, so the prefilter
  stays sound and `colab find` agrees with `refactor`. colab still
  computes nothing semantic.
- **Where it asks.** At the binding's last identifier, or, for a binding
  that ends in a call (`fs::create_dir_all(p)`), at its callee.
  `--lsp '<command>'` picks the server; without it, each language's usual
  one (`gopls`, `rust-analyzer`, `pyright-langserver --stdio`, …).
  `--lsp-timeout` (default 120 s) bounds its start-up and indexing.
- **`type "<text>"`** holds when the server's hover there contains
  `<text>` with no identifier character on either side, so `io.Reader`
  does not match `io.ReaderAt`. For a call it is the callee's return type
  (the text after its signature's `->`). The spelling is the server's:
  gopls writes `*os.File`, rust-analyzer `Option<u32>`, pyright
  `list[str]`. `colab find` with the constraint is the quickest way to see
  what matches.
- **`resolves "<path>.<name>"`** (or `::` between segments) holds when
  the server's definition there is named `<name>`, and its file's path
  holds each segment of `<path>`, in order (a module cache's `@v1.2.3`
  ignored), or the server's hover names the qualifier, as for a module
  that is not a file of its own (Rust's inline `mod`).
- **`implements "<path>.<name>"`** walks the server's type hierarchy
  (`textDocument/prepareTypeHierarchy`, then `typeHierarchy/supertypes`,
  up to 200 types) from `$T`, and holds when a supertype's declaration
  resolves to the target by the same path-and-name rule as `resolves`.
- **`calls` / `called_by "<path>.<name>"`** ask the server's call
  hierarchy (`textDocument/prepareCallHierarchy`, then
  `callHierarchy/outgoingCalls` / `incomingCalls`) about the function
  `$F`, and hold when one of its callees / callers resolves to the
  target. Both see across files, which no syntactic constraint can; a
  server without the hierarchy answers with an error, which fails the
  run like any other unanswerable question. `colab calls` and `colab
  implementations` show the same answers on their own.
- **The text it asks about is the run's.** Each file is sent to the
  server as the run has it, so a rule sees the types after earlier
  rules' edits.
- **Never silently wrong.** No server for a language, one that cannot
  start, one still indexing at the timeout, or an error answer fails the
  run before anything is written, naming the cause. The MCP server starts
  no language server, so it refuses these rules outright; run them with
  the CLI.
- **Cost.** A file's questions go to the server together: every match
  the other constraints keep is asked about in one batch (its `hover` or
  `definition` requests sent before any answer is read), so a file costs
  about one round trip per constraint, not one per match. `implements`,
  `calls` and `called_by` take a few steps each and are asked in turn.
  Answers are kept for the run, keyed by the file's text, so the same
  question from another rule, `find` or the near-miss probe is not asked
  again. On colab's own workspace rust-analyzer took about 10 s to index;
  after that the questions are quick. A kept server (`colab lsp-daemon`)
  saves the start-up on later runs.

### Derived values: `let`

A metavariable binds a whole node, so `Get$NAME` cannot match part of
an identifier. A `let` before the action computes the part from the
whole, for the template to use:

```
// Go getters drop the `Get` prefix (Effective Go).
match go::pattern '$X.$F()' where $F matches /^Get[A-Z]/ {
    let $NAME = $F | strip_prefix("Get")
    replace '$X.$NAME()'
}
// A camelCase client library moved to snake_case methods.
match python::pattern '$O.$M($$$A)' where $M matches /^[a-z]+[A-Z]/ {
    let $S = $M | snake
    replace '$O.$S($$$A)'
}
```

`let $NEW = $FROM | step | …` passes the text bound to `$FROM` through
each step in turn and binds the result to `$NEW`. The steps:

| Step | Gives |
| ---- | ----- |
| `lower`, `upper` | the text in lower or upper case |
| `capitalize` | the first character upper-cased, the rest kept |
| `snake`, `kebab`, `upper_snake` | `get_user_id`, `get-user-id`, `GET_USER_ID` |
| `camel`, `pascal` | `getUserId`, `GetUserId` |
| `strip_prefix("…")`, `strip_suffix("…")` | the text without it; unchanged when it is not there |
| `replace(/re/, "with")` | every match of the regex replaced; `$1` or `${name}` in `with` is a group |
| `substring(start, end)` | characters `start` to `end`; a negative index counts from the end, and `end` may be left out (`substring(1, -1)` unquotes a string) |
| `rewrite(name, …)` | the binding's code with each match of the named [rewriters](#rewriters-rewriter-and-rewrite) rewritten in place; the first step only |

- The case steps split the text into words at `_`, `-`, spaces and case
  changes, and keep an acronym whole: `HTTPServer` is `HTTP` and `Server`.
  Leading and trailing underscores stay: `_privateName` → `_private_name`.
- `$FROM` is a metavariable the pattern binds, or an earlier `let`.
  `$NEW` is a new name, written with one `$`: the pattern must not bind
  it already. Both are checked when the script compiles, as is every
  step name (with a "did you mean") and regex.
- A `let` narrows nothing: pair it with a `where` (above) to choose the
  matches it applies to. It only fills a `replace` template, so it is
  refused on `delete` and on modules other than `pattern`.
- A derived value is computed from the binding *after* nested matches
  inside it are rewritten, like the binding itself.

### Rewriters: `rewriter` and `rewrite`

A migration that changes a call and, inside it, each of its options, is
one rule with rewriters: the rule rewrites the call, and a `let` rewrites
the options in the code it binds.

```
refactor "client-v2" {
    rewriter timeout = 'timeout: $V' -> 'timeoutMs: $V * 1000'
    rewriter retries = 'retries: $N' -> 'maxRetries: $N'
    match js::pattern 'createClient({ $$$OPTS })' where not has 'legacy: true' {
        let $NEW = $$$OPTS | rewrite(timeout, retries)
        replace 'connect({ $NEW })'
    }
}
```

`createClient({ host: "db", timeout: 5, retries: 3 })` becomes
`connect({ host: "db", timeoutMs: 5 * 1000, maxRetries: 3 })`, and a
`timeout` that is not an option of `createClient` is left.

- `rewriter <name> = '<pattern>' -> '<template>'` is an item, like
  `pattern`: script-wide, shared through `include`, each name once.
- `rewrite(a, b, …)` is a `let`'s first step, from a metavariable of the
  pattern (`$X` or `$$$X`). It takes the code that binding covers and
  rewrites each outermost match of any of the rewriters there; the text
  between matches (other elements, separators, comments, line breaks)
  stays as it was. Later steps work on the result.
- There is no `join`: the separators are the original ones.
- A rewriter's template may use only its own pattern's metavariables,
  like a rule's. It rewrites code in the file, never its own output, so
  the corpus harness's idempotency check holds for it as for any rule.

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
- **Forgetting one is caught.** In Go, Python and JavaScript/TypeScript, a template that writes a
  qualifier (`fmt.`, `pathlib.`) the file neither used before nor imports
  afterwards is a [conflict](./cli.md#conflicts): `--write` writes
  nothing and exits 11. A later rule in the same script that adds the
  import (`match go::import "fmt" { ensure }`) also clears it.

### A lone metavariable

A pattern that is only a metavariable, `$X`, matches any node, so a
`where $X is <kind>` picks nodes by kind alone:

```
match rust::pattern '$X' where $X is foreign_mod_item, not $X matches /^unsafe/ {
    replace 'unsafe $X'
}
```

This is how the Rust 2024 pack finds `extern` blocks, whose item list a
`$$$` cannot stand for.

## Across files: scan, in and then

Every rule edits one file. A **scan** reads all of them first, so a rule
can narrow on what is true elsewhere:

```
refactor "context-first" {
    // Stage 1: exported api functions that can fail take a context first.
    match go::pattern 'func $F($$$P) error { $$$B }' in "api/**"
        where $F matches /^[A-Z]/, not $$$P matches /^ctx /
    {
        replace 'func $F(ctx context.Context, $$$P) error { $$$B }'
        requires import "context"
    }
    then {
        // Reads stage 1's result in api/; edits the callers anywhere.
        scan go::pattern 'func $F(ctx context.Context, $$$P) error { $$$B }' in "api/**" as takes_ctx
        match go::pattern 'api.$F($$$A)' where $F in takes_ctx, not $$$A matches /^ctx\b/ {
            replace 'api.$F(ctx, $$$A)'
        }
    }
}
```

- **`scan <lang>::pattern '<code>' [in "<glob>"] [where …] as <name>`**
  is read-only. It runs over every file the run covers, before the rules
  after it, and keeps the text of each binding of each match under
  `<name>`. It takes the `in` and `where` clauses a rule does.
- **`where $X in <name>`** keeps a match when the text bound to `$X` was
  bound by the scan, to the metavariable of the same name, or to the
  scan's only one; `$X in <name>.$F` names it. `not $X in <name>` keeps
  the others. Values are compared as text: nothing resolves names, so
  `local.Load` and `api.Load` are both `Load` (narrow by the qualifier,
  as above).
- **`then { … }`** starts a stage: its scans read every file as the
  stages before left it (in memory), so stage 2 can act on what stage 1
  wrote in another file. A scan written after rules, without `then`,
  likewise reads their result. Without scans, stages are only rule order.
- **What stays the same.** Every edit is still one rule's in one file,
  so conflicts, `colab:ignore`, per-rule attribution and idempotency
  hold; the corpus re-runs the whole phased script to check the last.
  A rule that uses `in` with no scan before it, in its own stage or an
  earlier one, fails the script.
- **Where it runs.** `colab refactor`, `colab fix` and `colab find` (which
  runs the scans, then locates the hits in the files as they are).
  `--stdin`, `--bisect`, `--verify`, `--commit-per-rule`, `--split-pr`,
  `--changed-since` / `--staged` and the MCP tools refuse such a script,
  saying so, rather than run it without its scans.
- **Not yet:** a `create` action that makes a new file from a match (it
  needs file operations in the `refactor` write path).

## Findings: `message`, `severity`, `id` and `url`

A rule can say what its finding means, after the action:

```
match go::pattern 'errors.Wrap($E, $M)' {
    replace 'fmt.Errorf($M+": %w", $E)'
    requires import "fmt"
    message "pkg/errors is archived; use fmt.Errorf with %w"
    severity warning
}
```

- `message "<text>"` goes with the rule into every report: `rules[]` in
  JSON, the rule's line in human output, and each `colab find` hit
  (`path:line:col: warning: <message> — <code>`). A rule enforced in CI
  or a hook then reads as an inspection, not a bare diff.
- `severity` is `error` (the default), `warning` or `info`. `--check`
  exits 10 only when a rule above `info` would change something, so an
  `info` rule reports without failing the check.
- `id "<name>"` names the rule (letters, digits, `-` `_` `.` `/`; unique
  in a script and its includes). It is what `--only`, `--off` and
  `--severity` select by, the SARIF rule id (`colab/<id>`), and the title
  of a GitHub annotation.
- `url "<link>"` says where the rule's rationale lives: SARIF `helpUri`,
  and "See <url>" after a GitHub annotation's message.
- Each may appear once per rule, in any order with the import clauses.

## Suppressing a site: `colab:ignore`

A comment containing `colab:ignore` keeps colab's hands off a site, in
`refactor`, `--check` and `find` alike, in any language:

```go
return errors.Wrap(err, "kept on purpose") // colab:ignore
// colab:ignore pkg-errors
x := errors.Wrap(err, "kept from this script only")
```

- After code, it covers that line; on a line of its own, the line below.
- Bare, it covers every rule. Names after it (spaces or commas) limit it
  to rules from the script with that name (`refactor "<name>"`) or of
  that `<lang>::<module>`: `# colab:ignore toml::key`.
- It works on the change, not the syntax: whatever a rule would change on
  a covered line is put back, so it needs no support from the backend. A
  `colab:ignore` inside a string counts too.

## Bare rules

The `refactor "<name>" { … }` wrapper is optional everywhere a script is
read: a script may be bare items (`match`, `include`, `pattern`,
`rewriter`, `scan`, `then`) instead. That holds for `-e` / `--rule`,
`--script` files and stdin, an `include`d file, the MCP tools' `script`,
and the `.colab` language server:

```sh
colab refactor -e 'match go::import "old/pkg" { replace "new/pkg" }' .
colab explain  -e 'match python::import "six" { delete }'
printf '%s\n' 'match go::import "old/pkg" { replace "new/pkg" }' > move.colab
colab refactor --script move.colab .
```

Bare items are one block. A file's is named after the file (`move`
above), as reports and `--split-pr` branches show it; `-e` rules, stdin
and MCP scripts are named `inline`. Nothing is wrapped around the text,
so parse errors point into it as written: each `-e` value is a line, so
rule N is line N, with the column inside that rule. A script whose first
token (past comments) is `refactor` is one named block, as before.

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

Each backend owns a namespace and exposes one or more *modules* within
it. Every source backend — `c`, `cpp`, `csharp`, `go`, `java`, `js`,
`kotlin`, `php`, `python`, `ruby`, `rust`, `swift` — provides an
import-equivalent (named for the language's own construct), a `symbol`
rename, a `call` rewrite and `pattern`; those with a namespace or
package declaration expose one too. The config backends `json`,
`markdown` and `toml` have modules of their own, and `text`
(`literal`, `word`) edits any file as plain text, whatever its language. The full list is
machine-discoverable via `colab schema` and `colab list-rules <lang>`,
and is documented in [features.md](./features.md).

Namespaces colab does not implement produce a clear
`Error::UnsupportedOperation` (CLI exit code 3) instead of silently
no-oping.

### Text in any file

`text::literal "<s>"` matches every occurrence of `s`, exactly;
`text::word "<s>"` only an occurrence that is not part of a longer word
(letters, digits and `_`). Both take `replace` and `delete`, and run on
every text file the walk reaches, so scope them:

```
refactor "rename-extension" {
    match text::word ".codemod" { replace ".colab" }
    match text::word "billing" in "deploy/**" { replace "invoicing" }
}
```

Use them for a string that runs through files no single grammar covers.
They see no structure, so a language's own modules are the better fit when
the change lives in one language. See
[features.md](./features.md#text-colab-lang-text).

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
| `ts::import` | As `js::import`, in `.ts` / `.mts` / `.cts` files on the TypeScript grammar. | `"./callbacks"` |
| `ts::symbol` | As `js::symbol`, plus `type_identifier` (a type's name). | `"Config"` |
| `tsx::import` / `tsx::symbol` | As `ts::`, in `.tsx` files on the TSX grammar (a component's name in its JSX tags too). | `"./ui"`, `"Button"` |

For the source modules, matching is on tree-sitter node text, not raw substrings
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
To narrow a rename within a file — one function, one `impl` — add a
[code scope](#code-scope-where-inside-on-symbol-and-call).

| Pattern | Matches |
| ------- | ------- |
| `core/**` | everything under a top-level `core/` |
| `**/core/**` | everything under any `core/` directory, at any depth |
| `src/*.rs` | Rust files directly in `src/`, not in subdirectories |
| `**/*_test.go` | test files anywhere |

`*` stops at a path separator; `**` crosses directories. Globs are relative
to the directory you invoke `colab` from (after any `-C`), or to the
`cwd` of an MCP tool call. A leading `./` is ignored, so
`colab refactor -C repo --script s .` and a glob of `core/**` agree, and an
absolute path under that directory loses it first, so
`colab refactor --script s "$PWD"` agrees too. An absolute path outside it
is matched whole, which a relative glob does not match (an absolute glob
does). An invalid glob is rejected when the script compiles rather than
silently matching nothing.

`in` scopes a single rule; [`--include` / `--exclude`](./cli.md) filter the
whole run. Reach for `--include` to narrow what colab looks at, and `in`
when one rule in a multi-rule script needs a tighter boundary than the
rest. See [`examples/rust/scoped_rename`](../examples/rust/scoped_rename/)
for a worked example.

### Code fences: `in fences` and `and fences`

A source rule can also run on the code examples in Markdown files:

```
match go::pattern 'errors.Wrap($E, $M)' and fences { replace 'fmt.Errorf($M+": %w", $E)' }
match rust::use "old_crate::Thing" in "docs/**" in fences { replace "new_crate::Thing" }
```

- `and fences` keeps the rule for its own files and adds a second rule,
  reported as `… in fences`, for the fences of its language in Markdown
  files. `in fences` keeps only the second.
- A fence belongs to a language by the first word of its info string,
  compared case-insensitively: ```` ```go ```` and ```` ```golang ````
  for Go, ```` ```ts ```` for `js` and `ts`, ```` ```tsx ```` for `tsx`, and so on (each backend
  lists its names). Other fences, and the prose around them, are left alone.
- Each fence is rewritten on its own. One indented inside a list item is
  dedented for the rule and re-indented after.
- An `in "<glob>"` clause narrows the Markdown files by path.
- `colab find` lists matches in fences at their place in the Markdown
  file.
- A snippet need not be a whole file: a Go fence without a `package`
  clause still matches. A template that leaves a snippet unparsable is
  not caught, since a snippet seldom parses cleanly to begin with;
  preview with `--format diff`.
- Each of `in "<glob>"`, `in fences` and `and fences` may appear once;
  the two fence clauses exclude each other.
- Import clauses (`requires import`, `drops import`) apply to the rule's own files, not to fences: a snippet in the docs seldom shows its imports.
- `markdown::text` takes the clauses too, as text rather than code: `in
  fences` changes the phrase only inside the section's code fences (of
  any language), `and fences` in its prose and its fences. Without them
  fences are skipped. No other Markdown module reads fences.

### Comments and strings: `and prose`

A `symbol` rename renames identifiers. `and prose` also renames the old
name where it appears as a whole word in comments and string contents,
which the backend finds with its own grammar:

```
match go::symbol "ParseConfig" and prose { replace "LoadConfig" }
```

So a Go doc comment (`// ParseConfig reads …`) follows the function.
Only `symbol` rules take it; a code occurrence that is not an
identifier the rule renames is never touched. Combine it with `and
fences` to cover the docs as well.

## Code scope: where inside on symbol and call

`in "<glob>"` narrows a rule by path. `where inside '<pattern>'` narrows
a `symbol` rename or a `call` rewrite by *code*: it changes only what lies
inside some match of the pattern, and `not inside` only what lies
outside every match.

```
// Rename the parameter of one function; same-named fields and locals elsewhere keep it.
match rust::symbol "config" where inside 'pub fn other($$$P) -> $R { $$$B }' { replace "limit" }

// Everything in one impl block.
match rust::symbol "cache" where inside 'impl Loader { $$$B }' { replace "store" }

// Move to slog everywhere except in tests.
match go::call "log.Print" where not inside 'func $F(t *testing.T) { $$$B }' {
    replace_call "slog.Info($args)"
    requires import "log/slog"
}
```

- The scope is an ordinary [structural pattern](#structural-patterns-langpattern)
  in the rule's language, so a function, a method, an `impl`, a class or
  any shape a pattern can name works; write it as the code reads,
  modifiers included (`pub fn …` does not match as `fn …`). A lone `$$$B`
  stands for a body's items even where the grammar wants an item
  (`impl Loader { $$$B }`).
- Several constraints all hold: `where inside 'impl A { $$$B }', not
  inside 'fn helper($$$P) { $$$C }'`. Only `inside` and `not inside` are
  allowed here; the other tests name a metavariable, which only a
  `pattern` binds.
- **Still syntactic.** The scope selects identifiers by *position*; it
  resolves no names. A scope that holds a name's uses but not its
  declaration (a field read as `self.config` inside an `impl`, declared in
  the `struct` above it) renames the uses and breaks the code. Scope a
  rename to where the name lives entirely — a parameter, a local, a
  private helper — and use [`colab rename`](./cli.md#colab-rename), which
  asks the language server, when the name crosses the scope.
- A rule that matched only outside its scope reports "matched no files",
  saying why; `colab find` lists exactly what `refactor` would change, as
  for every rule. `in "<glob>"` and `where inside` combine.

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

colab warns when it can tell, before anything runs: a `replace_call`
template that still calls the function (`f(ctx, $args)` for `f`), and a
`pattern` template that is itself a match of its pattern (`f($A)` →
`f(g($A))`). The warning is on every run's report (`the template matches
the pattern again, so every run rewrites its own output once more`), in
`colab.lint_script`'s `warnings`, and on the rule in a `.colab` editor.
It is a warning, not an error: a wrap applied once on purpose is fine.

## Conflicts

Some mistakes are visible in the file a rule just changed, and colab
checks for them before writing (see [Conflicts](./cli.md#conflicts) in
the CLI reference):

- `match <lang>::symbol "a" { replace "b" }` where `b` already occurs in
  the file: the rename would merge two names.
- A Go, Python or JS `pattern` template that introduces an unimported
  qualifier (above). Only these languages bind a qualifier by importing
  it; in Rust an external crate needs no `use`, in Java, Kotlin, C# and
  PHP a qualifier may be a class of the same package or namespace, and C,
  C++, Ruby and Swift imports bind no names, so a syntactic check there
  could only guess.
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
// java-jakarta-migration.colab
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

`refactor`, `match`, `in`, `include`, `pattern`, `rewriter`, `scan`, `as`, `then`, `replace`, `delete`, `ensure`,
`replace_call`, `set`, `insert`; for location clauses `and`, `fences`,
`prose`; for `where` / import clauses `where`, `not`, `matches`, `is`,
`count`, `inside`, `has`, `follows`, `precedes`, `directly`, `until`, `any`, `all`, `nth`, `field`, `top`, `requires`, `import`, `drops`, `when`,
`unused`, `type`, `resolves`, `implements`, `calls`, `called_by`; `let` for derived values; and for finding clauses `message`, `severity`, `id`, `url` (`error`,
`warning` and `info` are ordinary identifiers).

`::` is the namespace separator. `//` starts a line comment.

**Keywords other than `refactor`, `match` and `replace_call` are usable
as namespace segments.** A backend's module
is named after the language's own construct, and C's is `#include` — so
`match c::include "stdio.h" { ... }` parses fine even though `include` is
also the pack directive. The two positions are never ambiguous, because a
namespace only ever follows `match`. The same holds for `delete`,
`ensure`, `replace`, `in`, `set` and `insert`, and for every `where` and
import-clause keyword: `go::import` still parses.

**String literals have no escape sequences.** A backslash is a literal
backslash, which is what makes PHP namespaces (`"App\Old\Thing"`) work
naturally. There are three spellings:

| Spelling | Cannot contain | Use it for |
| -------- | -------------- | ---------- |
| `"…"` | `"` | most targets and values |
| `'…'` | `'` | values that are quoted text themselves: a TOML inline table, a JSON object, a pattern with string literals |
| `r#"…"#` | `"#` | text holding both quotes: Markdown about code, a Rust template with a lifetime and a string |

Every spelling may span lines, so a whole Markdown section can be a raw
string:

```
match markdown::section "Install" in "README.md" { set r#"Run `brew install colab`, or
`cargo install --git <repo> colab-cli` (the "no brew" route needs `cargo`)."# }
```

Text that itself contains `"#`, such as this section, takes two hashes:
`r##` before the opening quote and two after the closing one. Two is the
most the grammar accepts.

## Future directions

The DSL is intentionally small. Capabilities being considered:

- A `wrap` action shorthand for the common
  `replace_call "f(ctx, $args)"` pattern.
- A query namespace (e.g. `go::regex`) for opt-in regex-based
  rewrites where a tree-sitter rule does not suffice.
- Type constraints (`$X is type T`) via the language server's hover.
- A `<lang>::symbol` mode that respects shadowing. (`in "<glob>"` narrows a rename by path and `where inside` by syntactic region; neither resolves names — `colab rename` does, through the language server.)

Track these in [`development-plan.md`](development-plan.md) and the project issue tracker.
