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

4. **Faster.** Each way is also timed as an agent would spend it. An
   agent pays a round trip per tool call, writes slowly (output tokens
   come one at a time) and reads quickly, so time is modelled as calls ×
   2 s + written tokens ÷ 60/s + read tokens ÷ 2000/s, plus colab's
   *measured* run time, a language server's start-up included. The
   parameters are constants in `crates/colab-cli/tests/evals.rs`, and the
   results print calls and bytes so a reader can redo the sums with their
   own. Time is reported, not asserted, since run times vary by machine.

Faster change is the business case as much as tokens: fewer turns per
change, results correct the first time (every task is checked exactly),
and one reviewable, attributed diff instead of a sequence of edits.

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
- **Review and rework.** A smaller, attributed diff is quicker to review,
  and a change checked before it is written (conflicts, links,
  `--verify-lsp`) is not reworked after; neither is timed here.
- **Work the model gives `Edit` for free.** In `extract-function` the
  model assumes the agent writes the extracted function correctly by
  hand; colab has the language server compute it. On a one-site change
  the bytes are about equal (−3%), and colab is slower (0.6×): each of its
  three commands starts rust-analyzer afresh, 10 s in all. A long-lived
  server session would remove most of that.
