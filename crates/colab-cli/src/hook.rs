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
}

pub fn run(args: HookArgs, backends: &BackendRegistry) -> Result<u8> {
    let HookEvent::PostEdit { file } = args.event;
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
    }
    Ok(0)
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
