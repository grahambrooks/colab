# Dogfood log

What got in the way when using colab to develop colab. See
[`dogfooding.md`](dogfooding.md) for the practice. Newest first.

## Token cost (M27 + M22)

What each mechanical change cost in the agent's context, colab against
the cheapest realistic `Read` + `Edit` route to the same diff. The colab
column is measured: the bytes of every colab command and its output in
the session transcript, probes and failed attempts included. The
`Read` + `Edit` column is modelled by `scripts/price-edits.py` from the
diff colab produced, replayed in a worktree: one grep to find the sites,
one 25-line `Read` per file (Edit refuses an unread file), and one
`Edit` per hunk with an anchor line. Tokens ≈ bytes / 4. Research both
routes need (finding node kinds) is excluded from both.

| Change | Sites | colab calls | colab bytes (~tokens) | Read+Edit bytes (~tokens) | Saved |
| ------ | ----: | ----------: | --------------------: | ------------------------: | ----: |
| `FileChange::from_outcome` | 5 | 5 | 7,343 (~1,835) | 10,260 (~2,565) | 29% |
| `conflicts` field in literals | 7 | 4 | 3,800 (~950) | 11,013 (~2,753) | 66% |
| `PatternOp` forwarders | 12 | 2 | 2,946 (~736) | 18,788 (~4,697) | 85% |
| `SymbolRename::conflicts` | 12 | 2 | 2,938 (~734) | 21,032 (~5,258) | 87% |
| `prose_ranges` (M22) | 12 | 1 | 1,907 (~476) | 18,814 (~4,703) | 90% |
| `run_change_set` callers (M22) | 4 | 2 | 1,805 (~451) | 7,979 (~1,994) | 78% |
| rename `bound_names` (M22, rust-analyzer) | 2 | 2 | 966 (~241) | 3,553 (~888) | 73% |
| **Total** | 54 | 18 | **21,705 (~5,426)** | **91,439 (~22,859)** | **77%** |

- **The saving grows with the number of sites.** colab's cost is
  roughly constant (one script, one summary), while `Read` + `Edit` pays
  per file and per hunk. At 12 alike sites it was 85–90%; at 5 it was 29%.
- **What made the small case expensive was output, not the edit.** The
  `from_outcome` change cost 7.3 KB because it previewed a full
  `--format diff` (2.9 KB), debugged a pattern with `colab ast` (0.9 KB),
  and needed three rules for three spellings (shorthand fields). The M22
  changes asked for `--format json --detail summary` and cost about a
  quarter as much per site.
- **Practice that follows:** generate repetitive scripts from a table,
  preview with `find` (one line per site) rather than a diff, apply with
  `--format json --detail summary`, and let `--bisect` be the check.
- **Markdown (M28 phase 1).** Reading by structure paid off at once: four
  `outline` / `section` calls cost 12.7 KB, against 104 KB to read the
  four files whole. Writing paid off less: the docs script and its
  application cost 12.0 KB against 13.6 KB modelled for `Read` + `Edit`
  (12% less), because `set` re-sends a whole section and three previews
  failed on quoting before raw strings existed. Without the failed
  previews it was 8.2 KB (40% less). Phase 2's `item` and `row` address
  the re-sending.
- **Not counted:** the agent's own reasoning tokens, which favour colab
  further (no per-site Read to digest), and wall time, which favours
  `Edit` (each `--bisect` runs `cargo check`).

