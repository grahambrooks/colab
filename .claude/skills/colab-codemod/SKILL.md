---
name: colab-codemod
description: Run a mechanical, repo-wide code change with colab instead of editing files one at a time. Use when a rename or sweep touches many files and the change is structural — an import path, a dependency name, a symbol, a Java package, a Go struct tag, a call site. Covers writing the .codemod script, checking blast radius, and applying safely, via the CLI or the MCP tools.
---

# Running a codemod with colab

colab is a syntactic rewriter: it parses each file with tree-sitter and
rewrites specific node kinds. It is the right tool when a change is
**mechanical and structural** — the same edit, derived from the syntax,
repeated across many files.

## Is colab the right tool?

**Use colab when** the change is one of these, at any scale:

- an import path or module specifier changes (`old/pkg` → `new/pkg`)
- a dependency is renamed or dropped (`Cargo.toml` + every `use`)
- a type or function is renamed and you want every mention updated
- a Java package moves; a Go struct tag key or value changes
- a function's call sites need rewriting, including reordered arguments

**Do not use colab when** the change needs to understand meaning rather
than shape: type-directed edits, resolving overloads, moving a symbol to
a different module (colab will not update the imports for you), or
anything where the correct edit differs per call site. Those need real
edits, file by file. colab has no scope analysis — see "Symbol renames
are blunt" below.

**Rule of thumb:** if you can state the change as "find every X and make
it Y" without qualification, colab will do it. If your sentence needs an
"except when…", write the edits yourself.

## The loop

Always in this order. Each step is cheaper than the one after it, and
each one can save you the next.

```sh
# 1. Blast radius. A few hundred bytes. Run after every edit to the script.
#    This compiles the script too, so it reports syntax errors (exit 2)
#    and unknown namespaces/modules (exit 3) as well.
colab refactor --script fix.codemod --format json .

# 2. Sample the actual hunks, once the counts look right.
colab refactor --script fix.codemod --format diff .

# 3. Apply.
colab refactor --script fix.codemod --write .
```

`colab explain --script fix.codemod` dumps the parsed IR as JSON. It only
*parses* — it will not catch an unknown namespace — so use it to inspect
what a script (or an `include` chain) resolved to, not as a validity
check. Over MCP, `colab.lint_script` does compile and is the real
check.

**Step 1 is the one that matters.** Read three things in its output:

```json
{"summary":{"visited":412,"scanned":38,"changed":3,...},
 "rules":[{"i":0,"rule":"go::import \"a\" -> \"b\"","files":3},
          {"i":1,"rule":"go::symbol \"X\" -> \"Y\"","files":0}],
 "changed":["cmd/main.go","internal/a.go","internal/b.go"],
 "warnings":["matched no files: go::symbol \"X\" -> \"Y\""]}
```

1. **`rules[].files`** — a rule showing `0` never changed anything. It
   compiled and ran, so the match string is wrong, not the syntax. Fix it
   before going further; colab warns but will not stop you.
2. **`summary.changed`** — is this the number of files you expected? A
   number far larger than expected usually means a symbol rename is
   catching an unrelated name. Scope it (see below).
3. **`changed[]`** — are these the right files?

Never skip from writing a script to `--write`. The counts cost almost
nothing and catch the two failure modes that matter (rule matched
nothing / rule matched far too much).

## Writing the script

```
refactor "descriptive-name" {
    match <lang>::<module> "<target>" { <action> }
    match <lang>::<module> "<target>" in "<glob>" { <action> }
}
```

Rules run in source order against every file, so one script can span
languages — a `rust::crate` rule and a `rust::use` rule, or Go and Java
rules together. Rules that cannot apply to a file are skipped, not just
no-ops, so mixing languages costs nothing.

Discover what is available rather than guessing:

```sh
colab list-languages        # go, java, js, python, rust
colab list-rules go         # modules + actions for one backend
```

