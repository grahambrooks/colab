# CLI Reference

The `colab` binary ships its operations as subcommands: `refactor`,
`fix`, `review`, `find`, `rename`, `mv`, `action`, `references`, `safe-delete`,
`ast`, `outline`, `section`, `mv-section`, `links`, `explain`, `schema`,
`list-languages`, `list-rules`, `pack`, `undo`, `server`, `mcp` and
`lsp-daemon`. Output goes to stdout (one stream
per invocation); progress and error logging go to stderr so
JSON / diff pipelines stay clean.

## Subcommands

### `colab refactor`

Run a codemod script against one or more paths.

```
colab refactor (--script <path|-> | -e <rule>...) [--write|--dry-run|--check]
                                [--format human|json|ndjson|diff|sarif|github]
                                [--tidy[=<CMD>]] [--no-link-check]
                                [--stdin --path <hint>]
                                [-C|--change-dir <dir>]
                                [--include <glob>]... [--exclude <glob>]...
                                [--no-ignore]
                                [--changed-since <ref> | --staged]
                                [--jobs <N>]
                                [--verify <CMD>] [--commit-per-rule]
                                [--bisect <CMD>] [--verify-lsp[=<CMD>]]
                                [--allow-conflicts] [--backup <DIR>] [--split-pr[=<PREFIX>] [--stacked]]
                                [--detail summary|counts|diff] [--summary-only]
                                [--max-files <N>] [--max-diff-bytes <N>]
                                [paths...]
```

