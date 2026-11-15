# Security policy

## Supported versions

Fixes go into the latest release only. colab is released often (CalVer,
`vYYYY.M.N`); upgrade to the newest release before reporting.

## Reporting a vulnerability

Please do not open a public issue. Report it privately through GitHub:
**Security → Report a vulnerability** on
[the repository](https://github.com/grahambrooks/colab/security/advisories/new).
Include what an attacker controls (a script, a `colab.toml`, a pack
fetched from git, a repository colab runs over, a language server's
answers), what they gain, and a way to reproduce it.

You can expect an acknowledgement within a week and a fix or a plan
within a month; you will be credited in the advisory unless you ask not
to be.

## What colab trusts

So you can tell a vulnerability from intended behaviour:

- **It runs commands you configure.** `[run] check`, `[formatters]`,
  `--verify`, `--bisect`, `--tidy` and the Renovate preset run shell
  commands from the command line or `colab.toml`. A repository's
  `colab.toml` is trusted like its `Makefile`; do not run `colab fix` in a
  repository you do not trust.
- **It starts language servers** (rust-analyzer, gopls, …) for the
  commands that need one; they run with your permissions.
- **Packs from git** are fetched at a pinned tag or commit and must pass
  their own test cases before they run; a pack is code you choose to run.
- **It writes only inside the paths it is given**, plus its cache
  (`COLAB_CACHE_DIR` or `~/.cache/colab`), temporary worktrees, kept-server
  sockets in the temporary directory, and the agent config files
  `colab agent install` names. File moves and deletions go through `git`.
- **It makes no network requests** of its own, except the `git fetch` /
  `clone` of git packs and fleet repositories you configured, and
  `gh` / `git push` for `colab fleet --pr`.

A way to make colab write outside those places, run a command nobody
configured, or execute code from a file it only edits, is a vulnerability.
