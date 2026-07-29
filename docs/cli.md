# CLI Reference

The `colab` binary ships every operation under one of five
subcommands plus the LSP stub. Output goes to stdout (one stream
per invocation); progress and error logging go to stderr so
JSON / diff pipelines stay clean.

## Subcommands

### `colab refactor`

Run a codemod script against one or more paths.

```
colab refactor --script <path> [--write|--dry-run|--check]
                                [--format human|json|ndjson|diff]
                                [--stdin --path <hint>]
                                [-C|--change-dir <dir>]
                                [--include <glob>]... [--exclude <glob>]...
                                [--no-ignore]
                                [--changed-since <ref> | --staged]
                                [--jobs <N>]
                                [--verify <CMD>] [--commit-per-rule]
                                [--bisect <CMD>]
                                [--backup <DIR>]
                                [--detail summary|counts|diff] [--summary-only]
                                [--max-files <N>] [--max-diff-bytes <N>]
                                [paths...]
```

| Flag | Default | Meaning |
| ---- | ------- | ------- |
| `--script <path>` | required | The `.codemod` file to execute. |
| `-C`, `--change-dir <dir>` | `.` | Resolve `paths` relative to this directory. |
| `--format <h|j|n|d>` | `human` | Output format. `human` is coloured log lines plus a summary. `json` emits one JSON document for the whole run. `ndjson` streams one object per *changed* file then a summary. `diff` emits a unified diff per changed file. |
| `--write` | — | Apply changes in place. |
| `--dry-run` | — | Report what would change without writing. |
| `--check` | — | Like `--dry-run`, but exit code 10 if any file would change. CI-friendly. |
| `--stdin` | — | Read source from stdin instead of walking filesystem paths. Requires `--path`. |
| `--path <hint>` | — | Filename hint for `--stdin`; drives "is this file relevant?" routing. |
| `--include <glob>` | — | Whitelist files via gitignore-syntax glob. Repeatable. If any include is set, only matches are processed. |
| `--exclude <glob>` | — | Blacklist files via gitignore-syntax glob. Repeatable. Applied after `--include`. |
| `--no-ignore` | — | Don't honour `.gitignore`, `.git/info/exclude`, or hidden-file rules. By default the walker behaves like `git ls-files`. |
| `--changed-since <ref>` | — | Only files changed since the given git ref (`git diff --name-only --diff-filter=ACMRT <ref>`). Skips tree walking entirely. CI-friendly. |
| `--staged` | — | Only files in the git index (`git diff --name-only --cached`). Mutually exclusive with `--changed-since`. |
| `--jobs <N>` | `num_cpus` | Worker thread count for parallel file processing. Falls back to the `COLAB_JOBS` env var when unset. Set to `1` for sequential. |
| `--verify <CMD>` | — | Run `<CMD>` (a shell command) after each rule's edits. Non-zero exit reverts that rule's changes and aborts the run with exit 1. Implies `--write`. |
| `--commit-per-rule` | — | After each successful rule, `git add -u && git commit -m "colab: <rule>"`. Requires a git repo. Implies `--write`. |
| `--bisect <CMD>` | — | After applying every rule, run `<CMD>`. On failure, binary-search the rule list to identify the single breaking rule, then revert the working tree to its pre-run state. Conflicts with `--verify`. |
| `--backup <DIR>` | — | Before any rule writes a file, save the pre-modification contents to `<DIR>/<rel-path>`. Pair with `colab undo --from <DIR>` to roll back. |
| `--detail <level>` | `counts` (`diff` for `--format diff`) | How much to report. `summary`: aggregate counters only. `counts`: adds per-rule match counts and changed paths. `diff`: adds capped unified diffs. |
| `--max-files <N>` | `20` | At `--detail diff`, the maximum number of per-file diffs to render. The residue is reported under `truncated`. |
| `--max-diff-bytes <N>` | `2000` | At `--detail diff`, the maximum size of any single rendered diff. |
| `--summary-only` | — | Alias for `--detail summary`. |
| `paths...` | `.` | Files or directories to walk recursively. Multiple roots are walked in order. |

