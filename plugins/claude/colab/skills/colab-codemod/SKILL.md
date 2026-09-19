---
name: colab-codemod
description: Make a mechanical, repo-wide code change with colab instead of editing files one at a time or reaching for sed. Use when the same structural edit repeats across many files — an import path, a dependency rename, a symbol, a Java package, a Go struct tag, a call site, or a TOML/JSON/Markdown config key. Covers deciding whether colab fits, writing the .codemod script, checking blast radius before applying, and applying safely, via the CLI or the MCP tools.
---

# Running a codemod with colab

colab parses each file with tree-sitter and rewrites specific node
kinds. It is the right tool when a change is **mechanical and
structural** — the same edit, derived from the syntax, repeated across
many files.

Reach for it *instead of* a file-by-file sweep. Editing thirty call
sites by hand costs thirty chances to miss one; `sed` cannot tell a
symbol from the same letters inside a string or a comment. A colab rule
states the change once, reports what it matched before touching
anything, and is idempotent, so a second run is a no-op.

## Preflight: is colab available?

```sh
colab --version
```

If that fails, colab is not installed and none of this applies — say so
rather than improvising with `sed`. Installation, for a user who wants
it:

```sh
cargo install --git https://github.com/grahambrooks/colab --locked
```

There is also a Homebrew formula and per-platform release archives at
<https://github.com/grahambrooks/colab/releases>.

If the host has the colab **MCP server** connected instead of (or as
well as) the binary, use the tools — see [Driving it over
MCP](#driving-it-over-mcp). The workflow is identical.

## Is colab the right tool?

**Use colab when** the change is one of these, at any scale:

- an import path or module specifier changes (`old/pkg` → `new/pkg`)
- a dependency is renamed or dropped (`Cargo.toml` + every `use`)
- a type or function is renamed and every mention should follow
- a Java package moves; a Go struct tag key or value changes
- call sites need rewriting, including reordered arguments
- a config convention rolls out across repos — a `Cargo.toml` key, a
  `.claude/settings.json` entry, a managed block in `AGENTS.md`

**Do not use colab when** the change needs meaning rather than shape:
type-directed edits, resolving overloads, moving a symbol to another
module (colab will not update the imports for you), or anything where
the right edit differs per call site. Those need real edits, file by
file.

**Rule of thumb:** if you can state the change as "find every X and make
it Y" without qualification, colab will do it. If your sentence needs an
"except when…", write the edits yourself.

## The loop

Always in this order. Each step is cheaper than the one after it, and
each can save you the next.

```sh
# 1. Blast radius. A few hundred bytes. Re-run after every script edit.
#    This compiles the script too, so it also reports syntax errors
#    (exit 2) and unknown namespaces/modules (exit 3).
colab refactor --script fix.codemod --format json .

# 2. Sample the actual hunks, once the counts look right.
colab refactor --script fix.codemod --format diff .

# 3. Apply.
colab refactor --script fix.codemod --write .
```

**Step 1 is the one that matters.** Its output:

```json
{"summary":{"visited":412,"scanned":38,"changed":3,"skipped":0,...},
 "rules":[{"i":0,"rule":"go::import \"a\" -> \"b\"","files":3},
          {"i":1,"rule":"go::symbol \"X\" -> \"Y\"","files":0}],
 "changed":["cmd/main.go","internal/a.go"],
 "warnings":["matched no files: go::symbol \"X\" -> \"Y\""]}
```

Read three things:

1. **`rules[].files`** — a rule showing `0` never changed anything. It
   compiled and ran, so the match string is wrong, not the syntax. Fix
   it before going further; colab warns but will not stop you. Judge
   this on the *first* preview, against unmodified code: once a rule has
   been applied, re-running it correctly reports `0` and warns "matched
   no files", because the transform is idempotent and there is nothing
   left to change.
2. **`summary.changed`** — the number of files you expected? Far more
   usually means a symbol rename is catching an unrelated name.
3. **`changed[]`** — the right files?

Never skip from writing a script to `--write`. The counts cost almost
nothing and catch the two failure modes that matter: a rule that matched
nothing, and a rule that matched far too much.

Progress lines go to **stderr**, the report to **stdout**, so
`colab … 2>/dev/null | jq` behaves.

## Writing the script

```
refactor "descriptive-name" {
    match <lang>::<module> "<target>" { <action> }
    match <lang>::<module> "<target>" in "<glob>" { <action> }
}
```

Rules run in source order against every file, so one script can span
languages — a `rust::crate` rule beside a `rust::use` rule, or Go and
Java rules together. A rule that cannot apply to a file is skipped
before parsing, so mixing languages costs nothing.

Strings take double or single quotes and have **no escape sequences** —
a PHP separator is one backslash (`"App\Old"`), and single quotes are
there for values that are themselves quoted text (`set '"1.0"'`).

Discover what exists rather than guessing:

```sh
colab list-languages        # every registered backend
colab list-rules go         # modules + actions for one backend
```

The 15 backends: `c`, `cpp`, `csharp`, `go`, `java`, `js`, `kotlin`,
`php`, `python`, `ruby`, `rust`, `swift` for source, and `json`,
`markdown`, `toml` for config.

Every source backend has the same floor — an import-equivalent named for
the language's own construct (`include` / `using` / `import` / `use` /
`require`), taking `replace` / `delete` / `ensure`; a `symbol` rename;
and a `call` rewrite via `replace_call`. Languages with a namespace or
package declaration expose one; Go adds `struct_tag`, Rust adds `crate`.

**The match string means something different per namespace** — an exact
path for `go::import`, a segment prefix for `rust::use`, a `key:value`
pair for `go::struct_tag`. That is the most common cause of a rule that
matches nothing. The conventions and a worked example for every
namespace are in [references/recipes.md](references/recipes.md); read it
before writing a rule for a namespace you have not used.

