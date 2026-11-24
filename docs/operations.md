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
colab agent install --user       # every project of yours: user-level MCP servers and skill, no hook
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
path = ".colab/packs/house-style.colab"

[[packs]]
git = "https://github.com/acme/colab-packs"
tag = "v3"                       # or rev = "<commit>"
path = "go/errors.colab"
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
  `COLAB_JOBS` sets it, for `refactor`, `fix` and `find` alike.
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
  each command its own. This is on by default only for a binary named
  `colab`; a renamed copy (`colab-old`) needs `COLAB_KEEP_SERVER=1`.
- **Waiting for a server.** colab asks only once the server is done
  indexing. rust-analyzer says so (`experimental/serverStatus`), so after
  it reports `quiescent` colab waits 50 ms; servers that report only
  `$/progress` get 500 ms of silence.
- **Semantic constraints** (`where $X type "…"`) cost one round trip to
  the server per match their other constraints keep: on colab's own
  workspace rust-analyzer took about 10 s to index, then classified 17
  matches in under a second.
- **One walk, one parse per file.** Workers take the next file as soon
  as they are free (at most 4,096 ahead of the in-order output). A tree
  searched by a second rule is indexed by node kind, so later rules
  visit only the nodes their pattern can start at; a rewrite's output is
  re-parsed incrementally from the old tree, so checking that it still
  parses, and the next rule, cost little.
- **Measured** on colab's own repository (238 Rust files, 65k lines,
  Apple M2 Max, 12 cores; median of 5–10 runs, colab 2026.11.15.8c4a7d1,
  `dist` build):

  | Command | Time |
  | ------- | ---: |
  | `rg -c '\.unwrap\(\)' crates` (for scale) | 15 ms |
  | `find` `rust::pattern '$X.unwrap()'`: 1,129 hits in 82 files | 77 ms |
  | `find` a `fn $F(…) -> Result<$T>` pattern `where $F matches` | 55 ms |
  | `refactor` the `.unwrap()` rewrite, every diff (dry run) | 75 ms |
  | `refactor` a `rust::symbol` rename (dry run) | 18 ms |
  | `refactor` a dead `rust::symbol` rule, near misses included | 72 ms |
  | `refactor` `text::literal` over all 870 files, 297 changed, links checked | 237 ms |
  | `links docs` | 88 ms |
  | `references` / `calls` through a kept rust-analyzer | 115 ms |
  | `references` with a fresh rust-analyzer (`COLAB_KEEP_SERVER=0`) | 4.5 s |

### Against ast-grep

colab's patterns are ast-grep's engine (`ast-grep-core`) under colab's
own walker, prefilter and checks, so the two were compared on the same
patterns: colab 2026.11.15.8c4a7d1 (`dist` build) and ast-grep 0.51.3
(Homebrew), on colab's `crates/` (238 files) and on 10,025 `.rs` files
(458 crates, 4.1M lines) copied from a Cargo registry; Apple M2 Max,
12 cores; median of 10 runs (5 on the larger tree); every match
written as JSON (`colab find --table ndjson`, `ast-grep --json=stream`).
The column in brackets is colab 2026.11.15.70b21b0, before the walker,
parse and output changes of 8c4a7d1.

| Task | `crates/`: colab | ast-grep | 10k files: colab | ast-grep |
| ---- | ---: | ---: | ---: | ---: |
| Start-up (`--version`) | 10 ms | 9 ms | — | — |
| Search `$X.unwrap()` (24,278 matches on 10k) | 59 ms | 65 ms | 1.40 s (2.30 s) | 2.04 s |
| Search `fn $F($$$P) -> Result<$T> { $$$B }` | 54 ms | 65 ms | 1.15 s (1.42 s) | 2.06 s |
| Same, `$F` matching `^new` (colab `where`, ast-grep `constraints`) | 50 ms | 69 ms | 1.04 s (1.40 s) | 2.83 s |
| Search a pattern found nowhere | 17 ms | 56 ms | 0.24 s (0.25 s) | 2.02 s |
| Five rules in one run (56,620 matches) | 70 ms | 77 ms | 2.43 s (3.91 s) | 2.48 s |
| Rewrite `.unwrap()` → `.expect("bench")`, print the diff | 75 ms | 68 ms | 1.47 s (3.55 s) | 2.16 s |
| Same, written to disk | — | — | 1.94 s (3.63 s) | 2.40 s |

Peak memory on the 10k-file tree: colab uses less when its prefilter
skips files (216 MB against 430 MB for the `Result` search) and more
otherwise (495 MB against 439 MB for `$X.unwrap()`, 553 MB against
442 MB printing diffs, 647 MB against 419 MB writing).

