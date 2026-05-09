# Colab Development Plan

A roadmap for evolving colab from a single-purpose Go-import rewriter into a
maintainable, multi-language codemod platform usable by both humans and GenAI
agents.

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

## Current state (2026-05-09)

The 2026-05-09 version of this plan described colab as a single-crate
Go-import rewriter; M1–M8 plus several follow-ups have shipped since.
The repo is now:

- **Languages:** Go, Rust, Java, Python, JS/TS — five backends, each
  in its own `colab-lang-*` crate.
- **Operations:**
  - `go::import` (replace / delete / ensure), `go::symbol` (replace),
    `go::struct_tag` (replace), `go::call` (replace_call).
  - `rust::use` (replace / delete / ensure), `rust::symbol` (replace),
    `rust::crate` (replace / delete via `toml_edit`-validated line
    scan), `rust::call` (replace_call).
  - `java::import` (replace / delete / ensure), `java::package`
    (replace), `java::symbol` (replace).
  - `python::import` (replace / delete / ensure with segment-prefix
    matching), `python::symbol` (replace).
  - `js::import` (replace / delete) over ES module specifiers,
    `js::symbol` (replace). Applies to `.js`, `.mjs`, `.cjs`, `.jsx`,
    `.ts`, `.tsx`.
- **DSL:** Multi-rule scripts, `//` line comments, four actions
  (`replace`, `delete`, `ensure`, `replace_call`) with positional /
  `$args` / `$func` / `$$` template placeholders.
- **CLI:** `refactor`, `schema`, `list-languages`, `list-rules`,
  `explain`, `server`, `mcp`. `--format human|json|ndjson|diff`,
  `--write`/`--dry-run`/`--check`, `--stdin`/`--path`. Documented
  exit-code table (0/1/2/3/4/10).
- **LSP server:** `.codemod` diagnostics + namespace/module/action
  completion sourced from the live `BackendRegistry`.
- **MCP server:** four tools — `colab.schema`, `colab.lint_script`,
  `colab.preview`, `colab.apply` — over JSON-RPC 2.0 with
  `Content-Length` framing on stdio. Independent of every backend
  crate at runtime.
- **Modularity:** Cargo workspace with eight crates; `colab-dsl` and
  `colab-mcp` deliberately have no `colab-lang-*` runtime dep so a
  broken backend can't block unrelated work.
- **Tests:** 192 passing — unit + 28 corpus cases (idempotency
  enforced) + LSP unit + MCP unit + 16 CLI integration tests.
  Clippy / cargo-audit clean.
- **Docs:** `docs/{dsl,cli,features,architecture,development-plan}.md`,
  cross-linked, with capability data also discoverable at runtime via
  `colab schema` / `list-rules` / `explain`.

The `LanguageBackend`/`Operation`/`BackendRegistry` seam (introduced
in M2) plus the visitor-based walker (M3) are the right seams for
everything that follows.

### Goal-level status

- **Goal 1 — broader / deeper transformations.** ✅ Backends 1–5,
  multi-rule scripts, `replace` / `delete` / `ensure` /
  `replace_call`, idiom-pack skeletons. ⏳ remaining: `include`
  directive, populated idiom packs (Go 1.18 generics, Java 8→21
  full, Python 3.12 typing, JS/TS ESM migrations).
- **Goal 2 — modularity.** ✅ Workspace split, `LanguageBackend`
  trait + registry, per-crate CI matrix. ⏳ defer: dynamic plugins
  (`libloading`/WASM) until there is real demand.
- **Goal 3 — GenAI-friendly CLI.** ✅ Output formats, exec modes,
  exit codes, `schema`/`explain`/`list-*`, MCP server, LSP
  diagnostics + completion. ⏳ remaining: LSP hover/definition,
  the "preview this rule" code action, `--help-json`.
- **Goal 4 — operational concerns.** ✅ Test corpus + idempotency
  check, CI (build/test/clippy/audit). ⏳ remaining: property tests
  (`proptest`) for the parser and edit-application; `criterion`
  benches for the walker; signed release-binary pipeline.

---

## Goal 1 — Broader, deeper transformations

### 1.1 Multi-rule scripts (foundation; unblocks everything else)