## Config files are addresses, not syntax

`toml::key`, `json::key` and `markdown::block` edit data rather than
source, so they take `set` / `insert` / `delete` instead of the source
actions:

```
match toml::key "workspace.package.edition" in "Cargo.toml" { set '"2024"' }
match json::key "/scripts/test" in "package.json" { set '"vitest run"' }
match markdown::block "conventions" in "AGENTS.md" { set '- run `make check`' }
```

`set` overwrites or creates; `insert` only creates. Values are compared
by meaning, not text, so both are idempotent. Because they act where the
target is *missing*, **always scope them** with `in "<glob>"` or a named
file — an unscoped `set` adds the key to every file of that type.

## Symbol renames are blunt — scope them

`<lang>::symbol "X" { replace "Y" }` rewrites **every identifier whose
text is `X`, in every file processed**. There is no scope analysis: a
local variable, a field, and an unrelated type in another crate that
happen to share the name are all renamed.

Fine when the name is genuinely unique. When it is not, narrow it:

```
match rust::symbol "Config" in "crates/core/**" { replace "CoreConfig" }
```

`*` stops at a path separator, `**` crosses directories. The glob is
matched against the path as the walker yields it, so where you invoke
colab (and any `-C`) affects what it sees. Scoping also speeds the run
up — files outside the glob are never parsed.

Before a symbol rename, check how common the name really is:

```sh
rg -w 'Config' --stats     # everywhere? scope it, or don't use colab
```

## Applying safely

```sh
# Reversible: snapshot originals, roll back if you change your mind.
colab refactor --script fix.codemod --backup /tmp/snap --write .
colab undo --from /tmp/snap

# Apply rule by rule, running a build after each; auto-revert on failure.
colab refactor --script fix.codemod --verify 'cargo check' --write .

# Apply everything, then bisect to the single rule that broke the build.
colab refactor --script fix.codemod --bisect 'cargo check' .

# One git commit per rule, for a reviewable history.
colab refactor --script fix.codemod --commit-per-rule .
```

On a large or unfamiliar codebase prefer `--verify` over a bare
`--write`: it attributes a build failure to one rule instead of leaving
you to work out which of five did it. Whatever you use, **run the
project's own build or tests afterwards** — colab guarantees the edit
was syntactic, not that the result compiles.

Narrowing what gets touched:

```sh
--include 'src/**'        # gitignore syntax, repeatable
--exclude 'vendor/**'
--no-ignore               # also visit gitignored / hidden files
--changed-since main      # only files changed on this branch
--staged                  # only files in the git index
-C <dir>                  # resolve paths from <dir>
```

## CI

`--check` exits 10 if anything would change, 0 otherwise — so a codemod
doubles as a lint that keeps a convention from coming back:

```sh
colab refactor --script lint.codemod --check .
```

Exit codes: `0` success · `1` config error · `2` script parse error ·
`3` unsupported namespace/action · `4` I/O error · `10` `--check` found
pending changes.

Defaults worth knowing: `--format human` on a TTY implies `--write`;
every other format, and any non-TTY stdout, defaults to `--dry-run`.
`--check` overrides both. So a script run from an agent or a pipe never
writes unless you pass `--write`.

## Driving it over MCP

If the colab MCP server is connected, the same loop maps onto tools.
Always pass `cwd` — without it relative paths resolve against wherever
the server process started, and `include "..."` does not work at all.

| Step | Tool |
| ---- | ---- |
| discover | `colab.list_languages`, then `colab.list_rules {lang}` |
| check the script | `colab.lint_script {script, cwd}` |
| blast radius | `colab.preview {script, paths, cwd}` — defaults to `detail: "counts"` |
| sample hunks | `colab.preview {..., detail: "diff", max_files: 5}` |
| apply | `colab.apply {script, paths, cwd}` |

`colab.lint_script` compiles the script, so it catches unknown
namespaces as well as syntax. (The CLI's `colab explain` only *parses* —
use it to inspect what a script or an `include` chain resolved to, not
as a validity check.)

Prefer `colab.list_rules` over `colab.schema`: the full schema is
several KB and you rarely need more than one language.

A failed call returns `isError: true` with a structured body — for a
syntax error `{kind, message, exit_code, line, column, expected,
snippet}`. Use `line`/`column`/`expected` to fix the script directly
rather than guessing.

## When something looks wrong

The counters separate the three ways a run comes back empty:

| Symptom | Meaning |
| ------- | ------- |
| `visited: 0` | Paths or `--include`/`--exclude` matched nothing, or `.gitignore` excluded the tree. Try `--no-ignore`. |
| `visited: N, scanned: 0` | Files exist but none are in a language the script targets. Check the namespace. |
| `scanned: N, changed: 0` | Files were parsed; no rule matched. The match string is wrong — see the per-rule counts. |
| one rule at `files: 0` | That rule's match string is wrong. The others are fine. |
| `changed` far too high | A symbol rename is catching an unrelated name. Scope it with `in`. |

More diagnosis — idempotency failures, skipped files, oversized output,
the `replace_call` re-run trap — is in
[references/troubleshooting.md](references/troubleshooting.md).

## Two things that will bite you

**`replace_call` templates that keep the function name are not
idempotent.** `match go::call "f" { replace_call "f(ctx, $args)" }` wraps
again on every run. Verify with `--format diff`, apply with exactly one
`--write`, and never put such a rule in a CI `--check`. Templates that
*rename* the function are safe to re-run.

**colab does not follow a move.** Renaming a type does not update the
imports of the module it lives in. If a symbol moves to a new module,
pair the `symbol` rule with an `import` / `use` rule in the same script.
