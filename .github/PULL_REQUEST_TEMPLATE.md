## What this changes

<!-- One change per pull request: what, and why. -->

## How it was checked

- [ ] `make check` passes (fmt, clippy with `-D warnings`, all tests: what CI runs)
- [ ] A new or changed capability has a corpus case (`tests/corpus/<lang>/<case>/`) or a test beside the code
- [ ] Generated files are re-blessed where they changed (`COLAB_BLESS=1`: corpus, evals, `llms-full.txt`)
- [ ] Docs updated (`docs/dsl.md`, `docs/cli.md`, `docs/features.md`, `docs/operations.md`) and an example added for something new
- [ ] A mechanical change was made with colab, and the command is named in the commit body; anything colab could not do is in `docs/dogfood-log.md`

Labels are added from the paths this touches; see CONTRIBUTING.md.
