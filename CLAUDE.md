# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

```sh
cargo build --workspace                  # debug build of every crate
cargo build --release --workspace        # release build → target/release/colab
cargo test --workspace                   # run all unit + corpus tests
cargo test -p colab-dsl --test corpus    # run only the corpus harness
cargo test -p colab-cli --test evals     # the agent eval suite (COLAB_BLESS=1 rewrites evals/results.md)
cargo test <name>                        # run a single test (substring match)
cargo clippy --workspace --all-targets -- -D warnings  # CI-equivalent lint gate
cargo run -p colab-cli -- refactor --script <script> [paths...]  # run a codemod
cargo run -p colab-cli -- refactor -e '<match …>' [-e …] [paths...]  # inline rules, no script file
cargo run -p colab-cli -- refactor --script - [paths...]         # script from stdin
cargo run -p colab-cli -- refactor -C <dir> --script <script> .  # cd into <dir> first
cargo run -p colab-cli -- refactor --script <s> --check <paths>  # CI-friendly: exit 10 if changes pending
cargo run -p colab-cli -- refactor --script <s> --format json    # one JSON doc: counters + per-rule match counts + changed paths
cargo run -p colab-cli -- refactor --script <s> --detail diff     # add capped unified diffs to that document
cargo run -p colab-cli -- refactor --script <s> --format diff    # unified diff to stdout
cargo run -p colab-cli -- refactor --script <s> --stdin --path <hint>  # stdin → transformed source on stdout
cargo run -p colab-cli -- refactor --script <s> --tidy --write .     # then format the changed files: colab.toml [formatters], else their LSP
cargo run -p colab-cli -- find -e '<match …>' [paths...]         # read-only: where each rule matches (path:line:col)
cargo run -p colab-cli -- fix [--check|--write]                  # run the packs colab.toml declares, with its [run] settings
cargo run -p colab-cli -- fix --check --format github            # CI annotations (--format sarif: SARIF 2.1.0 for code scanning)
cargo run -p colab-cli -- pack list                              # built-in packs (examples/packs/) + .colab/packs/
cargo run -p colab-cli -- edit --symbol Config/load --in f.rs --replace-body --text-file new.rs  # one declaration by name path (--list: the file's)
cargo run -p colab-cli -- agent install [--claude|--cursor|--codex] [--dry-run]  # wire colab into an agent; `agent uninstall` takes it out
cargo run -p colab-cli -- hook post-edit [--file F]                # the colab.toml packs on one edited file (Claude Code PostToolUse hook)
cargo run -p colab-cli -- fleet fleet.toml [--write [--pr]]            # one migration across many repos (canary first, a worktree each)
cargo run -p colab-cli -- test [--bless]                         # each pack's <pack>.tests/<case>/{input,expected}/ (exit 12 on a failure)
cargo run -p colab-cli -- refactor --script <s> --split-pr .     # one git branch per rule (HEAD + that rule; --stacked chains them), tree untouched
cargo run -p colab-cli -- review run.ndjson [--rule N]           # a `--format ndjson --detail diff` log, by rule, paged
cargo run -p colab-cli -- ast --lang go --pattern '<code>'       # a pattern's (or --stdin snippet's) syntax tree
cargo run -p colab-cli -- outline docs/ --depth 2                # Markdown heading outline: addresses, anchors, line spans
cargo run -p colab-cli -- section docs/cli.md "Exit codes"       # print one Markdown section (heading path or #anchor)
cargo run -p colab-cli -- mv-section a.md "Old" --to b.md --under "Done"  # move a section to another file, links repointed
cargo run -p colab-cli -- rename --at f.rs:2:9 --to new           # scope-aware rename via the language's LSP server (dry-run)
cargo run -p colab-cli -- rename --symbol foo --in f.rs --to bar --include-text  # also comments, strings, docs, config
cargo run -p colab-cli -- mv src/a.rs src/b.rs                   # git mv + the server's reference updates (willRenameFiles)
cargo run -p colab-cli -- action --at f.rs:3:5-3:20 --list       # the server's code actions there (extract, inline, …)
cargo run -p colab-cli -- action --at f.rs:3:5-3:20 --kind refactor.extract --title function --format diff
cargo run -p colab-cli -- references --symbol foo --in f.rs      # scope-aware references
cargo run -p colab-cli -- calls --symbol foo --in f.rs [--outgoing]  # callers (callees) via the call hierarchy
cargo run -p colab-cli -- implementations --symbol Tr --in f.rs  # what implements an interface / trait
cargo run -p colab-cli -- safe-delete --symbol foo --in f.rs     # delete only if unreferenced (--tidy: organize imports + format)
cargo run -p colab-cli -- schema                                 # JSON capability schema
cargo run -p colab-cli -- <cmd> --help-json                      # any command's flags/subcommands as JSON
cargo run -p colab-cli -- list-languages                         # registered backends
cargo run -p colab-cli -- list-rules <lang>                      # modules + actions for one backend
cargo run -p colab-cli -- explain --script <s>                   # parsed IR as JSON, no execution
cargo run -p colab-cli -- links docs/                            # broken Markdown links (exit 10 if any)
cargo run -p colab-cli -- licenses                               # colab's and every shipped crate's license (THIRD-PARTY-LICENSES.txt)
scripts/third-party-licenses.py [--check]                        # regenerate that file (CI: --check) after changing dependencies
cargo run -p colab-cli -- undo --from <dir>                      # restore a run made with --backup <dir>
cargo run -p colab-cli -- lsp-daemon --stop                      # stop every kept language server
cargo run -p colab-cli -- server                                 # LSP for .colab files: diagnostics, hover, completion, code actions, go to include
cargo run -p colab-cli -- mcp                                    # MCP server on stdio (newline-delimited JSON-RPC)
```