| Date | Task | Command | Friction | Outcome |
| ---- | ---- | ------- | -------- | ------- |
| 2026-11-13 | M28: update four docs (two sections set, two inserted) | `refactor` with `markdown::section` rules | Every body quoting a colab rule held both `'` and `"`, which no DSL string could hold. Then the docs *about* raw strings contained `"#`, and the cli.md text quoted `"#anchor"`, each closing an `r#` string. | Fixed: raw strings `r#"…"#` and `r##"…"##` (LALRPOP caps it at two: its generated code quotes regexes in `r###`). |
| 2026-11-13 | same | `refactor --check` on the applied script | A satisfied `set` / `insert` reported "matched no files", as if the rule were dead. | Fixed for the markdown modules (`unchanged: already as the rule asks`). Open for `toml::key` / `json::key` / `markdown::block`. |
| 2026-11-13 | same | `markdown::section … { set }` | A small change inside a long section means sending the whole section back; bullets, table rows (these log rows, the plan's follow-ups) and the EBNF line in `dsl.md` were hand-edited. | Open: phase 2 (`item`, `row`). |
| 2026-11-13 | M28: read four docs (104 KB) to find what to change | `outline`, `section` | None. 12.7 KB in four calls, 88% less than reading the files whole. | Done by colab. |
| 2026-11-13 | M22: `prose_ranges` in 12 backends, each with its own node kinds | `refactor --bisect`, script generated from a table, `--format json --detail summary` | None; first time. Generating the 12 scoped rules from a table, and asking only for the summary, kept both the script and its output out of the conversation. | Done by colab. |
| 2026-11-13 | M22: `run_change_set(&label, …)` → `vec![…]` at 4 call sites | `refactor --bisect 'cargo check'` with one inline `rust::pattern` | My hand edit of the definition was incomplete (the body still used `label`); `--bisect` failed the build and reverted the call-site rewrite. Rerun after the fix passed. | Worked as designed: the check caught my mistake, not colab's. |
| 2026-11-13 | M22: rename `bound_names` → `imported_names` | `rename` (rust-analyzer) | None. | Done by colab. |
| 2026-11-13 | M22: probe `rename --include-text` on `rename_conflict` | `rename --include-text` | Found no text occurrences although CLAUDE.md names it: the server root is the nearest project marker, here one crate, so docs at the top were never searched. | Fixed: text occurrences are searched across the git work tree. |
| 2026-11-13 | re-pin after a commit | `scripts/dogfood.sh` | The pinned binary reported the previous commit: `build.rs` embedded `git rev-parse HEAD` but never asked to re-run when HEAD moved. | Fixed: `rerun-if-changed` on `HEAD`, the branch ref and `packed-refs`. |
| 2026-11-13 | M27: add a `conflicts` field to 12 `FileChange` literals | `refactor` with `rust::pattern 'FileChange { $$$F }'` | The template `{ $$$F, conflicts: … }` doubled the trailing comma (`$$$F` keeps it), so every rewrite failed to parse and was skipped. The run said only "matched no files", although `find` listed 12 matches. | Fixed: `Operation::why_unchanged`; the dead rule now reports `unchanged: the rewritten code would not parse…`. |
| 2026-11-13 | same | same | One literal sat inside `vec![…]`. A macro body is a token tree to tree-sitter, so no pattern matches inside it. | Open (by design of the grammar). Hand-edited; worth a note in the pattern docs. |
| 2026-11-13 | same | `where not has 'conflicts: $C'` | A nested pattern cannot be a struct-field fragment: Rust has no context in which `conflicts: $C` parses. | Open. Candidate: a struct-literal context in `rust::pattern`. |
| 2026-11-13 | M27: `FileChange::from_outcome` at 5 sites | `refactor`, 3 `rust::pattern` rules | `before: $B` does not match the shorthand field `before,`, so one rule per spelling was needed. | Open. Candidate: let `field: $X` match shorthand `field`, as IntelliJ SSR does. |
| 2026-11-13 | M27: forward `conflicts`/`why_unchanged` in 12 `PatternOp`s; `conflicts` in 12 `SymbolRename`s | `refactor --write --bisect` | None. Both first time. | Done by colab. |
| 2026-11-13 | `--verify` help | — | (open item above) | Fixed: `--verify`'s help names `--bisect` for rules that only build together. |
| 2026-11-13 | M26: a 4th `PatternRule::compile` parameter, threaded through 12 backends (5 rules, 49 sites); then `import_module` + `build_rule_where` in 12 backends (5 scoped rules) | `refactor --write --script` | Both jobs worked first time. The only hand-edit was `cargo fmt`, for argument lists the template had joined onto one line. | Done by colab. `--tidy` on `refactor` (M25 deferral) would have covered the formatting. |
| 2026-11-13 | same | `refactor --write --verify 'cargo check …'` | `--verify` checks after *each* rule, so a coordinated change (new parameter plus its callers) always fails at rule 1, and the run is reverted. The flag I wanted was `--bisect`. | Docs fixed (`dogfooding.md`, CLAUDE.md); `--verify` help fixed in M27. Open: suggest `--bisect` when rule 1 fails. |
| 2026-11-13 | M26 example | `inside 'func $F(t *testing.T) { $$$ }'` | A bare `$$$` failed to compile in every language whose expando is not `$`, because `expand` left it as `$$$`. | Fixed in `colab_rewrite::pattern::expand`, with a test. |
| 2026-11-13 | M26: add `import_module` to 12 backends | `refactor` with a `rust::pattern` heredoc | A template that needs both `'` (a lifetime) and `"` (a string) cannot be written, because DSL strings have no escapes. Workaround: capture `$RET` and reuse it. | Open. Candidate: raw strings (`r#'…'#`) in the DSL grammar. |
| 2026-11-13 | same | same | A pattern cannot contain a lifetime such as `&'static str` inside single quotes. | Workaround documented (capture the type). Fixed by the same raw-string item. |
| 2026-11-13 | same | same | The blank line in a multi-line template came out indented, leaving trailing whitespace. | Fixed: `colab_rewrite::pattern::strip_blank_line_indent`. |
| 2026-11-13 | same | `colab-rewrite` unit test | With tab-indented source (Go), the template's later lines are not re-indented to the match's column. | Open. Investigate the ast-grep `TemplateFix` indentation for tabs. |
| 2026-11-13 | survey `syntax_tree` impls | `find -e … crates` | Each scanned file wrote an `[INFO] Processing …` line to stderr: 124 log lines around 14 results. `RUST_LOG` was ignored. | Fixed: per-file line demoted to debug, and `RUST_LOG` honoured. |