- **Where colab is faster:** a rule whose pattern holds a literal that
  most files lack. colab parses only the files containing it (its
  prefilter), so a pattern found nowhere costs a read, not a parse: 8×
  on the large tree. It is level or ahead elsewhere on that tree, with
  fewer CPU seconds: ast-grep keeps more cores busy, colab does less work.
- **Where it is level:** several rules in one run. ast-grep's `scan`
  meets every rule in one walk of each tree; colab runs rules one after
  another, each on the previous one's output, so later rules use the
  kind index instead of a walk of their own.
- **What colab does that ast-grep does not:** before writing it checks
  conflicts and re-reads files changed on disk, checks that each rewrite
  still parses, and after the run explains the rules that left matches
  alone. Those costs are in the times above.
- **The results differ in three places:**
  - `if let Some($V) = $E { $$$B }`: colab 4,066 matches, ast-grep
    6,171. ast-grep's default strictness ignores trailing children, so it
    also matches 2,103 `if let … else` (a rewrite would drop the `else`);
    colab's matches are a subset of ast-grep's.
  - Nested matches (`a.unwrap().unwrap()`): ast-grep rewrites the outer
    one only, leaving 1,417 `.unwrap()` after one pass and 119 after a
    second; colab leaves none after one.
  - A match split across lines keeps its line break in colab
    (`.node_by_id(…)` then `.expect(…)` on the next line); ast-grep
    writes the template's text, joining them.
- One thread each (`colab find --jobs 1`, `ast-grep -j 1`), the
  `$X.unwrap()` search on the 10k-file tree: colab 8.06 s against
  ast-grep 11.4 s.

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
  install --git …`. After `colab agent install --user`, rerun it to
  refresh the skill it copied. A kept language server from the old build is not
  reused (its socket name includes the build); `colab lsp-daemon --stop`
  clears old ones at once.
- **Uninstall**: `colab agent uninstall` in each project (and once for
  Codex's `~/.codex/config.toml`), and `colab agent uninstall --user` if
  you installed with `--user`; remove the plugin from Claude Code's
  `/plugin` menu; `colab lsp-daemon --stop`; delete the cache directory above; then
  `brew uninstall colab` or remove the binary.

## Licenses

colab is MIT licensed. The binary links about two hundred crates, all
under permissive licenses (MIT, Apache-2.0, Unicode-3.0, Unlicense,
Zlib) except `colored`, MPL-2.0, which colab uses unmodified; its source,
like every crate's, is at `https://crates.io/crates/<name>/<version>`.
`colab licenses` prints colab's license and theirs, with the texts the
crates ship ([`THIRD-PARTY-LICENSES.txt`](../THIRD-PARTY-LICENSES.txt)),
so a binary copied out of a release archive still carries its notices.

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
| `OTEL_EXPORTER_OTLP_ENDPOINT`, `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT`, … | Send traces of colab's work to an OpenTelemetry collector; see [Traces](#traces-opentelemetry). |
| `XDG_CONFIG_HOME` | Where the user config file is read from (`$XDG_CONFIG_HOME/colab/config.toml`, else `~/.config/colab/config.toml`); today it holds only `[telemetry]`. |

## Traces (OpenTelemetry)

