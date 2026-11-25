//! `colab hook post-edit`: the repository's packs, run on the file an
//! agent just wrote (M20).
//!
//! Wired as a Claude Code `PostToolUse` hook on `Edit|Write|MultiEdit`
//! (the plugin's `hooks/hooks.json`, or `colab agent install`). The hook
//! reads the tool call as JSON on stdin, takes `tool_input.file_path`, and
//! runs the packs the nearest `colab.toml` declares over that one file, in
//! memory. What a rule above `severity info` would change is answered as
//! `{"decision": "block", "reason": "…"}` on stdout, which Claude Code
//! hands the model after the edit (a post-edit hook cannot undo the edit;
//! it says what to fix), with exit 0 — so the hook command can be
//! `colab hook post-edit 2>/dev/null || true`, and a colab too old to have
//! the command stays silent instead of erroring at the model. Nothing to
//! say, no `colab.toml`, a file outside its `[run] paths`, or packs with
//! scan phases (they need the whole tree): no output. With `--file` the
//! report is plain text and the exit code 10, as for `--check`.
//!
//! When the packs say nothing about an `Edit`, the hook generalizes its
//! `old_string` → `new_string` into a rule and, if that rule matches
//! elsewhere in the repository, says so as `additionalContext` with the
//! rule ready for `colab.apply` ([`suggest_generalize`]).
//!
//! `colab hook pre-bash` is the `PreToolUse` hook on `Bash`: a command
//! that edits tracked files through the shell is answered with a `deny`
//! naming them (see [`crate::shell_edit`]).
use std::fmt::Write as _;

use std::io::{self, Read};
use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};
use colab_core::walker::FileChange;
use colab_core::{BackendRegistry, CodeTransformer, Error, Result, Severity};
use serde_json::Value;

#[derive(Parser, Debug)]
pub struct HookArgs {
    #[command(subcommand)]
    event: HookEvent,
}

#[derive(Subcommand, Debug)]
enum HookEvent {
    /// After an agent edits a file: the repository's packs on it. Reads
    /// the hook's JSON on stdin (`tool_input.file_path`) and answers
    /// `{"decision": "block", "reason": …}` when a rule would change it.
    PostEdit {
        /// Check this file instead, reporting as text (exit 10 on findings).
        #[arg(long, value_name = "FILE")]
        file: Option<PathBuf>,
    },
    /// Before an agent runs a shell command: refuse one that edits
    /// tracked files (`sed -i`, `perl -pi`, `>`, `tee`, a Python script
    /// that writes), pointing at Edit and colab instead. Reads the hook's
    /// JSON on stdin (`tool_input.command`, `cwd`) and answers with a
    /// `PreToolUse` `deny`.
    PreBash {
        /// Judge this command instead, run from the working directory,
        /// reporting as text (exit 10 when it would be refused).
        #[arg(long, value_name = "COMMAND")]
        command: Option<String>,
    },
}

pub fn run(args: HookArgs, backends: &BackendRegistry) -> Result<u8> {
    let file = match args.event {
        HookEvent::PostEdit { file } => file,
        HookEvent::PreBash { command } => return pre_bash(command),
    };
    if let Some(file) = file {
        return match check(&file, backends)? {
            Some(report) => {
                print!("{report}");
                Ok(10)
            }
            None => Ok(0),
        };
    }
    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .map_err(crate::cli::io_to_error)?;
    let Some(file) = edited_file(&input) else {
        return Ok(0);
    };
    if let Some(report) = check(&file, backends)? {
        println!(
            "{}",
            serde_json::json!({"decision": "block", "reason": report})
        );
        return Ok(0);
    }
    if let Some(note) = suggest_generalize(&input, backends) {
        println!(
            "{}",
            serde_json::json!({"hookSpecificOutput": {
                "hookEventName": "PostToolUse",
                "additionalContext": note,
            }})
        );
    }
    Ok(0)
}

/// The most matches the suggestion looks for: enough to say "many".
const SUGGEST_LIMIT: usize = 50;

