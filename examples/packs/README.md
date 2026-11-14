# Codemod packs

A pack is a curated `.codemod` script that bundles related rules under
one name. Packs ship as plain DSL files; run them with
`colab refactor --script <pack>.codemod <target>`, or name them in a
repository's `colab.toml` and run `colab fix` (see
[`docs/cli.md`](../../docs/cli.md#colab-fix)).

## Conventions

- One file per migration target, named `<lang>/<from>-to-<to>.codemod`, or for the target alone (`go/1.18-modern`) when there is no single starting point.
- Each pack's `README.md` (next to the file or one directory up) lists
  what the pack **does not** cover — the boundary between mechanical
  syntax rewrites (in scope here) and semantic refactors (out of scope).
- Scripts must be idempotent: running them twice on the same tree must
  produce the same output as running them once. The corpus harness
  enforces this for the canonical examples.

## Available packs

| Path | Status | Summary |
| --- | --- | --- |
| `rust/edition-2021-to-2024.codemod` | ready (mechanical subset) | `unsafe extern`, `#[unsafe(…)]` attributes, `unsafe { set_var / remove_var }`, `gen` → `r#gen`. See [`rust/README.md`](rust/README.md) for what it leaves to `cargo fix --edition`. |
| `go/1.18-modern.codemod` | ready (mechanical subset) | `interface{}` → `any`; `io/ioutil` → `os` / `io`, imports fixed. See [`go/README.md`](go/README.md). |
| `java/8-to-21.codemod` | ready (mechanical subset) | Boxing constructors → `valueOf`; the diamond in typed locals and assignments. See [`java/README.md`](java/README.md). |
| `js/esm.codemod` | ready (top-level forms) | Top-level `require` / `module.exports` / `exports.x` → `import` / `export`. See [`js/README.md`](js/README.md). |
| `python/3.12-typing.codemod` | ready (mechanical subset) | PEP 585 builtins and PEP 604 `X \| None`, wherever they appear. See [`python/README.md`](python/README.md). |

Adding a new pack:

1. Add the `.codemod` file under `examples/packs/<lang>/`.
2. Add a corpus case under `tests/corpus/<lang>/<case>/` so the
   pack is exercised by `cargo test --workspace`.
3. Document any rules the pack deliberately leaves out so users (and
   AI agents) know when to stop.
4. List it in `BUILTIN` in `crates/colab-cli/src/packs.rs`, so
   `colab pack list` shows it and `colab.toml` can name it
   (`pack = "<lang>/<name>"`); a test compiles every built-in pack.
