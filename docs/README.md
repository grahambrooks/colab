# Colab documentation

Reference material for using and extending `colab`. For the
high-level overview, start at the project [`README.md`](../README.md).

| Document                                            | What it covers |
|-----------------------------------------------------| -------------- |
| [`dsl.md`](dsl.md)                                  | The codemod script language. Grammar, actions (`replace`/`delete`/`ensure`/`replace_call`/`set`/`insert`), structural patterns and `where`, import and finding clauses, template placeholders, idempotency rules, worked examples. |
| [`features.md`](features.md)                        | What every backend can do. Capability matrix, per-namespace caveats, file-extension routing, explicit non-goals. |
| [`cli.md`](cli.md)                                  | Every subcommand and flag, `colab fix` and `colab.toml`, exit codes, `--format` shapes (including SARIF and GitHub annotations), `--stdin` pipeline, the MCP server, environment variables. |
| [`architecture.md`](architecture.md)                | Workspace layout, runtime data flow, extension steps for new backends and actions. |
| [`development-plan.md`](development-plan.md)        | What is planned: the order of work, open milestones, follow-ups, non-goals and risks. |
| [`completed-milestones.md`](completed-milestones.md) | What has shipped, milestone by milestone, with what changed from the plan and what was deferred. |
| [`dogfooding.md`](dogfooding.md)                    | Developing colab with colab: the pinned binary, which changes to make with colab, how to apply and what to log. |
| [`dogfood-log.md`](dogfood-log.md)                  | Every time colab could not express a change, or got one wrong, and what was done about it. |

The same capability data is also available at runtime:

```sh
colab schema           # full JSON schema (backends × modules × actions)
colab list-languages   # registered backends only
colab list-rules <lang>
colab explain --script <path>   # parsed IR for one script
```

`colab` ships with `--help`/`-h` on every subcommand; that text is
authoritative for flag names and defaults.
