# Colab Development Plan

A roadmap for evolving colab from a single-purpose Go-import rewriter into a
maintainable, multi-language codemod platform usable by both humans and GenAI
agents.

## Vision

Colab becomes the tool you point at a repo when you want to:

1. Update dependencies *across breaking changes* (renamed packages, moved
   APIs, deprecations) — not just bump versions.
2. Modernise code to the current language version's idioms (Go 1.18 generics,
   Java 21 records/switch, Python 3.12, ES modules, Rust edition migrations).
3. Apply the above repeatably from CI, from a developer shell, or from an AI
   agent — with the same deterministic output everyone can audit.

## Current state (2026-05-09)

- **Languages:** Go only. One operation: `go::import` rename.
- **DSL:** One rule per script (`Refactoring { rule: Rule }`).
- **CLI:** `refactor` and `server` (LSP stub — lifecycle logging only).
- **Output:** In-place writes, human-formatted log lines (ANSI colours).
- **Modularity:** Single binary crate; language backend coupled to `Rule`
  enum.
- **VS Code extension:** Scaffold exists in `codemod/` but no real LSP
  features behind it.

The `Rule` enum + `CodeTransformer` trait are the right seams. The plan below
extends them rather than rewriting.

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

## Sequenced milestones

Roughly two-week increments; each is independently shippable.

### M1 — Refactor for growth (no user-visible change)
- Workspace split (`colab-core`, `colab-dsl`, `colab-lang-go`, `colab-cli`).
- Test corpus harness in place; existing Go tests migrated.
- CI runs clippy, tests, and `cargo-audit`.

### M2 — Multi-rule scripts + idempotency
- Grammar and IR support `Vec<Rule>`.
- Walker applies rules in order, single write per file.
- Idempotency assertion baked into corpus runner.

### M3 — GenAI surface v1
- `--format {human,json,diff}`, `--dry-run`, `--check`, `--write`, `--stdin`.
- Documented exit-code table.
- `colab schema`, `colab explain`, `colab list-languages|list-rules`.

### M4 — Second backend (Rust)
- `colab-lang-rust` with `use` rename and `Cargo.toml` crate rename.
- Edition-migration pack: `rust/edition-2021-to-2024.codemod`.

### M5 — Action vocabulary expansion
- New actions: `delete`, `wrap`, `replace_call`, `ensure_import`.
- Backfill into Go and Rust backends.

### M6 — MCP server + LSP diagnostics
- `colab-mcp` crate exposing `preview` / `apply` / `schema` / `lint_script`.
- LSP gains `.codemod` script diagnostics and namespace completion.

### M7 — Java backend + idiom pack
- `colab-lang-java`.
- `java/8-to-21.codemod` mechanical-subset pack.

### M8 — Python and JS/TS backends
- Driven by demand after the previous milestones land.

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
