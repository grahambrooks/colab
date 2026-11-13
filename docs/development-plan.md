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
  diagnostics + completion, inline `-e` rules, `colab.test_rule`,
  near-miss diagnostics, `--help-json` (M17); structural patterns,
  `colab find`, `colab ast` (M18); LSP hover / code actions /
  client-mode rename (M19); git-aware file operations and `colab mv`
  (M24). ⏳ next: delegated code actions and references (M25), then agent install + guardrails
  (M20) — see "Next steps: closing the IntelliJ gap" below.
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

### ✅ M12 — Verify-and-revert pipeline (Goal 5.3, partial)
- `--verify <CMD>`: each rule is applied in isolation (via a new
  `SingleRule<'a>` wrapper that impls `CodeTransformer` over one
  `&dyn Operation`), the supplied shell command runs after the
  rule's writes, and a non-zero exit triggers an in-memory backup
  restore of every file the rule touched. The CLI bails with the
  Config exit code (1) so CI sees the failure. Reverted file
  count is reported.
- `--commit-per-rule`: after each successful rule (and verify if
  set), `git add -u && git commit -m "colab: <rule>"`. The
  "nothing to commit" case is treated as success so a no-op rule
  doesn't poison the run. Requires a git repo.
- 3 new CLI integration tests cover both the verify-success and
  verify-fail paths plus a real-git two-rule commit-per-rule run.
- **Deferred to M12.5:** `--bisect <command>` and `colab undo`.

### ✅ M13 — Run summary + `--summary-only` (Goal 5.4, partial)
- `Reporter` trait gains `report_summary(&RunSummary)`. Both the
  human and json/ndjson reporters override it; the diff reporter
  intentionally does not (its output stays `patch`-consumable).
- Per-file json events now carry a `"type": "file"` discriminant;
  the new summary event is `"type": "summary"` with
  `files_seen / files_changed / bytes_before / bytes_after /
  elapsed_ms`. Existing consumers that filter on `.path` keep
  working; new consumers can switch on `.type`.
- `--summary-only` flag suppresses per-file events for both the
  full-tree path and the per-rule (`--verify` /
  `--commit-per-rule`) path. Useful on huge runs where the
  per-file trace is itself the bottleneck.
- 1 new CLI integration test (`summary_only_suppresses_per_file_events`)
  + the existing JSON test extended to assert the new event
  shape.
- **Deferred to M13.5:** MCP `notifications/progress` and LSP
  `WorkDoneProgress`.

### ✅ M12.5 — Bisect, --backup, colab undo (closes M12)
- `--bisect <CMD>`: full apply, then verify, and on failure
  binary-search the rule list (`rules[..mid]` invariant) until
  the single breaking rule is identified. The pre-run state is
  snapshotted in memory at full-apply time; each bisect iteration
  restores from that snapshot before applying the candidate
  prefix and re-running verify. The working tree is restored to
  the pre-run state on exit so the user can inspect the failure
  cleanly. Conflicts with `--verify` (bisect *is* verify+search).
- `--backup <DIR>`: opt-in pre-modification snapshots written to
  `<DIR>/<rel-path>` (mirrors absolute-path layout). First snapshot
  per file wins so multi-rule runs preserve the truly pre-run
  state.
- `colab undo --from <DIR>`: walks the backup directory and writes
  each backup back over its original location.
- 3 new CLI integration tests: bisect-success path, bisect
  finds-the-breaker (with full revert verified), --backup +
  `colab undo` round-trip.

### ✅ M13.5 — MCP notifications/progress (closes M13)
- New `handle_streaming<W: Write>` dispatcher in `colab-mcp` runs
  alongside the synchronous `handle`. When a `tools/call` for
  `colab.preview` / `colab.apply` carries `params._meta.progressToken`,
  the server emits `notifications/progress` JSON-RPC messages every
  64 files plus a final `progress == total` tick before writing the
  response.
- Notifications are best-effort: write failures during a tick are
  swallowed (the client may have closed early); the response write
  surfaces real errors.
- Calls without a progress token, calls to non-streaming tools
  (`colab.schema` / `colab.lint_script`), and every other JSON-RPC
  method fall through to the existing synchronous path.
- 2 new colab-mcp unit tests: progress fires when token present,
  no progress when absent. The test parses `Content-Length`-framed
  output buffers and asserts both the notification stream and the
  final response.
- **Deferred:** LSP `WorkDoneProgress`. The LSP currently exposes
  only diagnostics + completion (both fast). Once it gains
  preview/apply requests, the same shape applies.

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

