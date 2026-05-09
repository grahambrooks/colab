# Colab documentation

Reference material for using and extending `colab`. For the
high-level overview, start at the project [`README.md`](../README.md).

| Document                                            | What it covers |
|-----------------------------------------------------| -------------- |
| [`dsl.md`](dsl.md)                                  | The codemod script language. Grammar, actions (`replace`/`delete`/`ensure`/`replace_call`), template placeholders, idempotency rules, worked examples. |
| [`features.md`](features.md)                        | What every backend can do. Capability matrix, per-namespace caveats, file-extension routing, explicit non-goals. |
| [`cli.md`](cli.md)                                  | Subcommands, flags, exit codes, `--format` shapes, `--stdin` pipeline, environment variables. |
| [`architecture.md`](architecture.md)                | Workspace layout, runtime data flow, extension steps for new backends and actions. |
| [`development-plan.md`](development-plan.md)        | Roadmap and milestones. Captures non-goals and risks. |

The same capability data is also available at runtime:

```sh
colab schema           # full JSON schema (backends × modules × actions)
colab list-languages   # registered backends only
colab list-rules <lang>
colab explain --script <path>   # parsed IR for one script
```

`colab` ships with `--help`/`-h` on every subcommand; that text is
authoritative for flag names and defaults.