**Default execution mode** is resolved from `--format` and TTY state:

| Format | stdout is TTY | Default mode |
| ------ | :-----------: | ------------ |
| `human` | yes | `--write` |
| `human` | no  | `--dry-run` |
| `json`, `ndjson`, `diff` | any | `--dry-run` |
| `--check` | any | always wins (exit 10 if changes pending) |

Explicit `--write` / `--dry-run` / `--check` always override the
default. They are mutually exclusive (clap rejects combinations).

#### `--format` shapes

Files that were scanned but not changed produce no output in any
format. They are counted in the summary and nothing more.

- **`human`** — per-file lines go to stderr via the logger
  (`[INFO] Wrote /path/to/file.go`); the summary, the per-rule counts,
  and any warnings go to **stdout**:
  ```
  38 file(s) scanned, 3 changed, 91204 → 91250 bytes in 84 ms
    rule 1: 3 file(s) — go::import "a" -> "b"
    rule 2: 0 file(s) — go::symbol "X" -> "Y"
  warning: matched no files: go::symbol "X" -> "Y"
  ```
- **`json`** — one compact JSON document for the whole run:
  ```json
  {"summary":{"visited":412,"scanned":38,"changed":3,"skipped":0,"bytes_before":91204,"bytes_after":91250,"elapsed_ms":84},
   "rules":[{"i":0,"rule":"go::import \"a\" -> \"b\"","files":3},
            {"i":1,"rule":"go::symbol \"X\" -> \"Y\"","files":0}],
   "changed":["cmd/main.go","internal/a.go","internal/b.go"]}
  ```
  `--detail diff` adds a `diffs` array (capped by `--max-files` /
  `--max-diff-bytes`, with any residue reported under `truncated`).
  `--detail summary` emits the `summary` object alone.
- **`ndjson`** — one object per *changed* file, newline separated,
  then a final summary object carrying the per-rule counts:
  ```json
  {"type":"file","path":"cmd/main.go","bytes_before":42,"bytes_after":48,"rules":[0]}
  {"type":"summary","summary":{...},"rules":[{"i":0,"rule":"…","files":3}]}
  ```
- **`diff`** — unified diff per changed file:
  ```diff
  --- a/main.go
  +++ b/main.go
  @@ -3,4 +3,4 @@
   import (
   	"fmt"
  -	"some.module"
  +	"new.module"
   )
  ```

#### Reading the result

Three counters, not one, so an empty result says *which* kind of empty it
was:

| Counter | Meaning | If it is 0 |
| ------- | ------- | ---------- |
| `visited` | Regular files the walker yielded. | The paths or `--include`/`--exclude` globs matched nothing, or `.gitignore` excluded the tree (try `--no-ignore`). |
| `scanned` | Of those, the ones a rule considered relevant and read. | Nothing there is written in a language the script targets. |
| `changed` | Of those, the ones actually rewritten. | Files were parsed; no rule matched. Check the per-rule counts. |

`rules[].files` is how many files each rule changed. **A rule with
`files: 0` is dead** — it compiled and it ran, but it never changed a
byte, which almost always means the match string is wrong. Every format
warns about this; it is the cheapest check to make before `--write`.

`skipped` counts files that could not be read or decoded (a non-UTF-8
blob that survived the extension filter, say). These are recorded and the
run continues rather than aborting.

The useful loop, at roughly increasing cost:

```sh
# 1. Blast radius. Cheap enough to run on every edit of the script.
colab refactor --script s.codemod --format json .

# 2. Sample the hunks once the counts look right.
colab refactor --script s.codemod --format diff .

# 3. Apply.
colab refactor --script s.codemod --write .
```

#### `--stdin` pipeline

```sh
cat foo.go | colab refactor --script s.codemod --stdin --path foo.go
```

Reads the source from stdin, applies the script (using `--path` only
to decide which rules are relevant — no file is opened on disk), and
emits the rewritten source to stdout. Combine with `--format json`
or `--format diff` to emit a structured event instead of the
rewritten source.