## Agent of choice (M17–M23)

*Added 2026-11-11, from a survey of the tools agents reach for instead
of colab today: ast-grep, GritQL, codemod.com, OpenRewrite/Moderne,
Comby, Semgrep, Serena — and Anthropic's "writing tools for agents"
guidance.*

What sends an agent back to `Edit` / `sed` / ast-grep, in order of cost:

1. **Expressiveness.** colab's vocabulary is fixed (`import`, `symbol`,
   `call`, …). Every competitor an agent prefers accepts *code-shaped
   patterns with metavariables* (`console.log($A)`), which models write
   fluently. A change outside colab's modules cannot be stated at all.
2. **Round-trips.** A script file was mandatory; there was no way to try
   a rule on a snippet, no read-only structural search, and no AST dump
   to debug a pattern. A dead rule said *that* it matched nothing, never
   *what was there instead*.
3. **Reach.** No one-command agent setup, no `npx`/`uvx` channel, no
   post-edit guard, and colab's LSP stops at diagnostics.

Ranked by leverage per unit of work:

| Rank | Milestone | Borrowed from |
| ---- | --------- | ------------- |
| 1 | M17 Agent ergonomics | ast-grep `--inline-rules` / `test_match_code_rule`, Semgrep custom-rule tool |
| 2 | M18 Structural patterns | ast-grep, GritQL, Comby, codemod jssg |
| 3 | M19 LSP in both directions | Serena, Moderne `change_method_name` |
| 4 | M20 Agent install & guardrails | `mod config agent-tools install`, `npx codemod ai`, Semgrep's post-edit hook |
| 5 | M14 `colab.toml` + `colab fix` (above) | — |
| 6 | M21 Distribution & CI formats | ast-grep (npm/pip), Semgrep (SARIF) |
| 7 | M22 Workflow-shaped MCP tools | Moderne, Anthropic "fewer, workflow-shaped tools" |
| 8 | M23 Agent eval suite | Anthropic "evaluate your tools" |
| 9 | M16, M15 (above) — M15 largely superseded by M19 | — |

M17–M19 have shipped. The order after M19 is set by the IntelliJ
comparison in "Next steps: closing the IntelliJ gap" below: **M24 → M25 → M20 → M26 → M27/M22**,
then M14, M21, M23.

### ✅ M17 — Agent ergonomics
- **Inline rules.** `colab refactor -e '<match …>'` (repeatable,
  `--rule`) and `--script -`; `explain` too. Parse errors are re-anchored
  onto the rules as typed.
- **`colab.test_rule`** MCP tool: script + snippet + path hint (+
  optional `expected`) → output, diff, per-rule counts,
  `matches_expected` / `expected_diff`. In-memory; the twin of `--stdin`.
- **Near-miss diagnostics.** `Operation::near_miss_target` /
  `observed_targets`; `colab_core::near_miss::probe` samples up to 500
  scanned files for dead rules only, ranks by `suggest::near_miss_distance`,
  and both surfaces render `rules[].near` plus a `nearest seen:` warning.
  Implemented for import-equivalents, `call`, and `symbol` modules.
- **`--help-json`** on every command, generated from clap.
- **Plugin.** `plugin.json` drops `version` so it tracks git and can't
  lag a release; skill teaches `-e`, `test_rule`, `near`.

### ✅ M18 — Structural patterns
- `match <lang>::pattern '<code with $METAVARS>' { replace '<template>' | delete }`
  on all 12 source backends — no grammar or IR change; it fits
  `RuleSpec::Replace` / `Delete`.
- Metavariables are ast-grep's: `$NAME`, `$$$NAME`, `$_`; a repeated
  name must bind identical text.
- Engine: `ast-grep-core` 0.51 (same tree-sitter 0.27 as the workspace)
  behind `colab_rewrite::pattern`. Backends supply grammar, expando char,
  and `@@PATTERN@@` context wrappers; fragments that are not valid files
  compile through the first context whose parse is clean, with the
  deepest node exactly spanning the pattern as the selector.
- Prefilter = longest named-leaf literal of the compiled pattern (sound:
  leaves compare by exact text); `None` for all-metavariable patterns.
- `delete` removes only matches that stand alone on their line(s); nested
  ones stay (removing them leaves broken code). Any pattern rewrite that
  would turn a cleanly-parsing file into one with a syntax error is not
  applied to that file.
- Contexts are refused when the wrapper holds an earlier node of the
  selector's kind (ast-grep selects the first one) — otherwise an
  identifier pattern silently matched the wrapper's own name.
