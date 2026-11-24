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
| A clean `fix --check` warns that a finding rule matched no files, and so does a `--baseline` run whose findings were all the ref's: for a check, matching nothing is the state wanted, so the warning could be worded by what the run means, or dropped for rules with a `message` | M31 |
| Hover text is matched as the server spells it, so a callee's parameter types outside `->` (Go, TypeScript) can still match `type` (a file's questions are batched now, and answers kept for the run) | M32 |
| `create "<path>" '<template>'`: a new file from a match, made as a `FileOp::Create` so undo and `--verify` reverse it, a file that exists a conflict. Needs file operations in the `refactor` write path, which writes text edits only | M35 |
| Incremental reparse: after a rule changes a file, reparse with the old tree and an `InputEdit` from the rule's edits, instead of from scratch | M10.6 |
| `rust::use "tree_sitter::Parser" { delete }` does not remove one member of a brace list (`use tree_sitter::{Parser, Tree};`); a `rust::pattern` rule did | M10.6 |
| Patterns inside macros, the rest: evaluate `inside` / `follows` / `nth` / `field` from the macro node outward in the file's tree, and map a semantic question's position back to the file, so rules with those constraints read macro contents too; user-listed expression macros (a `colab.toml` key) beside the built-in list | 2026-11-24 |
| Verbatim captures in a template with a `let`: its derived values have no source span, so such a match keeps the token-level layout (`render_verbatim` steps aside). A derived value could be laid out against its base capture's span | 2026-11-24 |

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
- A colab-specific refactoring UI or editor extension. Preview, per-rule
  apply and undo are `colab server` code actions (see
  [`colab server`](cli.md#colab-server)), so any LSP editor shows them in
  its own refactor preview.
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
