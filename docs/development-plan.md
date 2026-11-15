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

**M20's install step**, now that the stable gate has passed, then the [decisions waiting](#decisions-waiting-from-m29) from M29 when they are made. M28 (Markdown by structure), the dogfood follow-ups, M23 (the eval suite), M14 (`colab.toml` + `colab fix`), M21 (CI formats), M16 (`colab review`, `--split-pr`), M15 (`where inside` on `symbol` / `call`), M29 (the follow-up sweep), M30 (rule expressiveness), M31 (CI fit), M32 (type and name constraints), M33 (fleet runs, packs from git), M34 (symbol-addressed edits), M35 (scan phases) and M10.6 (a parse cache) have shipped.

M20 puts colab in front of every Claude session, so a version is pinned
there only once it has passed the [stable gate](#stable-gate-before-m20)
by dogfooding. Until then, colab is developed with colab
([`dogfooding.md`](dogfooding.md)), and each milestone is chosen for
what it lets that dogfooding cover.

| Rank | Milestone | Why now |
| ---- | --------- | ------- |
| 1 | M20 install step | The stable gate's criteria hold as of 2026-11-14 (criterion 1's config keys through the eval task, not yet on this repo); its non-installing parts (`colab:ignore`, rule `message` / `severity`) shipped earlier. |

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
| 2. No silent failures | **Met: 2 of 2 clean milestones (M16, M15).** Before them, the follow-ups, M23, M14 and the docs review after M21 each found one or two failures that reported success (a row `set` re-adding its row, fenced imports, patterns dropping unmentioned children, Markdown prefilters skipping files, `ensure` above `//!` docs); all are fixed. M16's finding changed formatting only; M15's (a syntactic scope renaming a field's uses but not its declaration) is the documented, positional meaning of the scope, caught by `cargo check` before shipping. |
| 3. Little hand repair | Met: `--tidy` on `refactor` removed the `cargo fmt` step; one hand edit per milestone (a line inside a shell fence). |
| 4. Measured benefit | Met: `evals/results.md`, 8 tasks correct, 73% fewer bytes than `Read` + `Edit`, budgets checked in CI. |

## Planned milestones

### M20 — Agent install & guardrails
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
| `type` / `resolves` asked once per match; batching a file's questions (or a server's own type-of-range request) would cut the round trips. Hover text is matched as the server spells it, so a callee's parameter types outside `->` (Go, TypeScript) can still match | M32 |
| `create "<path>" '<template>'`: a new file from a match, made as a `FileOp::Create` so undo and `--verify` reverse it, a file that exists a conflict. Needs file operations in the `refactor` write path, which writes text edits only | M35 |
| Phased runs (scans) under `--bisect`, `--verify`, `--split-pr`, `--stdin`, `--changed-since` and MCP, which refuse them today | M35 |
| Incremental reparse: after a rule changes a file, reparse with the old tree and an `InputEdit` from the rule's edits, instead of from scratch | M10.6 |
| `--include` / `--exclude` match paths relative to the walked target (`colab-lang-json/**` for target `crates`), while a rule's `in "<glob>"` matches paths relative to the working directory; one of them should accept the other's form | M10.6 |
| `rust::use "tree_sitter::Parser" { delete }` does not remove one member of a brace list (`use tree_sitter::{Parser, Tree};`); a `rust::pattern` rule did | M10.6 |

## Unscheduled

- Call and type hierarchy (`callHierarchy` / `typeHierarchy` on the language-server client), the last IntelliJ navigation gap. Read-only, and `colab references` covers most of the need; schedule it if M23 shows agents need caller trees. Cheap then: the client already makes this kind of request.
- Property tests (`proptest`) for the DSL parser and tree-sitter edit
  application: random byte ranges must never produce invalid UTF-8
  boundaries ([Goal 4](completed-milestones.md#goal-4--operational-concerns)).
- Signed multi-platform release pipeline ([Goal 4](completed-milestones.md#goal-4--operational-concerns)).

---

## Non-goals (for now)

- Whole-program semantic analysis. Colab is a syntactic rewriter; if a rule
  needs type inference it belongs in a language-specific tool, not here. (M19 *delegates*
  scope-aware rename to the language's own server; colab still computes
  nothing semantic itself.)
- Refactoring UI. The VS Code extension stays minimal — a thin wrapper over
  the LSP and the CLI.
- Conflict resolution between simultaneous edits. Rules in one script are
  ordered and sequential; there is no "merge" semantic.
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
