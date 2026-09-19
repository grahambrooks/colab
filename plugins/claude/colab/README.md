# colab plugin for Claude Code

Teaches an agent to use [colab](https://github.com/grahambrooks/colab)
for repo-wide code changes instead of editing matching files one at a
time, and connects colab's MCP server so it can preview and apply
codemods as tool calls.

## What it provides

| Kind | Name | Purpose |
| ---- | ---- | ------- |
| Skill | `colab-codemod` | When a change is mechanical and structural, write a `.codemod` script, check the blast radius, and apply it safely. Includes a namespace/match-string reference and a diagnosis guide. |
| MCP server | `colab` | `colab.preview`, `colab.apply`, `colab.lint_script`, `colab.list_languages`, `colab.list_rules`, `colab.schema` over stdio. |

The skill triggers on mechanical multi-file changes — an import path, a
dependency rename, a symbol, a Java package, a Go struct tag, a call
site, or a TOML/JSON/Markdown config key.

## Requirements

The `colab` binary must be on `PATH`:

```sh
cargo install --git https://github.com/grahambrooks/colab --locked
colab --version
```

Release archives and a Homebrew formula are on the
[releases page](https://github.com/grahambrooks/colab/releases).

Without the binary the MCP server will fail to connect and the skill
says so in its preflight step rather than improvising with `sed` — but
install colab first and neither happens.

## Install

From the marketplace that lists this plugin:

```sh
claude plugin install colab@<marketplace>
```

Or point Claude Code straight at this repository as a marketplace and
install from there.

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

The skill is the single source of truth: the repo-local
`.claude/skills/colab-codemod` is a symlink to it, so working inside the
colab repo and consuming the plugin elsewhere load the same text.

`crates/colab-backends/tests/skill_matches_capabilities.rs` asserts that
every `<lang>::<module>` the skill mentions actually exists in the
registry, so the skill cannot drift into naming a namespace the binary
does not have.