### `colab schema`

Print the full capability schema as JSON. One object per registered
backend, including module/action descriptions:

```sh
colab schema
```

Use this from agents and editor extensions to discover what
`<lang>::<module>` / action pairs exist without parsing source.

### `colab list-languages`

List the registered backends with their top-level descriptions.
Lighter-weight than `schema` (no module-level detail):

```sh
colab list-languages
```

### `colab list-rules <lang>`

Modules and actions for one backend. Errors with exit code 3 if the
language is not registered.

```sh
colab list-rules go
colab list-rules rust
```

### `colab undo --from <DIR>`

Restore files from a backup directory produced by `colab refactor
--backup <DIR>`. The backup directory mirrors the absolute paths
of every file the run touched; `colab undo` walks it and writes
each backup over its original location.

```sh
colab refactor --script s.codemod --write --backup .colab-backup .
# … review / decide to roll back …
colab undo --from .colab-backup
```

### `colab pack list`

List discoverable `.codemod` packs. Lookup paths, in order:

1. `<repo>/.colab/packs/` — the project-local pack directory,
   discovered by walking up from the current directory looking for
   a `.git` marker.
2. `~/.colab/packs/` — the user-global pack directory.

```sh
colab pack list
```

Output (sorted by path):

```json
{
  "packs": [
    {
      "name": "javax-to-jakarta",
      "path": "/path/to/repo/.colab/packs/javax-to-jakarta.codemod",
      "source": "repo"
    }
  ]
}
```

Combine with `colab refactor --script` to run a pack:

```sh
PACK=$(colab pack list | jq -r '.packs[] | select(.name == "javax-to-jakarta") | .path')
colab refactor --script "$PACK" --check .
```

A pack is just a `.codemod` file. Use `include "<path>"` from a
top-level project script to compose multiple packs.

### `colab explain --script <path>`

Parse the script and emit its IR as JSON without running anything.
Useful for verifying syntax in CI before applying:

```sh
colab explain --script my-pack.codemod
```

Output:

```json
{
  "name": "two-renames",
  "items": [
    {"kind": "match", "namespace": "go::import", "match": "old.module", "scope": null, "action": "replace", "value": "new.module"},
    {"kind": "match", "namespace": "go::import", "match": "another", "scope": null, "action": "delete", "value": null},
    {"kind": "match", "namespace": "rust::symbol", "match": "Config", "scope": "core/**", "action": "replace", "value": "CoreConfig"},
    {"kind": "include", "path": "shared/javax-to-jakarta.codemod"}
  ]
}
```

`items` preserves source order — match clauses and `include`
directives are intermixed exactly as they appeared in the script.
The runtime IR sees the post-expansion flat list.