/// After an Edit: when the change, generalized into a rule
/// (`LanguageBackend::generalize`), matches elsewhere in the repository,
/// a note telling the agent so, with the rule ready for `colab.apply`.
/// `None` (silence) for anything else: another tool, a file no source
/// backend reads, an edit that does not generalize, no other match, or
/// `COLAB_HOOK_SUGGEST=0`.
pub fn suggest_generalize(input: &str, backends: &BackendRegistry) -> Option<String> {
    if std::env::var("COLAB_HOOK_SUGGEST")
        .is_ok_and(|v| matches!(v.as_str(), "0" | "false" | "off"))
    {
        return None;
    }
    let call: Value = serde_json::from_str(input).ok()?;
    // An Edit's one change, or each of a MultiEdit's.
    let pair = |edit: &Value| -> Option<(String, String)> {
        let before = edit.get("old_string")?.as_str()?;
        let after = edit.get("new_string")?.as_str()?;
        // A small change: a whole-file rewrite is not a pattern.
        (!before.trim().is_empty() && before.len() <= 2_000 && before.lines().count() <= 30)
            .then(|| (before.to_string(), after.to_string()))
    };
    let input_of = call.get("tool_input")?;
    let edits: Vec<(String, String)> = match call.get("tool_name").and_then(Value::as_str) {
        Some("Edit") => pair(input_of).into_iter().collect(),
        Some("MultiEdit") => input_of
            .get("edits")?
            .as_array()?
            .iter()
            .filter_map(pair)
            .collect(),
        _ => return None,
    };
    if edits.is_empty() {
        return None;
    }
    let file = edited_file(input)?;
    let lang = source_language(&file, backends)?;
    let backend = backends.get(&lang)?;
    let learned: Vec<colab_core::Generalized> = edits
        .iter()
        .filter_map(|(before, after)| backend.generalize(before, after).ok())
        .flatten()
        .collect();
    let root = crate::git::Repo::discover(file.parent()?)
        .map(|repo| repo.top().to_path_buf())
        .or_else(|| file.parent().map(Path::to_path_buf))?;
    // Only a rule anchored on a name is worth a word: `$A + 1` (from
    // `x + 1` → `x + 2`) matches every such sum and is noise.
    let anchored = |g: &colab_core::Generalized| {
        let script = format!(
            "match {lang}::pattern {} {{ delete }}",
            colab_dsl::ast::quote_double(&g.pattern)
        );
        colab_dsl::compile(&script, backends).ok().is_some_and(|r| {
            r.rules
                .first()
                .and_then(|op| op.prefilter())
                .is_some_and(|literal| {
                    literal.chars().count() >= 3 && literal.chars().any(char::is_alphabetic)
                })
        })
    };
    let learned: Vec<colab_core::Generalized> = learned
        .into_iter()
        .filter(anchored)
        .filter(|g| worth_suggesting(g, backend))
        .collect();
    if learned.is_empty() {
        return None;
    }
    let script: String = learned
        .iter()
        .map(|g| {
            format!(
                "match {lang}::pattern {} {{ delete }}\n",
                colab_dsl::ast::quote_double(&g.pattern)
            )
        })
        .collect();
    let refactoring = colab_dsl::compile(&script, backends).ok()?;
    let selector = colab_core::find::Selector::new(&refactoring.rules, &refactoring.name);
    let source = colab_dsl::pipeline::Source::Walk {
        targets: vec![root.clone()],
        options: colab_core::walker::WalkOptions::default(),
    };
    let mut hits: Vec<String> = Vec::new();
    let _ = source.visit(&selector, &mut |change: FileChange| {
        for (_, located) in selector.take(&change.path) {
            let rel = change.path.strip_prefix(&root).unwrap_or(&change.path);
            hits.push(format!("{}:{}", rel.display(), located.line));
        }
        if hits.len() >= SUGGEST_LIMIT {
            // Enough to say so; stop the walk.
            return Err(Error::Config("enough".into()));
        }
        Ok(())
    });
    if hits.is_empty() {
        return None;
    }
    let rules: Vec<Value> = learned
        .iter()
        .map(|g| serde_json::json!({"lang": lang, "match": g.pattern, "replace": g.template}))
        .collect();
    let rules = Value::Array(rules);
    let more = if hits.len() >= SUGGEST_LIMIT { "+" } else { "" };
    let mut note = format!(
        "colab: the edit you just made, as a rule, matches {}{more} more place(s) in this repository ({}{}). \
         To make the same change there, check the matches with colab.find, then call colab.apply \
         {{\"rules\": {rules}, \"paths\": [\".\"], \"cwd\": \"{}\"}} (or colab.generalize to adjust it). \
         Ignore this if the edit was meant for this place only.",
        hits.len(),
        hits.iter().take(3).cloned().collect::<Vec<_>>().join(", "),
        if hits.len() > 3 { ", …" } else { "" },
        root.display()
    );
    for warning in learned.iter().flat_map(|g| &g.warnings) {
        let _ = write!(note, " Note: {warning}");
    }
    Some(note)
}

