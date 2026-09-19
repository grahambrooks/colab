# Colab (Code Lab)

A scripted, AST-aware code refactoring (codemod) tool. Point it at a
repo, hand it a small `.codemod` script, and it will rewrite source
files deterministically — using tree-sitter for the matching, not
regex — across twelve languages: **C, C++, C#, Go, Java,
JavaScript/TypeScript, Kotlin, PHP, Python, Ruby, Rust, and Swift**. It
also edits **TOML, JSON and Markdown** configuration by key path, JSON
Pointer or managed block, so a repository convention can be rolled out
the same way (see [`examples/config`](examples/config/repo_migration/)).

```sh
colab refactor --script rename-tokio.codemod --check .
```

## Why

Codebase-wide refactoring lives in an awkward middle ground:

- *sed/grep* is fast and language-agnostic but accidentally rewrites
  string contents and comments.
- IDE refactoring is precise but ties you to one editor and one
  language at a time.
- Hand-rolled scripts work but are write-once.

Colab sits in the middle: a small DSL, syntax-aware matching via
tree-sitter, idempotent operations, and a deterministic CLI that
runs the same way on a developer machine, in CI, and from an AI
agent.

## Capabilities

Twelve languages, each with the same capability floor — an
import-equivalent, a symbol rename, and call rewriting:

| Language | Namespace | Modules |
| -------- | --------- | ------- |
| C          | `c`      | `include`, `symbol`, `call` |
| C++        | `cpp`    | `include`, `namespace`, `symbol`, `call` |
| C#         | `csharp` | `using`, `namespace`, `symbol`, `call` |
| Go         | `go`     | `import`, `package`, `symbol`, `struct_tag`, `call` |
| Java       | `java`   | `import`, `package`, `symbol`, `call` |
| JavaScript / TypeScript | `js` | `import`, `symbol`, `call` |
| Kotlin     | `kotlin` | `import`, `package`, `symbol`, `call` |
| PHP        | `php`    | `use`, `namespace`, `symbol`, `call` |
| Python     | `python` | `import`, `symbol`, `call` |
| Ruby       | `ruby`   | `require`, `symbol`, `call` |
| Rust       | `rust`   | `use`, `symbol`, `crate` (Cargo.toml), `call` |
| Swift      | `swift`  | `import`, `symbol`, `call` |

Actions: `replace`, `delete`, `ensure`, `replace_call` (with
`$1`/`$args`/`$func` template placeholders). The import-equivalent module
takes all three of `replace`/`delete`/`ensure`; `symbol` and the
namespace/package modules take `replace`; `call` takes `replace_call`.

Each backend's module is named for the language's own construct, so a
polyglot script reads naturally — see
[`examples/polyglot`](examples/polyglot/) for one script covering six
languages at once.

The full capability matrix lives in [`docs/features.md`](docs/features.md).
At runtime, ask the binary directly:

```sh
colab schema           # full JSON capability schema
colab list-languages   # backends registered in this build
colab list-rules go    # modules and actions for one backend
colab server           # LSP: diagnostics + completion for .codemod files
colab mcp              # MCP server: preview / apply / schema / list-* / lint_script tools
```

## Quick start

```sh
git clone https://github.com/grahambrooks/colab.git
cd colab
cargo build --release
# binary at target/release/colab
```

Run the bundled Rust dependency rename example:

```sh
target/release/colab refactor \
    --script examples/rust/rename_crate/rename.codemod \
    --format diff \
    examples/rust/rename_crate/
```

You'll see a unified diff against `Cargo.toml` and `src/main.rs`.
Replace `--format diff` with `--write` to apply, or `--check` for a
CI-friendly exit code (10 if changes are pending, 0 otherwise).

## A two-rule script

```
// rename-tokio.codemod
refactor "tokio-major-bump" {
    match rust::crate "tokio" { replace "async_tokio" }
    match rust::use   "tokio" { replace "async_tokio" }
}
```

```sh
colab refactor --script rename-tokio.codemod --write .
```

This rewrites the `[dependencies]` entry in every `Cargo.toml` and
every `use tokio::…` declaration in every `.rs` file under the
current directory. Re-running is a no-op.

A rule can be narrowed to part of the tree with `in "<glob>"`, which
matters most for `symbol` renames since they are syntactic and would
otherwise rewrite an unrelated type that happens to share a name:

```
match rust::symbol "Config" in "crates/core/**" { replace "CoreConfig" }
```

The complete DSL is documented in [`docs/dsl.md`](docs/dsl.md).

## Output formats

```sh
colab refactor --script s.codemod --format human .   # default on TTY: write + log
colab refactor --script s.codemod --format diff .    # unified diff to stdout
colab refactor --script s.codemod --format json .    # one JSON doc: counters + per-rule counts
colab refactor --script s.codemod --check .          # exit 10 if changes pending
cat foo.go | colab refactor --script s.codemod --stdin --path foo.go
```