`action` is always the action's name and `value` its argument (`null`
for `delete` / `ensure`), so a consumer handles one shape rather than
two. `scope` carries any [`in "<glob>"`](./dsl.md#path-scope-in-glob)
clause.

### `colab server`

Start the colab Language Server on stdio. Active features:

- **Diagnostics for `.codemod` files.** Every open / change runs
  `colab_dsl::compile` against the binary's default backend
  registry; parse errors and unsupported-namespace errors surface as
  LSP diagnostics with the matching exit code (2 / 3) embedded in
  the diagnostic `code` field.
- **Completion** for namespaces, modules, and actions. Sourced from
  the same registry as `colab list-rules`, so anything new the
  binary advertises is offered immediately.

```sh
colab server
```

`--port <N>` is reserved for a future TCP transport; currently
informational.

### `colab mcp`

Start the Model Context Protocol server on stdio. Wraps the same
operations as the CLI as MCP tools so an agent in Claude Code (or any
MCP-aware host) can call them directly:

| Tool | Inputs | Output |
| ---- | ------ | ------ |
| `colab.list_languages` | — | The registered backends, by name. Start here — it is far smaller than the full schema. |
| `colab.list_rules` | `lang` | One backend's modules and actions. |
| `colab.schema` | `lang?` | Full capability schema, or one language's slice of it. |
| `colab.lint_script` | `script`, `cwd?` | `{ok: true, name, rule_count, rules[]}`. |
| `colab.preview` | `script`, `paths[]`, `cwd?`, `detail?`, `max_files?`, `max_diff_bytes?` | Summary, per-rule match counts, changed paths, and (at `detail: "diff"`) capped diffs. Disk untouched. |
| `colab.apply` | same as preview | Same shape, but writes changes back. |

`detail` defaults to `counts`, which describes the blast radius without
paying for diffs. Every tool is annotated (`readOnlyHint`, and
`destructiveHint` on `colab.apply`) so a host can distinguish reads from
writes.

`cwd` is the directory relative `paths` resolve against and the base for
`include "..."` directives. Supply it: without it, relative paths depend
on wherever the server process happened to start, and `include` does not
work at all.

**Errors.** A tool that ran and rejected its input returns
`isError: true` with a structured body — `{"error": {kind, message,
exit_code, line?, column?, expected?, snippet?}}`. JSON-RPC error
responses (`-32602`) are reserved for malformed *calls*: unknown method
or tool, missing or mistyped arguments.

Wire format: JSON-RPC 2.0 over stdio with LSP-style
`Content-Length` framing. Methods supported: `initialize`,
`initialized` (notification), `tools/list`, `tools/call`. Exiting
the client (closing stdin or sending an `exit` notification) shuts
the server down cleanly.

**Progress.** `tools/call` for `colab.preview` / `colab.apply`
honours `params._meta.progressToken`. When present, the server
emits `notifications/progress` messages every 64 files (and once
more at 100% on completion) before writing the final response,
each carrying the original `progressToken`, the running
`progress` count, and `total` on the final tick. Hosts that
don't supply a token get the synchronous behaviour as before.

```sh
colab mcp
```

## Exit codes

The same table is in `colab --help`:

| Code | Meaning |
| ---- | ------- |
| 0 | Success — no changes needed, or `--write` succeeded. |
| 1 | Generic / configuration error. |
| 2 | Script parse error. |
| 3 | Unsupported namespace or operation. |
| 4 | I/O error (with the offending path in the log line). |
| 10 | `--check` found changes that would be made. |

Code 2 is what `clap` itself uses for argument-parsing errors, which
overlaps with the script parse code by design — both are "the input
was malformed".

## Useful pipelines

```sh
# Preview every change without touching disk.
colab refactor --script s.codemod --format diff path/

# Pre-commit gate: fail if anything would change.
colab refactor --script s.codemod --check . || exit $?

# Stream JSON to a structured log.
colab refactor --script s.codemod --format json . | tee changes.ndjson

# stdin rewriter for an editor pre-save hook.
cat current-buffer.go | colab refactor --script s.codemod --stdin --path current-buffer.go > rewritten.go

# Restrict a sweeping refactor to one subtree, skipping vendored code.
colab refactor --script s.codemod \
    --include 'cmd/**/*.go' --exclude '**/vendor/**' \
    --write .

# CI gate: only verify files changed on this branch.
colab refactor --script s.codemod --check --changed-since origin/main || exit $?

# Pre-commit hook: only the staged files.
colab refactor --script s.codemod --check --staged || exit $?

# Apply rules one-by-one with build-check after each; auto-revert on failure.
colab refactor --script s.codemod --write --verify "cargo check --quiet" .

# Same, plus a git commit per rule for clean review history.
colab refactor --script s.codemod --write --verify "cargo check --quiet" --commit-per-rule .
```

## Environment

- `NO_COLOR` — when set, ANSI colour is disabled in `human` output.
- `RUST_LOG` — standard `env_logger` filter; set to `debug` to see
  per-file "no changes" lines and tree-sitter parse hints.

## See also

- [`docs/dsl.md`](./dsl.md) — codemod script language.
- [`docs/features.md`](./features.md) — what each backend can do.
