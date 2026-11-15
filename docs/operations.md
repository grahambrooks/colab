# Installing and operating colab

How to install colab, put it in front of an agent, run it in CI, keep it
fast, and take it out again. For what a script can say, see
[`dsl.md`](dsl.md); for every flag, [`cli.md`](cli.md).

## Install

| Channel | Command | Notes |
| ------- | ------- | ----- |
| Homebrew (macOS, Linux) | `brew tap grahambrooks/colab https://github.com/grahambrooks/colab` then `brew install colab` | The formula is in this repository (`Formula/colab.rb`), updated by each release. |
| Release archive | download from the [releases page](https://github.com/grahambrooks/colab/releases) | `colab-<tag>-<target>.tar.gz` with the `colab` binary at its root, and `SHA256SUMS`. Targets: `aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, and from 2026.11.4 `x86_64-pc-windows-msvc` (a `.zip`). |
| From source | `cargo install --git https://github.com/grahambrooks/colab colab-cli` | Needs a Rust toolchain and git (the build stamps the commit into `--version`). |

Check it: `colab --version` prints the release and the commit it was built
from, e.g. `colab-cli 2026.11.3.266b50e`.

Known limits:

- **Linux** release binaries are built against glibc 2.39, so Ubuntu
  22.04 and Debian 12 cannot run them; build from source there (a static
  musl build is a [planned follow-up](development-plan.md#follow-ups-from-shipped-milestones)).
- **Windows**: everything works except kept language servers, which use
  Unix sockets: each command starts its own server, as with
  `COLAB_KEEP_SERVER=0`. Formatter, `--verify` and `--bisect` commands run
  under `cmd /C` there, `sh -c` elsewhere.

Language servers are only needed for the commands that delegate to one
(`rename`, `mv`, `action`, `references`, `safe-delete`, `--tidy` without a
`[formatters]` entry, `--verify-lsp`, and `where … type` / `resolves`).
colab looks for the usual one for each language on `PATH`:
`rust-analyzer`, `gopls`, `clangd`, `pyright-langserver --stdio`,
`typescript-language-server --stdio`, `jdtls`, `csharp-ls`,
`kotlin-language-server`, `intelephense --stdio`, `ruby-lsp`,
`sourcekit-lsp`. `--lsp '<command>'` names another.

## Put colab in front of an agent

### Claude Code: the plugin

```text
/plugin marketplace add grahambrooks/colab
/plugin install colab@colab
```

The plugin brings the `colab-codemod` skill, the MCP server (`colab mcp`:
20 tools, and colab's documentation as resources) and a **post-edit
hook**. The hook needs colab 2026.11.4 or later on `PATH`; with an older
one it stays silent. To try the plugin from a checkout:
`claude --plugin-dir plugins/claude/colab`.

### Any agent: `colab agent install`

```sh
colab agent install              # every agent found: Claude Code, Cursor, Codex
colab agent install --claude     # or name them: --claude --cursor --codex
colab agent install --no-hook    # without Claude Code's post-edit hook
colab agent install --dry-run    # the diffs, nothing written
colab agent uninstall            # take it out again
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

- **Idempotent.** A second install writes nothing and reports every file
  `unchanged`.
- **Uninstall takes back only colab's.** Those keys, the one hook, the
  table, the block and colab's own files go; everything else in each
  file stays exactly as it was, and a file or directory left with nothing
  else is removed. Install then uninstall gives back the files byte for
  byte.
- A file colab would edit that does not parse is an error; colab never
  overwrites it.

### The post-edit hook

After every `Edit`, `Write` or `MultiEdit`, the hook runs the packs of
the nearest `colab.toml` over the file just written (in memory) and hands the agent what they would change: each finding's
line, message, rule id and the replacement. A repository without a
`colab.toml` gets nothing; so does a file outside its `[run] paths`.
Check a file by hand with `colab hook post-edit --file <path>` (exit 10
when there is a finding). Full contract: [`cli.md`](cli.md#colab-hook-post-edit).

### MCP by hand

Any MCP client can run `colab mcp` (newline-delimited JSON-RPC on stdio):

```json
{ "mcpServers": { "colab": { "command": "colab", "args": ["mcp"] } } }
```

It answers `tools/list` (20 tools: preview, apply, find, ast, test_rule,
rename, edit_symbol, outline, …) and `resources/list`
(`colab://docs/dsl.md`, `cli.md`, `features.md`, `operations.md`, `README.md`,
`llms-full.txt`). The same documents as one text are in
[`llms-full.txt`](../llms-full.txt) at the repository root.

## A repository's own conventions

`colab.toml` declares the packs a repository keeps applied
([`cli.md`](cli.md#colab-fix)): built-in ones (`colab pack list`), files
in the repository, or packs shared from a git repository, pinned:

```toml
[run]
paths = ["src"]
exclude = ["vendor/**"]
check = "cargo check -q"        # after writing: a failure is bisected to one rule and reverted

[[packs]]
pack = "rust/edition-2021-to-2024"

[[packs]]
path = ".colab/packs/house-style.codemod"

[[packs]]
git = "https://github.com/acme/colab-packs"
tag = "v3"                       # or rev = "<commit>"
path = "go/errors.codemod"
```

Give each pack cases beside it (`<pack>.tests/<case>/input/` and
`expected/`) and `colab test` checks them: output, idempotency,
conflicts, and rules no case exercises (`--bless` writes `expected/`).
A pack fetched from git must pass its own cases before `fix` runs it.

## In CI

```sh
colab test                                    # the repository's packs against their cases (exit 12)
colab fix --check                             # exit 10 if a pack would change anything above `info`
colab fix --check --format github             # annotations on the pull request
colab fix --check --format sarif > colab.sarif   # code scanning
colab fix --check --baseline origin/main      # only findings new since main
colab fix --check --only no-dbg --severity legacy-*=info   # pick rules, soften some
```

colab's own CI runs `colab test` and `colab fix --check` on every push,
on Linux and Windows (`.github/workflows/ci.yml`). Exit codes are in
[`cli.md`](cli.md#exit-codes).

- `--baseline <ref>` runs the packs over a temporary worktree at the
  ref too, and reports only the findings the ref did not have, so a
  convention can gate a repository whose old code breaks it.
- **Renovate**: extend `github>grahambrooks/colab//renovate/colab` and
  Renovate runs `colab fix --write` after each update, so a bump and the
  code it needs land in one pull request (self-hosted Renovate must allow
  the command and have colab on `PATH`).
- **Many repositories**: `colab fleet fleet.toml [--write [--pr]]` runs
  one migration across them, canary first, a temporary worktree each, a
  branch and (with `--pr`) a pull request each; `colab fleet status`
  reports the pull requests. See [`cli.md`](cli.md#colab-fleet).

## Making a change safely

| Step | Command |
| ---- | ------- |
| See where it applies | `colab find -e '<rule>' .` (add `--table csv --group-by '$X'` to count shapes) |
| Preview | `colab refactor -e '<rule>' --format diff .` |
| Check the run | `--format json`: a rule with `"files": 0` matched nothing and is almost certainly wrong; its `near` and `unchanged` say why |
| Apply, verified | `colab refactor … --write --bisect 'cargo check -q'` (a failing rule is found and reverted) |
| Keep an undo | `--backup <dir>`, then `colab undo --from <dir>` |
| Too big to review | `colab review run.ndjson` (record with `--format ndjson --detail diff`), or `--split-pr` for a branch per rule |

colab writes nothing while any rule reports a conflict (exit 11;
`--allow-conflicts` overrides). File moves (`colab mv`, a server's
rename) go through `git mv` inside a git work tree and are refused on
paths with uncommitted changes unless `--allow-dirty`.

## Performance

- **Parallel.** The walker uses a thread per core; `--jobs N` or
  `COLAB_JOBS` sets it.
- **Prefilters.** A rule names a literal that must appear in a file for
  it to change anything; a file without it is not parsed for that rule.
- **One parse per text.** Consecutive rules over a file share a parse
  while the text is unchanged (M10.6). On 5,000 Rust files (55 MB), one
  thread: one rule 3.5 s; four rules that do not fire 13.3 s → 5.7 s; the
  built-in `rust/edition-2021-to-2024` pack (nine rules) 11.9 s → 8.0 s.
- **Kept language servers.** A server colab starts is kept running
  (`colab lsp-daemon`) and reused by the next command, so only the first
  pays its start-up and indexing; it exits after 600 s without a client.
  `colab lsp-daemon --stop` stops every one; `COLAB_KEEP_SERVER=0` gives
  each command its own.
- **Semantic constraints** (`where $X type "…"`) cost one round trip to
  the server per match their other constraints keep: on colab's own
  workspace rust-analyzer took about 10 s to index, then classified 17
  matches in under a second.

## Does it help? The evaluation suite

`cargo test -p colab-cli --test evals` runs eight tasks an agent is
typically given. Each task is checked for exact correctness, in a
temporary git repository. Its cost is compared with making the same
change by `Read` + `Edit`. Bytes are what the agent sends and reads
(tokens ≈ bytes / 4). Time is modelled: 2 s per tool call, 60 tokens/s
written, 2,000 tokens/s read, plus colab's measured run time. Method:
[`evals/README.md`](../evals/README.md); the latest results are in
[`evals/results.md`](../evals/results.md):

| Task | colab calls | bytes | time | Read + Edit calls | bytes | time | Bytes saved | Faster |
| ---- | ----: | ----: | ----: | ----: | ----: | ----: | ----: | ----: |
| Roll out a minimum toolchain | 1 | 698 | 3.8 s | 9 | 3,092 | 25.0 s | 78% | 6.5× |
| Docs upkeep after a release | 2 | 1,654 | 7.3 s | 11 | 4,096 | 30.9 s | 60% | 4.3× |
| Extract the total into a function | 3 | 1,563 | 12.5 s | 4 | 1,507 | 11.3 s | −3% | 0.9× |
| Migrate from `github.com/pkg/errors` to `fmt` | 2 | 1,310 | 6.4 s | 11 | 4,825 | 31.8 s | 73% | 5.0× |
| Rename the `util` module to `helpers` | 1 | 516 | 6.0 s | 5 | 1,407 | 13.4 s | 64% | 2.2× |
| Rename `ParseConfig` to `LoadConfig` | 1 | 714 | 3.0 s | 9 | 3,746 | 26.2 s | 81% | 8.8× |
| Apply the repository's conventions | 1 | 717 | 2.3 s | 12 | 6,245 | 36.7 s | 89% | 15.7× |
| Move the crate to Rust 2024 | 2 | 989 | 5.2 s | 11 | 4,964 | 31.7 s | 81% | 6.1× |
| **Total** | 13 | 8,161 | 46.5 s | 72 | 29,882 | 206.9 s | **73%** | **4.5×** |

Every task's result is exactly the expected one. colab wins wherever a change repeats across sites
or files. The one loss is a single extract-function in one file. There,
`Read` + `Edit` is as cheap: colab asks the language server, which costs
a start-up. The suite's byte budgets are checked in CI, so output that
grows fails the build.

## Files colab writes outside your tree

| What | Where | Remove with |
| ---- | ----- | ----------- |
| Packs fetched from git | `$COLAB_CACHE_DIR`, else `$XDG_CACHE_HOME/colab`, else `~/.cache/colab` (`packs/`) | deleting the directory (refetched on demand) |
| Fleet repositories cloned by URL | the same cache, `fleet/` | deleting the directory |
| Kept language-server sockets | `<temp>/colab-lsp-$USER/` | `colab lsp-daemon --stop` |
| `--baseline` and fleet worktrees | `<temp>/colab-baseline-…`, `<temp>/colab-fleet-…` | removed when the run ends |
| Backups | the `--backup <dir>` you name | `colab undo --from <dir>`, or deleting it |

Nothing is sent anywhere: colab makes no network requests except a
`git fetch` / `clone` you configured (git packs, fleet repositories) and
those of the language servers it runs.

## Upgrade and uninstall

- **Upgrade**: `brew upgrade colab`, a newer archive, or rerun `cargo
  install --git …`. A kept language server from the old build is not
  reused (its socket name includes the build); `colab lsp-daemon --stop`
  clears old ones at once.
- **Uninstall**: `colab agent uninstall` in each project (and once for
  Codex's `~/.codex/config.toml`); remove the plugin from Claude Code's
  `/plugin` menu; `colab lsp-daemon --stop`; delete the cache directory above; then
  `brew uninstall colab` or remove the binary.

## Environment variables

| Variable | Effect |
| -------- | ------ |
| `NO_COLOR` | No colour in `human` output. |
| `RUST_LOG` | Log filter (`debug` shows per-file decisions). |
| `COLAB_JOBS` | Worker threads when `--jobs` is not given. |
| `COLAB_KEEP_SERVER` | `0`: a language server per command instead of the kept one. |
| `COLAB_CACHE_DIR` | Where git packs and fleet clones are kept. |
| `XDG_CACHE_HOME`, `HOME` | The cache's fallback location; `HOME` also locates `~/.codex`. |
| `COLAB_BLESS` | `1`: tests rewrite their expected output (corpus, `colab test`, evals, `llms-full.txt`). |

## When something looks wrong

| Symptom | Look at |
| ------- | ------- |
| A rule reports `matched no files` | `--format json`: its `near` names the closest values the module saw, its `unchanged` why matches were left; `colab ast` shows how a pattern parses |
| A pattern matches too much or too little | `colab find -e '<rule>' --detail detailed` (the bindings), then narrow with `where` |
| Exit 11 | a rule reported a conflict (an identifier that already exists, an unimported qualifier); `--format json` lists them |
| "still indexing at the timeout" | raise `--timeout` / `--lsp-timeout`; the first run on a large workspace pays the server's indexing |
| A command refuses a script with `scan` | `--stdin`, `--bisect`, `--verify`, `--split-pr`, `--changed-since` and MCP cannot run scan phases yet; run without them |

More: the skill's troubleshooting guide,
[`plugins/claude/colab/skills/colab-codemod/references/troubleshooting.md`](../plugins/claude/colab/skills/colab-codemod/references/troubleshooting.md).
