# Agent eval suite (M23)

Each task is a change an agent is asked to make, with the colab commands
that make it. `cargo test -p colab-cli --test evals` runs every task and
checks three things:

1. **Correct.** The commands run in a temporary git repository holding
   `input/`. Every file in `expected/` must match exactly, every other
   file must be unchanged, files listed as `deleted` must be gone, and a
   moved file must keep its git history.
2. **Cheap enough.** colab's cost is the bytes an agent spends: each
   command, each script it writes, and everything the command prints,
   log included. A task fails when that passes its `budget` (about
   1.5 × today's cost), so output that grows is caught in CI.
3. **Cheaper than editing by hand.** The same change is priced as
   `Read` + `Edit` calls from the `input/` → `expected/` diff: one search
   for the sites, one 25-line read per file, one edit per hunk, a
   `git mv` per moved file (the model in `scripts/price-edits.py`). Over
   all tasks, colab must cost less.

Results: [`results.md`](results.md), rewritten by
`COLAB_BLESS=1 cargo test -p colab-cli --test evals`.

## A task

```
evals/tasks/<name>/
  task.md      the request, as an agent would get it (first line: title)
  meta         budget = <bytes>; requires = <tool>; deleted = <path>
  input/       the files before
  expected/    the files that change, as they must end up
  colab.sh     the colab commands, one per line ($TASK is this directory)
  *.codemod    scripts the commands use, counted as written by the agent
```

A task that `requires` a tool (`rust-analyzer`) is skipped where it is
not installed, and listed as skipped in the results.

## What it does not measure

- **The agent's own reasoning**, and its first attempts. The commands
  here are the ones that work; a real session also pays for finding
  them, as the dogfood log records.
- **A whole session.** Running real agents on these tasks, colab against
  `Edit`, is the next step; this suite keeps the scripted half honest
  and in CI.
- **Work the model gives `Edit` for free.** In `extract-function` the
  model assumes the agent writes the extracted function correctly by
  hand; colab has the language server compute it. On a one-site change
  the costs are about equal (−3%).