- `colab find` / `colab.find`: read-only, `path:line:col: text`, captures
  at `--detail detailed`, capped with `truncated`, near misses for dead
  rules. Works for **every** module: `Operation::locate` for patterns,
  changed-line fallback (`colab_core::find`) for the rest.
- `colab ast` / `colab.ast`: a pattern's tree as compiled (with its
  context and prefilter) or a snippet's, via `LanguageBackend::syntax_tree`.
- **Deferred:** the compile-time "template re-matches its own pattern"
  warning — rendering a template with synthetic captures and re-parsing
  it in every context proved fiddly; the corpus idempotency check and
  `colab find` on the output cover the same failure for now. Relational
  constraints (`inside` / `has` / `not`) are the natural next step.

### ✅ M19 — LSP in both directions
- **Server (`colab server`):** per-rule diagnostics via the new
  `colab_dsl::check` (each `match` block lowered alone; blocks carry
  byte spans), so pattern and module errors are underlined in place;
  hover for `<lang>::<module>` and for action keywords in context;
  code actions *apply this rule / the whole script to the workspace*
  returning a `WorkspaceEdit` (editor-native preview), resolved lazily.
- **Client (`colab rename`, new `colab-lsp-client` crate):** drives an
  external server's `prepareRename` + `rename` and feeds the
  `WorkspaceEdit` through `refactor`'s report / diff / `--check` /
  `--backup` / `--verify`. Waits for `$/progress` and rust-analyzer's
  `serverStatus` to go quiet (a rename mid-index can be partial); answers
  `workspace/configuration` and friends; retries `ContentModified`;
  negotiates UTF-8 / UTF-16 positions; refuses file operations.
  Verified against rust-analyzer (field vs same-named function) and
  clangd (function vs same-prefixed locals and comments); CI uses a
  deterministic fake server (`examples/fake_lsp.rs`).
- **Deferred:** a DSL form (`<lang>::lsp_rename`) — a server call is
  whole-project, not per-file, so it does not fit `Operation`; an MCP
  `colab.rename` tool — `colab-lsp-client` uses `colab-rewrite` (and so
  `tree-sitter`), which `colab-mcp` must not depend on. Agents can call
  the CLI. Go-to-definition in `.codemod` files (for `include`).

### M20 — Agent install & guardrails
- `colab agent install` detects Claude Code / Cursor / Codex and wires
  skill + MCP server (+ optional hook).
- Post-edit hook: run `colab fix --check` (M14 packs) on files the agent
  just wrote; feed failures back.
- MCP resources + `llms-full.txt` generated from `docs/`, including the
  per-backend caveats.

### M21 — Distribution & CI formats
- npm (`npx colab`) and PyPI (`uvx colab`) wrappers around the release
  binaries, for sandboxes with no Rust toolchain.
- `--format sarif` and `--format github` for `--check` annotations.

### M22 — Workflow-shaped MCP tools
- `colab.rename_symbol`, `colab.move_import`, `colab.set_config_key`:
  each generates the DSL internally and returns a preview, so a first
  call needs no DSL at all.

### M23 — Agent eval suite
- Scripted tasks (repo-wide rename, API migration, config rollout)
  scored on correctness, tool calls, and tokens for colab vs `Edit`/`sed`;
  run in CI to keep output sizes honest.

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

## Next steps: closing the IntelliJ gap (M24–M27)

M17–M19 are broad, scriptable mechanical changes, which IntelliJ does not
do. Measured against IntelliJ's refactoring menu, though, colab has
rename and nothing else:

| IntelliJ | colab today | Closed by |
| -------- | ----------- | --------- |
| Extract method / variable / constant / parameter, Inline, Change Signature, Pull Up / Push Down, Introduce Parameter Object | — | M25 (delegated code actions) |
| Move class / file / package, rename class *and* its file | `colab mv` + file operations in `colab rename`, through git (M24) | ✅ M24; more via M25 code actions |
| Find Usages, Safe Delete | syntactic `colab find` only | M25 |
| Call / type hierarchy | none | unscheduled (`callHierarchy` / `typeHierarchy` on the M25 client) |
| Optimize imports + reformat after every refactoring | hand-written `ensure` / `delete` rules | M25 `--tidy`, M26 |
| SSR constraints (count, regex, type, within) | none | M26 |
| Conflicts dialog before applying | after the fact only (`--verify`) | M27 |
| `//noinspection` suppression | none | M20 |
| Rename in comments, strings, non-code files | none (the server skips them) | M22 |