Exit codes from `colab refactor` (also documented in `--help`):

| Code | Meaning |
| ---- | ------- |
| 0    | Success — no changes needed, or `--write` succeeded. |
| 1    | Generic / configuration error. |
| 2    | Script parse error. |
| 3    | Unsupported namespace or operation. |
| 4    | I/O error. |
| 10   | `--check` found changes that would be made. |
| 11   | `--write` found conflicts and wrote nothing (`--allow-conflicts` overrides). |
| 12 | `colab test` found a pack case that fails. |

Defaults: `--format human` on a TTY implies `--write`; everything else (`json` / `ndjson` / `diff` / `sarif` / `github`, or non-TTY stdout) defaults to `--dry-run`. `--check` overrides both.

The repo is a Cargo workspace under `crates/`:

| Crate            | Role |
| ---------------- | ---- |
| `colab-core`     | `Error`/`Result` (+ `ParseDetail`), `CodeTransformer`, walker, `LanguageBackend` + `Operation` + `BackendRegistry`, `RunReport` (per-rule attribution), `render` (the shared JSON serializer), `ScopedOperation`, `suggest`. No internal deps. |
| `colab-rewrite`  | Tree-sitter rewriting primitives every backend shares: `apply_edits` (reverse byte order), `line_span`, `delete_lines`, `visit_all`, `rename_nodes_by_text`, `field_texts`, `calls` (the `replace_call` rewrite for the seven backends whose calls are one node kind with `function` / `arguments` fields: C, C++, C#, Go, JS, Python, Rust), and `pattern` — the structural matcher behind every `<lang>::pattern` (wraps `ast-grep-core`; no backend sees an ast-grep type). Also evaluates `where` constraints, and provides `build_rule_where`, the shared body of every source backend's `LanguageBackend::build_rule_where`. Depends on `tree-sitter` + `ast-grep-core` + `colab-core` (for `Constraint` / `RuleSpec` / `Operation`). `colab-core` never depends on it, so `colab-dsl`/`colab-mcp` never inherit a grammar engine. |
| `colab-backends` | The single composition root: `registry()` registers all 16 backends. `colab-cli` depends on it at runtime; `colab-dsl`/`colab-mcp` as a dev-dependency only. Owns the cross-backend contract tests. |
| `colab-dsl`      | LALRPOP grammar, AST, compiler, `Refactoring` IR. Depends only on `colab-core`; no compile-time knowledge of any backend. |
| `colab-lang-*`      | One crate per language, 16 in total: 12 source languages, 3 config formats (`json`, `markdown`, `toml`) and `text`, which reads any file as plain text. Every source backend provides an import-equivalent (`replace`/`delete`/`ensure`), `symbol` (rename), `call` (`replace_call`) and `pattern` (`replace`/`delete`); see the table below and [`docs/features.md`](docs/features.md) for the per-backend extras and caveats. |
| `colab-lsp-client`  | colab as an LSP *client*: spawns rust-analyzer / gopls / clangd / …, waits for indexing, runs `prepareRename` + `rename`, `workspace/willRenameFiles`, and (`actions.rs`) code actions — resolving them or executing their command and capturing the `workspace/applyEdit` it triggers —, references, document symbols, organize-imports and formatting, and turns the `WorkspaceEdit` (file operations included) into a `ChangeSet`. Used by `colab rename`; depends on `colab-core` + `colab-rewrite` only, and is deliberately not a dependency of `colab-mcp`. Tests drive `examples/fake_lsp.rs`. |
| `colab-mcp`         | MCP server: 22 tools (`preview`, `apply`, `find`, `ast`, `test_rule`, `schema`, `list_*`, `lint_script`, the workflow tools, …) over JSON-RPC 2.0 on MCP's stdio transport (newline-delimited JSON; only `colab server` uses Content-Length framing). Depends only on `colab-core` + `colab-dsl`; never pulls in a `colab-lang-*` crate. Workflow tools (`colab.move_import`, `colab.set_config_key`, in `workflows.rs`) generate a one-rule script and return it with the report. Tools that need other crates are injected by the binary through `colab_mcp::ToolProvider`: `colab.references` / `colab.calls` / `colab.implementations` / `colab.rename` / `colab.rename_symbol` / `colab.action` / `colab.mv` / `colab.edit_symbol` (`colab-cli/src/lsp_commands.rs::LspTools`) and `colab.outline` / `colab.section` (`colab-cli/src/markdown.rs`). |
| `colab-cli`         | The `colab` binary. Builds the default `BackendRegistry` (all 16 backends), serves the LSP (`colab server`) with diagnostics, hover, completion and code actions for `.colab` files, and launches the MCP server (`colab mcp`). Also hosts `colab fix` / `colab.toml` (`config.rs`, `packs.rs`), the CI formats (`ci_format.rs`), the LSP-client commands (`lsp_commands.rs`) and the Markdown commands (`markdown.rs`). |

Language backends and the modules each adds beyond the common floor
(`<import-equivalent>` / `symbol` / `call` / `pattern`):

| Crate | Namespace | Import module | Extra modules | Files |
| ----- | --------- | ------------- | ------------- | ----- |
| `colab-lang-c`      | `c`      | `include` | — | `.c`, `.h` |
| `colab-lang-cpp`    | `cpp`    | `include` | `namespace` | `.cpp`/`.cc`/`.cxx`/`.hpp`/`.hh`/`.h` … |
| `colab-lang-csharp` | `csharp` | `using`   | `namespace` | `.cs`, `.csx` |
| `colab-lang-go`     | `go`     | `import`  | `package`, `struct_tag` | `.go` |
| `colab-lang-java`   | `java`   | `import`  | `package` | `.java` |
| `colab-lang-js`     | `js`     | `import`  | — | `.js`/`.mjs`/`.cjs`/`.jsx`/`.ts`/`.tsx` |
| `colab-lang-kotlin` | `kotlin` | `import`  | `package` | `.kt`, `.kts` |
| `colab-lang-php`    | `php`    | `use`     | `namespace` | `.php`, `.phtml` |
| `colab-lang-python` | `python` | `import`  | — | `.py` |
| `colab-lang-ruby`   | `ruby`   | `require` | — | `.rb`, `Rakefile`, … |
| `colab-lang-rust`   | `rust`   | `use`     | `crate` (no `ensure`) | `.rs`, `Cargo.toml` |
| `colab-lang-swift`  | `swift`  | `import`  | — | `.swift` |

Config-file backends edit data, not source. `json` and `toml` have one
module, `key`; `markdown` has seven. Each module's target is an address,
and they take `set` / `insert` / `delete` where they apply, Markdown
adding `replace` (section, item, link, text) and `ensure` (item) (see
[`docs/features.md`](docs/features.md#config-file-backends-colab-lang-json-colab-lang-markdown-colab-lang-toml)):

| Crate | Namespace | Module | Address | Files |
| ----- | --------- | ------ | ------- | ----- |
| `colab-lang-json`     | `json`     | `key`   | JSON Pointer (`/a/b`) | `.json` |
| `colab-lang-markdown` | `markdown` | `section`, `row`, `item`, `text`, `link`, `frontmatter`, `block` | heading path `"Parent > Child"` or `"#anchor"` (`"New < Sibling"` to place a new one); `"Section[Col=value]"` (row; `< [Col=value]` before a row); `"Section[item text]"` (item); `"Section[phrase]"` (text); a link destination; a front-matter key; a managed-block name | `.md`, `.markdown` |
| `colab-lang-toml`     | `toml`     | `key`   | dotted path, `name[field=value]` selects an array element | `.toml` |

`text` (`colab-lang-text`) has no grammar: `text::literal` (every
occurrence) and `text::word` (not inside a longer word) take `replace` /
`delete` on any text file, skipping common binary formats. It is the one
backend whose `is_file_relevant` takes every path; the contract test
`no_backend_claims_a_path_it_cannot_parse` exempts it by name.

`.h` is claimed by both `c` and `cpp` — the extension cannot distinguish
them. Both run over `.h` files in a mixed script; every op is idempotent
so the result is the same, at the cost of one extra parse.

`crates/colab-dsl/build.rs` runs `lalrpop::process_root()` (editing `src/codemod.lalrpop` requires a rebuild — generated `codemod.rs` lives in `OUT_DIR`). `crates/colab-cli/build.rs` shells out to `git rev-parse --short HEAD` to embed a version suffix, so builds must happen inside a git checkout.

## Dogfooding — develop colab with colab

Follow [`docs/dogfooding.md`](docs/dogfooding.md). In short:

- Pin a binary with `scripts/dogfood.sh` and run `.dogfood/colab`, never
  `cargo run`, so a broken working tree cannot break the tool.
- For a mechanical change across several sites, use colab:
  `rust::pattern` via `refactor` / `find`, or `rename` / `mv` /
  `references` / `safe-delete` / `action` through rust-analyzer. Use
  `Edit` only for one-off edits.
- Before writing, preview with `find` or `--format diff`. Apply with
  `--write --bisect 'cargo check -q --workspace --tests'`. `--verify`
  checks after *each* rule, so it rejects a change whose rules only
  compile together.
- Log every case where colab could not express the change, or needed its
  output hand-fixed, in [`docs/dogfood-log.md`](docs/dogfood-log.md). Fix a
  small one in the same commit. Name the colab command in the commit body.

## Architecture

A codemod runs through a four-stage pipeline; see [`docs/architecture.md`](docs/architecture.md) for the diagram and module map.

```
script text  →  colab_dsl::parse        →  colab_dsl::ast (raw AST, possibly many Match blocks)
             →  colab_dsl::compile      →  colab_dsl::Refactoring (Vec<Box<dyn Operation>>)
                  uses BackendRegistry from colab_core
             →  colab_core::walker      →  Operation::apply (e.g. colab_lang_go::imports::ImportRename)
```

Key invariants when changing this code:

- **`CodeTransformer` is the only contract `walker.rs` knows about.** It takes `&str` (not `&String`) and returns the rewritten source. Returning the input unchanged is the signal to skip the write — preserve that. The walker calls `apply_at(path, source)`, which additionally reports which rules fired; `apply(source)` remains the path-blind fallback.
- **`LanguageBackend` is the extension point.** Adding a transform = new `Operation` impl in the appropriate `colab-lang-*` crate + matching arm in that crate's `LanguageBackend::build_rule` + a `registry.register(Box::new(<Backend>))` line in `colab-backends/src/lib.rs::registry` (the *only* registration site — `colab-cli`, the corpus harness, and the MCP tests all call it). Do **not** add backend dependencies to `colab-dsl` or plumb language details into `walker.rs` / `cli.rs` beyond that one registry line.
- **`colab-dsl` must not depend on any `colab-lang-*` crate at runtime.** Use it as a `dev-dependency` for tests only. The CI matrix relies on this so a broken backend cannot block unrelated work.
- **Rules are gated per file, not merely expected to no-op.** `Refactoring::apply_at` skips any rule whose `is_file_relevant` rejects the path, so a Go rule never parses a `.rs` file. `Operation::apply` must still be a no-op for irrelevant input (the path-blind `apply` composes rules unconditionally), but the engine no longer relies on it for cross-language safety.
- **`Operation::prefilter` must be a *necessary* condition, or `None`.** It returns a literal that has to appear in the source for the rule to change anything; when it is absent, the tree-sitter parse is skipped entirely. Returning a literal only promises that its absence implies a no-op — an over-broad literal costs performance, never correctness. **`ensure`-style operations must return `None`**, since they act precisely when the target is absent. A too-narrow prefilter shows up as a corpus idempotency failure.
- **`<lang>::pattern` is one engine, twelve thin adapters.** Matching, rewriting, deletion, prefilter extraction, and tree printing live in `colab_rewrite::pattern`; a backend's `patterns.rs` supplies only its grammar, an expando char (`µ` where `$` cannot start an identifier), and context wrappers containing `@@PATTERN@@`, plus `locate` and `syntax_tree`. Fix matching bugs there, never in a backend. The prefilter is the longest *named-leaf* literal of the compiled pattern (leaves are compared by exact text, so it is sound); a pattern that parses in no context is `Error::UnsupportedOperation` naming every attempt. A match must account for every named child of the node it matches unless a `$$$` absorbs them (`Shape`): ast-grep's default strictness ignores trailing children, which let a rewrite drop an `else`, a third `Union` member or an anonymous class body. Contexts in a backend's `also` list are compiled too and matched alongside, for text that parses as different nodes in different places (Python annotations: `generic_type`, not `subscript`).
- **`colab.toml` drives `colab fix`** (`colab-cli/src/config.rs`): `[run]`, `[formatters]`, `[[packs]]`; unknown keys are errors. Built-in packs are `include_str!`ed from `examples/packs/` in `colab-cli/src/packs.rs::BUILTIN` — adding a pack there needs a corpus case and a README. `fix` compiles every pack, merges them with `Refactoring::combine` (each rule keeps its pack's name via `RuleNote::script`), and runs them through `run_compiled` with the config's settings as `refactor` flags.
- **`where` constraints narrow, never widen.** `colab_core::Constraint` is plain data. `colab_rewrite::pattern` (`Check`) evaluates it as a filter on the match list, before `find`, `rewrite` and `delete`, so the prefilter stays sound and `find` agrees with `refactor`. Every metavariable, node kind, regex and nested pattern is validated at compile time. `pattern` takes every constraint (`any` / `all` groups, sibling and position tests via `Test::Related` / `Nth` / `Field`, which judge a match as the widest node spanning its text, and a template may use only the pattern's own metavariables and `let`s); `symbol` and `call` take only `inside` / `not inside`, a syntactic code scope (`colab_rewrite::scope::Scoped`: the rule runs, then only the changed tokens inside — or outside — the scope patterns' matches are kept); every other module keeps the default `build_rule_where`, which rejects them by name (contract-tested). A `let $N = $F | step …` clause (pattern `replace` only, checked in `compiler::lower_lets`) travels with the constraints as `Test::Let` so no backend signature changes; it narrows nothing, and `colab_rewrite::derive` applies its steps in `Rewriter::render`, after the bindings' nested matches are rewritten. A `pattern <name> = '…'` item names a constraint pattern; a reference is held as `colab_core::constraint::named_pattern` text until `compiler::resolve_names` replaces it (after includes are expanded), so backends only ever see code. A `rewriter <name> = '…' -> '…'` item is resolved there too, into a `let`'s first `rewrite(…)` step (`Transform::Rewrite`); the engine rewrites each of its matches inside the binding's span (`rewrite_with`, every compiled form), and nested `where` patterns likewise match in every form (`Forms`).
- **Import clauses are follow-ups, not rules.** `requires import` lowers to the backend's `import_module()` `ensure`; `drops import … when unused` lowers to `drop_unused_import`, a Go-only operation that errs towards "used". Both are wrapped with the rule in one `colab_core::WithFollowups` operation, which runs them only when the rule changed the file, drops before requires. Attribution stays with the rule, and idempotency holds.
- **DSL surface today: `match <lang>::<module> "<target>" [in "<glob>"] [in fences | and fences] [and prose] [where <constraint>, …] { [let $NEW = $FROM | <step> …] <action> [requires import "<x>"] [drops import "<x>" when unused] [message "<text>"] [severity error|warning|info] [id "<name>"] [url "<link>"] }`, with actions `replace "..."`, `delete`, `ensure`, `replace_call "<tmpl>"`, and — for the config-file backends — `set '<value>'` / `insert '<value>'`.** Strings are double- or single-quoted, or raw (`r#"…"#`, `r##"…"##`, which may hold both quotes), with no escapes; single quotes exist for values that are quoted text themselves. `delete`/`ensure` use the `match_string` as the target. `replace_call` rewrites a matched call expression with a template using placeholders `$1`/`$2`/… (1-indexed positional args), `$args` (full arg list), `$func` (matched function name), `$$` (literal `$`). Templates that *do not rename the function* (e.g. `match go::call "f" { replace_call "f(ctx, $args)" }` — wrap-style) are intentionally non-idempotent and must be applied once; the corpus harness's idempotency check will fail any rule that loops, so verify with `--format diff` and apply with a single `--write`. colab warns about such rules at compile time (`Operation::advice`, `RuleNote::advice` → report warnings, `RuleCheck::advice` → `.colab` editor warnings): a `replace_call` template that still calls the function, and a `pattern` template that is a match of its own pattern.
- **Conflicts are checked before anything is written.** `Operation::conflicts(before, after)` (default: none) is called for each rule that changed a file, with `before` as the rule found it and `after` as the whole script leaves it, so a later rule that adds a missing import clears the conflict. Checks must be syntactic and err towards silence: `symbol` renames onto an existing identifier (`colab_rewrite::rename_conflict`, all 12 backends), and pattern templates that introduce an unimported qualifier (`colab_rewrite::pattern::Qualifiers`, Go, Python and JS: the languages whose imports bind the qualifier; the others stay silent rather than guess). `--verify-lsp` adds the server's new error diagnostics as conflicts, credited to the rule whose own change holds the error's line (or the only rule that changed the file; `rule: None` when neither tells). Every write path (`refactor` normal, `--bisect`, `--verify`, MCP `colab.apply`) buffers its changes and writes nothing while any conflict stands, unless `--allow-conflicts` / `allow_conflicts`; exit 11. Each one also calls `colab_core::merge::prepare_writes` just before writing: a file changed on disk since the run read it is three-way merged (base = as read, ours = as the run leaves it, theirs = disk) when the edits do not overlap, and is a conflict when they do (adjacent lines count, as in git); a conflicting file is never written, not even under `--allow-conflicts`, and backups keep the on-disk text. Rules themselves never merge: each sees the previous one's output. A rule that matched but left matches alone explains itself through `Operation::why_unchanged` (`RuleStat::unchanged`): a dead rule always, a rule that changed other files too (probed on the sampled files holding its prefilter literal, "already as the rule asks" dropped). `why_unchanged` must return `None` for a file the rule would change.
- **Semantic constraints are asked, never computed.** `where $X type "…"` / `$F resolves "…"` / `$T implements "…"` / `$F calls "…"` / `$F called_by "…"` are `Test::Semantic` (the last three walk the server's type and call hierarchies); the pattern engine filters on its other checks first (semantic ones are sorted last), then asks about every surviving match in one batch per check (`semantic::prefetch` → `Oracle::ask_all`, which `LspOracle` pipelines with `Session::request_all`), and `semantic::ask` reads the run's answer cache (keyed by file, text hash, span and question) before asking, and the binary installs `colab_lsp_client::semantic::LspOracle` (`OracleGuard`) only when `Refactoring::needs_oracle`. The current file comes from `semantic::at_path`, set by `apply_at`, `find::locate_all` and the near-miss probe. Any unanswerable question is recorded and `semantic_failure()` fails the run at the end of the walk, before writes; `colab-mcp` refuses such rules. The oracle, a phased run's scan facts and those failures belong to a `colab_core::run::Run`, never a static: the CLI enters one per command, `colab mcp` one per tool call, `colab server` one per walk, and the walker enters the caller's run on each rayon worker. A new run path must enter a run; a new thread or pool that applies rules must enter the caller's (`Run::current`).
- **Semantic edits are delegated, never computed.** `colab rename` / `colab mv` ask the language's own server and report its answer; colab still resolves no names itself. A server still indexing at the timeout is an error, not a partial rename.
- **File operations go through git or not at all.** A server's create / rename / delete becomes a `colab_core::fileops::FileOp` in a `ChangeSet` (replayed in order over a virtual copy of the files by `colab_lsp_client::edit::to_change_set`, so an edit's `before` is always the original content and `ChangeSet::origin_of` maps a final path back). `colab-cli/src/apply.rs` makes them with `git mv` / `add` / `rm` only inside a git work tree, refuses dirty paths unless `--allow-dirty`, writes edits after the operations (unstaged), and logs operations to `--backup` so `colab undo` / `--verify` reverse them. Outside git, the client advertises no `resourceOperations` and any it receives are refused. Never fall back to a plain filesystem move: it loses history and undo.
- **Symbol rename is single-file and syntactic.** `<lang>::symbol "X" { replace "Y" }` rewrites every identifier-kind tree-sitter node whose text equals `X` *within each file processed*. Narrow it by path with `in "<glob>"` (`colab_core::ScopedOperation`) when two crates share a name, or by code with `where inside '<pattern>'` (one function, one `impl`). Both are syntactic: there is no scope analysis (a shadowed local with the same name is also renamed) and no cross-file linkage (a struct moved to a new module would still need the user to update imports — the `<lang>::import` and `rust::crate` operations cover that). Rules act on one file each. M35 relaxed the *reading* half only: a `scan … as <name>` phase collects facts (`colab_core::facts`) from every file for later per-file rules to narrow on (`where $X in <name>`), `then { … }` orders stages, and `Refactoring::run_phased` runs them; every edit is still made by a per-file rule, and nothing resolves names across files. Run paths that cannot make a phased run (`--stdin`, `--bisect`, `--verify`, `--split-pr`, MCP, …) refuse such a script ([`docs/architecture.md`](docs/architecture.md#phased-runs-scan-and-then)).
- **Transforms must be idempotent** on their own output. The corpus harness (`crates/colab-dsl/tests/corpus.rs`) re-applies every script and asserts the second pass is a no-op. When matching syntactic constructs, prefer exact-equality on the relevant tree-sitter node (e.g. an import path) over substring matching — substring matches break composition (see `colab-lang-go/src/imports.rs`).
- **Unknown namespaces must fail loudly, and name the alternatives.** `BackendRegistry::get` returning `None` is mapped to `Error::UnsupportedOperation` in `compiler::lower_match`; never silently fall back to a default transform. Backends reject unknown modules and unsupported actions through `LanguageBackend::unsupported`, which lists the valid names and adds a "did you mean" via `colab_core::suggest` — use it rather than hand-rolling a message, and never format a `RuleSpec` with `{:?}`.
- **A rule that matches nothing is a reportable event, not a silent success.** `RunReport` carries per-rule match counts (`RuleStat`); every output surface warns on `files_matched == 0`. Anything that adds a new way to run rules must keep that attribution intact. Dead rules are also *explained*: an op that targets a value (import path, call name, identifier) implements `Operation::near_miss_target` + `observed_targets`, returning values in the **same normalized form its matcher compares**, and every run path calls `colab_core::near_miss::probe` after the walk so `rules[].near` appears on CLI and MCP alike. `ensure`-style ops keep the defaults.
- **Output is for a machine first.** `colab-core::render` is the single serializer both `colab-cli/src/format.rs` and `colab-mcp` use — compact JSON, unchanged files omitted, diffs capped, and the `visited`/`scanned`/`changed` counters kept distinct so an empty result is self-explaining. Do not add a second, surface-specific shape.
- **Parse through `colab_rewrite::parse_cache::parse`.** Every source backend's `parse` and the pattern engine do, so consecutive rules over an unchanged text share one tree (M10.6; rules still see each other's output, so semantics are untouched). A new parser of a source text should use it too.
- **`llms-full.txt` is generated.** It is `colab_mcp::resources::llms_full()` (README, `docs/dsl.md`, `docs/cli.md`, `docs/features.md`, `docs/operations.md`), and a test fails when it is stale: after editing those, run `COLAB_BLESS=1 cargo test -p colab-mcp`.
- **Never hand-roll a tree walk or an edit application.** `colab-rewrite` owns `apply_edits` (reverse byte order, so earlier offsets stay valid), `delete_lines`, `visit_all`, and `rename_nodes_by_text`. These were duplicated across 12 backends until they were extracted; re-introducing a private copy re-introduces the bug-fixed-in-one-of-twelve problem. `<lang>::symbol` should be a single `rename_nodes_by_text` call plus a `RENAME_KINDS` list.
- **Cross-backend contracts are tested generically** in `colab-backends/tests/contracts.rs`: `ensure` ops must have no prefilter, a prefilter must be derived from the target and its absence must imply a no-op, and every module name must be spellable as a DSL namespace segment (the `c::include` collision). A new backend inherits all of these without anyone remembering to wire them up.
- **Every new backend must add at least one corpus case** under `tests/corpus/<lang>/<case>/{script,input/,expected/}` before it merges. Each case also carries generated `rules.expected` (per-rule attribution + near misses) and `find.expected` (`colab find` locations + bindings); write them with `COLAB_BLESS=1 cargo test -p colab-dsl --test corpus` and review the diff — see `tests/corpus/README.md`.
- **Markdown is addressed by structure** (`colab-lang-markdown`, on `tree-sitter-md`). `outline::headings` is the one source of sections: a section runs to the next heading of the same or higher level (computed, not the grammar's `section` node, which skips setext headings), and an address is a heading path matched on the text a reader sees, or a `#slug` (GitHub rules). `row` and `item` look only in a section's own text (before its first subsection); `link` matches destinations by trailing path segments. An ambiguous address changes nothing and says why (`why_unchanged`); `set` / `insert` / `ensure` create what is missing, so they have no prefilter. Links are checked per run, not per rule: `colab-cli/src/markdown.rs::check_links` compares every Markdown file under the run's paths as it is and as the run would leave it, and each newly broken link is a file-level conflict (`--no-link-check` skips it). `colab outline` / `section` / `links` and MCP `colab.outline` / `colab.section` / `colab.edit_markdown` read and write by the same rules.
- **Config-file backends compare values by meaning.** `set` must be a no-op when an equal value is present however it is formatted, and `set` / `insert` must return `None` from `prefilter` (they act when the target is absent; the contract test enforces it). A file that does not parse is returned unchanged.
- **Scope globs ignore a leading `./`** (`ScopedOperation`), so `colab refactor -C repo … .` and `in "core/**"` agree. A walk of `.` skips hidden directories such as `.claude/`; name such files explicitly.
- **provide examples** for user evaluation
- **update the dsl documentation** docs/dsl.md
- **`colab:ignore` is generic.** `colab_core::ignore::keep_ignored` diffs what a rule did and puts back changes on covered lines, in `Refactoring::apply_at` and `SingleRule`; `find::locate_all` drops covered hits. Backends need nothing for it. A rule's `message` / `severity` live in `Refactoring::notes`; seed reports with `Refactoring::report()` (or `annotate`) so they reach every output.
- **A kept server is a proxy.** By default (`COLAB_KEEP_SERVER=0` turns it off; only the `colab` binary itself defaults on), `Session::start` connects to `colab lsp-daemon` (`colab_lsp_client::daemon`) over a Unix socket instead of spawning the server. The daemon speaks the same protocol, so session code does not change: it caches the first `initialize`, swallows `shutdown` / `exit`, renumbers request ids and document versions per client, closes a client's documents when it goes, and sends `didChangeWatchedFiles` for files changed since. A result from a kept server must equal one from a fresh server (`tests/kept_server.rs`). The socket name includes the colab build, so a new build never reuses an old daemon; a daemon that cannot be reached falls back to a server per command, with a warning. CLI tests pin `COLAB_KEEP_SERVER=0`, since they check a fresh fake server's start-up.

- **Telemetry records, the binary exports.** `colab_core::telemetry` is a dependency-free span API (`root`, `span`, `client_span`, `annotate`), a no-op until the binary installs `colab-cli/src/telemetry.rs`'s OTLP/HTTP-JSON exporter, which only the standard `OTEL_*` variables configure. A root span ends and is exported per CLI command (`cli::run`), per MCP tool call (`handle_call`) and per `colab server` walk; workers join a trace through `telemetry::current()` / `Context::enter` (the walker does). Spans carry names, counts, flag names and outcomes only — never paths, contents, rule text or argument values. In async code, open and close a span on one side of an `.await` (the stack is per thread). Tests set `OTEL_SDK_DISABLED=true` so a developer's collector gets nothing.

## Error handling

All fallible code returns `crate::error::Result<T>`. I/O errors must be wrapped with `Error::io_at(path, source)` so the offending path appears in the message; `Error` deliberately has no `From<io::Error>`, so a bare `?` on an I/O result does not compile (the CLI's `io_to_error` is the explicit path-less case: stdout, stdin, child processes, the working directory). Pick the variant by who must act: `Config` when the request should change, `Server` when a language server failed, `Git` when a git command failed or the work tree is in the wrong state (`Error::kind` names them for MCP). The CLI maps any error into a single `error!` log line plus a non-zero exit code; library code never panics on user input. Reserve `expect`/`panic!` for genuinely unreachable conditions (e.g. loading a built-in tree-sitter grammar) and inside `#[cfg(test)]`.

## Coding standards (from `.github/copilot-instructions.md`)

- Idiomatic Rust and idiomatic module conventions.
- Prefer `Result`-returning APIs over `panic!` / `expect` outside tests.