/// Whether a rule learned from one edit is a change worth making
/// elsewhere, judged on a replay of 985 real edits: not one that matches
/// its own output (`$A.conflicts` → `$A.conflicts()` would also find, and
/// break, every call already migrated), not a one-off that replaces general
/// code with fixed code (a template that uses none of the bindings), and
/// not an edit that adds commentary (a comment line the pattern lacks
/// explains this site, not others).
fn worth_suggesting(
    g: &colab_core::Generalized,
    backend: &dyn colab_core::LanguageBackend,
) -> bool {
    let Ok(op) = backend.build_rule(
        "pattern",
        colab_core::RuleSpec::Replace {
            target: g.pattern.clone(),
            replacement: g.template.clone(),
        },
    ) else {
        return false;
    };
    let rematches = !op.advice().is_empty();
    let uses_a_binding = g
        .bindings
        .iter()
        .any(|(var, _)| g.template.contains(var.as_str()));
    let comment = |text: &str| {
        text.lines()
            .map(str::trim_start)
            .filter(|l| l.starts_with("//") || l.starts_with("/*") || l.starts_with("# "))
            .count()
    };
    // A file's imports serve its own code: adding one here is no reason to
    // add it elsewhere (deliberate import moves are `<lang>::import`'s).
    let import = ["use ", "import ", "#include", "using ", "from ", "require "]
        .iter()
        .any(|kw| g.pattern.trim_start().starts_with(kw));
    !rematches && !import && uses_a_binding && comment(&g.template) <= comment(&g.pattern)
}

/// The source backend for `path`: of those whose patterns read it, the
/// one named after its extension (`ts` over `js` for `.ts`), else the
/// first.
fn source_language(path: &Path, backends: &BackendRegistry) -> Option<String> {
    let ext = path.extension()?.to_str()?;
    let relevant: Vec<String> = backends
        .languages()
        .into_iter()
        .filter(|lang| {
            backends.get(lang).is_some_and(|b| {
                b.capabilities().iter().any(|c| c.module == "pattern")
                    && b.build_rule(
                        "pattern",
                        colab_core::RuleSpec::Delete {
                            target: "$X".into(),
                        },
                    )
                    .is_ok_and(|op| op.is_file_relevant(path))
            })
        })
        .map(str::to_string)
        .collect();
    relevant
        .iter()
        .find(|lang| lang.as_str() == ext)
        .or_else(|| relevant.first())
        .cloned()
}

/// `colab hook pre-bash`: a `deny` for a command that edits tracked files
/// through the shell, and nothing for any other (or input it cannot read).
fn pre_bash(command: Option<String>) -> Result<u8> {
    if let Some(command) = command {
        let cwd = std::env::current_dir().map_err(crate::cli::io_to_error)?;
        return Ok(match crate::shell_edit::verdict(&command, &cwd) {
            Some(reason) => {
                println!("{reason}");
                10
            }
            None => 0,
        });
    }
    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .map_err(crate::cli::io_to_error)?;
    let Some((command, cwd)) = bash_call(&input) else {
        return Ok(0);
    };
    if let Some(reason) = crate::shell_edit::verdict(&command, &cwd) {
        println!(
            "{}",
            serde_json::json!({"hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": reason,
            }})
        );
    }
    Ok(0)
}

