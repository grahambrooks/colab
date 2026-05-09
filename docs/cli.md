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
| `paths...` | `.` | Files or directories to walk recursively. |

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
  separated. Stable schema:
  ```json
  {"path": "main.go", "changed": true, "bytes_before": 42, "bytes_after": 48}
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
  "rules": [
    {"namespace": "go::import", "match": "old.module", "action": {"replace": "new.module"}},
    {"namespace": "go::import", "match": "another", "action": "delete"}
  ]
}
```

### `colab server`

Start the LSP stub on stdio. Today only logs lifecycle events; the
plan is to grow it into `.codemod` script diagnostics + namespace
completion (see `DEVELOPMENT_PLAN.md` M6).

```sh
colab server
```

`--port <N>` is reserved for a future TCP transport; currently
informational.

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
```

## Environment

- `NO_COLOR` — when set, ANSI colour is disabled in `human` output.
- `RUST_LOG` — standard `env_logger` filter; set to `debug` to see
  per-file "no changes" lines and tree-sitter parse hints.

## See also

- [`docs/dsl.md`](./dsl.md) — codemod script language.
- [`docs/features.md`](./features.md) — what each backend can do.
