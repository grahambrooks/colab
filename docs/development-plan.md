# Colab Development Plan

What is planned next. What has shipped, with the notes written as each
milestone landed, is in [`completed-milestones.md`](completed-milestones.md).

## Vision

Colab becomes the tool you point at a repo — or a monorepo of
millions of lines — when you want to:

1. Update dependencies *across breaking changes* (renamed packages, moved
   APIs, deprecations) — not just bump versions.
2. Modernise code to the current language version's idioms (Go 1.18 generics,
   Java 21 records/switch, Python 3.12, ES modules, Rust edition migrations).
3. Apply the above repeatably from CI, from a developer shell, or from an AI
   agent — with the same deterministic output everyone can audit.
4. **Run those refactors at scale** — across thousands of files in
   parallel, with selective targeting, post-apply verification,
   per-rule auditability, and review tooling that turns a 5k-file
   diff into something a human can confidently sign off on.

## Where things stand (2026-11-14)

M1–M19, M21–M35 and M20's non-installing parts have shipped
([details](completed-milestones.md)). colab has 12 source backends and
3 config backends, structural patterns with `where` constraints, delegated
rename, move, code actions, references and safe delete through the
language's own server, git-aware file operations, pre-apply conflict
checks, and a CLI, MCP server and LSP that share one report shape.

## Order of work

**The 2026.11.4 release** that M20's plugin pins, once the stable gate holds again (one more milestone with no silent failure), then the [decisions waiting](#decisions-waiting-from-m29) from M29 when they are made. M28 (Markdown by structure), the dogfood follow-ups, M23 (the eval suite), M14 (`colab.toml` + `colab fix`), M21 (CI formats), M16 (`colab review`, `--split-pr`), M15 (`where inside` on `symbol` / `call`), M29 (the follow-up sweep), M30 (rule expressiveness), M31 (CI fit), M32 (type and name constraints), M33 (fleet runs, packs from git), M34 (symbol-addressed edits), M35 (scan phases) and M10.6 (a parse cache) have shipped.