Defaults: `human` on a TTY implies `--write`; everything else
defaults to `--dry-run`. `--check` always overrides.

`--format json` answers "what would this do?" in a few hundred bytes —
how many files were visited, scanned, and changed, and how many each
rule matched. **A rule reported with `"files": 0` matched nothing** and
is almost certainly wrong; check that before you `--write`. Add
`--detail diff` when you want the hunks too. See
[`docs/cli.md`](docs/cli.md) for the full flag reference and exit
codes.

## How it works

```
script text  →  parse (LALRPOP)            →  raw AST
             →  compile (uses backend registry)  →  Refactoring (Vec<Box<dyn Operation>>)
             →  walk filesystem            →  Operation::apply per file
             →  reporter (human/json/diff) →  stdout / write back
```

The same `Operation` trait is implemented by every backend; the CLI
walks the filesystem, the visitor decides whether to write. Adding
a new language means a new `colab-lang-*` crate and one line in the
binary's default registry. See [`docs/architecture.md`](docs/architecture.md)
for the workspace layout and an extension walk-through.

## Documentation

- [`docs/dsl.md`](docs/dsl.md) — codemod script language reference.
- [`docs/features.md`](docs/features.md) — what each backend can do,
  with caveats.
- [`docs/cli.md`](docs/cli.md) — flags, formats, exit codes,
  pipelines.
- [`docs/architecture.md`](docs/architecture.md) — workspace layout
  and how to extend.
- [`docs/development-plan.md`](docs/development-plan.md) — roadmap
  and non-goals.

### Agent skills

The repo ships [Claude Code](https://claude.com/claude-code) skills in
[`.claude/skills/`](.claude/skills/). They are picked up automatically
when working in this repo:

- **`colab-codemod`** — deciding whether colab fits a change, writing
  the script, checking blast radius before applying, and diagnosing a
  run. Includes per-namespace recipes and troubleshooting.
- **`colab-extend`** — adding a backend, namespace, or action to colab
  itself, with the invariants a change must not break.

To use `colab-codemod` in *other* repos, copy it into your user skills
directory:

```sh
cp -r .claude/skills/colab-codemod ~/.claude/skills/
```

## Development

```sh
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo audit
cargo run -p colab-cli -- refactor --script ... .
```

The repo is a Cargo workspace under `crates/`:

```
crates/
  colab-core/         # Error, walker, CodeTransformer, LanguageBackend, registry
  colab-dsl/          # LALRPOP grammar, AST, compiler, Refactoring IR
  colab-lang-c/       # C backend
  colab-lang-cpp/     # C++ backend
  colab-lang-csharp/  # C# backend
  colab-lang-go/      # Go backend
  colab-lang-java/    # Java backend
  colab-lang-js/      # JS/TS backend
  colab-lang-kotlin/  # Kotlin backend
  colab-lang-php/     # PHP backend
  colab-lang-python/  # Python backend
  colab-lang-ruby/    # Ruby backend
  colab-lang-rust/    # Rust backend
  colab-lang-swift/   # Swift backend
  colab-mcp/          # MCP server (preview / apply / schema / list_rules /
                      # list_languages / lint_script)
  colab-cli/          # The `colab` binary; LSP server; `colab mcp` launcher
```

Test corpus: `tests/corpus/<lang>/<case>/{script,input/,expected/}`.
The harness in `crates/colab-dsl/tests/corpus.rs` walks every case,
asserts the rewrite produces `expected/`, and re-applies the rule to
prove idempotency. Every new backend (or new namespace/action) must
add a corpus case.

## Examples

| Example | What it does |
| ------- | ------------ |
| [`examples/go/imports/`](examples/go/imports/) | Single-rule Go import rename. |
| [`examples/rust/rename_crate/`](examples/rust/rename_crate/) | End-to-end crate rename across `Cargo.toml` and `*.rs`. |
| [`examples/rust/scoped_rename/`](examples/rust/scoped_rename/) | Scoping a symbol rename with `in "<glob>"` so a shared name is only renamed where it should be. |
| [`examples/polyglot/`](examples/polyglot/) | One script rewriting an import and a call site across C, C#, PHP, Ruby, Kotlin, and Swift. |
| [`examples/packs/rust/edition-2021-to-2024.codemod`](examples/packs/rust/edition-2021-to-2024.codemod) | Skeleton for an edition-migration pack. |
| [`examples/packs/java/8-to-21.codemod`](examples/packs/java/8-to-21.codemod) | Skeleton for a Java 8→21 pack. |

## Contributing

PRs welcome. Before opening one:

- `cargo test --workspace` is green.
- `cargo clippy --workspace --all-targets -- -D warnings` is clean.
- New capabilities have at least one corpus case under
  `tests/corpus/<lang>/<case>/`.
- Public surface changes are reflected in `docs/`.

## License

MIT. See [`LICENSE`](LICENSE).
