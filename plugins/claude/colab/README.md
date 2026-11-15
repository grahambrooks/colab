# colab plugin for Claude Code

Teaches an agent to use [colab](https://github.com/grahambrooks/colab)
for repo-wide code changes instead of editing matching files one at a
time, and connects colab's MCP server so it can preview and apply
codemods as tool calls.

## What it provides

| Kind | Name | Purpose |
| ---- | ---- | ------- |
| Skill | `colab-codemod` | When a change is mechanical and structural, write a `.codemod` script, check the blast radius, and apply it safely. Includes a namespace/match-string reference and a diagnosis guide. |
| MCP server | `colab` | 20 tools over stdio, and colab's documentation as resources (`colab://docs/dsl.md`, `colab://docs/cli.md`, `colab://docs/features.md`, `colab://llms-full.txt`). Codemods: `colab.preview`, `colab.apply`, `colab.find`, `colab.ast`, `colab.test_rule`, `colab.lint_script`, `colab.list_languages`, `colab.list_rules`, `colab.schema`. No-script workflows: `colab.move_import`, `colab.set_config_key`, `colab.rename_symbol`. Markdown: `colab.outline`, `colab.section`, `colab.edit_markdown`. Language server: `colab.references`, `colab.rename`, `colab.action`, `colab.mv`. One declaration: `colab.edit_symbol`. |
| Hook | post-edit | After `Edit` / `Write` / `MultiEdit`, runs the repository's `colab.toml` packs on the file (`colab hook post-edit`) and hands Claude what they would change. Silent where there is no `colab.toml`, and with a colab older than 2026.11.4. |

The skill triggers on mechanical multi-file changes — an import path, a
dependency rename, a symbol, a Java package, a Go struct tag, a call
site, or a TOML/JSON/Markdown config key.

## Requirements

The `colab` binary must be on `PATH`:

```sh
cargo install --git https://github.com/grahambrooks/colab --locked
colab --version
```

Or with Homebrew:

```sh
brew tap grahambrooks/colab https://github.com/grahambrooks/colab
brew install colab
```

Release archives are on the
[releases page](https://github.com/grahambrooks/colab/releases).

Without the binary the MCP server will fail to connect and the skill
says so in its preflight step rather than improvising with `sed` — but
install colab first and neither happens.

## Install

This repository is a marketplace with this one plugin:

```text
/plugin marketplace add grahambrooks/colab
/plugin install colab@colab
```

The plugin needs colab 2026.11.4 or later on `PATH` for the hook (an
older one leaves the hook silent). To try it from a checkout instead:
`claude --plugin-dir plugins/claude/colab`. To wire one project without
the plugin, or Cursor or Codex: `colab agent install` (and `colab agent
uninstall` to take it out again).

## Layout

```
plugins/claude/colab/
├── .claude-plugin/plugin.json
├── .mcp.json                       # colab mcp, over stdio
├── README.md
└── skills/colab-codemod/
    ├── SKILL.md
    └── references/
        ├── recipes.md              # every namespace, match strings, worked examples
        └── troubleshooting.md      # reading the counters, diagnosing an empty run
```

The manifest deliberately has no `version` field: Claude Code then
tracks the plugin by git commit, so it can never fall behind a release.

The skill is the single source of truth: the repo-local
`.claude/skills/colab-codemod` is a symlink to it, so working inside the
colab repo and consuming the plugin elsewhere load the same text.

`crates/colab-backends/tests/skill_matches_capabilities.rs` asserts that
every `<lang>::<module>` the skill mentions actually exists in the
registry, and that the skill names every backend, so the skill cannot
drift into naming a namespace the binary does not have. It does not
check flags, MCP tool names, exit codes or pack names.
