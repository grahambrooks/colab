# Dogfood log

What got in the way when using colab to develop colab. See
[`dogfooding.md`](dogfooding.md) for the practice. Newest first.

| Date | Task | Command | Friction | Outcome |
| ---- | ---- | ------- | -------- | ------- |
| 2026-11-13 | M26: a 4th `PatternRule::compile` parameter, threaded through 12 backends (5 rules, 49 sites); then `import_module` + `build_rule_where` in 12 backends (5 scoped rules) | `refactor --write --script` | Both jobs worked first time. The only hand-edit was `cargo fmt`, for argument lists the template had joined onto one line. | Done by colab. `--tidy` on `refactor` (M25 deferral) would have covered the formatting. |
| 2026-11-13 | same | `refactor --write --verify 'cargo check …'` | `--verify` checks after *each* rule, so a coordinated change (new parameter plus its callers) always fails at rule 1, and the run is reverted. The flag I wanted was `--bisect`. | Docs fixed (`dogfooding.md`, CLAUDE.md). Open: name the difference in `--verify`'s help, and suggest `--bisect` when rule 1 fails. |
| 2026-11-13 | M26 example | `inside 'func $F(t *testing.T) { $$$ }'` | A bare `$$$` failed to compile in every language whose expando is not `$`, because `expand` left it as `$$$`. | Fixed in `colab_rewrite::pattern::expand`, with a test. |
| 2026-11-13 | M26: add `import_module` to 12 backends | `refactor` with a `rust::pattern` heredoc | A template that needs both `'` (a lifetime) and `"` (a string) cannot be written, because DSL strings have no escapes. Workaround: capture `$RET` and reuse it. | Open. Candidate: raw strings (`r#'…'#`) in the DSL grammar. |
| 2026-11-13 | same | same | A pattern cannot contain a lifetime such as `&'static str` inside single quotes. | Workaround documented (capture the type). Fixed by the same raw-string item. |
| 2026-11-13 | same | same | The blank line in a multi-line template came out indented, leaving trailing whitespace. | Fixed: `colab_rewrite::pattern::strip_blank_line_indent`. |
| 2026-11-13 | same | `colab-rewrite` unit test | With tab-indented source (Go), the template's later lines are not re-indented to the match's column. | Open. Investigate the ast-grep `TemplateFix` indentation for tabs. |
| 2026-11-13 | survey `syntax_tree` impls | `find -e … crates` | Each scanned file wrote an `[INFO] Processing …` line to stderr: 124 log lines around 14 results. `RUST_LOG` was ignored. | Fixed: per-file line demoted to debug, and `RUST_LOG` honoured. |