| Flag | Default | Meaning |
| ---- | ------- | ------- |
| `--script <path>` | one of `--script` / `-e` | The `.colab` file to execute. `builtin:<pack>` runs a pack built into colab (`colab pack list`). `-` reads the script from stdin (not combinable with `--stdin`). |
| `-e`, `--rule <rule>` | one of `--script` / `-e` | An inline rule — one or more `match …` blocks, or an `include` — so no script file is needed. Repeatable; rules run in the order given, under the script name `inline`. A single value that is a whole `refactor "…" { … }` script is used as-is. `include` resolves against the working directory. |
| `-C`, `--change-dir <dir>` | `.` | Change to this directory first: `paths`, `include` and the `colab.toml` lookup (for `[formatters]`) are relative to it. |
| `--format <FORMAT>` | `human` | Output format: `human`, `json`, `ndjson`, `diff`, `sarif` or `github`. `human` is coloured log lines plus a summary. `json` emits one JSON document for the whole run. `ndjson` streams one object per *changed* file then a summary. `diff` emits a unified diff per changed file. `sarif` and `github` report each changed block for CI (SARIF 2.1.0, or GitHub Actions annotations); see [shapes](#--format-shapes). |
| `--write` | — | Apply changes in place. |
| `--dry-run` | — | Report what would change without writing. |
| `--check` | — | Like `--dry-run`, but exit code 10 if a rule above `severity info` would change a file. CI-friendly. |
| `--stdin` | — | Read source from stdin instead of walking filesystem paths. Requires `--path`. |
| `--path <hint>` | — | Filename hint for `--stdin`; drives "is this file relevant?" routing. |
| `--include <glob>` | — | Whitelist files via gitignore-syntax glob. Repeatable. If any include is set, only matches are processed. |
| `--exclude <glob>` | — | Blacklist files via gitignore-syntax glob. Repeatable. Applied after `--include`. Both read a glob as a rule's `in "<glob>"` does, relative to the working directory (`crates/json/**`), and also accept it relative to the walked target (`json/**` for target `crates`); an absolute target under the working directory reads the same. |
| `--no-ignore` | — | Don't honour `.gitignore`, `.git/info/exclude`, or hidden-file rules. By default the walker behaves like `git ls-files`. |
| `--changed-since <ref>` | — | Only files changed since the given git ref (`git diff --name-only --diff-filter=ACMRT <ref>`). Skips tree walking entirely. CI-friendly. |
| `--baseline <ref>` | — | Report only findings new since the ref: the script also runs over the tree as it was there, and its findings there are left out (see [Findings new since a ref](#findings-new-since-a-ref---baseline)). Never writes; `fix` takes it too. |
| `--lsp <command>` | each language's usual server | The language server that answers `type` / `resolves` constraints (also on `find`). With `--lsp-timeout <secs>` (default 120) for its start-up and indexing. |
| `--staged` | — | Only files in the git index (`git diff --name-only --cached`). Mutually exclusive with `--changed-since`. |
| `--jobs <N>` | available parallelism | Worker thread count for parallel file processing. Falls back to the `COLAB_JOBS` env var when unset. Set to `1` for sequential. |
| `--verify <CMD>` | — | Run `<CMD>` (a shell command) after each rule's edits. Non-zero exit reverts that rule's changes and aborts the run with exit 1. Implies `--write`. Every rule must pass on its own; for rules that only compile together (a new parameter and its callers), use `--bisect`. |
| `--commit-per-rule` | — | After each successful rule, `git add -u && git commit -m "colab: <rule>"`. Requires a git repo. Implies `--write`. |
| `--bisect <CMD>` | — | First run `<CMD>` on the starting tree: if it already fails, stop, change nothing, and say so (no rule is blamed for a broken tree). Then, after applying every rule, run `<CMD>`. On failure, binary-search the rule list to identify the single breaking rule, then revert the working tree to its pre-run state. Conflicts with `--verify`, `--commit-per-rule`, `--dry-run` and `--check`. |
| `--verify-lsp[=<CMD>]` | — | Before writing, open each changed file in its language server, change it in memory, and report every error diagnostic the change adds as a conflict. The server is chosen by extension; `=<CMD>` names one (the `=` is required). Combines with `--bisect`, not with `--verify` / `--commit-per-rule`. See [Conflicts](#conflicts). |
| `--allow-conflicts` | — | Write even when the run found conflicts. Without it, a `--write` run that finds any writes nothing and exits 11. |
| `--tidy[=<CMD>]` | — | After the rules, tidy each changed file in memory, before the conflict checks and the write: through the `[formatters]` command `colab.toml` gives its extension, or else its language server (organize imports, then format). Reported as its own rule, `tidy via <server or command>`. `=<CMD>` names one server for every file. Whole files are formatted, not only the changed lines. Not with `--verify` or `--commit-per-rule`; with `--bisect`, the full set is tidied once before the check. See [`--tidy`](#--tidy). |
| `--backup <DIR>` | — | Before any rule writes a file, save the pre-modification contents under `<DIR>`, mirroring each file's absolute path. Pair with `colab undo --from <DIR>` to roll back. |
| `--split-pr[=<PREFIX>]` | — | Make one git branch per rule instead of writing: `HEAD` plus that rule alone, named `<PREFIX>/<NN>-<rule>` (default `colab/<script>`). Leaves the work tree alone. `--stacked` bases each branch on the previous one instead. See [Splitting a run into pull requests](#splitting-a-run-into-pull-requests). |
| `--no-link-check` | — | Skip the Markdown link check; without it, each link the run would break is a conflict (see [`colab links`](#colab-links)). |
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
| `json`, `ndjson`, `diff`, `sarif`, `github` | any | `--dry-run` |
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
  then a final summary object carrying the per-rule counts. At `--detail
  diff` each file event adds its `diff` and `rule_lines`
  (`[{rule, added, removed}]`, the lines each rule changed there, which
  `colab review` reads):
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
- **`sarif`** — one [SARIF 2.1.0](https://docs.oasis-open.org/sarif/sarif/v2.1.0/sarif-v2.1.0.html)
  document: each rule is a `tool.driver.rules` entry (`colab/rule-N`,
  with its `message` and `severity`), each changed block a result at the
  lines of the file as it was, with the replacement as its `fixes`;
  conflicts are `error` results, and run warnings (a rule that matched
  nothing) are `toolExecutionNotifications`. For code scanning:
  ```yaml
  - run: colab fix --check --format sarif > colab.sarif || true
  - uses: github/codeql-action/upload-sarif@v3
    with: { sarif_file: colab.sarif }
  ```
- **`github`** — GitHub Actions workflow commands, which show as
  annotations on the pull request: one per changed block, then a summary
  line:
  ```
  ::warning file=pkg/main.go,line=3,endLine=3,title=colab%3A go%3A%3Aimport "a" -> "b"::a is gone, use b. Replace with:%0Aimport "b"
  colab: 38 file(s) scanned, 3 would change
  ```

  Both formats credit each block to the rules whose own change touches
  it (each rule that fired is applied alone to the original file to find
  out), so its level is that rule's severity (`error` / `warning` /
  `notice`, SARIF `note`) and its text the rule's `message`, or the rule.
  A block no rule claims on its own (one rule rewriting another's
  output) is credited to every rule that changed the file. Run warnings
  have no location: `github` prints them as `::warning title=colab::…`
  (`matched no files: …`). With `--check` the exit code is still 10 for
  pending changes. A check that finds nothing is clean, so it says nothing
  of it: a rule with a `message` (a finding) that matched no file passed,
  and "no rule matched" is left out. A rule without one is still reported
  when it matched nothing, and a check that read no file still says why.

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
colab refactor --script s.colab --format json .

# 2. Sample the hunks once the counts look right.
colab refactor --script s.colab --format diff .

# 3. Apply.
colab refactor --script s.colab --write .
```

#### Inline rules

```sh
# One rule, no file. Parse errors point into the -e text: rule N is line N.
colab refactor -e 'match go::import "old/pkg" { replace "new/pkg" }' --format json .

# Several, run in order.
colab refactor \
  -e 'match rust::crate "tokio" { replace "async_tokio" }' \
  -e 'match rust::use "tokio" { replace "async_tokio" }' --format diff .

# A script generated by another program.
generate-codemod | colab refactor --script - --format json .
```

`explain` takes the same `--script` / `-e` pair.

#### Near misses

A rule that changed nothing is reported with `files: 0` and a warning.
When its module saw values that resemble the target — import paths, call
names, identifiers, and for a Markdown rule the headings (the last segment
of `Parent > Child`, or an `#anchor`) — the closest few are attached as `near` (JSON) and
appended to the warning:

```json
{"i":0,"rule":"go::import \"github.com/acme/log\" -> \"x\"","files":0,
 "near":[{"value":"github.com/acme/log/v2","files":14}]}
```

```
warning: matched no files: go::import "github.com/acme/log" -> "x"; nearest seen: `github.com/acme/log/v2` (14 file(s))
```

A value is "near" when it is a small edit away, contains or is contained
by the target, or shares its last path segment. Probing for near misses
runs only for dead rules and reads at most 500 of the scanned files:

```
warning: matched no files: markdown::row "Follow-ups from shiped milestones[Item=x]" -> delete; nearest seen: `Follow-ups from shipped milestones` (1 file(s))
```

#### Unchanged matches

A rule that *did* match, but left matches alone, says why — whether it
changed nothing (a dead rule) or changed some files and skipped others.
The reasons:

- a `pattern` template whose output would not parse where it lands, so
  colab skips the rewrite rather than write broken code;
- a template that gives back what it matched apart from whitespace;
- a `where inside` scope that every match fell outside.

`find` lists the matches; the rule reports `unchanged`:

```json
{"i":0,"rule":"rust::pattern 'f($$$A)' -> 'g($$$A,)'","files":0,
 "unchanged":{"reason":"the rewritten code would not parse, so it was skipped; …","files":3}}
```

A rule that changed some files and skipped others adds a warning:

```
changed 1 file(s) but left its matches unchanged in 1 more: the rewritten code would not parse, … — go::pattern 'f($$$A)' -> 'g($$$A,)'
```

The usual cause is a `$$$` capture, which keeps its separators: in a
list with a trailing comma, `$$$ARGS,` doubles it. The check reads at
most 500 sampled files, and only those that hold the rule's prefilter
literal.

#### Conflicts

Before anything is written, each rule checks the change it made to each
file. A conflict is a syntactic warning that cannot be wrong, only
incomplete:

| Check | Raised when |
| ----- | ----------- |
| `<lang>::symbol` rename | The new name already occurs as an identifier in the file, so the rename would merge two names. |
| `go::pattern`, `python::pattern` template | The template introduces a qualifier (`fmt.`, `pathlib.`) that the file did not use before and does not import afterwards. A `requires import`, or a later rule in the script that adds the import, clears it. Python builtins (`str.`, `dict.`) and wildcard imports never conflict. |
| `<lang>::pattern` template, `<lang>::call` | The change makes a function or method call itself more often than it did: a rewrite to a new helper that also rewrote the call inside that helper (`old($X)` → `new($X)` inside `fn new`). Judged by name. Narrow the rule with `where not inside` the function, or allow the conflict when the recursion is meant. |
| `--verify-lsp` | The file's language server reports an error after the change that it did not report before. Errors are compared by message, so code that moves does not count. Credited to the rule whose change holds the error's line, or to the only rule that changed the file; otherwise the whole file's. |
| Changed on disk | A `--write` run re-reads every file just before writing. A file that changed since the run read it (an editor save, another agent, a formatter during a long `--verify`) is merged three ways — the file as read, as the run leaves it, and as it is now — and written with a warning when the two sides' edits do not overlap. Edits on the same or adjacent lines, as in git, are a conflict, and so is a file deleted since it was read. `colab rename` / `mv` / `action` check the same before moving or editing anything. |

Conflicts are judged against the file as the whole script leaves it.
They appear in every format: `conflicts` in JSON (`[{path, rule,
message}]`, with `rule` absent for a `--verify-lsp` error or a change on
disk), `conflict:`
lines in human output. A `--write` run that finds any **writes nothing**
and exits **11**; fix the rules, or pass `--allow-conflicts`. Dry runs
and `--check` report them and exit as usual. MCP `colab.apply` does the
same, returning `applied: false` unless `allow_conflicts` is true. Even
then, a file whose disk changes overlap the run's is never written:
colab writes no conflict markers, and rerunning starts from the new
text.

```
conflict: a.go: `parse` already occurs in this file (line 9), so renaming `load` to it merges two names (rule 1)
conflict: a.go: the rewrite uses `fmt.`, which this file does not import; add `requires import "…"` to the rule (rule 2)
warning: wrote nothing: 2 conflict(s) found; review them, then fix the rules or rerun with --allow-conflicts
```

`--verify-lsp` is cheaper than a build and runs before the write. It only
sees what the server checks as you type: gopls reports type and
resolution errors; rust-analyzer reports type mismatches and wrong
argument counts, but an unresolved name only shows in `cargo check`. Keep
`--verify` / `--bisect` for the full check.

#### `--stdin` pipeline

```sh
cat foo.go | colab refactor --script s.colab --stdin --path foo.go
```

Reads the source from stdin, applies the script (using `--path` only
to decide which rules are relevant — no file is opened on disk), and
emits the rewritten source to stdout. Combine with `--format json`
or `--format diff` to emit a structured event instead of the
rewritten source.

#### Findings new since a ref: --baseline

A convention adopted on old code fails every check until the old code
is fixed. `--baseline` enforces it on what changes from now on:

```sh
colab fix --check --baseline origin/main --format github
colab refactor --script no-log.colab --check --baseline v2.0 src
```

- The script runs twice: over a detached worktree at the ref (made under
  the system temp directory, removed afterwards), from the directory that
  is the working directory now, and over the tree as it is.
- A finding is a changed block: its file, the rules credited with it, the
  lines it replaces and the lines it puts there. Line numbers are not
  part of it, so a finding that only moved is still the ref's. Two equal
  findings at the ref excuse two now, not more.
- Each file keeps only its new findings, and its new text is rebuilt from
  them, so the diff, JSON, SARIF and GitHub output show only those, and
  `--check` exits 10 only for them. How many were left out is a warning.
  A rule whose findings were all the ref's matched, so it is not reported
  as matching nothing, and a run is quiet in the way a clean `--check` is.
- `--changed-since` narrows which files run; `--baseline` narrows the
  findings in them. They combine.
- With `--write` it is an error: writing a file that kept only its new
  findings would undo nothing and fix half the file.

#### Splitting a run into pull requests

`--split-pr[=PREFIX]` makes one git branch per rule instead of writing.
Each branch is `HEAD` plus that rule's changes alone, committed as
`colab: <rule>`, and named `<PREFIX>/<NN>-<rule>`; the default prefix is
`colab/<script name>`:

```sh
colab refactor --script migrate.colab --split-pr .
#   branch colab/migrate/01-go-import-old-log-new-log (e301afa50c): rule 1, 1 file(s)
#   branch colab/migrate/02-go-call-log-old-replace-call-log-new (cec17e8b2b): rule 2, 2 file(s)
git push origin 'colab/migrate/*'          # then one pull request per branch
git format-patch main..colab/migrate/01-…  # or a patch series
```

- The branches are made with git plumbing (a private index,
  `commit-tree`, a create-only `update-ref`): the work tree, the index
  and the current branch are not touched, and an existing branch is never
  overwritten.
- Everything is checked before any branch is made. The run stops, making
  none, if a branch name exists, if a file a rule would change has
  uncommitted changes or is untracked (so each branch really is `HEAD`
  plus one rule), or, unless `--allow-conflicts`, if a rule reports a
  [conflict](#conflicts) (exit 11).
- Rules are applied independently, each to `HEAD`'s files. A rule that
  changes nothing on its own (it rewrites what an earlier rule wrote)
  gets no branch and is reported as matching nothing. For those, add
  `--stacked`: each rule then applies to the previous rule's output and
  its branch starts from the previous branch, a chain of pull requests
  merged in order (`colab/<script>/02-…` is based on `…/01-…`).
- Not with `--write`, `--dry-run`, `--check`, `--verify`,
  `--commit-per-rule` (which commits each rule on the current branch),
  `--bisect`, `--tidy`, `--backup` or `--stdin`. The report lists the
  branches (`branches` in JSON: `rule`, `branch`, `commit`, `files`).

### colab fix

Run the packs a repository declares in `colab.toml`, as one run:

```sh
colab fix --check            # CI: exit 10 if any pack would change a file
colab fix                    # apply (on a TTY; --write elsewhere)
colab fix --format json      # one report, every pack's rules in it
```

`colab.toml` is found by walking up from the working directory (or
`-C <dir>`), stopping at the repository root; `--config <file>` names one.
Its paths are relative to its own directory, and `fix` runs from there.

```toml
[run]
paths = ["."]                         # where the packs run (default ".")
exclude = ["vendor/**", "gen/**"]     # gitignore-style globs
jobs = 8
check = "go build ./..."              # after writing; a failure is bisected and reverted
tidy = true                           # organize imports and format the changed files

[formatters]                          # per extension; --tidy uses these instead of a server
go = "gofmt"                          # reads the file on stdin, writes it on stdout
ts = "prettier --stdin-filepath {path}"

[patterns]                            # read by refactor, find, fix, the MCP tools and colab server too
macros = ["my_assert", "check"]       # Rust expression macros whose contents patterns read, as assert!'s

[[packs]]
pack = "go/1.18-modern"               # built in: `colab pack list`

[[packs]]
path = ".colab/packs/house-style.colab"
```

- Every pack is compiled first, so a broken pack fails before any file is
  read. The packs then run in the order listed, as one script: each rule
  keeps its pack's name (for `colab:ignore <name>` and the report).
- With `--write` (or a TTY), `[run] check` runs once after writing, like
  `refactor --bisect`; if it fails, the breaking rule is named and every
  file is restored.
- `--check` exits 10 only for rules above `severity info`, as for
  `refactor --check`.
- An unknown key or section is an error that names the allowed ones; an
  unknown built-in pack gets a "did you mean".
- Flags: `--check | --write | --dry-run`, `-C <dir>`, `--config <file>`,
  `--format`, `--detail`, `--allow-conflicts` and `--no-link-check`, the
  last four as for `refactor`. `fix` takes no `--script`, `-e` or paths:
  the packs and `[run] paths` come from `colab.toml`.

#### Packs from git

A team shares packs as an ordinary git repository, and each repository
that uses one pins it:

```toml
[[packs]]
git = "https://github.com/acme/colab-packs"
tag = "v3"                       # or rev = "<commit>"; never a branch
path = "go/errors.colab"       # the pack inside that repository
```

- A pin is fetched once, shallow, into a cache: `COLAB_CACHE_DIR`, else
  `$XDG_CACHE_HOME/colab`, else `~/.cache/colab`. The copy is built under
  a temporary name and moved into place only when complete, and is used
  from the cache afterwards, without the network.
- Before it runs, a fetched pack's own cases (`<pack>.tests/`, see
  [`colab test`](#colab-test)) must pass; a pack that fails them is not
  run, and `fix` says which case failed. `colab test` runs them too.
- Includes in the pack resolve inside its repository; a `path` outside
  it is refused.
- There is no registry: a URL and a pin are the whole address.

#### Renovate: fix what a dependency bump breaks

`renovate/colab.json` is a Renovate preset that runs `colab fix --write`
after each dependency update, once per branch, so the bump and the code
changes the repository's packs make for it land in one PR:

```json
{ "extends": ["github>grahambrooks/colab//renovate/colab"] }
```

Renovate runs post-upgrade commands only where it is allowed to:
self-hosted, set `"allowedCommands": ["^colab fix --write$"]` and put
colab on the runner's `PATH`. With `[run] check` in `colab.toml`, the
fix is checked, and a rule that breaks the build is bisected and
reverted, before Renovate commits.

### Selecting rules: --only, --off, --severity

`refactor`, `find` and `fix` run a subset of a script's rules, or change
how much one matters, without editing it:

```sh
colab fix --check --only no-dbg              # just this rule
colab fix --check --off 'legacy-*'           # all but these
colab fix --check --severity no-dbg=warning  # report it, still fail --check (info would not)
colab refactor --script s.colab --only 2   # the second rule, by number
```

- A selector is a rule's `id`, a glob over ids (`*` is any run of
  characters), or its number in the script, from 1. Each flag repeats.
- `--only` keeps the rules any of its selectors select (all, without it);
  `--off` then drops what its selectors select. For `fix`, the numbers
  run across the packs in `colab.toml` order.
- `--severity <selector>=<level>` sets `error`, `warning` or `info`; when
  several apply to a rule, the last wins.
- A selector that selects no rule is an error naming the ids (with a "did
  you mean"), and so is a selection that leaves no rule, so a typo never
  quietly runs everything or nothing.

### colab fleet

One migration across many repositories, each an ordinary colab run, no
coordinator: the manifest and git are the state.

```toml
[fleet]
script = "migrate.colab"     # relative to this file; or `fix = true`
branch = "colab/errors-to-fmt" # what --write commits on
title = "Move from pkg/errors to fmt"
body = "Made by colab fleet."
canary = "api"                 # runs first; default the first repo

[[repos]]
name = "api"
path = "../api"                # a local checkout

[[repos]]
git = "https://github.com/acme/billing"   # cloned into the cache
ref = "main"                   # the base; default the default branch
paths = ["src"]                # where the script runs; default "."
```

```sh
colab fleet fleet.toml                    # what each repository would change
colab fleet fleet.toml --write            # a commit on `branch` in each
colab fleet fleet.toml --write --pr       # …pushed, with `gh pr create`
colab fleet fleet.toml --repo api         # only some (repeatable)
colab fleet status fleet.toml --format json   # each branch's PR, from `gh pr view`
```

- Each repository runs in a temporary `git worktree` at its base, so a
  checkout of yours is never touched; colab runs there as a subprocess
  (`refactor --script …`, or `fix` with the repository's own
  `colab.toml`), and its JSON report gives the files it changed.
- `--write` commits the change on `branch`, title and body as the
  message. A branch that already exists is refused, never overwritten;
  a repository with nothing to change gets no branch.
- The canary runs first; if it fails, the others are reported as
  skipped and do not run.
- `git` repositories are cloned once into the cache (`COLAB_CACHE_DIR`
  …, as for [packs from git](#packs-from-git)) and fetched on later runs.
- Exit 0 when every repository succeeded, 1 when one failed. `--format
  json` gives `{branch, repos: [{repo, status, files, branch, pr,
  error}]}`, `status` one of `changed`, `unchanged`, `failed`,
  `skipped`.

### colab agent install and uninstall

Put colab in front of an agent, and take it out again:

```sh
colab agent install                       # every agent found: claude, cursor, codex
colab agent install --claude --no-hook    # one, without the post-edit hook
colab agent install --dry-run             # the diffs, nothing written
colab agent uninstall                     # what install wrote, and nothing else
colab agent install --user                # for every project of yours, not this one
```

| Agent | What | Where |
| ----- | ---- | ----- |
| Claude Code | MCP server `colab` | `.mcp.json`, key `mcpServers.colab` |
| Claude Code | post-edit hook | `.claude/settings.json`, the `PostToolUse` hook running `colab hook post-edit` |
| Claude Code | the skill | `.claude/skills/colab-codemod/` |
| Cursor | MCP server `colab` | `.cursor/mcp.json`, key `mcpServers.colab` |
| Cursor | the skill, as a rule | `.cursor/rules/colab.mdc` |
| Codex | MCP server `colab` | `~/.codex/config.toml`, table `[mcp_servers.colab]` |
| Codex | how to use colab | `AGENTS.md`, the managed block `colab` |

- **Idempotent.** Install compares by meaning, not bytes: a second run
  writes nothing and reports every file `unchanged`.
- **Uninstall takes back only colab's.** It removes those keys, the one
  hook, the table, the block and colab's own files; everything else in a
  file stays as it was, and a file (or directory) left with nothing else
  is removed. Install then uninstall gives back the files as they were,
  byte for byte.
- A file colab would edit that does not parse (a broken
  `settings.json`) is an error; it is never overwritten.
- Without `--claude` / `--cursor` / `--codex`, the agents found are used:
  on `PATH`, or with their directory in the project or home directory.
- `--format json` gives `{action, dry_run, files: [{tool, path,
  status}]}`, `status` one of `created`, `updated`, `removed`,
  `unchanged`.

With `--user` (not with `-C`), the same goes in for every project of
the user's instead:

| Agent | What | Where |
| ----- | ---- | ----- |
| Claude Code | MCP server `colab`, user scope | `claude mcp add --scope user colab -- colab mcp` (uninstall: `claude mcp remove --scope user colab`) |
| Claude Code | the skill | `~/.claude/skills/colab-codemod/` (`$CLAUDE_CONFIG_DIR/skills/` when set) |
| Cursor | MCP server `colab` | `~/.cursor/mcp.json`, key `mcpServers.colab` |
| Codex | MCP server `colab` | `~/.codex/config.toml`, table `[mcp_servers.colab]` |
| Codex | how to use colab | `~/.codex/AGENTS.md`, the managed block `colab` |

- **Claude Code's own CLI changes its user config.** `~/.claude.json`
  holds Claude Code's state and is rewritten while it runs, so colab only
  reads it, to tell whether `colab` is registered, and leaves the write
  to `claude mcp` (which must be on `PATH`). `--dry-run` shows the
  command it would run.
- **No post-edit hook.** A user-wide hook would run on every edit in
  every project; add it per project with `colab agent install --claude`.
- **No Cursor rule.** Cursor keeps user rules in its settings, not in
  files.
- **Rerun it after upgrading colab**: the skill is a copy of the
  installed build's, and install replaces an older one.
- `--format json` adds `scope: "user"` and `commands: [{tool, what,
  status, command}]`, `command` `null` when nothing needs running.

### `colab hook post-edit`

The post-edit hook (the Claude Code plugin's, or `colab agent install`'s)
reads the tool call as JSON on stdin and runs the packs of the nearest
`colab.toml` over the file `tool_input.file_path` names, in memory. What
a rule above `severity info` would change comes back as
`{"decision": "block", "reason": "…"}`, which Claude Code hands the
model after the edit, with each finding's line, message, `id` and the
replacement. No `colab.toml`, a file outside its `[run] paths` or in its
`exclude`, or packs with scans: no output. It always exits 0, so its
command can be `colab hook post-edit 2>/dev/null || true` and an older
colab stays silent. `colab hook post-edit --file <path>` prints the same
report as text and exits 10 when there is one.

### colab test

Check a repository's packs against cases kept beside them, the way
colab's corpus harness checks its own:

```text
.colab/packs/hygiene.colab
.colab/packs/hygiene.tests/
    removes-a-dbg/input/crates/x/src/lib.rs
    removes-a-dbg/expected/crates/x/src/lib.rs     # what the pack makes of it
    keeps-tests-and-other-trees/input/…            # no expected/: must not change
```

```sh
colab test                       # colab.toml's `path` packs, and .colab/packs/
colab test .colab/packs/hygiene.colab
colab test --bless               # write each case's expected/ (also COLAB_BLESS=1)
colab test --format json         # {packs: [{pack, cases, untested_rules}], passed, failed}
```

- A case runs the pack over its `input/` files in memory, at their paths
  relative to `input/`, so `in "<glob>"` scopes apply as in the repository.
- It passes when every file comes out as in `expected/`. A file that
  `expected/` lacks must come out unchanged, and a case with no
  `expected/` must change nothing: that is how a case says what the pack
  leaves alone.
- A second run over the output must change nothing (the pack is
  idempotent), and no rule may report a conflict.
- A rule that fires in no case is reported as untested, a warning.
- `--bless` rewrites `expected/` from the pack's output (removing it for
  a case that changes nothing); review the diff before committing it.
  Failures of the pack itself (a second run, a conflict) still fail.
- Exit 0 when every case passes, 12 when one fails.

### colab review

Review a large run by rule, from its log:

```sh
colab refactor --script big.colab --format ndjson --detail diff . > run.ndjson
colab review run.ndjson                       # by rule: files, lines, most edited, spread
colab review run.ndjson --rule 2              # page through rule 2's diffs (10 a page)
colab review run.ndjson --rule 2 --page 3 --per-page 20
colab review run.ndjson --format json         # one document
```

```
61 file(s) changed by 2 rule(s), +199 -215 lines
rule 1: 61 file(s), +199 -215 lines — rust::pattern 'crate::parse($S)' -> 'crate::parse_source($S)'
  24 of them also changed by other rules: their lines count for each
  most edited: crates/colab-lang-c/src/calls.rs (+7 -9), crates/colab-lang-cpp/src/calls.rs (+7 -9)
  lines changed per file: 1-2: 17 · 3-5: 9 · 6-20: 35
next: colab review run.ndjson --rule 1   # page through its diffs
```

- The log is `refactor`'s (or `fix`'s) `--format ndjson --detail diff`
  output, a file or `-` for stdin. Without `--detail diff` there are no
  line counts or pages, and the review says so.
- Rules are numbered from 1, as in the human output; JSON's `i` is from 0.
  A rule that changed nothing is listed as such.
- With `--detail diff`, each file event carries `rule_lines` — the lines
  each rule added and removed there, from the blocks it changed — so a
  file several rules changed is split between them. A log without it (or
  a block two rules both touch) counts the file's lines for each rule,
  flagged as `shared` in JSON.
- Flags: `--rule N`, `--page P` (with `--rule`), `--per-page K`
  (default 10), `--top N` most-edited files per rule (default 5),
  `--format human|json`.

To review and merge a run rule by rule, split it into branches with
[`refactor --split-pr`](#splitting-a-run-into-pull-requests).

### `colab find`

Read-only search: where does each rule match? Nothing is written.

```
colab find (--script <path|-> | -e <rule>... | [--pattern] <code> [--lang <lang>])
           [--format human|json] [--detail concise|detailed] [--max-results <N>]
           [-C <dir>] [--include <glob>]... [--exclude <glob>]... [--no-ignore]
           [--jobs <N>] [paths... | --stdin --path <name>]
```

`--stdin --path <name>` searches the text on stdin instead of files:
`--path` names it, for its language and any `in` scope, and locations are
in that text (`echo 'fn f() { old(1); }' | colab find --stdin --path x.rs 'old($A)'`).

A bare pattern is enough, as grep takes a regex: `colab find 'errors.Wrap($E,
$M)' .` runs `match <lang>::pattern '…'`. The first argument that is not a
path on disk is the pattern (or give it with `--pattern`). The language is
`--lang`, or the one most of the files searched are in. When the pattern
is not code in that language (a function signature without its body), the
error says so and names `colab ast` to see how it parses.

```sh
$ colab find -e "match go::pattern 'errors.Wrap(\$E, \$M)' { delete }" .
internal/load.go:14:15: errors.Wrap(err, "read config")
cmd/main.go:22:15: errors.Wrap(err, "startup")
2 match(es) in 2 file(s); 38 scanned
$ colab find 'errors.Wrap($E, $M)' .          # the same, the language from the files
```

- Output is grep-shaped `path:line:col: text` (first line of the match);
  with several rules each hit is tagged `[rule N]`.
- `--detail detailed` adds metavariable captures (`$E = err`) and, in
  JSON, the full match text (capped at 2000 bytes).
- `--max-results` (default 50) caps the listed hits; the rest are counted
  (`truncated` in JSON).
- Any rule works, not only `pattern`. A `<lang>::pattern` rule reports its
  exact matches; every other module reports the original lines its
  rewrite would change.
- Rules are searched independently against the original files, not run
  in sequence.
- `--jobs N` (else `COLAB_JOBS`, else one per core) sets the worker
  threads, as for `refactor`; `--jobs 1` searches one file at a time.
  The hits and their order are the same for any count.
- A rule that finds nothing is warned about, with
  [near misses](#near-misses) where its module saw something close.

`--format json` is one document:

```json
{"matches":[{"path":"cmd/main.go","line":22,"col":15,"rule":0,"text":"errors.Wrap(err, \"startup\")"}],
 "summary":{"visited":412,"scanned":38,"files":2,"matches":2},
 "rules":[{"i":0,"rule":"go::pattern 'errors.Wrap($E, $M)' -> delete","files":2,"matches":2}]}
```

#### Data tables: --table and --group-by

Before a migration, count the call sites and their shapes. `--table`
prints every match as a row (all of them: `--max-results` caps it only
when given), and `--group-by` counts distinct values instead:

```sh
colab find -e "match rust::pattern 'Error::\$K(format!(\$\$\$A))' { delete }" \
    --table csv --group-by '$K' crates
```

```text
$K,count
Config,93
UnsupportedOperation,41
```

- Columns: `path`, `line`, `column`, `rule` (its number), `id` (its `id`,
  if any), one per metavariable any match binds (`$K`, `$A`, …; empty
  where a match does not bind it), and the match `text`.
- `--table csv` quotes a cell holding a comma, quote or line break
  (RFC 4180); `--table ndjson` is an object per row, with `line`,
  `column`, `rule` and `count` as numbers.
- `--group-by <column>` repeats for a key of several columns
  (`--group-by rule --group-by '$K'`). Rows come most common first. An
  unknown column is an error listing the ones there are.

### `colab rename`

Scope-aware rename, **delegated to the language's own server** and
reported like a codemod. colab starts the server, waits for it to finish
indexing, asks it for `textDocument/rename`, and runs the answer through
the same report / diff / `--check` / `--backup` / `--verify` pipeline as
`refactor`. colab itself still does no semantic analysis — the server
resolves which `value` is which; colab makes its answer reviewable and
revertable.

```
colab rename (--at FILE:LINE:COL | --symbol NAME --in FILE) --to NEW
             [--lsp '<command>'] [--root DIR] [-C DIR]
             [--format human|json|ndjson|diff] [--write|--dry-run|--check]
             [--backup DIR] [--verify CMD] [--verify-lsp[=CMD]] [--allow-dirty] [--commit-moves]
             [--tidy] [--include-text] [--timeout SECS] [--detail LEVEL]
```

The server renames code only. colab also looks for the old name, as a
whole word, in comments and string literals of every source file and
anywhere in config and docs files (TOML, JSON, YAML, Markdown, text)
across the git work tree, not just the server's project root. Without
`--include-text` they are counted in a warning (`` `old` also appears on
3 line(s) in 2 file(s) the server left alone …, e.g. README.md:12``);
with it they become a second rule, `text occurrences of `old` -> `new``,
so they can be reviewed separately in `--format json`. A code occurrence
the server did not rename is a different symbol and is never touched.

```sh
# Rename the field, not the function that shares its name.
$ colab rename --at src/config.rs:2:9 --to amount --format diff
--- a/src/config.rs
-    pub value: u32,
+    pub amount: u32,
…
--- a/src/main.rs
-    println!("{} {}", c.value, value());
+    println!("{} {}", c.amount, value());
```

- **Where:** `--at` is exact (1-based line and column, in characters).
  `--symbol NAME --in FILE` takes the first whole-word occurrence of
  `NAME` in `FILE` — point it at the file that *declares* the symbol.
  The server rejects a position with nothing renameable
  (`prepareRename`), so a miss is an error, not a silent no-op.
- **Which server:** by extension — `rust-analyzer`, `gopls`, `clangd`,
  `pyright-langserver --stdio`, `typescript-language-server --stdio`,
  `sourcekit-lsp`, `ruby-lsp`, `kotlin-language-server`, `jdtls`,
  `csharp-ls`, `intelephense --stdio`. Override with `--lsp`.
- **Root:** the nearest ancestor holding `.git`, `Cargo.toml`, `go.mod`,
  `package.json`, `pyproject.toml`, `compile_commands.json`, … — the
  server needs the project to resolve references. Override with `--root`.
- **Readiness:** the rename is sent only once the server reports no work
  in progress (`$/progress`, rust-analyzer's `serverStatus`) and has been
  quiet for 500 ms; a server still indexing at `--timeout` (default 120 s)
  is an error, because its answer could be partial.
- **File operations** (jdtls renaming a class *and* its file, a module
  rename) are made through git; see [File operations](#file-operations).
  Outside a git work tree the server is told it may not send any, and one
  it sends anyway is refused (exit 3).
- **Refused, not half-done:** a server without `renameProvider`; a server
  that is not installed (the error says to pass `--lsp`).
- **Modes and safety** match `refactor`: dry-run unless `--write` (or
  `--format human` on a TTY); `--check` exits 10; `--backup` pairs with
  `colab undo`; `--verify` restores every file and exits 1 if the command
  fails.

The report has one rule, `lsp rename `old` -> `new` via <server>`, so
`--format json` has the same shape as any `refactor` run.

### `colab mv`

Move a file or directory with `git mv`, and let the language server
update what refers to it (`workspace/willRenameFiles`: a Rust `mod`
declaration and paths, TypeScript imports, Java packages).

```
colab mv FROM TO [--lsp '<command>' | --no-lsp] [--root DIR] [-C DIR]
         [--format human|json|ndjson|diff] [--write|--dry-run|--check]
         [--backup DIR] [--verify CMD] [--verify-lsp[=CMD]] [--allow-dirty] [--commit-moves]
         [--tidy] [--timeout SECS] [--detail LEVEL]
```

```sh
$ colab mv src/util.rs src/helpers.rs --format diff     # rust-analyzer
--- a/src/main.rs
+++ b/src/main.rs
-mod util;
+mod helpers;
 fn main() {
-    util::go();
+    helpers::go();
 }
diff --git a/src/util.rs b/src/helpers.rs
similarity index 100%
rename from src/util.rs
rename to src/helpers.rs

$ colab mv src/util.rs src/helpers.rs --write --verify "cargo check -q"
$ git status --short
R  src/util.rs -> src/helpers.rs
 M src/main.rs
```

- **`TO`** that is an existing directory means "into it", as for `mv`. An
  existing file is never overwritten.
- **Server:** defaults by `FROM`'s extension, as for `rename`. One that
  does not advertise `willRename` still gets the move, with a warning that
  references were not updated; so does a path with no default server.
  `--no-lsp` moves with git alone.
- Everything else (modes, `--check` exit 10, `--backup`, `--verify`,
  `--format diff` as `git apply` input) is as for `rename`.

### `colab action`

Run one of the language server's **code actions** — the refactorings an
IDE offers: extract function / variable / constant, inline, change
signature, convert, organize imports — and report it like a codemod.
colab computes none of them; the server does, and its answer goes
through the same report, diff, `--check`, `--backup`, `--verify`, and
git-aware file operations as `rename`.

```
colab action --at FILE:LINE:COL[-LINE:COL] (--list | [--kind KIND] [--title TEXT])
             [--lsp '<command>'] [--root DIR] [-C DIR] [--timeout SECS]
             [--format …] [--write|--dry-run|--check] [--backup DIR]
             [--verify CMD] [--verify-lsp[=CMD]] [--allow-dirty] [--commit-moves] [--tidy] [--detail LEVEL]
```

```sh
$ colab action --at src/main.rs:11:16-11:25 --list            # rust-analyzer
refactor.extract                 Extract into variable
refactor.extract                 Extract into constant
refactor.extract                 Extract into static
refactor.extract                 Extract into function

$ colab action --at src/main.rs:11:16-11:25 --kind refactor.extract --title function --format diff
-        sum += p * 2 + 1;
+        sum += fun_name(p);
…
+fn fun_name(p: &u32) -> u32 {
+    p * 2 + 1
+}
```

- **`--at`** is a position (`FILE:LINE:COL`) or a selection
  (`FILE:LINE:COL-LINE:COL`), 1-based, columns in characters. Extract
  actions need a selection; most others a position.
- **Choosing.** `--kind` is a dotted prefix (`refactor.extract` matches
  `refactor.extract.function`); `--title` a case-insensitive substring.
  Exactly one *enabled* action must match — several, or none, is an error
  that lists every action offered there, disabled ones with the server's
  reason, so the next call can be exact. An exact title breaks a tie.
  Servers differ: rust-analyzer gives every extract the kind
  `refactor.extract`, so its titles are what tell them apart.
- **How it runs.** An action without an edit is resolved
  (`codeAction/resolve`). One that carries a *command* is executed
  (`workspace/executeCommand`), and the edit the server then sends back
  (`workspace/applyEdit`) becomes the result — accepted only in that
  window, and written only through colab's report.

### colab edit

A one-off edit an agent would make with an exact-string match, addressed
by declaration instead:

```sh
colab edit --in src/config.rs --list                  # the file's name paths
colab edit --symbol Config/load --in src/config.rs --replace-body --text-file new.rs
colab edit --symbol main --in main.go --insert-before --text 'func helper() {}'
colab edit --symbol A/m --in a.py --insert-after --text-file - --write < method.py
```

- A **name path** is the end of a declaration's full path: `load` finds
  every `load`, `Config/load` only the one in `Config`. One that names
  several is an error listing each, with its line; one that names none
  lists the file's paths, with a "did you mean".
- **Declarations** come from tree-sitter for every source language: a
  Rust `impl` is named by its type (`impl Display for Config` is
  `Config`), a Go method sits under its receiver type, a C or C++
  function is named by its declarator. A declaration includes the doc
  comments and attributes directly above it, and a decorator or `export`
  around it. With `--lsp '<command>'`, or when a kept server for the
  file is already running, the language server's `documentSymbol`
  answers instead.
- `--replace-body` replaces the whole declaration, doc comments
  included; `--insert-before` / `--insert-after` add the text, with a
  blank line, before or after it. The text is re-indented to the
  declaration's indentation, so it can be written flush-left.
- The result is an ordinary report: `--format diff` / `json` preview it,
  `--write` (or `--check`, `--backup`, `--verify`) as for `colab rename`.
- MCP: `colab.edit_symbol`.

### `colab references`

```
colab references (--at FILE:LINE:COL | --symbol NAME --in FILE) [--include-declaration]
                 [--format human|json] [--max-results N] [--lsp …] [--root DIR] [-C DIR]
                 [--timeout SECS]
```

The references the server resolves to the symbol — scope-aware, unlike
`rg -w`, which also matches every other symbol with the same name.
`human` prints `path:line:col: <the whole line>`; `json` is
`{references: [{path, line, col, text}], summary: {files, references},
truncated?}`, the `colab find` shape. Use it to see a change's reach
before making it.

### `colab calls`

```
colab calls (--at FILE:LINE:COL | --symbol NAME --in FILE) [--outgoing]
            [--format human|json] [--max-results N] [--lsp …] [--root DIR] [-C DIR]
            [--timeout SECS]
```

Who calls the function, or with `--outgoing` what it calls, from the
server's call hierarchy (`textDocument/prepareCallHierarchy`, then
`callHierarchy/incomingCalls` / `outgoingCalls`): each caller's or
callee's declaration, in any file. The output is `colab references`'
shape under `callers` / `callees`. A server without a call hierarchy is
an error that names the missing capability. The `calls` / `called_by`
constraints ask the same question inside a rule
([`docs/dsl.md`](dsl.md#asking-the-language-server-type-resolves-implements-calls-called_by)).

### `colab implementations`

```
colab implementations (--at FILE:LINE:COL | --symbol NAME --in FILE)
                      [--format human|json] [--max-results N] [--lsp …] [--root DIR] [-C DIR]
                      [--timeout SECS]
```

What implements the interface, trait or abstract method
(`textDocument/implementation`): each implementation's declaration, under
`implementations`. Use it before changing an interface to see every type
that must follow; the `implements` constraint narrows a rule to them.

### `colab safe-delete`

```
colab safe-delete (--at FILE:LINE:COL | --symbol NAME --in FILE) [--lsp …] [--root DIR] [-C DIR]
                  [--format …] [--write|--dry-run|--check] [--backup DIR] [--verify CMD] [--tidy]
                  [--allow-dirty] [--commit-moves] [--timeout SECS] [--detail LEVEL]
```

Delete a declaration only if nothing outside it refers to it. The
declaration's extent is the server's (`textDocument/documentSymbol`, so
doc comments and attributes go with it where the server counts them);
references inside it, such as recursion, do not keep it alive. If
references remain, nothing is changed: they are printed (or, with
`--format json`, returned as `{deleted: false, symbol, references, …}`)
and the command exits 1.

### `--tidy`

On `rename`, `mv`, `action`, `safe-delete` and `refactor`: after the
change, each changed file is tidied — the new contents, in memory, so it
works in a dry run too. Tidying is reported as its own rule, `tidy via
<server or command>`, so the report shows which changes are the
refactoring's and which are tidying's; one that changes nothing is not
reported as a dead rule.

- A file whose extension the nearest `colab.toml`'s `[formatters]` names
  goes through that command (`sh -c`, the file on stdin, the result on
  stdout; `{path}` is replaced by the file's quoted path). A formatter that
  fails leaves the file as the rules made it, with a warning.
- Otherwise the file's language server organizes imports
  (`source.organizeImports`) and formats (`textDocument/formatting`); a
  server without one of the two is warned about and skipped. A default
  server that is not on `PATH` is skipped with a warning, so one missing
  server does not stop the rest.
- `--tidy=<CMD>` names the server for every file and ignores
  `[formatters]`.
- With `refactor --bisect`, the full set is tidied once before the check;
  subsets tried while bisecting are not.

### File operations

`colab rename` and `colab mv` can move, create, or delete files. They do
it only inside a git work tree (a repository or a linked worktree), and
only with git, so a mistake is recoverable and history survives:

- **Moves** use `git mv`, so the index records the rename at once.
  Creates are `git add`ed and deletes are `git rm`ed. Content edits are
  written after the operations and left **unstaged**, so
  `git diff --staged` shows the file operations and `git diff` the edits.
  An untracked file is moved on disk and the report says git has no
  history to keep.
- **Clean paths.** A run is refused if any path it touches (both ends of
  a move, every edited file) has uncommitted changes, so undoing colab can
  never discard your work. `--allow-dirty` overrides. The check runs in
  every mode, so a dry run tells you whether the write would succeed.
- **History.** Git does not store renames. It infers them when a deleted
  and an added file are at least 50% similar, so a file that is moved
  *and* heavily rewritten in one commit looks like a delete plus an add,
  and `git log --follow` / `blame` stop there. colab warns about each such
  file. `--commit-moves` commits the file operations on their own first
  (`colab: move a.rs -> b.rs`, only those paths, with your hooks), and the
  edits stay for you to commit, so history follows the file.
- **Undo.** `--backup DIR` also records the operations
  (`DIR/colab-file-ops.json`). `colab undo --from DIR` moves files back
  with git, removes created files, restores deleted ones, then restores
  contents. A failed `--verify` does the same, after dropping the
  `--commit-moves` commit if it is still `HEAD`.
- **Checking the server's edit.** `rename`, `mv` and `action` take
  `--verify-lsp` as `refactor` does: before anything is written, each
  changed file is opened in its language server with the change made,
  and every error the change adds is a conflict that refuses the write
  (exit 11). It sees the changed files only; a change that breaks a file
  it did not touch (rust-analyzer's "Extract Module" leaves callers in
  other files pointing at a now-private item) needs `--verify 'cargo
  check -q'`, which runs after the write and undoes it on failure.
- **Output.** JSON gains `moved: [{from, to}]`, `created`, and `deleted`
  (plus counts in `summary`) only when there are file operations. ndjson
  emits `{"type": "move"|"create"|"delete", …}` events first. Diff output
  carries git's `rename from` / `rename to`, `new file mode`, and
  `deleted file mode` headers, so `git apply --index` replays it.
  `--check` counts a pending file operation as a change (exit 10).

### `colab ast`

Print a syntax tree — of a pattern exactly as `<lang>::pattern` compiles
it, or of a snippet — to see why a pattern does or does not match.

```
colab ast --lang <lang> (--pattern '<code>' | --stdin) [--format human|json]
```

```sh
$ colab ast --lang go --pattern 'errors.Wrap($E, $M)'
context: package p⏎func _() {⏎@@PATTERN@@⏎}⏎
prefilter: errors
call_expression
  function: selector_expression
    operand: identifier  "errors"
    field: field_identifier  "Wrap"
  arguments: argument_list
    identifier  "$E"
    identifier  "$M"
```

Only named nodes are shown, one per line, with field names — the
structure a pattern is compared on. `context` is the wrapper the pattern
needed to parse (none if it parsed on its own); `prefilter` is the literal
a file must contain to be considered. A snippet (`--stdin`) adds
`line:col` positions and flags syntax errors (`ERROR` / `MISSING` nodes).

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

Restore files from a backup directory produced by `colab refactor`,
`rename`, or `mv` with `--backup <DIR>`. The backup directory mirrors the
absolute paths of every file the run touched; `colab undo` walks it and
writes each backup over its original location. If the run made file
operations, they are undone first, through git (see
[File operations](#file-operations)).

```sh
colab refactor --script s.colab --write --backup .colab-backup .
# … review / decide to roll back …
colab undo --from .colab-backup
```

### `colab licenses`

Print colab's license, then every crate the binary is built from (on
any release target) with its declared license, then the license texts
those crates ship: the contents of
[`THIRD-PARTY-LICENSES.txt`](../THIRD-PARTY-LICENSES.txt), compiled in.
A release archive holds only the binary, so this is where its notices
travel.

```sh
colab licenses | less
```

The file is generated by `scripts/third-party-licenses.py` (`make
licenses`) from `cargo metadata`; CI fails when it no longer matches
`Cargo.lock`, and `cargo deny check licenses` holds every dependency to
the allowlist in [`deny.toml`](../deny.toml).

### `colab pack list`

List the packs: those built into colab, then discoverable `.colab`
(or, deprecated, `.codemod`) files. Lookup paths, in order:

1. Built in: the packs under `examples/packs/`, named by path
   (`go/1.18-modern`, `java/8-to-21`, `js/esm`, `python/3.12-typing`,
   `rust/edition-2021-to-2024`). Name them in `colab.toml` with
   `pack = "<name>"` and run them with `colab fix`.
2. `<repo>/.colab/packs/` — the project-local pack directory,
   discovered by walking up from the current directory looking for
   a `.git` marker.
3. `~/.colab/packs/` — the user-global pack directory.

```sh
colab pack list
```

Output (sorted by path; a built-in pack's path is `builtin:<name>`):

```json
{
  "packs": [
    { "name": "go/1.18-modern", "path": "builtin:go/1.18-modern", "source": "builtin" },
    {
      "name": "javax-to-jakarta",
      "path": "/path/to/repo/.colab/packs/javax-to-jakarta.colab",
      "source": "repo"
    }
  ]
}
```

Run a pack with `colab refactor --script`: a built-in one by the path
`pack list` prints, a file pack by its file:

```sh
colab refactor --script builtin:go/1.18-modern --format diff .
PACK=$(colab pack list | jq -r '.packs[] | select(.name == "javax-to-jakarta") | .path')
colab refactor --script "$PACK" --check .
```

A pack is just a `.colab` file. Use `include "<path>"` from a
top-level project script to compose multiple packs, or list them in
`colab.toml` (see `colab fix`).

### `colab explain (--script <path> | -e <rule>...)`

Parse the script and emit its IR as JSON without running anything.
Useful for verifying syntax in CI before applying:

```sh
colab explain --script my-pack.colab
```

Output:

```json
{
  "name": "two-renames",
  "items": [
    {"kind": "match", "namespace": "go::import", "match": "old.module", "scope": null, "action": "replace", "value": "new.module"},
    {"kind": "match", "namespace": "go::import", "match": "another", "scope": null, "action": "delete", "value": null},
    {"kind": "match", "namespace": "rust::symbol", "match": "Config", "scope": "core/**", "action": "replace", "value": "CoreConfig"},
    {"kind": "include", "path": "shared/javax-to-jakarta.colab"}
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

Start the colab Language Server on stdio, for editing `.colab` files:

- **Diagnostics, per rule.** Every open / change lowers each `match`
  block on its own, so every bad rule — an unknown module, an action the
  module lacks, a `pattern` that is not valid code — is underlined where
  it is written, with the exit code (2 / 3) in the diagnostic `code`. A
  syntax error is reported alone, at its token; `include` problems land
  on the first `include`. A rule that compiles but would rewrite its own
  output on every run is underlined as a warning.
- **Hover.** On `<lang>::<module>`: the module's description and every
  action it supports. On an action keyword: what it does *for that
  rule's module* — or that the module has no such action.
- **Code actions: preview, apply, undo.** On any rule of a script that
  compiles, for that rule and for the whole script:
  - *colab: preview `<rule>` across the workspace* — offered when the
    client takes change annotations
    (`workspace.workspaceEdit.changeAnnotationSupport` with
    `documentChanges`). The edit's hunks are annotated with the rules
    that made them, each marked `needsConfirmation`, so the editor opens
    its refactor preview grouped by rule and applies only the hunks and
    files kept there.
  - *colab: apply `<rule>` to the workspace* — when the client applies
    server-sent edits (`workspace.applyEdit`), a `colab.apply` command:
    the server computes the edit, sends `workspace/applyEdit`, and keeps
    what it changed. Otherwise the action carries the edit itself.
  - *colab: undo applying `<rule>`* — after a `colab.apply`, the
    `colab.undo` command puts the newest applied run back (up to ten
    deep). Each file is three-way merged — the file now, the file before
    the run, based on the file as the run left it — so edits made since
    survive when they do not overlap the run's; a file whose later edits
    do is left alone and named in a message.

  Edits are minimal line hunks under the workspace roots, computed on
  `codeAction/resolve` (up front for clients without lazy resolution).
  While a walk runs, a client that shows server-initiated progress
  (`window.workDoneProgress`) sees `colab: <script>` with the files
  scanned so far, every 25 files.
- **Go to definition** on an `include "<path>"` opens that script,
  resolved as the compiler resolves it (against the including file's
  directory); nothing happens when the file does not exist.
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
| `colab.lint_script` | `script`, `cwd?` | `{ok: true, name, rule_count, rules[], warnings?}`; `warnings` lists rules that would rewrite their own output again. |
| `colab.test_rule` | `script`, `source`, `path`, `expected?`, `cwd?` | Runs the script over one in-memory snippet; `path` is only a filename hint. `{changed, output, rules[], diff?, matches_expected?, expected_diff?, warnings?}`. Nothing on disk is read or written. |
| `colab.find` | `script`, `paths[]`, `cwd?`, `detail?` (`concise`/`detailed`), `max_results?` | Read-only search; same document as `colab find --format json`. Paths are reported relative to `cwd`. |
| `colab.ast` | `lang`, one of `pattern` / `source` | A pattern's tree as compiled (with `context` and `prefilter`), or a snippet's (with `has_error`). |
| `colab.preview` | `script`, `paths[]`, `cwd?`, `detail?`, `max_files?`, `max_diff_bytes?`, `no_link_check?` | Summary, per-rule match counts, changed paths, and (at `detail: "diff"`) capped diffs. Disk untouched. Runs scripts with `scan` phases, and the Markdown link check `colab refactor` makes (each link the change would break is a conflict; `no_link_check: true` skips it). |
| `colab.apply` | same as preview, `allow_conflicts?` | Same shape, but writes changes back; writes nothing while a rule reports a conflict. |
| `colab.move_import` | `lang`, `from`, `to?`, `paths[]`, `cwd?`, `write?`, `detail?`, `allow_conflicts?` | No DSL needed: generates `match <lang>::<import module> "<from>" { replace "<to>" }` (`delete` without `to`) and returns the preview / apply report with the generated `script`. |
| `colab.set_config_key` | `key`, `value` or `delete: true`, `only_if_absent?`, `paths[]`, `lang?`, `write?`, … | No DSL needed: `json::key` / `toml::key` `set` (`insert` with `only_if_absent`, or `delete`), or `markdown::block`; the format comes from the first path's extension. Returns the report with the `script`. |
| `colab.references` | `at` (`FILE:LINE:COL`) or `symbol` + `in`, `include_declaration?`, `max_results?`, `lsp?`, `root?`, `cwd?` | References the language server resolves; same document as `colab references --format json`. Read-only. |
| `colab.calls` | `at` or `symbol` + `in`, `outgoing?`, `max_results?`, `lsp?`, `root?`, `cwd?` | Callers (or callees) from the server's call hierarchy; same document as `colab calls --format json`. Read-only. |
| `colab.implementations` | `at` or `symbol` + `in`, `max_results?`, `lsp?`, `root?`, `cwd?` | Implementations the server finds; same document as `colab implementations --format json`. Read-only. |
| `colab.rename` | position as above, `to`, `write?`, `tidy?`, `detail?`, `lsp?`, `root?`, `cwd?` | Scope-aware rename via the server; the `colab.preview` report plus `written`. |
| `colab.action` | `at` (position or `FILE:L:C-L:C` selection), `list?`, `kind?`, `title?`, `write?`, `tidy?`, … | With `list: true`, `{actions: [{title, kind, preferred, disabled?}]}`; otherwise runs the one matching action and returns the report. |
| `colab.mv` | `from`, `to`, `lsp?`, `no_lsp?`, `root?`, `cwd?`, `write?`, `tidy?`, `detail?` | `colab mv`: the move through `git mv` plus the server's reference edits (`workspace/willRenameFiles`); the `colab.preview` report plus `written`. |
| `colab.rename_symbol` | as `colab.rename`, plus `include_text?` | The server's rename, plus the old name's text occurrences the server leaves alone: comments and string contents in source files, and anywhere in config/docs files (TOML, JSON, YAML, Markdown, text) in the git work tree. Listed under `text: {files, lines, included, sample[]}`; changed as a second rule only with `include_text: true`. |
| `colab.edit_symbol` | `in`, `symbol`, `edit` (`replace_body`, `insert_before`, `insert_after`), `text`, `list?`, `lsp?`, `cwd?`, `write?`, `detail?` | `colab edit`: the declaration found by name path, the edit as the `colab.preview` report plus `written`; with `list: true`, `{declarations: [{path, kind, line}], by}`. |
| `colab.outline` | `paths[]`, `depth?`, `cwd?` | Each Markdown file's headings: `address`, `anchor`, `level`, `line` / `end_line`, and counts of tables, lists and fences. Read-only. |
| `colab.section` | `path`, `section`, `body?`, `cwd?` | One section's `text`, by heading path or `#anchor`. Read-only. |
| `colab.edit_markdown` | `module` (section, row, item, link, frontmatter, block), `action`, `target`, `value?`, `paths[]`, `write?`, … | No DSL needed: one `markdown::` rule, quoted as the value needs (raw strings included). Returns the preview / apply report with the `script`, and conflicts for any link it would break. |

The four language-server tools are served by the `colab` binary: `colab-mcp` stays free of
the language-server client, and the binary registers them through
`colab_mcp::ToolProvider`. They preview unless `write: true`, and are
annotated `destructiveHint` because they can write.

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
exit_code, line?, column?, expected?, snippet?}}`. `kind` says what to do
next: `parse`, `unsupported` and `config` mean the request should change;
`server` (a language server failed to start, timed out, exited or sent an
unusable answer), `git` (a git command failed, or the work tree is not a
repository or has uncommitted changes) and `io` mean the environment
should. All but `parse` (2), `unsupported` (3) and `io` (4) exit 1. JSON-RPC error
responses are reserved for malformed *calls*: `-32601` for an unknown
method, `-32602` for an unknown tool or missing or mistyped arguments.

Wire format: JSON-RPC 2.0 over stdio using MCP's stdio transport:
one JSON message per line, newline-delimited (no `Content-Length`
headers; those are only for the LSP `colab server`). Methods supported: `initialize`,
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

### `--help-json`

Every command accepts `--help-json`. It prints that command's shape as
JSON instead of prose: subcommands, every flag with its `long`/`short`
spelling, whether it takes a value, allowed `values`, `default`,
`conflicts_with`, `repeatable`, and any `required_groups` (such as
"one of `--script` / `--rule`"). It is generated from the parser itself,
so it cannot drift, and it works even where required arguments are
missing:

```sh
colab --help-json                 # the root: subcommands
colab refactor --help-json        # every refactor flag
```

### `colab outline` and `colab section`

Read Markdown by structure instead of whole files. An outline costs a
few lines; a section costs only itself.

```
colab outline <files or dirs>... [--depth N] [--format human|json]
colab section <file> "<heading path or #anchor>" [--body] [--format human|json]
```

```
$ colab outline docs/development-plan.md --depth 2
docs/development-plan.md (329 lines)
    1-329   # Colab Development Plan                    4 tables, 16 lists, 1 fence
    6-20      ## Vision                                 1 list
   31-51      ## Order of work                          1 table
  ...
$ colab section docs/development-plan.md "Order of work" --body
```

- Each heading is listed with its line span (the section runs to the next
  heading of the same or a higher level, so it includes its subsections)
  and the tables, lists and code fences it holds. `--format json` gives
  each heading's `address` (what `section` and `markdown::section` rules
  take), `anchor`, `line` and `end_line`.
- A `#` line inside a code fence is code, not a heading, which a
  `grep '^#'` outline gets wrong.
- A section is addressed as in [`markdown::section`](features.md#markdownsection--a-section-by-heading):
  `"Parent > Child"` (the outer segments need not be direct parents) or
  `"#anchor"`. An ambiguous or missing address is an error naming the
  candidates.
- Directories are searched for `.md` / `.markdown` files, honouring
  `.gitignore`.
- MCP: `colab.outline {paths, depth?, cwd?}` and
  `colab.section {path, section, body?, cwd?}`, both read-only.

### colab mv-section

Move a Markdown section, subsections and all, to another file:

```sh
colab mv-section docs/plan.md "Old item" --to docs/done.md --under "Shipped" --format diff
colab mv-section docs/plan.md "#old-item" --to docs/done.md --before "Earlier" --write
colab mv-section docs/plan.md "Old item" --to docs/done.md --show-script
```

- Rules act on one file each, so a move is a script colab writes for
  you and runs like `refactor`: a `markdown::section` insert in the
  target (the body read from the source), a delete in the source, and
  `markdown::link` rules that repoint links to it. `--show-script`
  prints it instead.
- Without `--under` or `--before` the section goes at the end of the
  target, at its top level. Its subsection headings shift to the level
  it lands at; `#` lines inside code fences stay as they are.
- Links to it in the source and the target are rewritten, and, when the
  two files share a directory, links from every other file too (only the
  file name changes). The section's own links are rebased: an anchor to
  a section left behind gains the path back to the source, a relative
  file link is recomputed from the target's directory, and an anchor
  that moved with it stays. Across directories a link from a third file
  differs per linking file, so those are left to the link check: a move
  that would break one writes nothing and exits 11 (`--allow-conflicts`
  overrides).
- A target that already has a section with that title there, a heading
  holding ` > ` or ` < `, or a move within one file is refused.
- Flags: `--write | --dry-run | --check`, `--format`, `--allow-conflicts`,
  `-C`.

### `colab links`

```
colab links <files or dirs>... [--format human|json]
```

Lists the broken links in Markdown files: a relative link whose file does
not exist, or whose `#anchor` matches no heading (GitHub's anchor rules)
and no HTML `id` / `name` in its target. Links with a scheme
(`https:`, `mailto:`) and absolute paths are not checked. Exits 10 when it
finds any, so it can gate CI.

```
$ colab links docs README.md
docs/dsl.md:18: ./cli.md#explain — no section `#explain` in `docs/cli.md`
```

`colab refactor` runs the same check on the files it would change, and on
every other Markdown file under its paths, before writing: a link the run
would break is a conflict ([Conflicts](#conflicts)). `--no-link-check`
turns that off.

### Kept language servers

Every command that asks a language server (`rename`, `mv`, `action`,
`references`, `safe-delete`, `--tidy`, `--verify-lsp`, and the MCP tools
built on them) needs a server that has indexed the project. The first such command starts `colab lsp-daemon`,
which keeps the server running for that server command and project root,
and later commands connect to it:

```sh
colab action --at src/main.rs:3:5-6:6 --list          # starts rust-analyzer: 3.5 s
colab action --at src/main.rs:3:5-6:6 --kind refactor.extract --title function --write   # 0.6 s
colab rename --symbol fun_name --in src/main.rs --to total_price --write                 # 0.6 s
colab lsp-daemon --stop                                # stop them all (or --root DIR)
```

```
colab lsp-daemon [--stop] [--socket PATH] [--root DIR] [--idle SECS] [-- SERVER...]
```

You rarely start one by hand: `--socket`, `--root` and the server command
are what a command passes when it starts a daemon, and `--idle` (default
600 seconds) is how long one waits without a client.

Each command still sees the files as they are on disk: when a command
ends, the documents it opened are closed, and when the next connects the
server is told which files changed in between. A daemon exits after ten
minutes without a command. This is the default for the `colab` binary. `COLAB_KEEP_SERVER=0` gives every command a server of its own; if the daemon cannot be reached, a command warns and starts its own. A rebuilt or upgraded colab starts daemons of its own rather than reuse ones running older code. Kept servers use Unix sockets, so on Windows every command starts its own server, and `colab lsp-daemon` says so.

## Exit codes

The same table is in `colab --help`:

| Code | Meaning |
| ---- | ------- |
| 0 | Success — no changes needed, or `--write` succeeded. |
| 1 | Generic / configuration error. |
| 2 | Script parse error. |
| 3 | Unsupported namespace or operation. |
| 4 | I/O error (with the offending path in the log line). A reader that closes the pipe early (`colab … \| head`) is not one: colab exits 0 quietly, as `cat` does. |
| 10 | `--check` found changes a rule above `severity info` would make. |
| 11 | `--write` found [conflicts](#conflicts) and wrote nothing (`--allow-conflicts` overrides). |
| 12 | `colab test`: a case of a pack failed (its output, a second run, or a conflict). |

Code 2 is what `clap` itself uses for argument-parsing errors, which
overlaps with the script parse code by design — both are "the input
was malformed".

## Useful pipelines

```sh
# Preview every change without touching disk.
colab refactor --script s.colab --format diff path/

# Pre-commit gate: fail if anything would change.
colab refactor --script s.colab --check . || exit $?

# Stream one object per changed file to a structured log.
colab refactor --script s.colab --format ndjson . | tee changes.ndjson

# stdin rewriter for an editor pre-save hook.
cat current-buffer.go | colab refactor --script s.colab --stdin --path current-buffer.go > rewritten.go

# Restrict a sweeping refactor to one subtree, skipping vendored code.
colab refactor --script s.colab \
    --include 'cmd/**/*.go' --exclude '**/vendor/**' \
    --write .

# CI gate: only verify files changed on this branch.
colab refactor --script s.colab --check --changed-since origin/main || exit $?

# Pre-commit hook: only the staged files.
colab refactor --script s.colab --check --staged || exit $?

# Apply rules one-by-one with build-check after each; auto-revert on failure.
colab refactor --script s.colab --write --verify "cargo check --quiet" .

# Same, plus a git commit per rule for clean review history.
colab refactor --script s.colab --write --verify "cargo check --quiet" --commit-per-rule .
```

## Environment

- `NO_COLOR` — when set, ANSI colour is disabled in `human` output.
- `RUST_LOG` — standard `env_logger` filter; set to `debug` to see
  per-file "no changes" lines and tree-sitter parse hints.
- `COLAB_JOBS` — worker threads when `--jobs` is not given.
- `COLAB_KEEP_SERVER` — `0` gives each command its own language server instead of the kept one (the default); see [Kept language servers](#kept-language-servers).
- `COLAB_CACHE_DIR` — where packs from git and fleet clones are kept;
  default `$XDG_CACHE_HOME/colab`, else `~/.cache/colab`.
- `HOME` — also where `colab agent install --codex` finds `~/.codex`.
- `COLAB_BLESS` — `1` makes the tests (and `colab test`) rewrite their
  expected output: corpus, evals, `llms-full.txt`, a pack's cases.
- `OTEL_EXPORTER_OTLP_ENDPOINT` (and the other standard `OTEL_*`
  variables) — send traces of colab's work to an OpenTelemetry collector;
  off when unset. The `[telemetry]` table of the user config file
  (`$XDG_CONFIG_HOME/colab/config.toml`, else `~/.config/colab/config.toml`)
  sets the same, beneath the variables; its `enabled = true` alone sends
  to `http://127.0.0.1:4318`. What a trace holds, every variable
  and key: [operations.md#traces-opentelemetry](operations.md#traces-opentelemetry).
- `XDG_CONFIG_HOME` — where that user config file is read from.

## See also

- [`docs/dsl.md`](./dsl.md) — codemod script language.
- [`docs/features.md`](./features.md) — what each backend can do.