colab still computes nothing semantic. Each gap is closed either by
asking the language's own server, as M19 does for rename, or with a
syntactic check that cannot be wrong, only incomplete. The non-goals
below stand.

Order: M24 → M25 → M20 → M26 → M27 (with M22), then M14, M21, M22, M23.

### ✅ M24 — Git-aware file operations

Agents work in a git repo or worktree, where a mistake is recoverable.
So a file operation is made through git rather than refused, and it
never loses history.

- **Precondition.** File operations need the run's root inside a git
  work tree: `git rev-parse --is-inside-work-tree`, which is also true in
  a linked worktree. Every git command runs with `-C <root>`. Outside
  git, file operations stay refused, with an error that says why. There
  is no plain-filesystem fallback: it would lose history, and undo with
  it.
- **Clean paths.** Every path an operation touches (both ends of a move,
  and every file whose content changes) must have no uncommitted
  changes, or the run refuses up front and names them. That keeps
  recovery (`git restore`, `git reset`, `colab undo`) from ever throwing
  away the user's own edits. `--allow-dirty` overrides it.
- **Moves and renames.** `git mv <old> <new>`, after creating the
  destination's parent directories. The index records the move at once.
  An untracked file has no history to keep; it is renamed on disk and
  the report says so.
- **History survives content edits.** Git does not store renames. It
  infers them by content similarity (50% by default), so a file that is
  moved *and* heavily rewritten in the same commit shows up as a delete
  plus an add, and `git log --follow` and `blame` stop at the move.
  - colab applies the move first, then the content edits.
  - `--commit-moves` commits the pure moves on their own
    (`colab: move a -> b`) before the edits. `--commit-per-rule` implies
    it.
  - Without it, the report warns for each moved file whose similarity
    would drop below the threshold, and recommends `--commit-moves`.
- **Creates and deletes.** A created file is written and then staged
  with `git add`. A deleted tracked file is removed with `git rm`, and
  its content is kept in the backup snapshot.
- **Reporting.** `FileChange` gains an operation: edit, move
  `{from, to}`, create or delete. `colab_core::render` emits `moved`,
  `created` and `deleted` beside `changed`, and `--check` counts them as
  pending (exit 10). `--format diff` uses git's `rename from` /
  `rename to` headers. There is still one serializer.
- **Undo.** The `--backup` snapshot records the operation list.
  `colab undo` replays the inverses through git (`git mv` back,
  `git rm --cached` on created files) and then restores contents.
  `--verify` reverts the same way.
- **Users.** `colab rename` accepts a server's `RenameFile`, `CreateFile`
  and `DeleteFile` resource operations, which jdtls and kotlin-language-
  server use to rename a class together with its file. The new
  `colab mv <from> <to>` asks the server for the import and reference
  updates (`workspace/willRenameFiles`: rust-analyzer, the TypeScript
  server, jdtls), then makes the move as above. A server that offers no
  `willRenameFiles` still gets the git move, with a warning that
  references were not updated.
- **Invariant change.** "Semantic edits are delegated, never computed"
  stays. "Anything it cannot report and revert as text is refused"
  becomes "anything it cannot report and revert *through git* is
  refused".

- **Shipped as planned**, with these specifics:
  - `colab_core::fileops::{FileOp, ChangeSet}`, with
    `ChangeSet::origin_of` to map a final path back.
  - `colab_lsp_client::edit::to_change_set` replays `documentChanges`
    in order on a virtual copy of the files, and refuses overwrites and
    directory deletes.
  - `colab_lsp_client::will_rename`, and `colab-cli/src/{git,apply}.rs`.
  - Edits are left unstaged after the operations, so the index holds
    exactly the file operations.
  - `--commit-moves` commits with `--only`, so whatever the user had
    staged stays staged.
  - `--commit-per-rule` is unchanged: `refactor` makes no file
    operations.
  - Verified against rust-analyzer (`colab mv src/util.rs
    src/helpers.rs` updates `mod util;` and `util::go()`). CI uses the
    fake server's `file-op` / `no-will-rename` modes on real git repos.

### M25 — Delegated code actions, references, and `--tidy`

M19's client already starts the server, waits out indexing, and turns a
`WorkspaceEdit` into `FileChange`s. A code action is the same pipeline
with a different request.

