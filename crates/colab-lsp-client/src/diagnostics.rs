//! `--verify-lsp`: does a change add errors the language server can see?
//!
//! Each file is opened with its current text, then changed in memory to
//! the new text, and the server's error diagnostics are compared. Nothing
//! is read from or written to disk, so the check runs before a write.
//! It is cheaper than a full build and catches what a server checks as
//! you type (unresolved names, type errors in gopls, syntax); a build
//! (`--verify`) remains the full check.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use colab_core::Result;
use serde_json::{Value, json};

use crate::ServerCommand;
use crate::edit::path_to_uri;
use crate::session::Session;

/// One file's change, in memory.
pub struct Changed {
    /// Absolute path: the server is told about it by URI.
    pub path: PathBuf,
    pub before: String,
    pub after: String,
}

/// Errors that `after` has and `before` did not, for one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddedErrors {
    pub path: PathBuf,
    /// `line N: message`, 1-based, in the new text.
    pub errors: Vec<String>,
}

/// Ask `server`, started in `root`, which of `files`' changes add errors.
/// Files whose error list did not grow are left out.
pub fn added_errors(
    server: &ServerCommand,
    root: &Path,
    timeout: Duration,
    settle: Duration,
    files: &[Changed],
) -> Result<Vec<AddedErrors>> {
    let mut session = Session::start(&server.program, &server.args, root, timeout, false)?;
    let result = compare(&mut session, settle, files);
    session.shutdown();
    result
}

fn compare(session: &mut Session, settle: Duration, files: &[Changed]) -> Result<Vec<AddedErrors>> {
    let pull = !matches!(
        session.capabilities.get("diagnosticProvider"),
        None | Some(Value::Null) | Some(Value::Bool(false))
    );
    for file in files {
        session.open(&file.path, &file.before)?;
    }
    session.wait_until_quiet(settle)?;
    let mut before: HashMap<&Path, Vec<Value>> = HashMap::new();
    for file in files {
        before.insert(&file.path, errors(session, pull, &file.path)?);
    }

    for file in files {
        session.forget_published(&file.path);
        session.open(&file.path, &file.after)?;
    }
    session.wait_until_quiet(settle)?;

    let mut out = Vec::new();
    for file in files {
        let after = errors(session, pull, &file.path)?;
        let added = added(&before[file.path.as_path()], &after);
        if !added.is_empty() {
            out.push(AddedErrors {
                path: file.path.clone(),
                errors: added,
            });
        }
    }
    Ok(out)
}

/// The error-severity diagnostics for `path`, pulled or as last pushed.
/// A diagnostic with no severity counts as an error, as clients may
/// choose.
fn errors(session: &mut Session, pull: bool, path: &Path) -> Result<Vec<Value>> {
    let all = if pull {
        let report = session
            .request_retrying(
                "textDocument/diagnostic",
                json!({"textDocument": {"uri": path_to_uri(path)}}),
            )?
            .map_err(|e| session.server_error("textDocument/diagnostic", &e))?;
        report
            .get("items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    } else {
        session
            .published(path)
            .map(<[Value]>::to_vec)
            .unwrap_or_default()
    };
    log::debug!(
        "{} diagnostic(s) for {} ({}): {}",
        all.len(),
        path.display(),
        if pull { "pulled" } else { "pushed" },
        Value::Array(all.clone())
    );
    Ok(all
        .into_iter()
        .filter(|d| {
            d.get("severity")
                .and_then(Value::as_u64)
                .is_none_or(|s| s == 1)
        })
        .collect())
}

/// Errors in `after` beyond those in `before`, compared by message as a
/// multiset: positions move when code is edited, messages do not.
fn added(before: &[Value], after: &[Value]) -> Vec<String> {
    let message = |d: &Value| {
        d.get("message")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    let mut remaining: HashMap<String, usize> = HashMap::new();
    for d in before {
        *remaining.entry(message(d)).or_default() += 1;
    }
    let mut out = Vec::new();
    for d in after {
        let text = message(d);
        match remaining.get_mut(&text) {
            Some(n) if *n > 0 => *n -= 1,
            _ => {
                let line = d
                    .pointer("/range/start/line")
                    .and_then(Value::as_u64)
                    .map_or(0, |l| l + 1);
                out.push(format!("line {line}: {text}"));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diag(line: u64, message: &str) -> Value {
        json!({"range": {"start": {"line": line}}, "severity": 1, "message": message})
    }

    #[test]
    fn only_errors_beyond_the_old_ones_are_added() {
        let before = [diag(1, "a"), diag(4, "b")];
        // `a` moved; a second `b` and a new `c` are added.
        let after = [diag(2, "a"), diag(5, "b"), diag(7, "b"), diag(9, "c")];
        assert_eq!(added(&before, &after), ["line 8: b", "line 10: c"]);
        assert!(added(&after, &before).is_empty());
    }
}
