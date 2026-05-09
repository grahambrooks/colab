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
                                [--summary-only]
                                [paths...]
```

| Flag | Default | Meaning |
| ---- | ------- | ------- |
| `--script <path>` | required | The `.codemod` file to execute. |
| `-C`, `--change-dir <dir>` | `.` | Resolve `paths` relative to this directory. |
| `--format <h|j|n|d>` | `human` | Output format. `human` is coloured log lines on a TTY (auto-disabled when stdout is not a TTY or when `NO_COLOR` is set). `json` and `ndjson` emit one JSON event per file. `diff` emits a unified diff per changed file. |
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
| `--summary-only` | — | Suppress per-file events (json/diff/human); emit only the final aggregate summary. Pairs well with `--format json` for headless runs. |
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

- **`human`** — coloured log lines to stderr like
  `2026-05-09 12:34:56 [INFO] Wrote /path/to/file.go`. No stdout
  output during a refactor; the file system is the side-effect.
- **`json` / `ndjson`** — one object per processed file, newline
  separated, followed by a final summary event. Stable schema:
  ```json
  {"type": "file", "path": "main.go", "changed": true, "bytes_before": 42, "bytes_after": 48}
  {"type": "summary", "files_seen": 1, "files_changed": 1, "bytes_before": 42, "bytes_after": 48, "elapsed_ms": 12}
  ```
  Combine with `--summary-only` to skip the per-file events and
  emit just the summary.
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
    {"kind": "match", "namespace": "go::import", "match": "old.module", "action": {"replace": "new.module"}},
    {"kind": "match", "namespace": "go::import", "match": "another", "action": "delete"},
    {"kind": "include", "path": "shared/javax-to-jakarta.codemod"}
  ]
}
```

`items` preserves source order — match clauses and `include`
directives are intermixed exactly as they appeared in the script.
The runtime IR sees the post-expansion flat list.

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
operations as the CLI as four MCP tools so an agent in Claude Code
(or any MCP-aware host) can call them directly:

| Tool | Inputs | Output |
| ---- | ------ | ------ |
| `colab.schema` | — | Full JSON capability schema (matches `colab schema`). |
| `colab.lint_script` | `script` | `{ok: true, name, rule_count}` or `{ok: false, error, exit_code}`. |
| `colab.preview` | `script`, `paths[]` | Per-file `{path, changed, bytes_before, bytes_after, diff?}`. Disk untouched. |
| `colab.apply` | `script`, `paths[]` | Same shape as preview, but writes changes back. |

Wire format: JSON-RPC 2.0 over stdio with LSP-style
`Content-Length` framing. Methods supported: `initialize`,
`initialized` (notification), `tools/list`, `tools/call`. Exiting
the client (closing stdin or sending an `exit` notification) shuts
the server down cleanly.

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
