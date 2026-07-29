# Diagnosing a colab run

colab is designed so the output tells you which thing went wrong. Read
the counters and the per-rule counts before changing anything.

## Nothing changed

The three file counters exist to separate three different failures that
would otherwise look identical.

```json
"summary":{"visited":412,"scanned":38,"changed":0}
```

| Counters | What it means | Fix |
| -------- | ------------- | --- |
| `visited: 0` | The walker never yielded a file. | Wrong path, or `--include`/`--exclude` excluded everything, or `.gitignore` covers the tree. Try `--no-ignore`, and check you are pointing at the right directory. |
| `visited: N, scanned: 0` | Files exist; none belong to a language the script targets. | Wrong namespace for this codebase, or the files have an extension the backend does not claim. `js::*` covers `.js .mjs .cjs .jsx .ts .tsx`; `rust::crate` only ever matches `Cargo.toml`. |
| `scanned: N, changed: 0` | Files were parsed; no rule matched. | The match string is wrong. Go to the per-rule counts. |

colab also prints a `warnings` array saying which of these it was, so you
usually do not have to work it out yourself.

## One rule matched nothing

```json
"rules":[{"i":0,"rule":"go::import \"a\" -> \"b\"","files":3},
         {"i":1,"rule":"go::symbol \"X\" -> \"Y\"","files":0}]
```

Rule 1 (`i: 1`) compiled and ran but changed nothing. This is nearly
always a match-string convention mismatch, not a typo. Check the target
against the convention for that namespace in
[recipes.md](recipes.md#what-the-match-string-means):

- **`go::import`** is an *exact* path. `"logger"` will not match
  `"internal/logger"`.
- **`rust::use` / `python::import`** are *segment prefixes*. `"tokio"`
  matches `use tokio::sync::Mutex`, but `"tokio::sync::Mut"` matches
  nothing — a partial segment is not a segment.
- **`go::struct_tag`** takes `key:value` with **no quotes**, and tag
  options are part of the value. `"json:name"` does not match
  `json:"name,omitempty"`.
- **`go::call` / `rust::call`** match the function text verbatim. If the
  source says `pkg.Old(…)`, the target is `"pkg.Old"`, not `"Old"`.
  `rust::call` never matches method calls (`x.foo()`).
- **`*::symbol`** matches identifier node text exactly. It will not match
  a substring of a longer name.

Confirm the text you are targeting actually appears:

```sh
rg -n 'old/logger' --type go | head
```

If the string is in the file but the rule does not fire, you are matching
the wrong *node kind* — e.g. using `go::symbol` for something that is
part of an import path.

## Far too many files changed

Almost always an unscoped `symbol` rename. `<lang>::symbol` has no scope
analysis: every identifier with that text, in every file, including
locals, fields, and unrelated types in other crates.

```sh
rg -w 'Config' --stats     # how common is this name, really?
```

Fix by scoping to the part of the tree that owns the name:

```
match rust::symbol "Config" in "crates/core/**" { replace "CoreConfig" }
```

If the name is common *within* the crate that owns it too — a local
variable also called `config` — colab is the wrong tool. Make the edits
directly.

## The script will not compile

**Exit 2, parse error.** The message carries the position and what would
have been valid there:

```
parse error at line 2, column 26: unexpected token `replac`; expected one of: "delete", "ensure", "replace", "replace_call"
```

Over MCP the same information is structured (`line`, `column`,
`expected`, `snippet`) on an `isError: true` response.

**Exit 3, unsupported operation.** Two distinct cases, and the message
says which:

```
unknown module `go::improt`; known modules: call, import, struct_tag, symbol (did you mean `import`?)
`js::import` does not support the `ensure` action; known actions: delete, replace
```

The second is not a typo — some backends genuinely lack some actions.
Check `colab list-rules <lang>`.

**`include` fails over MCP.** Pass `cwd`. Without a base path colab
cannot resolve a relative include.

## Applying twice changed the file again

The transform is not idempotent. The usual cause is a `replace_call`
template that keeps the function name:

```
match go::call "f" { replace_call "f(ctx, $args)" }   // wraps on every run
```

Every run adds another `ctx`. Apply once with a single `--write`, verify
with `--format diff` first, and keep the rule out of CI. A template that
*renames* the function (`f` → `g`) is safe because the second pass finds
no `f` to match.

The other cause is a rename whose replacement still matches the target
(`io` → `my_io` under substring semantics). colab matches node text, not
substrings, so this is rare — but check that your replacement cannot be
re-matched by the same rule.

## Some files were skipped

```json
"skipped":[{"path":"gen/blob.go","reason":"stream did not contain valid UTF-8"}]
```

A file that could not be read or decoded is recorded and the run
continues rather than aborting. Usually a generated or binary file that
happens to carry a source extension. Exclude it:

```sh
--exclude 'gen/**'
```

## The build broke after applying

Do not guess which rule did it. Let colab find out:

```sh
# Revert first if you already applied.
colab refactor --script fix.codemod --bisect 'cargo check' .
```

`--bisect` applies everything, runs the command, and on failure binary
-searches the rule list to name the single breaking rule, then reverts
the tree.

To avoid the situation next time, apply rule-by-rule with a build check
after each — a failure reverts just that rule:

```sh
colab refactor --script fix.codemod --verify 'cargo check' --write .
```

## Rolling back

If you applied with `--backup`:

```sh
colab refactor --script fix.codemod --backup /tmp/snap --write .
colab undo --from /tmp/snap
```

Otherwise use git. `--commit-per-rule` gives you one commit per rule,
which makes a partial revert straightforward.

## Output is bigger than expected

The default `--detail counts` should be a few hundred bytes. If you are
getting diffs, something set `--detail diff` (or you used `--format
diff`, which implies it). Cap it:

```sh
--detail diff --max-files 5 --max-diff-bytes 1000
```

Truncation is reported under `truncated`, so a capped response never
silently pretends to be complete.

## Reference

- Exit codes: `0` success · `1` config · `2` parse · `3` unsupported ·
  `4` I/O · `10` `--check` found pending changes.
- `--format human` on a TTY defaults to `--write`. Everything else
  defaults to `--dry-run`. `--check` always overrides.
- Per-file progress lines go to **stderr**; the summary and warnings go
  to **stdout**, so `colab … | jq` and `2>/dev/null` both behave.