colab can report what it does as OpenTelemetry traces, to any collector
that takes OTLP over HTTP: a local usage service such as
[habit](https://github.com/grahambrooks/habit), an OpenTelemetry
Collector, Jaeger. It is **off** unless an endpoint is set, either by the
standard variables or by the `[telemetry]` table of your user config
file (where `enabled = true` alone means the local `http://127.0.0.1:4318`); colab has no collector of its own and no SDK dependency.

**What a trace holds.** One trace per command — the root span
`colab <subcommand>`, with `process.exit.code`, `error.type` when it
failed, the *names* of the flags given (`format`, `write`, never their
values) and the run's counts (`colab.files.visited` / `scanned` /
`changed`, `colab.rules`, `colab.rules.unmatched`, `colab.conflicts`) —
and inside it `compile`, `walk`, `prepare writes`, `write`, and a client
span per language-server request (`lsp textDocument/hover`, `lsp start`,
with `rpc.method` and the server's program name). `colab mcp` reports a
trace per tool call (`colab mcp colab.preview`), `colab server` one per
walk. Paths, file contents, script text, rule text and argument values
never leave the process. The resource carries `service.name` (`colab`),
`service.version`, and `vcs.repository.name` — the repository's
directory *name*, not its path.

| Variable | Effect |
| -------- | ------ |
| `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` | The traces URL, used as given (`http://127.0.0.1:4318/v1/traces`). |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | The collector's base URL; `/v1/traces` is appended. A gRPC endpoint (port 4317, or `OTEL_EXPORTER_OTLP_PROTOCOL=grpc`) is reached on the same host's OTLP/HTTP port, 4318. |
| `OTEL_SERVICE_NAME` | The service name (default `colab`). |
| `OTEL_RESOURCE_ATTRIBUTES` | More resource attributes, `k=v,k2=v2` (percent-encoded values). |
| `OTEL_EXPORTER_OTLP_HEADERS`, `…_TRACES_HEADERS` | Request headers, `k=v,…` (an ingest token). |
| `OTEL_EXPORTER_OTLP_TIMEOUT`, `…_TRACES_TIMEOUT` | Milliseconds one export may take (default 500). |
| `OTEL_SDK_DISABLED=true`, `OTEL_TRACES_EXPORTER=none` | Off, whatever else is set. |

**The config file.** The same settings can live in
`$XDG_CONFIG_HOME/colab/config.toml` (else `~/.config/colab/config.toml`).
That matters for agents: the processes Claude Code starts — its Bash
tool's shells and its MCP servers — do not inherit the `OTEL_*` variables
in its `settings.json`, so colab run there sees no endpoint unless the
file names one.

```toml
[telemetry]
endpoint = "http://127.0.0.1:4318"   # OTLP/HTTP base URL; /v1/traces is appended
# enabled = true                     # on; alone, sends to http://127.0.0.1:4318
# enabled = false                    # off, even when OTEL_* sets an endpoint
# service_name = "colab"
# timeout_ms = 500
```

| Key | Effect |
| --- | ------ |
| `endpoint` | The collector's base URL, as `OTEL_EXPORTER_OTLP_ENDPOINT` (a URL ending in `/v1/traces` is used as given; port 4317 is reached on 4318). |
| `enabled` | `false`: off, whatever else is set. `true`: on, sending to the endpoint set here or in the environment, else `http://127.0.0.1:4318`. Absent: on only when an endpoint is set. |
| `service_name` | The service name, below `OTEL_SERVICE_NAME` and a `service.name` in `OTEL_RESOURCE_ATTRIBUTES`. |
| `timeout_ms` | Milliseconds one export may take, below `OTEL_EXPORTER_OTLP_(TRACES_)TIMEOUT`. |

The environment wins key by key; `OTEL_SDK_DISABLED=true` and
`OTEL_TRACES_EXPORTER=none` still turn export off. A missing file is no
configuration; an unreadable or malformed one is ignored
(`RUST_LOG=debug` says why). It is a user's setting: a repository's
`colab.toml` is never read for telemetry, so a checkout cannot redirect
your traces.

Traces go as OTLP/JSON over plain `http://`; colab ships no TLS stack, so
an `https://` endpoint turns export off (use a local collector to
forward). An export that cannot be made — nothing listening, a timeout,
an error answer — is dropped (`RUST_LOG=debug` says so) and never
changes a command's output or exit code. A one-shot command exports as
it ends, within the timeout; `colab mcp`, `colab server` and
`colab lsp-daemon` export from a background thread, so no response waits
on the collector.

**With habit.** `habit setup claude-code` puts
`OTEL_EXPORTER_OTLP_ENDPOINT=http://127.0.0.1:4317` into Claude Code's
settings, but that reaches Claude Code itself, not the shells and MCP
servers it starts. To have every colab run report to habit — yours and an
agent's — set `enabled = true` (habit's address is the default) or
`endpoint = "http://127.0.0.1:4318"` in the config file above; for one terminal, `export OTEL_EXPORTER_OTLP_ENDPOINT=http://127.0.0.1:4318`.
colab labels each trace in habit's terms, which other collectors ignore:
a command's resource carries `habit.kind=command` and its root span
`habit.subcommand` and `habit.flags`, so habit stores a `command` record
(tool, subcommand, version, exit code, success, duration, flag names,
repo); an MCP tool call's carries `habit.kind=mcp`; a `colab server` walk
is a plain `span`. A `habit.kind` set in `OTEL_RESOURCE_ATTRIBUTES` wins.
habit counts the child spans but does not store them; `habit status`
shows the records arriving.

## When something looks wrong

| Symptom | Look at |
| ------- | ------- |
| A rule reports `matched no files` | `--format json`: its `near` names the closest values the module saw, its `unchanged` why matches were left; `colab ast` shows how a pattern parses |
| A pattern matches too much or too little | `colab find -e '<rule>' --detail detailed` (the bindings), then narrow with `where` |
| Exit 11 | a rule reported a conflict (an identifier that already exists, an unimported qualifier); `--format json` lists them |
| "still indexing at the timeout" | raise `--timeout` / `--lsp-timeout`; the first run on a large workspace pays the server's indexing |

More: the skill's troubleshooting guide,
[`plugins/claude/colab/skills/colab-codemod/references/troubleshooting.md`](../plugins/claude/colab/skills/colab-codemod/references/troubleshooting.md).