M20 puts colab in front of every Claude session, so a version is pinned
there only once it has passed the [stable gate](#stable-gate-before-m20)
by dogfooding. Until then, colab is developed with colab
([`dogfooding.md`](dogfooding.md)), and each milestone is chosen for
what it lets that dogfooding cover.

| Rank | Milestone | Why now |
| ---- | --------- | ------- |
| 1 | Release 2026.11.4 | M20's install step has shipped (installer, uninstaller, post-edit hook, marketplace, MCP resources); the plugin pins 2026.11.4, which waits for stable-gate criterion 2 (1 of 2 clean milestones). |

## Next improvements (2026-11-24 architecture review)

The review that produced `colab_dsl::pipeline` (one run sequence for the
CLI and MCP) and `colab_rewrite::language` (one common floor for the 12
source backends) left these, in order. The first four are structural and
cheapest now, while the pipeline is fresh. The rest are the open dogfood
items, with their designs in [Follow-ups](#follow-ups-from-shipped-milestones).

| Rank | Improvement | Why, and where |
| ---- | ----------- | -------------- |
| 1 | ~~**Encapsulate `RunReport`.**~~ Done 2026-11-24: fields `pub(crate)`, accessors of the same names, writes through `warn` / `add_conflict(s)` / `add_rule` / `rules_mut` / `set_counts` …; the per-rule run now counts the first walk once (it used to append `skipped` once per rule). | — |
| 2 | ~~**One driver for the one-rule-at-a-time runs.**~~ Done 2026-11-24: `pipeline::RuleRun` runs one rule over a `Source` (or one text) and credits its changes to the rule's script index; `--verify`, `--commit-per-rule`, `--split-pr` and the bisect search use it, and each builds its `Source` once instead of asking `git` per rule. Still open: scans under these runs (scan once, then each rule). | — |
| 3 | ~~**Scan phases on the remaining paths.**~~ Done 2026-11-24 for `--stdin`, MCP `colab.test_rule` and MCP `colab.find`. The per-rule runs and `--changed-since` / `--staged` remain; see the M35 follow-up row. | — |
| 4 | ~~**Split the oversized, high-churn modules.**~~ Done 2026-11-24, except `lsp_commands.rs`: each command's clap `Args` is in `cli/<cmd>.rs` (`cli.rs` 1,185 → 690 lines); `colab-mcp/src/lib.rs` is transport and dispatch, with `scripts.rs` / `args.rs` / `progress.rs` / `schema.rs`; `colab server` plans edits through the pipeline. Still open: split `lsp_commands.rs` (1,700 lines) into its CLI commands and the MCP `LspTools`. | — |
| 5 | ~~**Parse errors name the file.**~~ Done 2026-11-24: `ParseDetail::path`, set for a script file and for each `include` (the innermost wins), shown as `parse error in <path> at line …` and as MCP `error.data.path`. The editor already puts an include's error on the `include`. | — |
| 6 | ~~**One base for every path glob.**~~ Done 2026-11-24: `--include` / `--exclude` already took both forms for a relative target (`rebase`); an absolute target, and a file named directly, are now read relative to the run's base (or the working directory) first, as `in` scopes are. The target-relative form stays accepted without a warning: both forms name the same files. | — |
| 7 | ~~**Patterns inside macros** (Rust token trees).~~ | Done 2026-11-24 for find / replace / delete, nested macros included, from an allow-list of expression macros (and a project's own, `[patterns] macros`); rules with relational or semantic constraints read them too, judged where the macro is in the file. See [Patterns inside macros](#patterns-inside-macros). |
| 8 | ~~**Captures rendered verbatim** in `keep_layout`, plus "would not parse" naming the first match that breaks.~~ | Done 2026-11-24: each capture is laid out as one sentinel token and put back verbatim, re-indented with its line (reproduced with `swap($A, $B)` → `swapped($B, $A)`); text appended after a one-per-line sequence starts its own line; `why_unchanged` names the first match whose rewrite alone breaks the parse. |
| 9 | ~~**More pattern contexts:** C# class members, switch / when cases (Go, JS, Kotlin), and a bare `$$$` in item lists.~~ | Done 2026-11-24: a bare-compiled pattern tries every context; Go / JS `switch` and Kotlin `when` contexts; a bare `$$$` is a placeholder; a node that owns its trailing newline (Go's `expression_case`) matches and is replaced without it. Corpus `go/pattern_switch_case`. |

### Next round

What the round above left, and what it found, in order.

| Rank | Improvement | Why, and where |
| ---- | ----------- | -------------- |
| 1 | ~~**Split `lsp_commands.rs`**~~ | Done 2026-11-24 with rust-analyzer's own "Extract Module" / "Extract module to file" through `colab action`: `lsp_commands/{mcp,moves,text,tidy,verify}.rs`, 652 lines left for the CLI commands. |
| 2 | ~~**Scan phases under the one-rule-at-a-time runs**, and under `--changed-since` / `--staged`.~~ | Done 2026-11-24: `pipeline::prepare_scans` (the whole script phased once, facts left installed) before each per-rule run; `Pipeline::scan_over` for git-limited runs. No run path refuses a scan script now. |
| 3 | ~~**Macro contents for constrained rules.**~~ | Done 2026-11-24: a match in a macro's contents is judged in a `Frame` (`pattern/macros.rs`): `inside` reads the contents' own nodes, then the macro call and its ancestors in the file (never the wrapper); `follows` / `precedes` / `nth` read the contents' own neighbours; a semantic check asks about the file at the mapped position. Frames nest, and a stage that edits the contents derives the frame for the text it left (`Frame::after`), so later stages and nested macros are judged and asked about where they now are. Nothing is skipped any more. |
| 4 | ~~**User-listed expression macros**~~ | Done 2026-11-24: `colab.toml` `[patterns] macros = ["my_assert"]`, per run (`Run::set_macros`); read by `refactor` / `find` (from the nearest `colab.toml`; a broken one is only a warning there), `fix` (its config) and MCP (`ToolProvider::macros(cwd)`, the `colab.toml` at or above the call's `cwd`) and `colab server` (the one at or above the script). |
| 5 | ~~**The kept server's stderr.**~~ | Done 2026-11-24: `Transport` drains a server's stderr and keeps its last lines (a server that logs a lot can no longer block on a full pipe either); a session that started the server, and the daemon for a kept one (as the error of each pending request), report an exit with what it last said. The `--version` probe stays as the fallback for a server that said nothing. |
| 6 | ~~**Verbatim layout for templates with `let`.**~~ | Done 2026-11-24: a `let`'s value gets a sentinel of its own, placed by the template, and its later lines move with its line as its base capture's would (through earlier `let`s to the capture); only real captures are marked in the old text. Reproduced with `pair($A, $B)` → `pair($B, $X)`, `let $X = $A \| upper`, which reflowed a one-per-line call. |
| 7 | ~~**`language_server.rs` (1,807 lines) and `pattern.rs` (2,157).**~~ | Done 2026-11-24: `language_server/{completion,hover,workspace}.rs` through rust-analyzer's assists (1,217 lines left, about 560 of them tests); `pattern.rs` a 281-line root over `compile.rs`, `find.rs`, `macros.rs`, `delete.rs`, `rewrite.rs` and `tests.rs`. |
| 8 | ~~**Semantic dogfooding needs `rust-analyzer` for the pinned toolchain.**~~ | Done 2026-11-24: installed for 1.100; `docs/dogfooding.md` says how, and when to prefer `where $R type …` to a name regex. Used for real the same day: `lsp_commands.rs` and `language_server.rs` were split through rust-analyzer's assists. |

### After the next round

Found while doing the round above, in order.

| Rank | Improvement | Why, and where |
| ---- | ----------- | -------------- |
| 1 | ~~**A kept server sees colab's own writes before the next command.**~~ | Investigated 2026-11-24 in a scratch crate: it does. A warm kept rust-analyzer renamed a reference a previous colab command had just written, as a fresh one did. The missed callers come from rust-analyzer's "Extract Module", which does not rewrite callers in other files, cold or warm, fresh or kept (item 4 guards against that). Found and fixed on the way: `colab lsp-daemon --stop` returned before the daemon let go of its socket, so the next command reached it on its way out and failed "exited while initialize" (4 runs in 5). `stop` now waits for the socket to go, and a session whose daemon closes before `initialize` without a word starts its own server (0 in 10). Then the `compiler.rs` split hit the real staleness, on files another tool had edited: the daemon sent `didChangeWatchedFiles` and then replayed its old "quiescent" status, so the session acted while rust-analyzer was still reloading ("Mismatching assist"). Fixed: no stale status for a client that starts with changed files, and `colab action` retries once on that error. |
| 2 | ~~**"Top level only" in a pattern rule.**~~ | Done 2026-11-24: `where top` / `where not top` (`Test::Top`): the match, judged as the widest node spanning its text, is a direct child of the file's root; never in a macro's contents. Generic for every pattern backend; corpus `rust/pattern_top`. |
| 3 | ~~**`replace` of an item takes its attached attributes and docs**~~ | Done 2026-11-24, for a template that is only metavariables (it unwraps the match: `mod tests { $$$B }` → `$$$B` drops the `#[cfg(test)]`); any other template keeps them, so a rename-style rewrite of an attributed item keeps its attributes. `pattern/rewrite.rs::with_attachments`. |
| 4 | ~~**Check rust-analyzer's assists after `colab action`.**~~ | Done 2026-11-24: `--verify-lsp` on `action`, `rename` and `mv` (the shared apply flags): each error the change adds to the files it changes is a conflict that refuses the write. It does not see files the change left alone, so a caller elsewhere that an "Extract Module" broke still needs `--verify 'cargo check'`; a server-wide check would ask for every file's diagnostics (`workspace/diagnostic`), which is next if that case recurs. |
| 5 | ~~**Split `crates/colab-cli/tests/cli.rs`**~~ | Done 2026-11-24: five binaries, `cli` (refactor, walking, verify / bisect, inline rules: 49 tests), `find` (7), `lsp` (21), `workflows` (31), `agent` (5), over `tests/common/mod.rs`; 113 tests before and after. Moved with rust-analyzer's assists, whose "Extract module to file" put each module beside `cli.rs`, which is where Cargo wants a test binary. |
| 6 | ~~**`colab-dsl/src/compiler.rs`** (1,928 lines) **and `colab-mcp/src/lib.rs`'s tests** (most of its 1,915 lines).~~ | Done 2026-11-24 through rust-analyzer's assists: `compiler.rs` a 290-line root (compile, check) over `compiler/{syntax,includes,names,lower,tests}.rs`; MCP's tests in `src/tests.rs` (they drive private tool functions, so `tests/` cannot hold them), `lib.rs` 674 lines. |
| 7 | **Release.** The 2026.11.4 release the M20 plugin pins is still waiting on the stable gate. | Everything since is on `main`, unreleased; see [Stable gate](#stable-gate-before-m20). |

## Review 2026-11-25: safety, robustness and dependencies

A critical pass over the application rather than its features: a
security review of every surface that writes or runs something, probes
with hostile inputs (CRLF, BOM, Latin-1, 20,000-deep nesting, 200,000
matches in one file, syntax errors), and the dependencies' fit. Ranked
by severity; struck through when done.

| Rank | Finding | Where, and what to do |
| ---- | ------- | --------------------- |
| 1 | **A cloned repo's `colab.toml` runs commands through a git pack.** `[[packs]] git` / `rev` reach `git fetch` as positional arguments with no `--` and no validation, so `--upload-pack=<cmd>` runs `<cmd>` (reproduced). `hook post-edit` compiles packs, so with the hook installed an agent editing any file in a malicious clone triggers it, with no prompt; so do `fix --check` in CI on a PR and `fleet`. | `git_packs.rs:115`, `config.rs:200`. Refuse a url or rev that starts with `-`, put `--` before the url, allow only https / ssh / file urls (`-c protocol.allow=user`), revs as hex or tag characters; `hook post-edit` uses cached packs only. With `colab.toml` trusted (rank 2's decision) this is no longer a privilege boundary, since the same file may name any formatter; it stays worth fixing as input validation, cheaply, so a typo in `rev` is an error rather than a git option, and so the hook does no network fetch behind an edit. |
| 2 | **A repo's `colab.toml` commands run in modes that look read-only.** `[formatters]` and `[run] check` run through `sh -c`, including under `fix --check` when the file sets `tidy = true`, and in every repo of a `fleet` with `run = fix`. | `tidy.rs:93`, `refactor.rs:366,381,401`, `fleet.rs:413`. **Decided 2026-11-25: a repository's `colab.toml` is trusted**, as its `Makefile`, `build.rs` or CI config is: running colab in a checkout means trusting that checkout. No trust list or flag. What stays worth doing is making it visible: the docs say plainly that `colab.toml` commands run, including under `--check`, and `fleet` over repositories the operator does not own should say so in its manifest docs. |
| 3 | ~~**Text lost beside a syntax error.**~~ | Done 2026-11-25 (`982e9f5`): ast-grep passes over an `ERROR` node like any unnamed child, so `old($A)` matched `old(x :: y)` and the rewrite dropped `:: y`, and a `js::` rule over a `.ts` file turned `old(x as any)` into `neu(any)`. A match is now kept only when every error inside it lies within a capture (`find.rs::errors_captured`). |
| 4 | **`create` paths can leave the workspace.** `create "../x"` wrote above the run's root; the check reads the template before captures fill it. | `pattern/rule.rs:262,291`. Check the filled path; refuse one that does not stay under the run base. |
| 5 | **Nothing confines where MCP tools and server edits write.** Any `cwd` and paths are accepted, and a language server's `WorkspaceEdit` may name any `file://` uri. MCP tools are often auto-approved where Bash is not. | `colab-mcp/src/args.rs:23`, `colab-lsp-client/src/edit.rs`. Refuse writes outside the MCP roots (or the server's start directory), and server edits outside the project root unless `--allow-outside`. |
| 6 | **The daemon's socket directory is predictable and unprotected on Linux.** `temp_dir()/colab-lsp-$USER`, default mode, no owner or peer check; another user can pre-create it and stand in for the language server, which then feeds rank 5. macOS's per-user `TMPDIR` mitigates it. | `colab-lsp-client/src/daemon.rs:142,174,329`. `$XDG_RUNTIME_DIR` where set, else a 0700 directory checked to be the user's and not a symlink, named by uid; check `peer_cred` on accept. |
| 7 | ~~**Quadratic rewriting.**~~ | Done 2026-11-25 (`982e9f5`): `Rewriter::outermost` compared every match with every other, once per binding; 48,000 matches took 24.5 s, now 1.95 s (200,000: 8.6 s, mostly the linear self-call check, which parses each function body again). |
| 8 | **Unbounded recursion.** ~~A walk over several files aborted at ~4,000 levels of nesting (rayon's 2 MiB worker stack).~~ Workers now have 64 MiB (`982e9f5`) and 8,000 levels run, but one file nested ~20,000 deep still overflows the main thread and aborts the process (an MCP server with it). | Bound the depth of the engine's own walks, or run every walk on the pool. |
| 9 | **`js` claims `.ts` / `.tsx`.** Since `ts` and `tsx` exist, a `js::` rule reads TypeScript with the JavaScript grammar. Rank 3 stops it corrupting, but it still misses or misreads. | `colab-lang-js/src/lib.rs:28`. Drop the extensions, or point `js::` rules at the TypeScript grammars for them. Needs a decision. |
| 10 | **The pre-bash hook is a nudge, not enforcement.** `bash -c`, `eval`, `sh <<<`, `cp`, `dd of=`, `git apply`, `ruby -i`, `awk -i inplace` all pass. | Say so in the docs; do not present it as a boundary. |
| 11 | **Latin-1 and other non-UTF-8 files are skipped** (reported, never corrupted), so colab cannot edit them. | Low; decode-and-reencode only if a user needs it. |

Sound: telemetry exports counts and flag names only (an unknown MCP tool
name reaches a span name verbatim); the walker follows no symlinks; every
spawn but `cli::shell` is an argument list; CRLF and BOM files round-trip.

### Dependencies: what each one buys

The tree is current and clean (`cargo outdated`, `cargo audit`,
`cargo deny` on 2026-11-25), so the question is fit, not hygiene. 171
crates build `colab` (142 at run time); the binary is 44 MB.

**ast-grep-core** (0.51, pre-1.0; 6,800 lines). colab uses four things
from it: turning a parsed pattern into a pattern tree with its
metavariables (`Pattern`, `PatternBuilder`, the `Language` trait's
`extract_meta_var`); the matcher that walks a node against that tree with
`$X` / `$$$X` backtracking (`match_tree`, ~1,000 lines); the binding
environment that makes a repeated `$X` agree (`meta_var`, ~400); and
template filling with re-indentation (`replacer`, ~850). Its traversal,
rule combinators, `Doc` abstraction and most of its node API go unused.
Against that, colab's own 5,700-line pattern module spends much of its
size correcting ast-grep's matching for rewriting: `Shape` (every named
child accounted for, since ast-grep ignores trailing ones), `led_forms`
and the modifier sentinels (it skips anonymous tokens before a literal,
not before a metavariable), surgery on `PatternNode` internals,
`first_of_kind` (its contextual pattern takes the first node of the
kind), `errors_captured` (it skips `ERROR`), and layout keeping on top of
its templates. Each correction is another compiled form per pattern, and
the internals colab edits are not semver-stable. **Assessment: replace
it** with a matcher of colab's own over tree-sitter (~1,500–2,000 lines),
whose rules are the ones colab now bolts on: every child accounted for,
anonymous tokens treated alike, an error node never trivial. Do it behind
the existing `StructuralPattern` API, with the conformance matrix, the
property suite and the corpus as the parity bar, then delete the
corrections it makes redundant.

**tree-sitter and its grammars.** Kept. The alternatives are worse: a
native parser per language (`syn`, `swc`, …) means fourteen ASTs and no
shared engine, most without error recovery; a language server offers no
syntax tree to match on (document symbols and selection ranges are not
patterns), is slow to start and missing for many languages; delimiter
matching in the style of comby needs no grammar and would reach any
language, but loses node kinds, `where … is`, declarations and the
structure the Markdown backend depends on. tree-sitter is error-tolerant,
incremental, uniform across fourteen languages and the industry default.
Its costs are real and are what to manage: grammar quality varies (the
C++ brace-initializer, Kotlin annotation and C# anonymous-object misparses
are all grammar bugs colab works around); six grammars have had no
crates.io release since late 2024 or January 2025 (C++, Ruby, TypeScript,
JSON, Java, Kotlin) though their repositories are active; and the
grammars are most of the binary (`parser.c` sources: C# 37 MB, Swift 23,
Kotlin 22, C++ 17, TypeScript 17 twice). Options: a `colab-grammars`
crate that vendors chosen upstream commits and builds them with `cc`
(fresh grammars, still publishable, one place to patch); per-language
Cargo features for smaller builds; fixes sent upstream for the misparses.
YAML, Dockerfile and shell backends, the largest capability gap for
fleet maintenance, would come from tree-sitter grammars too.

**The LSP client.** Already minimal: `colab-lsp-client` speaks JSON-RPC
over Content-Length framing on `serde_json` alone, with no `lsp-types` or
async runtime. Its risk is behavioural (a server release that answers
differently), which the weekly `real_lsp` job now covers for
rust-analyzer; gopls and clangd would be the next servers to add there.

**The LSP server (`colab server`, for `.colab` files).** `tower-lsp-server`
brings 45 crates nothing else needs (tokio, tower, futures, dashmap,
tracing, `ls-types`, …) and is the only async code in colab. The MCP
server is already a synchronous JSON-RPC loop, and the client's transport
already frames Content-Length messages, so the server can run on that
transport synchronously and drop all 45. With rules now writable as JSON,
whether `.colab` editor support is worth keeping at all is a fair
question. **Decided 2026-11-25: dropped.** Refactoring is done by agents,
which write rules through MCP (or as scripts they run), not by people
editing `.colab` files in an editor, so a language server for them earns
nothing. `colab server`, its module and its test are gone, and with them
`tower-lsp-server`, `tokio`, and the `futures` / `tower` dev-dependencies:
142 run-time crates became 101. The per-rule checks the editor showed
are still `colab_dsl::check`, which the corpus harness uses; agents get
the same compile errors and warnings from `colab.lint_script`.

### Direction: pure-Rust syntax trees (decided to explore, 2026-11-25)

tree-sitter's grammars are C, compiled by `cc` in every build; the
preference is a pure-Rust colab, at the cost of some languages if need
be. A spike on 2026-11-25 built each candidate parser and fed it a
realistic snippet, the same snippet with a syntax error in its middle
function, and a 5,000-line file:

| Language | Crate | Lossless | Recovers from an error | Pure Rust | Risk | Verdict |
| -------- | ----- | -------- | ---------------------- | --------- | ---- | ------- |
| Rust | `ra_ap_syntax` 0.0.364 (rust-analyzer, rowan) | yes, comments are tokens | yes, one `ERROR` node, the rest intact | yes | weekly 0.0.x releases that break freely | strong |
| JS / TS / TSX | `biome_js_parser` 0.7.3 (rowan-style) | yes, comments are trivia | yes, `BOGUS` nodes | yes | sporadic crates.io releases; types moved between crates | strong |
| JSON | `biome_json_parser` 0.7.3 | yes | yes | yes | as above | strong |
| YAML | `biome_yaml_parser` 0.7.3 | yes | partly | yes | new on crates.io | usable |
| Python | `ruff_python_parser` 0.0.23 | AST with ranges; tokens cover the rest | yes, except an unclosed bracket swallows the file | yes | "internal component crate of Ruff", 0.0.x | usable |
| PHP | `mago-syntax` 1.61 | nodes and trivia have spans; brackets are span-only fields | yes | no: `cc` through `blake3` (its `pure` feature may avoid it; untested) | one owner | usable |
| Markdown | `pulldown-cmark` 0.13 | events with byte ranges, no token tree | no errors to recover from | yes | very active | usable as an index |
| C / C++ | `cpp_parser` 0.4 (rowan) | yes | yes | yes | a month old, one author, wants preprocessed input | risky |
| JS / TS | `oxc_parser` 0.160 | AST with spans | **no**: any error leaves an empty program | yes | — | weak |
| Go | `gosyn` 0.2.16 | no (start offsets only) | **no** | yes | one owner | weak |
| Ruby | `lib-ruby-parser` 4.0.6 | AST with locations | sometimes | yes | last release 2024-04, Ruby 3.1 | weak |
| C | `lang-c` 0.15 | no, rejects comments | **no** | yes | last release 2023-09 | none |
| YAML | `saphyr-parser` 0.4 | events; markers count characters | **no** | yes | — | weak |
| Java, C#, Kotlin, Swift | — | — | — | — | nothing credible; ANTLR's pure-Rust runtimes (`antlr4rust`) with the grammars-v4 grammars are the only route, unproven for recovery or losslessness | none |

So pure Rust covers **Rust, JavaScript, TypeScript, TSX, Python, PHP,
JSON, YAML, TOML (`toml_edit`, already) and Markdown** well, and **Go,
Java, Kotlin, C#, Swift, C, C++ and Ruby** not at all today. Go, Java and
Kotlin are the losses that hurt most: the workspace colab maintains has
about seven Go repositories and six Maven / Gradle ones.

What the move takes, whichever languages stay:

1. **A tree of colab's own.** The engine reads tree-sitter's node API
   (kind, named children, fields, byte ranges) throughout. A generic
   lossless tree (`rowan`, itself pure Rust, or a small type of colab's)
   with a tree-sitter adapter first changes nothing and gives every later
   step a target. The rowan-based parsers (rust-analyzer, biome) map onto
   it almost directly; the AST-with-spans ones (ruff, mago) need the
   text between child spans made into anonymous tokens, and comments
   attached from their side list. None of them exposes tree-sitter's
   *named fields* generically, so each language needs a field table
   (written, or generated from its typed layer) for `replace_call`,
   `where … field`, declarations and the like.
2. **The matcher of colab's own** ([above](#dependencies-what-each-one-buys)),
   over that tree. ast-grep is tree-sitter's; this step was due anyway.
3. **One language at a time, behind the parity bar** (conformance
   matrix, property suite, corpus): Rust first (the best parser, and
   colab's own code to dogfood on), then JS / TS / TSX / JSON, Python,
   Markdown, PHP.
4. **The rest: drop, write, or gate.** Per language: drop it; write a
   lossless recovering parser of colab's own (Go is the candidate: a
   small, regular grammar, perhaps 4–6k lines in rust-analyzer's style;
   Java is larger, Kotlin much larger); or keep tree-sitter for it behind
   a Cargo feature, so the default build is pure Rust while the
   languages last.

The cost to weigh: four upstreams that break on most bumps (rust-analyzer
weekly at 0.0.x, Ruff "internal", Biome's moves) instead of one stable
runtime API, so each must be pinned exactly and bumped on purpose, with
the conformance matrix as the check. The spike's crates are in the
session scratchpad, `parser-spike/`, and are not kept.

### Building colab's own parsers (direction, 2026-11-25)

The aim is a set of parsers written for colab, each producing a
**lossless syntax tree** (every byte of the file in some token, comments
and layout included) with a typed AST generated over it, as
rust-analyzer does. A plain AST would drop what a rewrite has to keep.

**Why custom parsers improve outcomes, checked against the evidence.**
Error nodes and comments are *not* what custom parsers add: tree-sitter
already has both. The same Rust file with `a + ( let = ;` in its middle
function, parsed both ways on 2026-11-25:

| | tree-sitter (today) | rust-analyzer's hand-written parser |
| --- | --- | --- |
| Recovery | one `ERROR` node inside `broken_here`; `last` intact | one `ERROR` node (`;`) inside `broken_here`; `last` intact |
| Lossless | yes | yes (token texts rebuild the file) |
| Comments | nodes, but **floating siblings**: `/// Doc comment` sits beside `struct_item`, `/* block */` beside `function_item` | trivia **attached** to what they belong to: `/* block */` inside its `FN`, the file's first comment inside the `USE` |

What custom parsers do add:

1. **No wrong tree without an error.** The failures that cost colab most
   were never error nodes; they were grammars returning a confident wrong
   tree: C++ `T f() { x }` read as a declaration with a brace
   initializer, a Kotlin annotation read as `annotated_expression` on
   `("x")`, a C# anonymous object read as a flat list. A wrong tree
   cannot be caught downstream; an error node can (`errors_captured`
   leaves it alone). A colab parser's first rule is to emit an error
   node where it is unsure, never a guess.
2. **Comments that belong to something.** Attached trivia replaces the
   per-backend `Leading` tables and doc-comment heuristics that a
   delete or move needs so that an item's docs and attributes go with
   it.
3. **Fragments parsed directly.** An entry point per fragment kind
   (expression, statement, item, parameter, type, case, argument)
   replaces the wrapper contexts, the "wrapper has an earlier node of
   that kind" refusals, `terminate_lone_sequences` and the `also`
   readings.
4. **Fields and kinds designed for rewriting**, generated from one
   description per language, so `replace_call`, `where … field`,
   declarations and modifiers stop being per-backend configuration.
5. **Safe growth.** Because an error region outside every capture is
   never rewritten (`982e9f5`), a parser that does not yet handle a
   construct matches less there and corrupts nothing, so a parser can
   ship covering the common syntax and grow from the property suite's
   failures, as long as rule 1 holds.

**How to build them: hand-written, on a shared toolkit.** Parser
generators fit poorly: LR generators (`lrpar`, LALRPOP) cannot express
C++, Kotlin or Swift without heavy disambiguation and repair errors by
inserting or deleting tokens rather than making error regions; PEG and
combinator libraries (pest, `chumsky`, `winnow`) have weak recovery or
become hard to compile and debug at a full language's size; ANTLR
(`antlr4rust`, grammars-v4) has every grammar but they were written for
valid code, its parsing can be pathologically slow (C++), and its
rule-per-node trees need reshaping. rust-analyzer, Biome and Ruff all
chose hand-written recursive descent for the same reasons. The shared
part is most of the work, once:

- `colab-syntax`: the lossless tree (`rowan`), an event-based builder,
  recovery helpers (recovery sets, error nodes), a Pratt expression
  engine, lexer support (`logos` or hand-written), and a nesting limit
  (the stack-overflow finding, rank 8);
- per language, an `ungrammar` description generating the node kinds,
  the field table and the typed AST, a lexer, and the grammar module.

`ra_ap_syntax` (Rust) and Biome (JS / TS / TSX / JSON) are built this
way already, so they plug in almost directly instead of being rewritten.

**How to keep them correct.** For every parser: a lossless round trip
over a large real corpus (the workspace's own repositories); differential
tests against tree-sitter while it is still there (a file it parses
cleanly must parse cleanly) and against the language's own tool on
accept / reject (`gofmt -e`, `javac`, the compiler); fuzzing for no
panic, losslessness and the nesting limit; and the conformance matrix,
property suite and corpus unchanged as the bar. Each language release
adds a conformance case for its new syntax; Go and Java move slowly,
Kotlin, Swift and TypeScript fast. These are well-specified tasks with
strong oracles, which suits agent-written code.

**Rough size per language**, grammar code alone (the toolkit is separate):
Go 4–6k lines (small and regular; `gofmt -e` is a perfect oracle), C
4–6k (preprocessor directives as trivia), Java 6–8k, PHP ~6k, Python ~6k
(the indentation lexer; Ruff's parser serves meanwhile), Kotlin 8–10k
(newline-sensitive), C# and Swift 10k+, Ruby very hard (heredocs, an
ambiguous grammar), C++ not realistic (the preprocessor, ambiguity).

**Order.**

1. `colab-syntax` and the matcher of colab's own, with tree-sitter
   feeding the tree: needed whichever way the parsers go. **Started
   2026-11-25:** the crate holds the tree, its `Builder` and
   `Tree::from_tree_sitter`; over the repo's 436 source files in all 14
   languages it equals tree-sitter's tree node for node (kind, field,
   named flag, range) and rebuilds each file byte for byte
   (`colab-backends/tests/syntax_tree.rs`). Building it costs about 40%
   of tree-sitter's parse (84 ms beside 216 ms over those files,
   release), paid only until a language has its own parser, which emits
   the tree directly. **The matcher, started 2026-11-25:**
   `colab_syntax::pattern` compiles a pattern from a parsed fragment and
   matches it with colab's rules built in, not checked afterwards: every
   named child accounted for (what `Shape` does now), an error node taken
   only by a capture (what `errors_captured` does now; tree-sitter marks
   some recovered `ERROR` nodes *extra*, like comments, which the first
   version wrongly passed over), anonymous tokens passed over alike
   everywhere and reported in the match (what `led_forms` and
   `keep_modifier_tokens` work around now), `$$$` that backtracks (a last
   one takes the rest), and a repeated metavariable compared token by
   token. Comments and trivia are invisible; matching recurses only as
   deep as the pattern. Held to self-match, soundness (an exact pattern
   matches only the same code), all-names and whole-sequence
   generalization over 22,589 nodes of the repo's 439 source files in all
   14 languages (`colab-backends/tests/matcher.rs`). **Fragment
   compiling, 2026-11-25:** `colab_syntax::fragment::compile` reads a
   pattern's text bare or in a backend's contexts (the parser is the
   caller's), keeps every other reading as a form, accepts a
   placeholder-only error when nothing reads it cleanly, and falls back
   to `{ $$$B; }`; it needs no "earlier node of that kind" refusal, since
   it picks the pattern by its span rather than ast-grep's first node of
   the kind. `colab_rewrite::syntax::compile_fragment` runs it with a
   backend's `PatternLanguage`. The property suite's new `matcher`
   column self-matches the same sampled nodes with it: equal to the
   ast-grep engine in 12 languages and ahead in two (Go 40 / 40 against
   38, Swift 32 against 31). **The engine's forms, 2026-11-25:** modifiers
   are native, not forms: `pattern::Rules` absorbs the leading modifiers a
   pattern node leaves out (its kinds and the modifier tokens, listed in
   `Match::absorbed` for a rewrite to give back) and holds a spelled
   modifiers node to its tokens, replacing the sentinel text, the
   `k`-placeholder forms, `Visible::bound`, `led_forms` and
   `Shape::Modifiers` / `Tokens`. The rest are text forms,
   `colab_rewrite::syntax::compile_forms`: qualified and rooted calls
   (`Role::Qualified`), field shorthand (`Role::Shorthand`), the trailing
   `;` a node does not hold and the class-field reading, and the trailing
   `,` that changes nothing. `colab-backends/tests/engines.rs` runs both
   engines on every `<lang>::pattern` in the conformance matrix and the
   corpus (256 comparisons) and requires the same matched ranges; the
   one difference left was matching inside Rust macro contents.
   **Macro contents and `where`, 2026-11-25:** `syntax::find_where`
   searches a file's tree and the contents of the macro calls the
   language reads (nested ones too), and filters with
   `syntax::Checks`, every `where` test of `pattern/check.rs` over the
   new matches, a macro's matches judged in a `Frame` as the old engine
   does. The comparison now takes each rule with its constraints from the
   DSL's parser: 256 comparisons, 81 with `where`, 2 refused by both,
   none differing. **Rewriting, 2026-11-25:** `syntax::rewrite` fills
   templates itself (no `TemplateFix`): bindings rewritten first, each
   laid out as one mark against the match (`keep_layout`) and put back
   verbatim, forms' `Role`s (requalified, shorthand, loose `;`), an empty
   sequence's separator tightened, attachments taken by an unwrapping
   template, and a match's absorbed modifiers given back to a template
   that is the same kind of item in any reading. Rooted qualified forms
   now go before plain ones, and the matcher no longer passes over the
   token right after a `$$$` (it is the sequence's: a TypeScript member's
   `;`). The comparison gains `both_engines_rewrite_alike`: of 196
   `replace` rules without `let`s, 195 rewrite to the same text; the one
   left was rewriting inside Rust macro contents. **Macro rewriting,
   `let`s and rewriters, 2026-11-25:** macro calls' contents are
   rewritten in their frames and put back (`syntax::edit_macros`, one
   walk with the search, `each_macro`); `syntax::Lets` compiles a rule's
   `let`s, a first `rewrite(…)` step's rewriters included, and the
   rewriter derives their values as the old engine does. The comparison
   now reads each rule as the compiler hands it to a backend
   (`colab_dsl::compiler::resolved_matches`: includes, named patterns
   and rewriters resolved; `lower_lets` for the `let`s): 254 finds with
   none refused by both, and 200 rewrites, 4 `let`s and a rewriter among
   them, none differing. **Delete and the prefilter, 2026-11-25:**
   `syntax::delete` removes matches with their line, attached docs and
   attributes, or list comma, then inside macro contents;
   `syntax::prefilter` is the longest named-token literal every form
   spells. The comparison adds 52 deletes, all identical, and checks the
   prefilter on every find (a matching file always holds it). Every part
   of a `<lang>::pattern` rule now runs on colab's matcher and agrees with
   the ast-grep engine. **The switch, 2026-11-25:** `PatternRule` holds a
   `syntax::RulePattern` (forms, checks, `let`s, prefilter), so every
   `<lang>::pattern` rule runs on colab's matcher. The full suite found
   five things, all fixed: the contexts rely on refusing a wrapper with
   an earlier node of the pattern's kind (restored; it costs the two Go
   self-matches the new matcher had gained); a nested macro needs its
   frame resized after its contents are rewritten; a `$$$` binding takes
   the comments beside it (a `create` lost a doc comment) and not the
   whitespace ending it (three corpus captures, reblessed); TypeScript's
   modifier keywords stay loose even when a pattern spells others
   (`Rules::loose`, put back by the old engine's word placement, now
   `keep_modifier_words`); and a pattern only the file's root spans is
   several nodes, refused (the hook replay caught it proposing one). The
   property summary is the old engine's, number for number. **ast-grep
   removed, 2026-11-25:** `StructuralPattern` and everything only it used
   (`Shape`, the modifier sentinels and placeholder forms, `Visible`,
   `led_forms`, `errors_captured`, its checks, rewriter, macro frames and
   finder) are deleted (with colab: `rust::pattern` deletes by item name),
   `Modifiers` keeps only `kinds`, `colab ast` prints a pattern's tree
   from the new compiler's form, and `ast-grep-core` is gone from the
   workspace (101 run-time crates to 97; 4,326 lines removed). The old
   engine's 67 unit tests run unchanged on the new one through a small
   adapter; four exposed differences, all fixed: "did you mean" hints for
   an unknown kind or field, a template's later lines taking the match
   line's indentation with spaces as well as tabs (`indent_later_lines`;
   ast-grep did spaces, colab patched tabs), and a parse error tree-sitter
   reports without a node (a missing hidden token, Go's final newline),
   which `Tree::has_error` now takes from the parser, so Go patterns read
   in their context as before. `tests/engines.rs` had no second engine
   left and is gone. Matching no longer depends on any third-party
   matcher; parsing still uses tree-sitter (step 2 onwards). After that, the engine's other
   tree readers (`declarations`, `calls`, rename, generalize), each
   behind the same parity bar.
2. **Tested 2026-11-25.** colab builds on `mesquite` 0.26.1 with its
   grammar crates (`mesquite_rust` … `mesquite_tsx`, from the same
   upstream versions as colab's except C#, 0.23.5 against 0.24.0) by
   renaming the dependencies and changing three imports (TSX, PHP-only
   and inline Markdown are separate crates), plus dropping `.clone()` on
   `Language`, which is `Copy` there (clippy). The build then has no `cc`
   and no `-sys` crate. All 70 test binaries pass (corpus, conformance,
   property suite with an identical summary, matcher properties, CLI end
   to end). A tool parsing with both runtimes compared every node (kind,
   field, range, error, missing, extra) over ~5,900 real files from the
   workspace and the Cargo registry: identical in Go, Java, JavaScript,
   JSON, Kotlin, PHP, Python, Ruby, Rust, TypeScript and TSX; C# differs
   by the older grammar; Markdown's `info_string` lost its field name in
   185 files (a mesquite bug colab does not depend on); and 10 files of
   ambiguous C, C++ and Swift parse differently (a `FOO(x);` statement read
   as a type; one Swift file C cannot parse at all, which mesquite
   parses). Parsing is 9–38% faster per language; a registry-wide
   `colab find '$X.unwrap()'` (122,614 files) finds the same 457,760
   matches in 16.6 s against 25.4 s, and the release binary is 27.1 MB
   against 39.9 MB. The risk is provenance: one owner, account and repo
   created in September / October 2026, 1 star; MIT (tree-sitter's
   licence), `cargo deny` passes. **Decided 2026-11-25: not adopted.**
   colab keeps C tree-sitter through the transition: moving to mesquite
   and then to colab's own parsers would be two migrations of every
   language instead of one, and step 3 replaces the parser per language
   anyway. The worktree was removed; the comparison tool and its numbers
   above are the reference if the question comes back. Original notes: a
   short spike on a pure-Rust tree-sitter runtime (`mesquite`, a
   safe-Rust port of the 0.25 runtime, five weeks old; tree-sitter's own
   generator, `tree-sitter-generate`, is already Rust). If colab's
   grammars give the same trees under it on the samples and the corpus,
   C goes at once with every language kept, at the cost of porting each
   grammar's external scanner (Python 437 lines, Kotlin 459, C# 713,
   Ruby 1,107, Swift 1,119, C++ 148, PHP 17, Go and Java none) and of
   keeping tree-sitter's misparses.
3. **Rust started 2026-11-25:** `colab-parse-rust` turns rust-analyzer's
   parse (`ra_ap_syntax`, 34 pure-Rust crates) into a colab tree
   through `colab_syntax::SourceParser`. Over the Cargo registry's
   122,558 `.rs` files (2 GB) every tree is lossless, and rust-analyzer
   reads real Rust far better than tree-sitter: 5,347 files tree-sitter
   gives an error that rust-analyzer parses cleanly (macros, mostly),
   against 51 the other way (a few macro bodies; `gen` handled by parsing
   as 2021, then 2024 when that reads a file with fewer errors). Parsing
   takes 120 s against tree-sitter's 124 s. Two things it does not give
   colab as tree-sitter did: **fields** (rust-analyzer's live in its typed
   AST) and **tree-sitter's kind names** (`call_expr`, not
   `call_expression`), which `where … is`, `field`, the backend's
   modifier, attribute and macro kinds, and `colab ast` all expose.
   **Then:** the engine reads a `PatternLanguage`'s own parser when it has
   one (`PatternLanguage::parser`), and `colab_lang_rust::on_rust_analyzer()`
   is Rust's pattern configuration on rust-analyzer's trees (its macro
   kinds; `attr` passed over as a loose modifier, since it is the item's
   child there and tree-sitter's `attribute_item` its sibling; doc comments
   extras). Of the 27 Rust patterns the repo's corpus, conformance cases
   and examples spell, 26 find the same matches over the repo's Rust on
   both parsers (compared by end and bindings, since rust-analyzer's items
   start at their attributes). The one that differs is a decision, not a
   bug: `$F($$$A)` matches method calls on tree-sitter (`a.b(x)` is a call
   whose function is `a.b`) and not on rust-analyzer (a `method_call_expr`),
   27,851 matches against 9,341. Still to do before `rust::pattern` moves:
   a rewrite replaces the whole item node, so it loses the doc comments
   and attribute layout it should keep (start the edit at the first child
   the pattern covers); attached docs, separators and `colab ast` still
   read tree-sitter; the corpus and conformance matrix on the new parser;
   and the user-visible names (`where … is`, `field`) for rust-analyzer's
   kinds.
   **Done 2026-11-25: Rust has no tree-sitter.** `tree-sitter-rust` is out
   of the dependency graph, tests included. `rust::pattern`, `symbol`,
   `call`, `use`, declarations and `colab ast` read rust-analyzer: the
   pattern engine through `Syntax::Own`; `use`, declarations and calls
   from its typed AST (`colab-lang-rust/src/{uses,own}.rs`, through
   `SourceLanguage::own`). Today's behaviour is kept where users see it:
   tree-sitter's kind names are aliases (`call_expression` is `call_expr`
   and `method_call_expr`), its fields are put on rust-analyzer's nodes,
   and `$F($$$A)` matches method calls (`PatternLanguage::method_calls`),
   with rust-analyzer's own names for finer control (`where $F is
   path_expr` for plain calls only). Both tables came from lining up the
   two parsers' nodes over the Cargo registry. Engine changes this needed,
   all language-neutral: delete's attachments, `colab ast` and
   `generalize` on colab's own tree; a match starting after the docs and
   attributes its node holds; one match per run of nodes over the same
   text; missing tokens invisible to matching. The corpus, conformance
   matrix, evals and hook replay pass (the replay's one change is a rule
   the hook now rightly withholds: on rust-analyzer's tree `let $A`
   binds `mut x`, so the learned rule would rematch its output); the
   property suite's Rust row went from 37/40 to 37/37. Over the registry
   `colab find '$X.unwrap()'` finds 457,704 matches against
   tree-sitter's 457,760 (the files the two parsers read differently), in
   21.7–24.5 s against 25.4 s. Known difference: a field-access pattern
   (`$X.f`) no longer matches a method's callee (`x.f()`), which
   tree-sitter read as a field expression.
   Parsers in order of value, each replacing tree-sitter for its
   language once it passes the bar: Rust (`ra_ap_syntax`) and JS / TS /
   TSX / JSON (Biome) through adapters; then Go, Java and Kotlin
   hand-written. Drop C++; decide Ruby, Swift and C# by use. Until a
   language has its parser it keeps tree-sitter behind a Cargo feature,
   or the bridge from step 2.
   **The rest, assessed 2026-11-25** (crates.io and a spike, after Rust):

   | Language | Pure-Rust parser | Shape | Effort |
   | --- | --- | --- | --- |
   | JS / TS / TSX | `biome_js_parser` 0.7.3 (110 crates, no C) | lossless tree with trivia, bogus nodes on errors: rust-analyzer's shape | **next**. Spike over the workspace's 27,566 files: all lossless; 2.7× faster than tree-sitter (3.3 s against 9.1 s); syntax errors in 156 files only Biome sees, 124 only tree-sitter (`.js` read with JSX as tree-sitter does; the rest are parser options: CommonJS top-level `return`, parameter decorators) |
   | JSON | `biome_json_parser` (shares Biome's crates), or a small own parser | lossless | easy, with JS |
   | Python | `ruff_python_parser` 0.0.23 (63 crates, no C) | AST with ranges plus a token stream, error-recovering | medium: the tree is built from node ranges and tokens |
   | Markdown | `markdown` 1.0 (3 crates) or `comrak` | mdast / AST with source positions | medium: colab's sections, rows and links read positions, not a CST |
   | PHP | `mago-syntax` 1.61 (87 crates; C only through blake3's SIMD, `pure` avoids it) | spans and trivia, error-recovering | medium |
   | Go | `gosyn` 0.2.16 (AST), or hand-written | AST with positions | medium: a small grammar; hand-written is likely the sturdier path |
   | Java | none | — | hard: a parser of colab's own; a regular grammar, but large |
   | Ruby | `lib-ruby-parser` (Ruby 3.1, last release 2024-04); Prism's Rust crate wraps C | — | **very difficult**. **Dropped 2026-11-26**: the backend, its corpus, conformance cases and `ruby-lsp` mapping are gone (17 backends, 13 source languages) |
   | Kotlin | none | — | **very difficult**: newline-sensitive, large grammar |
   | Swift | none | — | **very difficult**: large, context-sensitive grammar |
   | C# | none | — | **very difficult**: one of the largest grammars |
   | C | `lang-c` (needs preprocessed input, 2023) | — | **very difficult**: the preprocessor; macros make source unparseable as C |
   | C++ | none | — | **very difficult**: the hardest grammar there is, plus the preprocessor |

   The tree-sitter runtime goes only when the last of these does, so the
   very difficult ones decide when: drop them, keep tree-sitter for them
   alone, or write parsers. Ruby was dropped (2026-11-26).
   **Decided 2026-11-26:** parsers of colab's own for every remaining
   language, written in order of popularity (GitHub's rankings): JavaScript
   with TypeScript and TSX (one crate, `colab-parse-js`, with dialects),
   then C#, C++, PHP, C, Kotlin, Swift. tree-sitter leaves the binary with
   the last of them.
   **Decided 2026-11-25:** no further third-party parsers to adapt
   (rust-analyzer's suits Rust; others would each be an integration to
   carry); colab writes its own, Go and Markdown first, the most used in
   Graham's projects and Markdown in every agent project. **The tree
   shape is transitional:** an own parser emits tree-sitter's kinds and
   fields while tree-sitter is still there to hold it to (and the
   backends, corpus and users' rules keep working); once tree-sitter is
   gone the shape is to be redesigned for what colab's matcher and
   rewrites need, so each parser names its kinds and fields in one table
   (`names.rs`) apart from its parsing.
   **Go, done 2026-11-25:** `colab-parse-go`, a hand-written lexer and
   recursive-descent parser (~2,000 lines, no dependency but
   `colab-syntax`). Over the repo's 91 Go files the trees are
   tree-sitter-go's node for node (a test holds it); over the workspace's
   22,537 files tree-sitter reads cleanly, 22,353 (99.2%) are, in 12.4 s
   against tree-sitter's 20.0 s. Of the 184 left, 121 are where
   tree-sitter misreads Go and colab does not copy it (a parameter group
   cut after nine names; `*(*T)(p)` as a conversion); the rest are
   ambiguous generic forms and empty-statement placement. The Go backend
   runs on it (its `import`, `package` and `struct_tag` modules on colab's
   tree), the floor's declarations, calls and qualifier check moved to
   colab's tree for every language, and tree-sitter-go is now only a test
   dependency, kept for the comparison.
   **Markdown, done 2026-11-25:** `colab-parse-markdown`, colab's own
   CommonMark + GFM parser (blocks line by line through containers, and
   inline links), ~1,500 lines with no dependency but `colab-syntax`. Its
   tree is shaped for colab from the start (the shape question above
   answered here first): tree-sitter-md's names for what the backend
   reads, none of its `section` nodes, continuation tokens or inline
   punctuation. The bar is the backend's views of a file (heading levels
   and text, list items, table cells, fences, link destinations), held to
   tree-sitter-md's by `colab-backends/tests/markdown_parser.rs`: all of
   the repo's 78 files but one tree-sitter-md misreads; over the
   workspace's 5,816, 5,679 (97.6%); over the knowledge vault's 123, all
   but one. What differs is tree-sitter-md misreading CommonMark: links
   whose destination holds `<` or whose text holds brackets (97 files),
   links after a code span holding a triple backtick, an item in a block
   quote taking the next line's `> `, an ordered list that cannot
   interrupt a paragraph, a list item's text lost after an escape. A
   fence delimiter is the fence characters alone, so `in fences` strips
   an indented fence's indentation (tree-sitter-md's delimiter sometimes
   took it, leaving nothing to strip). The backend runs on it and
   tree-sitter-md leaves the shipped binary (a test dependency, for the
   comparison).
   **JSON, done 2026-11-25:** no syntax tree at all. `json::key` needs
   values, members with decoded keys and byte ranges, so
   `colab-lang-json/src/tree.rs` (~250 lines) reads exactly that, with
   comments allowed as tree-sitter-json allowed them. Over the
   workspace's 14,855 `.json` files it reads what serde_json reads, with
   the same top-level keys, every one. tree-sitter-json is gone from the
   workspace.
   **Python, done 2026-11-26:** `colab-parse-python`, built as Go's is
   (~3,100 lines, no dependency but `colab-syntax`), tree-sitter-python's
   kinds and fields for the transition. Layout follows tree-sitter's
   scanner, since it decides where blocks end (a block reaches over the
   comments after it indented as deep as it is). Over the repo's 33 files
   the trees are tree-sitter's node for node; over the workspace's 8,763
   that tree-sitter reads cleanly, 8,728 (99.6%), in 1.4 s against
   tree-sitter's 3.5 s. The other 35 are tree-sitter misreading Python
   (`type(x).a = v` as a type alias in 20; `*f(x)` as a call of `*f` in
   12; `a := b if c else d` as a conditional). Its invalid fixtures (ruff's)
   read as tree-sitter reads them, newline-as-whitespace after a dangling
   operator included. tree-sitter-python is a test dependency only.
   Go over the module cache (186,104 files, 185,743 clean to
   tree-sitter): 184,382 the same (99.3%), in 188 s against
   tree-sitter's 365 s, once comments were placed in one pass (placing
   them one at a time was quadratic in a long literal: x/text's
   `date/tables.go`, 7,000 comments, took 16 s; now 1 s).
   **Java, done 2026-11-26, as a check of the estimates** (difficulty
   ranking of 2026-11-26: Java first, 3–4k lines): `colab-parse-java`
   came to 3,444 lines (parser 2,361, lexer 498, names 323, emission
   262), no dependency but `colab-syntax`, written in one session. Over
   14,827 Java files (the workspace's 270, and 14,557 in shallow clones
   of Spring Framework, Guava and JUnit 5 for a wider corpus), every one
   of the 14,738 that tree-sitter reads cleanly is the same tree node for node,
   in 2.8 s against tree-sitter's 6.1 s; the first run had 99.7%, and the
   two differences were tree-sitter's (each statement-less `case` label
   its own group; a record pattern's name an `identifier`). Go took
   ~2,900 lines and Python ~3,100, so a regular grammar costs about 3k
   lines here and the size column of the ranking holds; the harder
   languages' estimates (Kotlin 6–8k, C# and Swift 8–10k, C++ 12k+)
   stand as they are. The backend runs on it: patterns `Syntax::Own`,
   `imports.rs` and `packages.rs` on colab's tree, and calls by a new
   `CallShape::Prefixed` (the callee is the text before `arguments`, as
   Java's tree-sitter locator read it, now on colab's tree; Kotlin and PHP
   can use it too). tree-sitter-java is a test dependency only.
   **JavaScript, done 2026-11-26:** `colab-parse-js`, 3,560 lines (parser
   2,223, lexer 675, names 403, emission 259), JSX included, no
   dependency but `colab-syntax`. Its
   lexer is pulled by the parser and lexes a token again where the mode
   matters (regex or division, JSX text and tags); automatic semicolons
   follow tree-sitter-javascript's scanner, comment rule included. Over
   the workspace's 11,143 JavaScript files (11,058 clean to tree-sitter):
   11,039 the same (99.8%), in 7.5 s against 14.1 s; the other 19 are
   `await (f)(x)`, which tree-sitter misreads. The first run had 95.1%,
   the differences nearly all one rule (a statement ending at an
   automatic semicolon holds the comment left on its line, when an
   expression ends it and no `}` follows). The `js` backend runs on it
   (patterns, `imports.rs`, which `ts` and `tsx` share and now read
   through colab's tree whatever the parser); tree-sitter-javascript is a
   test dependency (and the parse cache's). TypeScript and TSX next, as
   dialects of the same parser.
   **TypeScript and TSX, done 2026-11-26:** dialects of `colab-parse-js`
   (`Dialect::Ts` / `Tsx`), adding 1,567 lines (5,127 in all: the types,
   declarations and class members in `parser/ts.rs`, 1,147, the rest
   hooks in the shared parser). `>>` stays one token and the type parser
   splits it where a type-argument list ends; automatic semicolons follow
   tree-sitter-typescript's older scanner, which differs from
   JavaScript's (`(` / `[` on the next line continues only after an
   expression, and a statement or block always holds the comment left on
   its line). Over the workspace's 9,586 TypeScript files (9,550 clean to
   tree-sitter): 9,354 the same (97.9%), in 1.9 s against 3.6 s; TSX's
   6,853 (6,804 clean): 6,757 the same (99.3%), in 1.1 s against 1.7 s.
   All but two of the 243 left are where tree-sitter misreads TypeScript
   and colab does not copy it: a non-null `!` bound looser than a binary
   operator (`a! - b!` as `(a! - b)!`, `x!.a + y!.b` as `(x!.a + y!).b`),
   and `await f<T>(x)` as a comparison. The first run had 96.6% for
   TypeScript; TSX's 63 error files were all JSX tags with type arguments
   (`<Select<Option> …>`). `ts` and `tsx` run on it; tree-sitter-typescript
   is a test dependency.
   **C#, done 2026-11-26:** `colab-parse-csharp`, 5,749 lines (parser
   2,388, expressions and patterns 1,704, types 451, lexer 817, names 105,
   emission 284), no dependency but `colab-syntax`. The workspace has only
   162 C# files, so it was held to a corpus of four public repositories
   (jellyfin, Newtonsoft.Json, eShop, efcore; 9,745 files, 9,179 clean to
   tree-sitter): 9,082 the same (98.9%), in 4.0 s against 15.8 s, and no
   clean file read with an error; the workspace's 108 clean files are all
   the same. The first run had 64%, most of it two things: the line end
   of a directive tree-sitter takes over a run of newlines, and a BOM
   before `#region`. Most of the work after that was tree-sitter's GLR
   choices, which the grammar alone does not predict and were found by
   probing: an `is` chain's first test takes the rest of it into its
   constant (`x is null || y` is `x is (null || y)`) unless the chain
   holds two tests that could be constants and does not end in one, or a
   `? :` follows, and then a bare name before `&&` is a type
   (`is_expression`); `(a) - b` is a cast; conditional access binds at the
   conditional's precedence (`x + b?.c` is `(x + b)?.c`); a parenthesized
   pattern is a tuple constant whenever every element is an argument
   (`not null` reads as a declaration there); `(T)F<A>(x)` and
   `new X<A>(y) { … }` are comparisons, a cast's dynamic precedence or the
   smaller tree winning. The 97 left are more of the same tie-breaks.
   `csharp` runs on it (`usings.rs` and `namespaces.rs` ported to colab's
   tree); tree-sitter-c-sharp is a test dependency.
   **C++, done 2026-11-26:** `colab-parse-c`, 5,421 lines (declarations
   1,992, expressions 1,221, items, statements and directives 1,179,
   lexer 662, names 106, emission 261), no dependency but `colab-syntax`;
   C is to be its second dialect, as TypeScript is `colab-parse-js`'s.
   Held to six public repositories (abseil-cpp, Catch2, fmt, googletest,
   nlohmann/json, leveldb; 2,297 files, only 1,048 clean to tree-sitter,
   macros defeating it) and to a Python environment's 9,288 headers
   (torch, onnx; 3,784 clean): 994 and 3,747 the same (94.8% and 99.0%),
   in 3.5 s against 10.1 s; the workspace's five clean files are all the
   same. The first run had 71%: function definitions whose declarator is
   under a pointer or reference (`T& f() {`), `MACRO(x);` in a file read as
   a constructor's declaration, `virtual f() = 0;`. tree-sitter's GLR
   choices copied since: `a * b;` and `a < b > c;` declare; `T x(a, b)`
   declares a function (parameters preferred to arguments); `struct
   TORCH_API Name { … };` is a function definition of a declarator `Name`
   with a compound statement for a body (1,300 torch headers), whose
   `Name() = default;` is a constructor's definition and `private:` a
   label; `(f)(x)` is a cast, `(f)()` a call; `sizeof(x)` takes a
   parenthesized value, `sizeof(ns::T)` a type. The 91 left are more of the
   same. `cpp` runs on it (`includes.rs` and `namespaces.rs` ported);
   tree-sitter-cpp is a test dependency. C next, as its dialect.

**LALRPOP** builds the 299-line DSL grammar with 61 build-time crates and
a `build.rs`. A hand-written recursive-descent parser (~700 lines) would
drop them and could give agent-oriented error messages; worth it when the
grammar next changes.

**Logging.** `env_logger` brings `jiff`, `anstream` and `env_filter` for
a format colab overrides anyway; a ~40-line `log::Log` would do, and
`colored` could go with it. Small.

Kept as they are: `clap`, `ignore`, `globset`, `regex`, `rayon`,
`similar`, `toml_edit`, `serde_json`.

## Stable gate before M20

A version is pinned into Claude (plugin, MCP config, hook) when all of
these hold:

1. **Coverage.** The change kinds an agent makes most (code at many
   sites, renames, markdown sections, rows and links, config keys) have
   each been made with colab in at least one dogfooding session.
2. **No silent failures.** The last two milestones logged no entry where
   colab reported success, or "matched nothing", while doing the wrong
   thing (as `why_unchanged` and the crate-rooted text search were).
3. **Little hand repair.** At most one logged hand fix per milestone
   (was: `cargo fmt` after templates, removed by `--tidy` on `refactor`).
4. **Measured benefit.** The M23 suite shows colab at least as correct as
   `Read` + `Edit` and cheaper in tokens on its tasks, in CI.

---

### Status (2026-11-14)

| Criterion | Status |
| --------- | ------ |
| 1. Coverage | Met for code at many sites, renames, Markdown sections, rows, items and links; config keys only in the eval task, not yet on this repo. |
| 2. No silent failures | **Reset, 1 of 2.** M10.6 logged one: `--exclude crates/x/**` walking `crates` excluded nothing, and the run reported success (fixed in M20: such a glob is rebased onto the walk root). M20 logged none. One more clean milestone, then the plugin's 2026.11.4 is released. |
| 3. Little hand repair | Met: `--tidy` on `refactor` removed the `cargo fmt` step; one hand edit per milestone (a line inside a shell fence). |
| 4. Measured benefit | Met: `evals/results.md`, 8 tasks correct, 73% fewer bytes than `Read` + `Edit`, budgets checked in CI. |

## Planned milestones

### M20 — Agent install & guardrails
- ✅ Shipped 2026-11-14 (see [`cli.md`](cli.md#colab-agent-install-and-uninstall)): `colab agent install` / `uninstall` (idempotent; uninstall takes back only colab's keys, hook, table, block and files), the post-edit hook (`colab hook post-edit`, in the plugin's `hooks/hooks.json`), MCP resources and a generated `llms-full.txt`, a marketplace (`.claude-plugin/marketplace.json`) and the plugin's version (2026.11.4). **Remaining:** cut that release once the stable gate holds again (criterion 2, below).
- `colab agent install` detects Claude Code / Cursor / Codex and wires
  skill + MCP server (+ optional hook).
- Post-edit hook: run `colab fix --check` (M14 packs) on files the agent
  just wrote; feed failures back. First in M20: Semgrep's Claude Code
  plugin ships the same guardrail, and it makes colab run on every edit
  instead of when the agent remembers it.
- MCP resources + `llms-full.txt` generated from `docs/`, including the
  per-backend caveats.
- ✅ Shipped 2026-11-14: `colab:ignore [names]` (after code: that line; on its own line: the line below; names are script names or `<lang>::<module>`) for find, refactor and check, by putting back what a rule changed on covered lines. Rules take `message "…"` and `severity error|warning|info`; `--check` fails only above `info`.

### Decisions waiting (from M29)

Wave D of the M29 follow-up sweep: decide before building.

- Static Linux release binaries (musl via release-kit's
  `EXTRA_TARGETS`): changes what every release ships, so it waits for
  a go-ahead.

## Follow-ups from shipped milestones

Deferred when a milestone shipped, or found while dogfooding
([`dogfood-log.md`](dogfood-log.md)). Each is small enough to fold into
whichever milestone touches the same code.

| Item | From |
| ---- | ---- |
| Remote pack registry and publishing | M11 |
| Linux release binaries that run on older glibc: they need 2.39, so Ubuntu 22.04 and Debian 12 cannot run them. A static `x86_64-unknown-linux-musl` build (release-kit's `EXTRA_TARGETS`) is one way; it built cleanly in M21 | M21 |
| Constraints on a nested pattern: `inside` / `has` / … take a pattern, not a rule, so a scope cannot say "a `mod` that follows `#[cfg(test)]`" (a Rust attribute is the item's sibling). ast-grep's relational rules take a whole rule | M30 |
| Packs that name the dependency range they apply to, so `colab fix` after a Renovate bump runs only the packs for what changed (each ecosystem's lockfile read for the version) | M31 |
| Modifiers in item patterns, the rest (Rust, Java, Kotlin, Swift, C# and PHP done 2026-11-24 with one `$COLAB_MODS` sentinel form each; JS and the new `ts` backend with `modifier_tokens`): C# matches at most four modifiers before a metavariable type (`$T $F() {}`); and in C++, `void $F() { $$$B }` is refused, since the grammar reads `void F() { x }` as a declaration with a brace initializer in every context (C# and PHP lone bodies fixed 2026-11-24) | dogfood 2026-11-24 |
| Hover text is matched as the server spells it, so a callee's parameter types outside `->` (Go, TypeScript) can still match `type` (a file's questions are batched now, and answers kept for the run) | M32 |
| Incremental reparse: after a rule changes a file, reparse with the old tree and an `InputEdit` from the rule's edits, instead of from scratch | M10.6 |
| Suggest `colab.generalize` from the post-edit hook: the tool is built (2026-11-24: one hand-made edit → a rule, its other matches, `rules` for `colab.apply`), but an agent has to think of it. The PostToolUse hook sees every Edit's old and new text, so it could generalize the edit in the background and say "this edit matches 14 other sites; `colab.generalize` it?" when it does. Also: two nodes changed far apart in one Edit (`before` not one node) are refused today. The second step after `rules` (JSON, 2026-11-24): telemetry shows pattern authoring, not the DSL, is most of the cost (28 refused patterns and 62 `colab ast` calls against 14 DSL parse errors in 7 days) | interfaces 2026-11-24 |
| Fewer, better-shaped MCP tools: in 7 days agents called `colab.apply` 76 times and `colab.preview` 7, and never `move_import`, `set_config_key`, `edit_markdown` or `rename_symbol`. Have `apply` return the change summary with diffs, and fold or redescribe the unused tools (structured `rules` covers most of what they do). Measure the failure rate by input form (`script` vs `rules`) once `rules` has been used for a while | interfaces 2026-11-24 |
| Measure how often the post-edit hook's `colab.generalize` note is taken up (habit telemetry: a `colab.apply` with `rules` soon after a hook note), and widen or narrow its filters from that. Two-place edits, context lines and MultiEdit were done 2026-11-24; replayed over this repository's 985 Edits the hook spoke on 12, and the remaining doubt is a rule that copies a literal from the example (`set_counts(10, 4, $B)`) | interfaces 2026-11-24 |
| Point `colab hook pre-bash` at symgraph for symbol lookups: when a Bash command searches source for a name (`grep -rn <identifier>`, `rg <identifier>`, `sed -n 'a,bp' <src file>`) rather than filtering a pipe, answer with `additionalContext` (never `deny`) naming the symgraph tool that does it (`symgraph-search`, `-definition`, `-callers`, `-references`). habit counted the colab session of 2026-11-24/25 at ~2,900 Bash greps (811 for a symbol, 1,006 filtering output) and ~1,000 `sed -n` reads against 3 symgraph calls, with the user-level instruction to use symgraph already loaded. Measure before and after with habit: Bash greps per symgraph call in the same repo | habit 2026-11-25 |
### Patterns inside macros

**Built 2026-11-24** (`colab_rewrite::pattern::MacroContents`, Rust
only; corpus `rust/pattern_macro_contents`), as designed below, with two
differences. Macros are read from an allow-list of expression macros
(the standard, `log`/`tracing` and `anyhow` ones), not from every macro
whose contents happen to parse: `matches!(x, Some(_))` parses as
arguments, but its argument is a pattern, so parsing alone is not a
sound test. And `has` (which looks only inside the match) stays
available; the rest of point 4 is the follow-up row above.

tree-sitter parses a macro call's arguments as a `token_tree`, so
`rust::pattern 'is_relevant($P)'` misses `assert!(is_relevant(p))`, and
agents fall back to `text::literal`. A sound design:

1. For each `macro_invocation` that a file holds and whose token tree
   contains the pattern's prefilter literal, take the tree's contents
   and parse them in a wrapper as call arguments (`fn _w() { _m(<contents>); }`).
   Only contents that parse cleanly are used: `assert!`, `vec!`,
   `format!`, `println!` and most expression macros. `macro_rules!`
   bodies, `matches!(x, Some(_))` patterns, and DSL macros (`html!`,
   `sqlx::query!`) fail to parse, so they are skipped, never guessed at.
2. Match the pattern in each parsed wrapper. Each `Found` maps back by
   one offset (wrapper position → token-tree start), and nested macros
   recurse.
3. Rewrite and delete run on the wrapper text. Their edits are mapped
   back into the file, and a delete's comma handling (`with_separator`)
   uses the wrapper's tree, where the arguments are real `arguments`.
4. Constraints have to be defined there. `inside` / `has` / `follows`
   would see the wrapper, not the file, so a rule with a relational
   constraint should skip macro contents, or evaluate the relation on
   the file's tree from the macro node out. Semantic constraints need
   the mapped position for the language server. Until both are
   defined, matching inside macros should apply only to rules without
   such constraints.
5. Opt-in per pattern language (`PatternLanguage::macro_contents:
   Option<MacroSpec { invocation, token_tree, wrapper }>`), so only
   Rust (and later C/C++ macro calls, which are already
   `call_expression`s) pays the extra parse, and only on files whose
   macro bodies hold the prefilter literal.

## Unscheduled

- Property tests (`proptest`) for the DSL parser and tree-sitter edit
  application: random byte ranges must never produce invalid UTF-8
  boundaries ([Goal 4](completed-milestones.md#goal-4--operational-concerns)).
- Signed multi-platform release pipeline ([Goal 4](completed-milestones.md#goal-4--operational-concerns)).

---

## Non-goals (for now)

- Computing semantics itself. colab infers no types and resolves no names:
  whole-program questions — a binding's type, what a name resolves to,
  who calls a function, what implements an interface — are *asked* of
  the language's own server (`type`, `resolves`, `calls`, `called_by`,
  `implements`; `colab rename`, `references`, `calls`, `implementations`).
- A colab-specific refactoring UI or editor extension, or a language
  server for `.colab` files: agents write the rules (as JSON through MCP
  more often than as scripts), and preview, per-rule apply and undo are
  `colab refactor --format diff`, `colab review` and `colab undo`.
  `colab server` was removed on 2026-11-25 for this reason.
- Merging rules' edits with each other. Rules in one script are ordered
  and sequential, each seeing the previous one's output, so there is
  nothing to merge between them. (A file that changed on disk while a
  run was in progress *is* merged three ways before writing — see
  [Conflicts](cli.md#conflicts).)
- Plugin marketplace, or a hosted pack registry. Packs are shared as git
  sources (M33); backends live in the workspace.
- Dynamic plugin loading (`libloading` / WASM). No one has asked to ship an out-of-tree backend, adding one in the workspace is cheap, and the cross-backend contract tests rely on every backend being in it. Revisit only if external contributors ask.

---

## Risks and how we manage them

| Risk | Mitigation |
| --- | --- |
| Tree-sitter grammar drift across languages produces brittle rewrites. | Pin grammar versions per backend crate; corpus tests catch regressions on upgrade. |
| Grammars go unreleased for years, and colab works around their misparses. | The conformance matrix and property suite name each workaround; see [Dependencies](#dependencies-what-each-one-buys) for vendoring chosen upstream commits. |
| colab's matching depends on ast-grep internals that are not semver-stable, and corrects them with more forms per pattern. | Pinned exactly in `Cargo.lock`; the plan is a matcher of colab's own ([Dependencies](#dependencies-what-each-one-buys)). |
| A repository's own `colab.toml` runs commands (git packs, formatters, `check`). | Open, ranks 1–2 of the [2026-11-25 review](#review-2026-11-25-safety-robustness-and-dependencies). |
| Multi-rule scripts make scripts harder to debug. | `colab explain` shows the full lowered IR; `--format diff` previews each rule's effect. |
| Idiom packs encourage unsafe mass rewrites. | Each pack ships a README listing exclusions; `--check` mode lets users gate adoption. |
| Workspace split temporarily slows iteration. | Migrate in one PR, no behaviour change, full CI green before any backend work begins. |
| GenAI-driven misuse (agent runs `--write` without review). | Default to `--dry-run` in non-TTY contexts; MCP `apply` tool returns the diff and requires explicit confirmation in the host. |
| Mass refactor lands a silent regression on a large repo. | M12: `--verify <command>` runs the project's check after each rule and auto-reverts on failure. `--commit-per-rule` makes each rule individually revertable. `colab fix --check` (M14) gates merges in CI. |
| The walker is too slow on a 100k-file monorepo. | M10: parallel walker + per-thread parser pool; M10.6: one parse per text, shared by consecutive rules (`colab_rewrite::parse_cache`). `criterion` benches under `benches/` track regressions. |
| Reviewers cannot audit a 5k-file diff produced by one run. | M16: per-rule attribution in JSON output, `colab review` paginated mode, `--split-pr` emits one branch per rule for separate review. |
| Mass `<lang>::symbol` rename touches more than the user intended. | Documented as syntactic and verified with `--format diff`. Narrowed by path (`in "<glob>"`) and by code (`where inside '<pattern>'`, M15); `colab rename` for a semantic rename. |
| Walking a vendored / generated tree wastes CPU and risks corrupting third-party code. | M9: default-on `.gitignore` awareness via the `ignore` crate; `--include`/`--exclude` glob filters; `--changed-since` for CI runs. |
| Long-running `colab.preview` / `colab.apply` MCP calls hang the host. | M13: streaming progress over MCP `notifications/progress` (LSP `$/progress` is a follow-up). `--summary-only` for headless runs that just need the aggregate. |