/// The command a Bash tool call runs, and its directory, from the hook's
/// JSON. `None` for another tool.
fn bash_call(input: &str) -> Option<(String, PathBuf)> {
    let call: Value = serde_json::from_str(input).ok()?;
    if call
        .get("tool_name")
        .and_then(Value::as_str)
        .is_some_and(|t| t != "Bash")
    {
        return None;
    }
    let command = call.pointer("/tool_input/command")?.as_str()?.to_string();
    let cwd = match call.get("cwd").and_then(Value::as_str) {
        Some(cwd) => PathBuf::from(cwd),
        None => std::env::current_dir().ok()?,
    };
    Some((command, cwd))
}

/// The file a tool call wrote, from the hook's JSON: `tool_input.file_path`
/// (Edit, Write, MultiEdit), made absolute with the call's `cwd`.
pub fn edited_file(input: &str) -> Option<PathBuf> {
    let call: Value = serde_json::from_str(input).ok()?;
    let path = PathBuf::from(call.pointer("/tool_input/file_path")?.as_str()?);
    if path.is_absolute() {
        return Some(path);
    }
    let cwd = call.get("cwd").and_then(Value::as_str).map(PathBuf::from);
    Some(cwd.unwrap_or_default().join(path))
}

/// What the packs would change in `file`, as the model should read it, or
/// `None` when there is nothing to say.
pub fn check(file: &Path, backends: &BackendRegistry) -> Result<Option<String>> {
    let Some(start) = file.parent() else {
        return Ok(None);
    };
    let Some(config_path) = crate::config::find_from(start) else {
        return Ok(None);
    };
    let config = crate::config::load(&config_path)?;
    let dir = if config.dir.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        config.dir.clone()
    };
    let (Ok(dir), Ok(file)) = (dir.canonicalize(), file.canonicalize()) else {
        return Ok(None);
    };
    let Ok(rel) = file.strip_prefix(&dir) else {
        return Ok(None);
    };
    let rel = rel.to_path_buf();
    if !covered(&rel, &config) {
        return Ok(None);
    }
    let parts = crate::cli::compile_packs(&config, backends)?;
    let refactoring = colab_dsl::Refactoring::combine("fix", parts);
    if refactoring.has_scans() || !refactoring.is_file_relevant(&rel) {
        return Ok(None);
    }
    let source = std::fs::read_to_string(&file).map_err(|e| Error::io_at(&file, e))?;
    // Rule globs (`in "src/**"`) are relative to colab.toml's directory.
    let outcome = colab_core::semantic::at_path(&rel, || refactoring.apply_at(&rel, &source));
    if outcome.output == source {
        return Ok(None);
    }
    let report = refactoring.report();
    let change = FileChange {
        path: rel.clone(),
        before: source,
        after: outcome.output,
        rules_fired: outcome.rules_fired,
        conflicts: outcome.conflicts,
        created: false,
    };
    let findings: Vec<_> = colab_core::render::findings(&change, &report)
        .into_iter()
        .filter(|f| f.severity > Severity::Info)
        .collect();
    if findings.is_empty() {
        return Ok(None);
    }
    let mut out = format!(
        "colab: {} breaks this repository's conventions (colab.toml):\n",
        rel.display()
    );
    for f in &findings {
        let rule = f.rules.first().and_then(|&i| report.rules().get(i));
        let what = rule.map_or_default(|r| match (&r.message, &r.id) {
            (Some(m), Some(id)) => format!("{m} [{id}]"),
            (Some(m), None) => m.clone(),
            (None, Some(id)) => format!("[{id}] {}", r.rule),
            (None, None) => r.rule.clone(),
        });
        let line = if f.end_line > f.start_line {
            format!("{}-{}", f.start_line, f.end_line)
        } else {
            f.start_line.to_string()
        };
        let _ = writeln!(
            out,
            "  {}:{line}: {}: {what}",
            rel.display(),
            f.severity.as_str()
        );
        let replacement = f.replacement.trim_end();
        if replacement.is_empty() {
            out.push_str("    delete these lines\n");
        } else {
            for l in replacement.lines().take(6) {
                let _ = writeln!(out, "    | {l}");
            }
        }
    }
    let _ = writeln!(
        out,
        "Fix it as shown, or run `colab fix --write` in {}; a deliberate exception takes a `colab:ignore` comment.",
        dir.display()
    );
    Ok(Some(out))
}

