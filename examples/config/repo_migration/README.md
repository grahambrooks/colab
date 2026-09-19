# Migrate a repository's configuration

The config-file backends — `toml::key`, `json::key` and `markdown::block` —
apply a repository convention the way a source codemod applies a rename:
scoped, previewable, idempotent, and gated in CI with `--check`.

`repo/` stands in for a project: a prek hook file, Claude Code settings, an
`AGENTS.md` with a managed block, and a `myspec.toml` recording which
version of the convention the repository is at. `migration.codemod` moves it
to the next version.

```sh
# Preview. -C makes the scope globs relative to the repository. Name the
# files: a walk of `.` skips hidden directories such as .claude/ (unless
# --no-ignore), but a file named on the command line is always processed.
FILES="prek.toml .claude/settings.json AGENTS.md myspec.toml"
colab refactor -C examples/config/repo_migration/repo \
    --script ../migration.codemod --format diff $FILES

# Would anything change? Exit 10 means yes; wire this into CI.
colab refactor -C examples/config/repo_migration/repo \
    --script ../migration.codemod --check $FILES
```

What to look for in the diff:

- **`prek.toml`** gains a `cargo-clippy` hook after the existing ones. It
  takes the previous hook's place in the list (a new line, same indent);
  its own layout is the value's, written multi-line in the script to match.
  The hand-written comment at the top survives.
- **`.claude/settings.json`** gains one `enabledPlugins` entry; the
  developer's own entries and key order are untouched.
- **`AGENTS.md`** changes only between the `myspec:begin` / `myspec:end`
  markers.
- **`myspec.toml`**: `profile_version` goes from 1 to 2 in place, and the
  trailing comment on `name` is kept.

Run the script a second time and nothing changes: `insert` leaves an
existing hook alone, `set` compares values by meaning, and the block's
content already matches.
