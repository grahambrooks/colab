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

M1–M13.5, M17–M19, M22 and M24–M27 have shipped
([details](completed-milestones.md)). colab has 12 source backends and
3 config backends, structural patterns with `where` constraints, delegated
rename, move, code actions, references and safe delete through the
language's own server, git-aware file operations, pre-apply conflict
checks, and a CLI, MCP server and LSP that share one report shape.

## Order of work

**M20 → M14 → M21 → M23**, then M16, M15 and M10.6. The rationale:

| Rank | Milestone | Why now | Borrowed from |
| ---- | --------- | ------- | ------------- |
| 1 | M20 Agent install & guardrails | Setup cost is now the largest barrier to an agent using colab; suppression comments are the last open IntelliJ gap. | `mod config agent-tools install`, `npx codemod ai`, Semgrep's post-edit hook |
| 2 | M14 `colab.toml` + `colab fix` | Repo-declared migrations that agents and CI can discover; also unblocks M20's hook and M25's per-language formatters. | — |
| 3 | M21 Distribution & CI formats | Works in sandboxes with no Rust toolchain; CI annotations. | ast-grep (npm/pip), Semgrep (SARIF) |
| 4 | M23 Agent eval suite | Turns the "agent of choice" claim and the dogfood token figures into a measured, guarded result. | Anthropic "evaluate your tools" |
| 5 | M16 Diff review aggregation | Large-run review. | — |
| 6 | M15 Scope-bounded symbol rename | Largely superseded by delegated rename (M19). | — |
| 7 | M10.6 Parse-once batching | Performance; needs its own design pass. | — |

---

## Planned milestones

### M20 — Agent install & guardrails
- `colab agent install` detects Claude Code / Cursor / Codex and wires
  skill + MCP server (+ optional hook).
- Post-edit hook: run `colab fix --check` (M14 packs) on files the agent
  just wrote; feed failures back.
- MCP resources + `llms-full.txt` generated from `docs/`, including the
  per-backend caveats.
- `// colab:ignore <rule-or-script-name>`, on the line or the
  line above, skips that site for find, refactor and check. Rules gain an
  optional `message` and `severity`, so a rule enforced by the post-edit
  hook reads as an inspection rather than a bare diff.

### M14 — Repo-level config + `colab fix`
- `colab.toml` schema (roots / excludes / packs / verify / jobs).
- `colab fix` umbrella subcommand; `colab fix --check` for CI.
- Pack discovery shared with M11.
- Per-language formatter commands (`gofmt -w`, `rustfmt`,
  `prettier --write`) for `--tidy`, deferred from M25.

Design, from the original plan:

Project teams want to declare their canonical refactors once and
have CI / pre-commit / agents pick them up:

- **`colab.toml`** at repo root. Schema:
  ```toml
  [run]
  roots = ["."]
  exclude = ["vendor/**", "third_party/**"]
  jobs = 8
  verify = "cargo check"

  [[packs]]
  path = "scripts/refactor/2024-q4.codemod"

  [[packs]]
  path = "scripts/refactor/javax-to-jakarta.codemod"
  ```
- **`colab fix`** umbrella subcommand: discovers `colab.toml`,
  applies every configured pack to the configured roots in order,
  honours the configured `verify` command. Idempotent by
  construction.
- **`colab fix --check`** for CI: exit 10 if any pack would change
  anything.

### M21 — Distribution & CI formats
- npm (`npx colab`) and PyPI (`uvx colab`) wrappers around the release
  binaries, for sandboxes with no Rust toolchain.
- `--format sarif` and `--format github` for `--check` annotations.

### M23 — Agent eval suite
- Scripted tasks (repo-wide rename, API migration, config rollout)
  scored on correctness, tool calls, and tokens for colab vs `Edit`/`sed`;
  run in CI to keep output sizes honest.
- Add IntelliJ-shaped tasks ("extract this function", "move
  this type to its own file, history intact") so the gap closing is
  measured, not assumed.
- Start from `scripts/price-edits.py` and the token table in
  [`dogfood-log.md`](dogfood-log.md#token-cost-m27--m22), which already
  price a change as colab calls versus `Read` + `Edit`.

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
| `--tidy` on `refactor` (organize imports + format the changed files, as its own rule) | M25 |
| A `colab.mv` MCP tool | M25 |
| Qualifier conflict checks (`imported_names`) for the ten source backends other than Go and Python | M27 |
| Attribute a `--verify-lsp` error to the rule that caused it | M27 |
| Type constraints in `where` (via the server's hover) | M26 |
| Compile-time warning when a template re-matches its own pattern | M18 |
| LSP `WorkDoneProgress` for long previews / applies | M13.5 |
| Go-to-definition for `include` in `.codemod` files | M19 |
| Remote pack registry and publishing | M11 |
| Raw strings in the DSL (a value or pattern holding both `'` and `"`, or a Rust lifetime) | dogfood |
| Re-indent a template's later lines in tab-indented source (Go) | dogfood |
| Suggest `--bisect` when `--verify` fails on rule 1 | dogfood |
| Let `field: $X` match Rust's shorthand field `field`; a struct-field context for nested patterns | dogfood |

## Unscheduled

- Idiom packs populated end-to-end (Go 1.18 generics, Java 8→21
  full, Python 3.12 typing, JS/TS ESM migration, Rust edition
  2021→2024). Each pack ships a README of what it does *not* cover.
- Call and type hierarchy (`callHierarchy` / `typeHierarchy` on the
  language-server client), the last IntelliJ navigation gap.
- Property tests (`proptest`) for the DSL parser and tree-sitter edit
  application: random byte ranges must never produce invalid UTF-8
  boundaries ([Goal 4](completed-milestones.md#goal-4--operational-concerns)).
- Signed multi-platform release pipeline ([Goal 4](completed-milestones.md#goal-4--operational-concerns)).
- Dynamic plugin loading (`libloading` / WASM), only if external
  contributors want to ship out-of-tree backends.

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