/// Whether `rel` (relative to colab.toml) is in its `[run] paths` and not
/// in `exclude`.
fn covered(rel: &Path, config: &crate::config::Config) -> bool {
    let in_paths = config.paths.is_empty()
        || config.paths.iter().any(|p| {
            let p = p.strip_prefix(".").unwrap_or(p);
            p.as_os_str().is_empty() || rel.starts_with(p)
        });
    let excluded = config
        .exclude
        .iter()
        .any(|glob| globset::Glob::new(glob).is_ok_and(|g| g.compile_matcher().is_match(rel)));
    in_paths && !excluded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_edit_that_matches_elsewhere_is_suggested_as_a_rule() {
        let root = std::env::temp_dir().join(format!("colab-hook-suggest-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("a.go");
        std::fs::write(
            &file,
            "package p\n\nfunc a() {\n\trequire.Equal(t, got, 1)\n\tz := x + 2\n}\n",
        )
        .unwrap();
        std::fs::write(
            root.join("b.go"),
            "package p\n\nfunc b() {\n\tassert.Equal(t, want, f(x))\n\ty := x + 1\n}\n",
        )
        .unwrap();
        let backends = colab_backends::registry();
        let call = |old: &str, new: &str, tool: &str| {
            serde_json::json!({"tool_name": tool, "tool_input": {
                "file_path": file.to_string_lossy(), "old_string": old, "new_string": new
            }})
            .to_string()
        };
        let note = suggest_generalize(
            &call(
                "assert.Equal(t, 1, got)",
                "require.Equal(t, got, 1)",
                "Edit",
            ),
            &backends,
        )
        .expect("a suggestion");
        assert!(
            note.contains("matches 1 more place(s)") && note.contains("b.go:4"),
            "{note}"
        );
        assert!(
            note.contains(r#""match":"assert.Equal($A, $B, $C)""#),
            "{note}"
        );
        // `x + 1` → `x + 2` is `$A + 1`: no name to anchor on, so no word.
        assert!(suggest_generalize(&call("x + 1", "x + 2", "Edit"), &backends).is_none());
        // Other tools, and edits with nothing elsewhere, stay quiet.
        assert!(
            suggest_generalize(
                &call(
                    "assert.Equal(t, 1, got)",
                    "require.Equal(t, got, 1)",
                    "Write"
                ),
                &backends
            )
            .is_none()
        );
        assert!(
            suggest_generalize(
                &call("other.Call(t, 1)", "other.Call(t)", "Edit"),
                &backends
            )
            .is_none()
        );
        // A MultiEdit whose old text carries a context line, as agents
        // write it: the rule is the changed call alone.
        let multi = serde_json::json!({"tool_name": "MultiEdit", "tool_input": {
            "file_path": file.to_string_lossy(),
            "edits": [{"old_string": "func a() {\n\tassert.Equal(t, 1, got)", "new_string": "func a() {\n\trequire.Equal(t, got, 1)"}]
        }})
        .to_string();
        let note = suggest_generalize(&multi, &backends).expect("a suggestion for the MultiEdit");
        assert!(
            note.contains(r#""match":"assert.Equal($A, $B, $C)""#),
            "{note}"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_edited_file_comes_from_the_tool_call() {
        let call = r#"{"tool_name":"Edit","cwd":"/repo","tool_input":{"file_path":"src/a.rs","old_string":"x","new_string":"y"}}"#;
        assert_eq!(edited_file(call), Some(PathBuf::from("/repo/src/a.rs")));
        let absolute = r#"{"tool_input":{"file_path":"/elsewhere/b.go"}}"#;
        assert_eq!(
            edited_file(absolute),
            Some(PathBuf::from("/elsewhere/b.go"))
        );
        assert_eq!(edited_file(r#"{"tool_input":{"command":"ls"}}"#), None);
        assert_eq!(edited_file("not json"), None);
    }
}
