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

## Where things stand (2026-11-13)

M1–M13.5 (but M10.6), M14, M17–M19, M21, M22–M28 and M20's non-installing parts have shipped
([details](completed-milestones.md)). colab has 12 source backends and
3 config backends, structural patterns with `where` constraints, delegated
rename, move, code actions, references and safe delete through the
language's own server, git-aware file operations, pre-apply conflict
checks, and a CLI, MCP server and LSP that share one report shape.

## Order of work

**M20 (its install step once the gate passes)**, then M16, M15 and M10.6. M28 (Markdown by structure), the dogfood follow-ups, M23 (the eval suite), M14 (`colab.toml` + `colab fix`) and M21 (CI formats) have shipped.

M20 puts colab in front of every Claude session, so a version is pinned
there only once it has passed the [stable gate](#stable-gate-before-m20)
by dogfooding. Until then, colab is developed with colab
([`dogfooding.md`](dogfooding.md)), and each milestone is chosen for
what it lets that dogfooding cover.

| Rank | Milestone | Why now |
| ---- | --------- | ------- |
| 1 | M20 Agent install & guardrails | Its parts that install nothing (`// colab:ignore`, rule `message` / `severity`) go first; the install itself waits for the gate's second criterion. |
| 2 | M16, M15, M10.6 | Review aggregation; scoped rename (largely superseded by M19); parse-once batching. |

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
   (today: `cargo fmt` after templates, which `--tidy` on `refactor`
   would remove).
4. **Measured benefit.** The M23 suite shows colab at least as correct as
   `Read` + `Edit` and cheaper in tokens on its tasks, in CI.

---

### Status (2026-11-14)

| Criterion | Status |
| --------- | ------ |
| 1. Coverage | Met for code at many sites, renames, Markdown sections, rows, items and links; config keys only in the eval task, not yet on this repo. |
| 2. No silent failures | **Not met.** The follow-ups found a row `set` that re-added its row on the next run; M23 found fenced rules adding imports to doc snippets; M14 found patterns matching a node with children they never mention (an `else`, a third `Union` member, an anonymous class body) and a rewrite dropping them. All reported success; all are fixed. The next two milestones must log none. |
| 3. Little hand repair | Met: `--tidy` on `refactor` removed the `cargo fmt` step; one hand edit per milestone (a line inside a shell fence). |
| 4. Measured benefit | Met: `evals/results.md`, 7 tasks correct, 70% fewer bytes than `Read` + `Edit`, budgets checked in CI. |

## Planned milestones

### M20 — Agent install & guardrails
- `colab agent install` detects Claude Code / Cursor / Codex and wires
  skill + MCP server (+ optional hook).
- Post-edit hook: run `colab fix --check` (M14 packs) on files the agent
  just wrote; feed failures back.
- MCP resources + `llms-full.txt` generated from `docs/`, including the
  per-backend caveats.
- ✅ Shipped 2026-11-14: `colab:ignore [names]` (after code: that line; on its own line: the line below; names are script names or `<lang>::<module>`) for find, refactor and check, by putting back what a rule changed on covered lines. Rules take `message "…"` and `severity error|warning|info`; `--check` fails only above `info`.

### M16 — Diff review aggregation
- Rule attribution in `--format json` / `ndjson` has shipped (`rules[]`,
  and `rules` on each ndjson file event). What remains:
- `colab review <run.ndjson>` paginated, per-rule grouped review.
- `--split-pr` emits one branch / patch series per rule, paired
  with `--commit-per-rule` from M12.

Design, from the original plan:

When a single run touches 5k files, the diff is unreviewable as a
single blob. Provide tools to slice it:

- **Per-rule grouping in `--format json`.** Each event already
  records the rule that fired; surface that for downstream
  filtering (`jq 'select(.rule == "rust::use::tokio")'`).
- **`colab review <run.ndjson>`** interactive mode: paginate
  diffs, group by rule, show top-N most-edited files, distribute
  edits per file (histogram) so reviewers can sample.
- **`--split-pr`** writeable mode: emit one git branch (or one
  `git format-patch` series) per rule, ready for separate PRs.
  Pairs with `--commit-per-rule`.

### M15 — Scope-bounded symbol rename
- `--scope <pattern>` filter on `<lang>::symbol` rename, narrowing
  rewrites to identifiers under a named function / class / impl /
  module node.
- Stays syntactic — no type info.
- Largely superseded by M19: `colab rename` is scope-aware through the
  language's server. Keep only if a syntactic, server-free narrowing is
  still wanted.

Design, from the original plan:

Today's `<lang>::symbol` renames every matching identifier in a
file — fast, but with a non-trivial blast radius at scale. Add
opt-in narrowing without crossing into whole-program type
inference:

- **`--scope <pattern>`** restricts the rename to identifiers inside
  a tree-sitter scope: a function/method by name, a class, an
  `impl` block, or a module.
- The pattern language is intentionally limited: identifier text
  only, no type-relative resolution. Implementation walks the tree,
  filters byte ranges that fall outside the scope node, then
  applies the existing edit logic.
- Documented as "narrows the rename to a syntactic region; still
  not a semantic rename". Matches IDE rename when there are no
  shadows; weaker but predictable when there are.

### M10.6 — Parse-once-per-backend batching (deferred from M10)
- Add `Operation::lang() -> &'static str` so the composer can
  group consecutive same-backend rules.
- Backend-level `apply_batch(ops, source) -> String` that parses
  once and applies every rule's edits, with conflict resolution
  on overlapping byte ranges (first rule wins, error on overlap,
  or merge — to be designed).
- This is a real `Operation` trait change with conflict
  semantics to settle; queued separately because it deserves its
  own design pass.

---

## Follow-ups from shipped milestones

Deferred when a milestone shipped, or found while dogfooding
([`dogfood-log.md`](dogfood-log.md)). Each is small enough to fold into
whichever milestone touches the same code.

| Item | From |
| ---- | ---- |
| A `colab.mv` MCP tool | M25 |
| Qualifier conflict checks (`imported_names`) for the ten source backends other than Go and Python | M27 |
| Attribute a `--verify-lsp` error to the rule that caused it | M27 |
| Type constraints in `where` (via the server's hover) | M26 |
| Compile-time warning when a template re-matches its own pattern | M18 |
| LSP `WorkDoneProgress` for long previews / applies | M13.5 |
| `--bisect`: check the starting tree first, and say so when it already fails | M20 (dogfood) |
| Go-to-definition for `include` in `.codemod` files | M19 |
| Remote pack registry and publishing | M11 |
| Moving a section between files: rules act on one file each, so today a move is an `insert` in one file and a `delete` in the other | M28 (dogfood) |
| Near misses for Markdown addresses: an address that names no section lists the nearest headings | M14 dogfooding |
| `markdown::item` placement: insert a bullet before or after a given item | M14 dogfooding |
| Report the files a rule skipped (`why_unchanged`) even when it changed others | M14 dogfooding |
| An eval task for `colab fix` | M14 |
| One call rewriter in `colab_rewrite` for the seven backends whose `calls.rs` differ only in the call node's kind (C, C++, C#, Go, JS, Python, Rust); RustRover's duplicate-code inspection finds them | RustRover inspections |
| `markdown::text` inside a code fence (a line in a README `sh` block needed a hand edit) | M21 dogfooding |
| Linux release binaries that run on older glibc: they need 2.39, so Ubuntu 22.04 and Debian 12 cannot run them. A static `x86_64-unknown-linux-musl` build (release-kit's `EXTRA_TARGETS`) is one way; it built cleanly in M21 | M21 |

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
- Plugin marketplace. Premature until the workspace + capability registry
  have shipped and stabilised.
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
| The walker is too slow on a 100k-file monorepo. | M10: parallel walker + per-thread parser pool, single-parse multi-rule batching. `criterion` benches under `benches/` track regressions. |
| Reviewers cannot audit a 5k-file diff produced by one run. | M16: per-rule attribution in JSON output, `colab review` paginated mode, `--split-pr` emits one branch per rule for separate review. |
| Mass `<lang>::symbol` rename touches more than the user intended. | Today: documented as syntactic and verified with `--format diff`. M15: `--scope <pattern>` narrows the blast radius to a named function / class / module node. |
| Walking a vendored / generated tree wastes CPU and risks corrupting third-party code. | M9: default-on `.gitignore` awareness via the `ignore` crate; `--include`/`--exclude` glob filters; `--changed-since` for CI runs. |
| Long-running `colab.preview` / `colab.apply` MCP calls hang the host. | M13: streaming progress over MCP `notifications/progress` and LSP `$/progress`. `--summary-only` for headless runs that just need the aggregate. |
