# Colab (Code Lab)

A scripted, AST-aware code refactoring (codemod) tool. Point it at a
repo, hand it a small `.codemod` script, and it will rewrite source
files deterministically — using tree-sitter for the matching, not
regex — across five languages: **Go, Rust, Java, Python, and
JavaScript/TypeScript**.

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

| Language | Namespaces |
| -------- | ---------- |
| Go     | `import`, `symbol`, `struct_tag`, `call` |
| Rust   | `use`, `symbol`, `crate` (Cargo.toml), `call` |
| Java   | `import`, `package`, `symbol` |
| Python | `import`, `symbol` |
| JS/TS  | `import` (module specifiers), `symbol` |

Actions: `replace`, `delete`, `ensure`, `replace_call` (with
`$1`/`$args`/`$func` template placeholders).

The full capability matrix lives in [`docs/features.md`](docs/features.md).
At runtime, ask the binary directly:

```sh
colab schema           # full JSON capability schema
colab list-languages   # backends registered in this build
colab list-rules go    # modules and actions for one backend
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

The complete DSL is documented in [`docs/dsl.md`](docs/dsl.md).

## Output formats

```sh
colab refactor --script s.codemod --format human .   # default on TTY: write + log
colab refactor --script s.codemod --format diff .    # unified diff to stdout
colab refactor --script s.codemod --format json .    # one JSON object per file
colab refactor --script s.codemod --check .          # exit 10 if changes pending
cat foo.go | colab refactor --script s.codemod --stdin --path foo.go
```

Defaults: `human` on a TTY implies `--write`; everything else
defaults to `--dry-run`. `--check` always overrides. See
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
binary's default registry. See [`ARCHITECTURE.md`](ARCHITECTURE.md)
for the workspace layout and an extension walk-through.

## Documentation

- [`docs/dsl.md`](docs/dsl.md) — codemod script language reference.
- [`docs/features.md`](docs/features.md) — what each backend can do,
  with caveats.
- [`docs/cli.md`](docs/cli.md) — flags, formats, exit codes,
  pipelines.
- [`ARCHITECTURE.md`](ARCHITECTURE.md) — workspace layout and how to
  extend.
- [`DEVELOPMENT_PLAN.md`](DEVELOPMENT_PLAN.md) — roadmap and
  non-goals.

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
  colab-lang-go/      # Go backend
  colab-lang-java/    # Java backend
  colab-lang-js/      # JS/TS backend
  colab-lang-python/  # Python backend
  colab-lang-rust/    # Rust backend
  colab-cli/          # The `colab` binary; LSP stub
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
