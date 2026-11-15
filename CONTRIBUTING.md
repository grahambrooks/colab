# Contributing to colab

Thanks for helping. colab is a codemod tool whose promise is that its edits
are exact, idempotent and explained, so most of what follows is about
keeping that promise when you change it.

## Before you start

- **A bug**: open an issue with the [bug form](https://github.com/grahambrooks/colab/issues/new?template=bug_report.yml):
  the script, a small input, what colab did and what you expected. A
  `colab ... --format json` of the run usually says the most.
- **A change colab cannot express** (a language construct, a kind of edit):
  the [gap form](https://github.com/grahambrooks/colab/issues/new?template=gap.yml).
  These are the most useful issues: they shape the roadmap
  ([`docs/development-plan.md`](docs/development-plan.md)).
- **A feature**: the [feature form](https://github.com/grahambrooks/colab/issues/new?template=feature_request.yml),
  or a pull request for something small.
- **A vulnerability**: not in an issue; see [`SECURITY.md`](SECURITY.md).

Everyone taking part follows the [code of conduct](CODE_OF_CONDUCT.md).

## Set up

```sh
git clone https://github.com/grahambrooks/colab.git && cd colab
cargo build --workspace          # needs the toolchain in rust-toolchain.toml
make hooks                       # optional: prek pre-commit and pre-push hooks
make check                       # the pre-push gate: exactly what CI runs
```

The build must happen in a git checkout: `crates/colab-cli/build.rs`
stamps the commit into `colab --version`. `make check` is `cargo fmt
--check`, `cargo clippy --workspace --all-targets --all-features --locked
-- -D warnings` and `cargo test --workspace --all-features --locked`, with
`RUSTFLAGS=-D warnings`, as CI. CI also builds on Windows and runs
`colab test` and `colab fix --check` on this repository.

Language servers (rust-analyzer, gopls, …) are not needed to build or
test: the tests drive a fake one (`crates/colab-lsp-client/examples/fake_lsp.rs`).

## Where things are

[`docs/architecture.md`](docs/architecture.md) has the workspace layout,
the pipeline (`parse → compile → walk → apply`) and how to add a backend
or a module. [`CLAUDE.md`](CLAUDE.md) lists the invariants a change must
not break, with the reason for each; read it before changing the engine.
The ones most often at stake:

- A rule only edits files its backend is relevant to, and its
  `prefilter` must be a *necessary* condition (or `None`).
- Every transform is idempotent on its own output.
- A rule that matches nothing is reported, never silent.
- A run writes nothing while any rule reports a conflict.
- `colab-dsl` and `colab-mcp` never depend on a `colab-lang-*` crate.

## Tests that come with a change

| You changed | Add |
| ----------- | --- |
| A backend, module or action | a corpus case: `tests/corpus/<lang>/<case>/{script,input/,expected/}` (the harness also re-runs it for idempotency) |
| The pattern engine, `where`, the DSL | unit tests beside the code, and a corpus case that shows it |
| A command or flag | a CLI test in `crates/colab-cli/tests/cli.rs` |
| Agent-facing output | the eval suite still passes its byte budgets (`cargo test -p colab-cli --test evals`) |

Generated expectations are rewritten with `COLAB_BLESS=1`; review the
diff before committing it:

```sh
COLAB_BLESS=1 cargo test -p colab-dsl --test corpus   # rules.expected, find.expected
COLAB_BLESS=1 cargo test -p colab-cli --test evals    # evals/results.md
COLAB_BLESS=1 cargo test -p colab-mcp                 # llms-full.txt, after editing README / docs
colab test --bless                                    # a pack's <pack>.tests/
```

## Documentation

A change people can see updates its docs in the same pull request:
[`docs/dsl.md`](docs/dsl.md) for the language, [`docs/cli.md`](docs/cli.md)
for commands and flags, [`docs/features.md`](docs/features.md) for what a
backend can and cannot do, [`docs/operations.md`](docs/operations.md) for
installing and running. `llms-full.txt` is generated from these; a test
fails when it is stale. New capabilities get a runnable example under
`examples/`.

## Develop colab with colab

Mechanical changes to this repository are made with colab itself, from a
pinned binary, so a broken working tree cannot break the tool
([`docs/dogfooding.md`](docs/dogfooding.md)):

```sh
scripts/dogfood.sh                       # pin .dogfood/colab
.dogfood/colab find -e '<rule>' crates   # where it applies
.dogfood/colab refactor --script s.codemod --write --bisect 'cargo check -q --workspace --tests' crates
```

Docs are edited the same way, with `markdown::` rules. Where colab could
not express a change, or its output needed fixing by hand, add a row to
[`docs/dogfood-log.md`](docs/dogfood-log.md); that log is how gaps are
found.

## Commits and pull requests

- Commit messages follow Conventional Commits: `feat:`, `fix:`, `docs:`,
  `perf:`, `test:`, `build:`, `refactor:`, with a scope where it helps
  (`feat(dsl):`). The body says why, and names the colab command used for
  a mechanical change.
- Keep a pull request to one change; the template asks what it covers.
- Labels are applied for you: issue forms label new issues, and pull
  requests are labelled by the paths they touch (`.github/labeler.yml`).
  The label set is defined in [`.github/labels.json`](.github/labels.json)
  and kept in sync by a workflow; add one there, not in the UI.

## Releases

Releases are cut by the maintainer: a CalVer tag (`vYYYY.M.N`) runs
`.github/workflows/release.yml`, which builds the archives, publishes
`SHA256SUMS` and updates the Homebrew formula. `scripts/release.py` and
`release.yml` are shared across projects (release-kit) and are not edited
here; repository settings live in `.release.env`.

## License

By contributing you agree that your contributions are licensed under the
[MIT License](LICENSE).
