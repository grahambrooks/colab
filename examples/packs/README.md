# Codemod packs

A pack is a curated `.codemod` script that bundles related rules under
one name. Packs ship as plain DSL files; run them with
`colab refactor --script <pack>.codemod <target>`.

## Conventions

- One file per migration target, named `<lang>/<from>-to-<to>.codemod`.
- Each pack's `README.md` (next to the file or one directory up) lists
  what the pack **does not** cover — the boundary between mechanical
  syntax rewrites (in scope here) and semantic refactors (out of scope).
- Scripts must be idempotent: running them twice on the same tree must
  produce the same output as running them once. The corpus harness
  enforces this for the canonical examples.

## Available packs

| Path | Status | Summary |
| --- | --- | --- |
| `rust/edition-2021-to-2024.codemod` | placeholder | Extension point for project-specific renames that ship alongside an edition migration. |

Adding a new pack:

1. Add the `.codemod` file under `examples/packs/<lang>/`.
2. Add a corpus case under `tests/corpus/<lang>/<case>/` so the
   pack is exercised by `cargo test --workspace`.
3. Document any rules the pack deliberately leaves out so users (and
   AI agents) know when to stop.