The full namespace matrix, the match-string convention for each one, and
worked examples per language are in
[references/recipes.md](references/recipes.md). Read it before writing a
script for a namespace you have not used — the match string means
something different for each (`go::import` is an exact path,
`rust::use` is a segment prefix, `go::struct_tag` is a `key:value`
pair).

## Symbol renames are blunt — scope them

`<lang>::symbol "X" { replace "Y" }` rewrites **every identifier whose
text is `X`, in every file processed**. There is no scope analysis: a
local variable, a field, and an unrelated type in another crate that
happen to share the name are all renamed.

That is fine when the name is genuinely unique. When it is not, narrow
the rule with `in "<glob>"`:

```
match rust::symbol "Config" in "crates/core/**" { replace "CoreConfig" }
```

`*` stops at a path separator, `**` crosses directories. The glob is
matched against the path as the walker yields it, so where you invoke
colab (and any `-C`) affects what it sees. Scoping also makes the run
faster — files outside the glob are never parsed.

Before a symbol rename, check how common the name is:

```sh
rg -w 'Config' --stats     # if this is everywhere, scope it or don't use colab
```

## Applying safely

```sh
# Reversible: snapshot originals, then roll back if you change your mind.
colab refactor --script fix.codemod --backup /tmp/snap --write .
colab undo --from /tmp/snap

# Apply rules one at a time, running a build after each; auto-revert on failure.
colab refactor --script fix.codemod --verify 'cargo check' --write .

# Apply everything, then bisect to the single rule that broke the build.
colab refactor --script fix.codemod --bisect 'cargo check' .

# One git commit per rule, for a reviewable history.
colab refactor --script fix.codemod --commit-per-rule .
```

On a large or unfamiliar codebase, prefer `--verify` over a bare
`--write`: it attributes a build failure to one rule instead of leaving
you to work out which of five did it.

Narrowing what gets touched:

```sh
--include 'src/**'        # gitignore-syntax, repeatable
--exclude 'vendor/**'
--no-ignore               # also visit gitignored / hidden files
--changed-since main      # only files changed on this branch
--staged                  # only files in the git index
```

## CI

`--check` exits 10 if anything would change, 0 otherwise:

```sh
colab refactor --script lint.codemod --check .
```

Exit codes: `0` success · `1` config error · `2` script parse error ·
`3` unsupported namespace/action · `4` I/O error · `10` `--check` found
pending changes.

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

Prefer `colab.list_rules` over `colab.schema`: the full schema is several
KB and you rarely need more than one language.

A failed call comes back with `isError: true` and a structured body —
for a syntax error, `{kind, message, exit_code, line, column, expected,
snippet}`. Use `line`/`column`/`expected` to fix the script directly
rather than guessing.

## When something looks wrong

Read the counters together — they distinguish the three ways a run comes
back empty:

| Symptom | Meaning |
| ------- | ------- |
| `visited: 0` | Paths or `--include`/`--exclude` matched nothing, or `.gitignore` excluded the tree. Try `--no-ignore`. |
| `visited: N, scanned: 0` | Files exist but none are in a language the script targets. Check the namespace. |
| `scanned: N, changed: 0` | Files were parsed; no rule matched. The match string is wrong — see the per-rule counts. |
| one rule at `files: 0` | That rule's match string is wrong. The others are fine. |
| `changed` far too high | A symbol rename is catching an unrelated name. Scope it with `in`. |

More diagnosis, including idempotency failures and the `replace_call`
re-run trap, is in
[references/troubleshooting.md](references/troubleshooting.md).

## Two things that will bite you

**`replace_call` templates that keep the function name are not
idempotent.** `match go::call "f" { replace_call "f(ctx, $args)" }` will
wrap again on every run. Verify with `--format diff`, apply with exactly
one `--write`, and never put such a rule in a script that runs in CI.
Templates that *rename* the function are safe to re-run.

**colab does not follow a move.** Renaming a type does not update the
imports of the module it lives in. If a symbol moves to a new module,
pair the `symbol` rule with an `import` / `use` rule in the same script.