Today `Refactoring { rule: Rule }` allows exactly one match/action. A real
dependency migration is dozens of coordinated rewrites.

**Work:**
- `Refactoring { rules: Vec<Rule> }`; grammar accepts any number of `match`
  blocks inside one `refactor "name" { … }`.
- Walker invokes each rule per file; transformers compose left-to-right and
  the final string is written once.
- Add `include "other.codemod"` so library packs (e.g. "go 1.21 → 1.23
  idioms") are reusable.

### 1.2 New language backends

Add backends in this order — each is a separate `codemod/<lang>/` module and
new `Rule` variants. Order is driven by where dependency churn hurts most.

| Order | Language | Initial operations |
| --- | --- | --- |
| 1 | Rust   | `use` rename, crate rename in `Cargo.toml` (already have `tree-sitter-toml`), edition migration helpers (`try!` → `?`, `extern crate` removal). |
| 2 | Java   | Import rename, package rename, simple API renames (`oldClass.method` → `newClass.method`). |
| 3 | Python | Import rewrite (`import x` / `from x import y`), `print` statement → function (legacy 2→3 leftovers), `typing.List` → `list[…]`. |
| 4 | JS/TS  | Module specifier rewrite, default↔named import swap, `require` → ESM. |
| 5 | Go (depth) | Symbol rename within a package, function-call signature changes, struct-tag rewrites. |

### 1.3 Operations beyond rename

Add new `Action` variants and matching `Rule` variants:

- `delete` — remove an import or call.
- `wrap` — wrap a call site in another (e.g. add `context.TODO()` first arg).
- `replace_call` — match `pkg.Old(a, b)` and produce `pkg.New(b, a, nil)`
  with positional argument templates.
- `add_import` / `ensure_import` — idempotently add a missing import after
  another rule introduces a new symbol.

### 1.4 Language-version idiom packs

Ship curated `.codemod` packs in `examples/packs/` (and later as a separate
publishable directory):

- `rust/edition-2021-to-2024.codemod`
- `go/1.18-generics.codemod` (replace common `interface{}` patterns where
  safe)
- `java/8-to-21.codemod` (lambdas, `var`, switch expressions, records — only
  the mechanical subset)
- `python/3.12-modern-typing.codemod`

Each pack ships with a `README.md` explaining what it does *not* cover (so
humans and agents know when to stop).

---

## Goal 2 — Modularity

The current single-crate layout will not scale to five language backends + a
plugin story. Restructure as a Cargo workspace:

```
colab/
  Cargo.toml                 # workspace
  crates/
    colab-core/              # error, model, walker, CodeTransformer trait
    colab-dsl/               # ast, grammar, compiler
    colab-lang-go/           # depends on colab-core + tree-sitter-go
    colab-lang-rust/
    colab-lang-java/
    colab-lang-python/
    colab-lang-js/
    colab-lsp/               # current language_server.rs, grown up
    colab-cli/               # the binary; thin shell over the above
    colab-mcp/               # see Goal 3
```

Migration steps:

1. Introduce the workspace, move existing modules into `colab-core`,
   `colab-dsl`, `colab-lang-go`, `colab-cli` without behaviour changes. Tests
   stay green.
2. Replace the hard-coded `match (lang, module, …)` in `compiler::lower_rule`
   with a `LanguageBackend` trait that each `colab-lang-*` crate implements
   and registers. `colab-cli` wires the registry. Unknown namespaces still
   fail loudly — that invariant moves into the registry lookup.
3. Land the CI matrix: `cargo build -p colab-lang-<x>` per crate so a broken
   backend cannot block unrelated work.

Stretch (defer until two backends ship): dynamic plugins via `libloading` or
WASM. Not worth the complexity until there is real demand.

---

## Goal 3 — GenAI-friendly CLI (and human-friendly too)

A CLI that works well for an AI agent is one that is also easier to script
and to reason about. The principles:

- **Stable structured output** alongside human output.
- **No surprises**: dry-run by default in any "advisory" mode, explicit opt-in
  for in-place writes.
- **Read from stdin / write to stdout** so transforms compose.
- **Exit codes carry meaning** beyond 0/1.
- **Deterministic ordering** of files, edits, and diagnostics.

### 3.1 Output modes

Add `--format {human,json,ndjson,diff}`:

- `human` (default when stdout is a TTY): today's coloured log lines.
- `json` / `ndjson`: one JSON object per file processed:
  ```json
  {"path":"foo.go","changed":true,"edits":[{"start":42,"end":58,"old":"old.module","new":"new.module"}],"rule":"go::import"}
  ```
- `diff`: unified diff per file, suitable for piping to `patch` or showing in
  a code-review UI.

Auto-disable ANSI colour when not a TTY (already idiomatic) and when
`NO_COLOR` is set.

### 3.2 Execution modes

- `--dry-run` (default for `--format json|diff`): never writes; reports what
  would change.
- `--write` (default for `--format human` when stdout is a TTY): apply
  changes in place.
- `--check`: exit non-zero if any file would change. CI- and pre-commit-
  friendly. Mirrors `gofmt -l`, `prettier --check`.
- `--stdin --path foo.go`: read source from stdin, write transformed source
  to stdout. The `--path` hint drives the language detection.

### 3.3 Exit codes

| Code | Meaning |
| --- | --- |
| 0 | Success, no changes needed (or `--write` succeeded). |
| 1 | Generic error (kept for backwards compat). |
| 2 | Script parse error. |
| 3 | Unsupported namespace/operation. |
| 4 | I/O error. |
| 10 | `--check` found changes that would be made. |

Document this table in `--help` and the README.

### 3.4 Discoverability for agents

- `colab schema` — print a JSON schema describing every supported namespace,
  operation, and option. An agent can fetch this once and stop guessing.
- `colab explain --script foo.codemod` — parse and print a JSON IR
  (`Refactoring`) without running it. Lets agents validate before executing.
- `colab list-languages` / `colab list-rules <lang>` — machine-readable
  capability discovery.
- Keep `--help` clean for humans; surface the same data via `--help-json`.

### 3.5 MCP server (`colab-mcp` crate)

Wrap the same operations as MCP tools so an agent in Claude Code (or any
MCP-aware host) can call them directly:

- `colab.preview` — apply script(s) to a path, return diff.
- `colab.apply` — same, but write.
- `colab.schema` — capability discovery.
- `colab.lint_script` — parse without executing.

This is strictly additive: the CLI remains the source of truth for
behaviour; MCP is one more frontend.

### 3.6 Grow the LSP stub

Today `language_server.rs` only logs lifecycle events. Add, in order:

1. Diagnostics for `.codemod` script syntax/semantic errors (reuse
   `compiler::compile` and surface its errors as LSP diagnostics).
2. Completion for namespaces and actions (data sourced from the same
   registry as `colab list-rules`).
3. Hover/definition for namespace symbols.
4. Code action: "preview this rule against the current workspace" — bridges
   LSP back to `colab.preview`.

---

## Goal 4 — Operational concerns

Required to make any of the above safe to ship.

- **Test corpus.** Add `tests/corpus/<lang>/<case>/` with `input/`, `script`,
  and `expected/` trees. A single `corpus_test.rs` walks them. Every new
  backend must add corpus cases before merging.
- **Idempotency check.** Run every transform twice in tests; second run must
  be a no-op. This catches rules that re-rewrite their own output.
- **Property tests** (`proptest`) for the DSL parser and for tree-sitter
  edit-application logic (random byte ranges must never produce invalid
  UTF-8 boundaries).
- **Performance baseline.** Add a `benches/` directory using `criterion` for
  the walker and per-backend rewrites. Track regressions in CI once any
  backend touches >1k files in real use.
- **Release pipeline.** GitHub Actions: build, test, clippy, audit
  (`cargo-audit`), and a release job that publishes binaries for
  macOS/Linux/Windows. Needed before agents can install colab via a single
  command.

---

## Goal 5 — Operating at scale

The shipped surface (M1–M8) handles a handful of files cleanly. Real
codebase migrations sweep across 10⁴–10⁶ files, run in CI, are
reviewed in chunks, and need to be safe to revert. This goal is
about turning colab from "works on my repo" into a tool a team can
point at a monorepo and trust.

### 5.1 Selective targeting

A whole-tree walk is the wrong default for big repos. Add a
filtering layer above the walker:

- **Glob include/exclude.** `--include 'cmd/**/*.go'` /
  `--exclude '**/vendor/**'`. Multiple of each, OR-combined. Use
  `globset` so patterns compile once and match cheaply.
- **`.gitignore` awareness.** Default-on; honours `.gitignore`,
  `.git/info/exclude`, and the global ignore file. `--no-ignore`
  for full-tree mode. Use `ignore` (the crate behind `ripgrep`).
- **Git-diff scoping.**
  - `--changed-since <ref>`: only files modified relative to a git
    ref (typically `main`). The CI use case.
  - `--staged`: only the git index. The pre-commit-hook use case.
  - Implementation walks the tree, then intersects with the path
    set that `git diff --name-only` reports.
- **Multiple roots.** Accept several positional path arguments and
  walk each; output stays grouped by root for review ergonomics.

This is invariant-preserving: filters operate above
`is_file_relevant`; nothing about the `Operation` contract changes.

### 5.2 Parallelism and parser pooling

The current walker is sequential and constructs a fresh tree-sitter
`Parser` for every file × every rule. Both are fixable without
touching the `Operation` contract:

- **Parallel walker.** Replace `walker::walk`'s recursive sync loop
  with a `rayon`-style parallel iterator that fans out over files
  and joins back into a sorted-by-path event stream so output stays
  deterministic. Per-file work is CPU-bound (parsing) and I/O-bound
  (read/write); both benefit.
- **Per-thread parser pool.** Each backend's `Operation` impl
  currently calls `Parser::new()` + `set_language(...)` inside
  `apply`. Replace with a thread-local parser cache so the
  language-table load happens once per thread per language.
- **Rule batching.** When the same backend has multiple rules in a
  refactoring, parse the source once and apply every rule's edits
  before serialising. Today each rule re-parses.
- **Knobs.** `--jobs N` (default `num_cpus`), `COLAB_JOBS` env var.
- **Benchmarks.** `criterion` suite under `benches/` driving
  synthetic 1k / 10k / 100k file corpora. Track regressions in CI
  (Goal 4 already promised this and it has not landed).

### 5.3 Verify-and-revert pipeline

Mass refactors break things. Make the breakage cheap to find:

- **`--verify <command>`.** After applying each rule, run
  `<command>`. Non-zero exit reverts that rule's edits and
  surfaces the rule's name in the diagnostic. Verify commands are
  language-agnostic (`cargo check`, `go vet ./...`,
  `pytest --collect-only`, `npm run typecheck`).
- **`--commit-per-rule`.** After each successful rule + verify,
  `git add -u && git commit -m "colab: <rule name>"`. Users get a
  clean per-rule history that's trivial to bisect or revert.
- **`--bisect <command>`.** Given a failing verify command, split
  the rule list and re-run; isolate the single rule that breaks
  the build. Mirrors `git bisect run` semantics.
- **Atomic backup.** When `--verify` is not configured, take a
  per-file snapshot to a hidden `.colab-backup/` so users can
  `colab undo` after the fact.

### 5.4 Streaming progress and run summaries

Long-running ops are unobservable today: agents wait on a single
return value. Add a steady stream of progress events.

- **LSP `$/progress`.** When the LSP runs a preview/apply on the
  workspace, stream `WorkDoneProgress` notifications.
- **MCP `notifications/progress`.** Same for `colab.preview` and
  `colab.apply`. Each progress message reports `(rule, files seen,
  files changed, bytes touched, elapsed_ms)`.
- **`--format ndjson` enrichment.** Today emits one event per file.
  Add per-rule "rule started"/"rule finished" events and a final
  "summary" event listing files changed per rule.
- **`--summary-only`.** Suppress per-file events; emit only the
  final aggregate. Useful on millions-of-files runs where the
  per-file trace is itself the bottleneck.

### 5.5 Repo-level config and `colab fix`

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

### 5.6 Scope-bounded symbol rename

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

### 5.7 Diff review aggregation

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

---

## Sequenced milestones

Roughly two-week increments; each is independently shippable.

### ✅ M1 — Refactor for growth (no user-visible change)
- Workspace split (`colab-core`, `colab-dsl`, `colab-lang-go`, `colab-cli`).
- Test corpus harness in place; existing Go tests migrated.
- CI runs clippy, tests, and `cargo-audit`.

### ✅ M2 — Multi-rule scripts + idempotency
- Grammar and IR support `Vec<Rule>`.
- Walker applies rules in order, single write per file.
- Idempotency assertion baked into corpus runner.

### ✅ M3 — GenAI surface v1
- `--format {human,json,diff}`, `--dry-run`, `--check`, `--write`, `--stdin`.
- Documented exit-code table.
- `colab schema`, `colab explain`, `colab list-languages|list-rules`.

### ✅ M4 — Second backend (Rust)
- `colab-lang-rust` with `use` rename and `Cargo.toml` crate rename.
- Edition-migration pack: `rust/edition-2021-to-2024.codemod` (skeleton).

### ✅ M5 — Action vocabulary expansion (partial)
- Shipped: `delete`, `ensure`, `replace_call`. Backfilled into Go and Rust.
- Deferred: `wrap` (subsumable as `replace_call "f($args)"`).

### ✅ M6 — MCP server + LSP diagnostics
- `colab-mcp` crate exposing `preview` / `apply` / `schema` / `lint_script`.
- LSP gains `.codemod` script diagnostics and namespace completion.
- Deferred: hover, go-to-definition, "preview this rule" code action.

### ✅ M7 — Java backend + idiom pack
- `colab-lang-java`. Pack stub at `examples/packs/java/8-to-21.codemod`.

### ✅ M8 — Python and JS/TS backends
- Both shipped: `python::import` (rename / delete / ensure) +
  `python::symbol`; `js::import` (rename / delete) + `js::symbol`.

### ✅ Bonus (post-M8 follow-ups already shipped)
- `<lang>::symbol` rename across all five backends.
- `go::struct_tag` rewrites.
- `replace_call` action and `<lang>::call` namespace for Go and Rust.
- `//` line comments in the DSL grammar.

---

The block below captures Goal 5 work — large-scale refactoring
ergonomics. Each milestone is independently shippable in roughly
two-week increments.

### ✅ M9 — Selective targeting (Goal 5.1)
- `--include`/`--exclude` glob filters via `globset` + `ignore::Override`.
- Default-on `.gitignore` awareness via the `ignore` crate;
  opt-out with `--no-ignore`.
- `--changed-since <ref>` and `--staged` git scopes (shell out to
  `git diff --name-only`; no tree walk in those modes).
- Multiple positional roots; deterministic path-sorted output
  preserved.
- 4 new walker unit tests + 4 new CLI integration tests; doc
  updates in `docs/cli.md` (flag table + sample pipelines).

### ✅ M10 — Parallelism (Goal 5.2, partial)
- `rayon`-based parallel walker. Three-phase design: discovery
  (`ignore::WalkBuilder` → sorted path list), chunked parallel
  process (`par_iter` over batches of 256 paths), in-order
  delivery to the visitor (`par_iter().collect()` preserves
  source order).
- Per-walk thread pool sized by `--jobs N` (falling back to
  `COLAB_JOBS`, then `num_cpus`). Each invocation gets its own
  pool so back-to-back walks honour per-call settings.
- `criterion` walker bench under `crates/colab-cli/benches/`
  (closes the open Goal 4 item). Sequential vs parallel on a
  synthetic 1k-file Go corpus shows ~2× speedup with default
  threads on this hardware.
- 2 new walker unit tests (parallel determinism across 6 runs,
  `--jobs 1` smoke) + 1 new CLI integration test (`--jobs 1` and
  `--jobs 8` produce bit-identical output).
- **Deferred to M10.5 / M10.6** (see below).

### ✅ M10.5 — Per-thread parser cache (Phase A of M10's deferred set)
- Each `colab-lang-*` crate gets a `thread_local!`
  `RefCell<Parser>` with the language pre-loaded, exposed as
  `pub(crate) fn parse(&str) -> Option<Tree>`. Every callsite
  across go (4 modules), java (3), js (2), python (2), and rust
  (3) uses the cached helper instead of constructing a fresh
  Parser per call. ~150 LoC of duplication removed; no API
  change.
- Bench numbers on the 1k synthetic Go corpus are essentially
  unchanged — `Parser::new() + set_language()` wasn't the
  dominant cost for moderate files — but the change tightens
  hot paths for the many-tiny-files regime where setup overhead
  dominates, and removes per-rule allocations.
- All 204 tests stay green.

### M10.6 — Parse-once-per-backend batching (Phase B, deferred)
- Add `Operation::lang() -> &'static str` so the composer can
  group consecutive same-backend rules.
- Backend-level `apply_batch(ops, source) -> String` that parses
  once and applies every rule's edits, with conflict resolution
  on overlapping byte ranges (first rule wins, error on overlap,
  or merge — to be designed).
- This is a real `Operation` trait change with conflict
  semantics to settle; queued separately because it deserves its
  own design pass.

### ✅ M11 — DSL `include` directive + pack catalog (closes Goal 1.1)
- `include "<path>"` in the DSL with cycle detection. Resolution
  relative to the including file (or absolute). Compile-time
  expansion via `expand_includes` recursively splices match
  clauses into the parent in source order. AST gains an `Item`
  enum (`Match` | `Include`) so the parsed shape preserves
  ordering; runtime IR sees the flat post-expansion list.
- New `compile_at_path(&Path, &registry)` entry point: reads the
  file, threads its parent as the include base path. Existing
  `compile(text, &registry)` keeps working for in-memory scripts
  but raises a clear error if it sees an `include` (suggesting
  `compile_at_path` in the message).
- CLI uses `compile_at_path` so includes "just work" from a script
  on disk.
- `colab pack list` enumerates `.codemod` files under
  `<repo>/.colab/packs/` (discovered by walking up to the nearest
  `.git`) and `~/.colab/packs/`. Sorted by path; emits `{name,
  path, source}` per entry.
- 4 new compiler unit tests (parses include, rejects without
  base path, expands at path, detects cycles), 1 new pack-catalog
  unit test, 1 new CLI integration test for `colab pack list`,
  1 new corpus case (`go/include_directive`) exercising a real
  shared.codemod.
- `colab explain` JSON shape changed: `rules: [...]` →
  `items: [{kind: "match"|"include", ...}]` to surface includes
  alongside matches in source order. Documented in `docs/cli.md`.
- **Defer:** remote registry / publishing.

### M12 — Verify-and-revert pipeline (Goal 5.3)
- `--verify <command>` per-rule check + auto-revert.
- `--commit-per-rule` git history per applied rule.
- `--bisect <command>` to isolate a breaking rule.
- `colab undo` over per-file backups when no verify command is set.

### M13 — Streaming progress + run summaries (Goal 5.4)
- LSP `WorkDoneProgress` notifications during preview/apply.
- MCP `notifications/progress` for the same.
- ndjson run-summary event; `--summary-only` mode.

### M14 — Repo-level config + `colab fix` (Goal 5.5)
- `colab.toml` schema (roots / excludes / packs / verify / jobs).
- `colab fix` umbrella subcommand; `colab fix --check` for CI.
- Pack discovery shared with M11.

### M15 — Scope-bounded symbol rename (Goal 5.6)
- `--scope <pattern>` filter on `<lang>::symbol` rename, narrowing
  rewrites to identifiers under a named function / class / impl /
  module node.
- Stays syntactic — no type info.

### M16 — Diff review aggregation (Goal 5.7)
- Rule attribution in `--format json`/`ndjson` events.
- `colab review <run.ndjson>` paginated, per-rule grouped review.
- `--split-pr` emits one branch / patch series per rule, paired
  with `--commit-per-rule` from M12.

### M-future — Unscheduled
- Idiom packs populated end-to-end (Go 1.18 generics, Java 8→21
  full, Python 3.12 typing, JS/TS ESM migration).
- LSP hover / go-to-definition / "preview this rule" code action.
- Property tests for the DSL parser and tree-sitter edit
  application (Goal 4).
- Signed multi-platform release pipeline (Goal 4).
- Dynamic plugin loading (`libloading` / WASM) — only if external
  contributors want to ship out-of-tree backends.

---

## Non-goals (for now)

- Whole-program semantic analysis. Colab is a syntactic rewriter; if a rule
  needs type inference it belongs in a language-specific tool, not here.
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