- **`colab action --at FILE:LINE:COL[-LINE:COL]`**
  - `--list` shows the server's actions there: kind, title, and whether
    each is resolvable.
  - `--kind refactor.extract.function` (or `--title <substring>`) picks
    exactly one; an ambiguous or missing choice is an error that lists
    the candidates.
  - The pick is resolved with `codeAction/resolve` and run through the
    usual report / diff / `--check` / `--backup` / `--verify`.
  - Some actions (gopls, rust-analyzer) return a `command` rather than an
    edit. colab runs `workspace/executeCommand` and takes the server's
    `workspace/applyEdit` request as the result, accepted in this mode
    only and never written behind the report's back.
- **`colab references --at … | --symbol NAME --in FILE`**: locations in
  the `find` shape (shared renderer), so an agent can see a change's
  reach before making it. **`colab safe-delete`** deletes a declaration
  (its range from `textDocument/documentSymbol`) only when nothing
  outside it refers to it, and otherwise lists the references.
- **`--tidy`** on `refactor`, `rename` and `action`: after the edit, run
  the server's `source.organizeImports` code action and
  `textDocument/formatting` on the changed files. A per-language command
  configured in `colab.toml` (`gofmt -w`, `rustfmt`, `prettier --write`)
  takes precedence when set. Tidy edits are reported as their own rule
  (`tidy via <server>`), so attribution stays honest.
- **MCP.** `colab.references` and `colab.action` are registered by the
  binary into the MCP server through an injected tool provider, the same
  composition-root pattern as `colab_backends::registry()`, so
  `colab-mcp` still depends on neither tree-sitter nor
  `colab-lsp-client`. That also unblocks the `colab.rename` tool deferred
  in M19.
- **Server coverage.**
  - rust-analyzer, gopls, jdtls and the TypeScript server offer the
    extract / inline / move / change-signature families.
  - pyright, clangd and sourcekit-lsp offer few code actions; for Python
    the docs should recommend a rope-backed server (pylsp-rope).
  - `docs/features.md` gets a per-server capability table, and the CI
    fake server gains code-action, `executeCommand` and
    `willRenameFiles` modes.

### M26 — Pattern constraints and import-aware templates

- **Constraints**, an optional `where` clause after the pattern:

  ```
  match go::pattern 'errors.Wrap($E, $M)'
      where $M matches /^"/, not inside 'func Test$_($$$) { $$$ }'
      { replace 'fmt.Errorf($M+": %w", $E)' }
  ```

  The forms are `$X matches /re/`, `$X is <node kind>`, `inside` /
  `not inside` / `has '<pattern>'`, and `$$$X count <n>..<m>`. This is a
  grammar change (LALRPOP plus `docs/dsl.md`). Evaluation lives in
  `colab_rewrite::pattern`, on ast-grep-core's matcher combinators, never
  in a backend. A prefilter stays sound because constraints only narrow a
  match. Type constraints (via the server's hover) wait for demand.
- **Templates declare their imports.**
  `replace '…' requires import "fmt"` lowers to the pattern rule plus an
  `ensure` that runs only on files the pattern changed. That needs a
  "fires after rule N" composite `Operation`, kept within the walker's
  existing per-file sequencing. The counterpart,
  `drops import "github.com/pkg/errors" when unused`, needs an optional
  per-backend `import_is_used(source, path)`; backends without it reject
  the clause rather than guess.

### M27 — Pre-apply conflict checks (ships with M22)

Syntactic warnings, raised before writing:

- **`symbol` rename:** the new name already occurs as an identifier in
  the file, so the rename would merge two names.
- **Pattern templates:** the template introduces a qualifier
  (`fmt.`, `os.path.`) the file does not import, and no `requires import`
  covers it. This needs a per-backend `imported_names(source)`.
- **`--verify-lsp`:** compare the server's diagnostics
  (`textDocument/diagnostic`) on the changed files before and after the
  edit, and revert when errors were added. It is a cheaper first check
  than a full build, and `--verify <CMD>` remains for the full one.

Added to existing milestones:

- **M20:** `// colab:ignore <rule-or-script-name>`, on the line or the
  line above, skips that site for find, refactor and check. Rules gain an
  optional `message` and `severity`, so a rule enforced by the post-edit
  hook reads as an inspection rather than a bare diff.
- **M22:** `colab.rename_symbol` combines the server's rename with
  text-occurrence matches in comments, strings and the config backends
  (`toml` / `json` / `markdown`). The text matches are returned as a
  separate, opt-in change set, like IntelliJ's "search in comments and
  strings".
- **M23:** add IntelliJ-shaped tasks ("extract this function", "move
  this type to its own file, history intact") so the gap closing is
  measured, not assumed.

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
