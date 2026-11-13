//! Refactorings and analyses the server already knows how to do: code
//! actions (extract, inline, change signature, …), references, safe
//! delete, and tidying (organize imports + format).
//!
//! Each runs in one session and comes back as a [`ChangeSet`] or a list
//! of locations — never as a write. The CLI reports, checks, backs up,
//! verifies, and writes, exactly as for a codemod.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use colab_core::fileops::ChangeSet;
use colab_core::find::Located;
use colab_core::walker::FileChange;
use colab_core::{Error, Result};
use serde_json::{Value, json};

use crate::edit::{self, Encoding, Files, offset_of, path_to_uri, position_of, uri_to_path};
use crate::session::Session;
use crate::{ServerCommand, as_document_changes};

/// A place in a file, and the server to ask about it.
#[derive(Debug, Clone)]
pub struct Target {
    /// The project root the server is started in.
    pub root: PathBuf,
    /// Absolute path of the file.
    pub file: PathBuf,
    /// Byte range in the file; `start == end` for a cursor position.
    pub start: usize,
    pub end: usize,
    pub timeout: Duration,
    pub settle: Duration,
    /// Whether the caller can make file operations (it is in git).
    pub file_operations: bool,
}

/// One code action the server offers at a [`Target`].
#[derive(Debug, Clone)]
pub struct ActionSummary {
    pub title: String,
    /// `refactor.extract.function`, …; `None` for a bare command.
    pub kind: Option<String>,
    pub preferred: bool,
    /// Why the server says the action cannot run here, if it says so.
    pub disabled: Option<String>,
}

/// Which action to run: by kind (a prefix match on the dotted kind, so
/// `refactor.extract` matches `refactor.extract.function`) and/or by a
/// case-insensitive substring of the title.
#[derive(Debug, Clone, Default)]
pub struct Select {
    pub kind: Option<String>,
    pub title: Option<String>,
}

/// The action that ran and what it changes.
#[derive(Debug)]
pub struct ActionOutcome {
    pub title: String,
    pub kind: Option<String>,
    pub changes: ChangeSet,
}

/// One reference to a symbol.
#[derive(Debug, Clone)]
pub struct Reference {
    pub path: PathBuf,
    pub at: Located,
    /// The whole source line, trimmed — what a reader needs to judge it.
    pub line_text: String,
}

/// What [`safe_delete`] found.
#[derive(Debug)]
pub enum SafeDelete {
    /// Nothing outside the declaration refers to it; this deletes it.
    Unused { symbol: String, changes: ChangeSet },
    /// These references outside the declaration keep it alive.
    Referenced {
        symbol: String,
        references: Vec<Reference>,
    },
}

/// Start the server, open the target file, wait for indexing.
fn start(server: &ServerCommand, target: &Target) -> Result<(Session, String)> {
    let text = std::fs::read_to_string(&target.file).map_err(|e| Error::io_at(&target.file, e))?;
    let mut session = Session::start(
        &server.program,
        &server.args,
        &target.root,
        target.timeout,
        target.file_operations,
    )?;
    session.open(&target.file, &text)?;
    session.wait_until_quiet(target.settle)?;
    Ok((session, text))
}

fn require(session: Session, capability: &str, what: &str) -> Result<Session> {
    let value = session.capabilities.get(capability);
    if matches!(value, None | Some(Value::Null) | Some(Value::Bool(false))) {
        let name = session.name().to_string();
        session.shutdown();
        return Err(Error::UnsupportedOperation(format!(
            "`{name}` does not support {what} (no {capability})"
        )));
    }
    Ok(session)
}

fn range(text: &str, start: usize, end: usize, encoding: Encoding) -> Value {
    let (sl, sc) = position_of(text, start, encoding);
    let (el, ec) = position_of(text, end, encoding);
    json!({"start": {"line": sl, "character": sc}, "end": {"line": el, "character": ec}})
}

/// `textDocument/codeAction` over a range, optionally only of `kind`.
fn request_actions(
    session: &mut Session,
    file: &Path,
    text: &str,
    (start, end): (usize, usize),
    kind: Option<&str>,
) -> Result<Vec<Value>> {
    let mut context = json!({"diagnostics": [], "triggerKind": 1});
    if let Some(kind) = kind {
        context["only"] = json!([kind]);
    }
    let params = json!({
        "textDocument": {"uri": path_to_uri(file)},
        "range": range(text, start, end, session.encoding),
        "context": context,
    });
    let answer = session
        .request_retrying("textDocument/codeAction", params)?
        .map_err(|e| session.server_error("textDocument/codeAction", &e))?;
    Ok(answer.as_array().cloned().unwrap_or_default())
}

fn summarize(action: &Value) -> ActionSummary {
    ActionSummary {
        title: action["title"].as_str().unwrap_or("").to_string(),
        kind: action["kind"].as_str().map(str::to_string),
        preferred: action["isPreferred"].as_bool().unwrap_or(false),
        disabled: action
            .pointer("/disabled/reason")
            .and_then(Value::as_str)
            .map(str::to_string),
    }
}

/// Does `kind` fall under `want` (`refactor.extract` ⊇
/// `refactor.extract.function`)? The empty kind matches everything.
fn kind_matches(kind: Option<&str>, want: &str) -> bool {
    want.is_empty() || kind.is_some_and(|k| k == want || k.starts_with(&format!("{want}.")))
}

/// Every action the server offers at `target`, optionally only of `kind`.
pub fn list_actions(
    server: &ServerCommand,
    target: &Target,
    kind: Option<&str>,
) -> Result<Vec<ActionSummary>> {
    let (session, text) = start(server, target)?;
    let mut session = require(session, "codeActionProvider", "code actions")?;
    let actions = request_actions(
        &mut session,
        &target.file,
        &text,
        (target.start, target.end),
        kind,
    )?;
    session.shutdown();
    Ok(actions
        .iter()
        .map(summarize)
        .filter(|a| kind.is_none_or(|k| kind_matches(a.kind.as_deref(), k)))
        .collect())
}

/// Run the one action `select` picks at `target` and return its changes.
///
/// Exactly one enabled action must match; none or several is an error
/// that lists what is available, so the next call can be precise. An
/// action without an edit is resolved (`codeAction/resolve`); one that
/// carries a command is executed, and the edits the server then sends
/// back (`workspace/applyEdit`) become part of the result.
pub fn run_action(
    server: &ServerCommand,
    target: &Target,
    select: &Select,
) -> Result<ActionOutcome> {
    if select.kind.is_none() && select.title.is_none() {
        return Err(Error::Config(
            "choose an action with --kind and/or --title (list them with --list)".into(),
        ));
    }
    let (session, text) = start(server, target)?;
    let mut session = require(session, "codeActionProvider", "code actions")?;
    let actions = request_actions(
        &mut session,
        &target.file,
        &text,
        (target.start, target.end),
        select.kind.as_deref(),
    )?;
    let title = select.title.as_deref().map(str::to_lowercase);
    let matching: Vec<&Value> = actions
        .iter()
        .filter(|a| {
            let s = summarize(a);
            s.disabled.is_none()
                && select
                    .kind
                    .as_deref()
                    .is_none_or(|k| kind_matches(s.kind.as_deref(), k))
                && title
                    .as_deref()
                    .is_none_or(|t| s.title.to_lowercase().contains(t))
        })
        .collect();
    let chosen = match matching.as_slice() {
        [one] => (*one).clone(),
        [] | [_, _, ..] => {
            // Several: an exact title wins.
            let exact = title.as_deref().and_then(|t| {
                matching
                    .iter()
                    .find(|a| summarize(a).title.to_lowercase() == t)
            });
            match exact {
                Some(action) => (*action).clone(),
                None => {
                    let name = session.name().to_string();
                    session.shutdown();
                    return Err(no_single_action(&name, &actions, matching.len()));
                }
            }
        }
    };
    let summary = summarize(&chosen);
    let changes = perform(&mut session, chosen, &edit::Disk, target.file_operations)?;
    session.shutdown();
    Ok(ActionOutcome {
        title: summary.title,
        kind: summary.kind,
        changes,
    })
}

fn no_single_action(server: &str, actions: &[Value], matched: usize) -> Error {
    let listing: Vec<String> = actions
        .iter()
        .map(summarize)
        .map(|a| {
            let mut line = format!("{} [{}]", a.title, a.kind.as_deref().unwrap_or("command"));
            if let Some(reason) = a.disabled {
                line.push_str(&format!(" (disabled: {reason})"));
            }
            line
        })
        .collect();
    let what = if matched == 0 {
        "no action matched"
    } else {
        "several actions matched; narrow with --title or a longer --kind"
    };
    if listing.is_empty() {
        Error::Config(format!("{what}: `{server}` offers no code actions here"))
    } else {
        Error::Config(format!("{what}. `{server}` offers: {}", listing.join("; ")))
    }
}

/// Turn a chosen action into its changes: resolve it if it has no edit,
/// run its command if it has one, and merge every edit in order.
fn perform(
    session: &mut Session,
    action: Value,
    files: &dyn Files,
    file_operations: bool,
) -> Result<ChangeSet> {
    let is_bare_command = action["command"].is_string();
    let resolvable = session
        .capabilities
        .pointer("/codeActionProvider/resolveProvider")
        .and_then(Value::as_bool)
        == Some(true);
    // Resolve only what is left to resolve: no edit yet, and either data
    // for the server to work from or no command to run instead.
    let needs_resolve = !is_bare_command
        && action.get("edit").is_none()
        && (action.get("data").is_some() || action.get("command").is_none());
    let action = if needs_resolve && resolvable {
        session
            .request_retrying("codeAction/resolve", action)?
            .map_err(|e| session.server_error("codeAction/resolve", &e))?
    } else {
        action
    };

    let mut edits: Vec<Value> = action.get("edit").cloned().into_iter().collect();
    let command = if is_bare_command {
        Some(action.clone())
    } else {
        action.get("command").cloned().filter(|c| c.is_object())
    };
    if let Some(command) = command {
        session.capture_edits(true);
        let outcome = session.request_retrying(
            "workspace/executeCommand",
            json!({
                "command": command["command"],
                "arguments": command.get("arguments").cloned().unwrap_or(json!([])),
            }),
        );
        let captured = session.capture_edits(false);
        outcome?.map_err(|e| session.server_error("workspace/executeCommand", &e))?;
        edits.extend(captured);
    }

    let mut merged = Vec::new();
    for edit in &edits {
        if let Some(entries) = as_document_changes(edit)["documentChanges"].as_array() {
            merged.extend(entries.iter().cloned());
        }
    }
    edit::to_change_set(
        &json!({"documentChanges": merged}),
        session.encoding,
        files,
        file_operations,
    )
}

/// References to the symbol at `target` (`textDocument/references`).
pub fn references(
    server: &ServerCommand,
    target: &Target,
    include_declaration: bool,
) -> Result<Vec<Reference>> {
    let (session, text) = start(server, target)?;
    let mut session = require(session, "referencesProvider", "references")?;
    let found = references_in(
        &mut session,
        &target.file,
        &text,
        target.start,
        include_declaration,
    );
    session.shutdown();
    found
}

fn references_in(
    session: &mut Session,
    file: &Path,
    text: &str,
    offset: usize,
    include_declaration: bool,
) -> Result<Vec<Reference>> {
    let (line, character) = position_of(text, offset, session.encoding);
    let answer = session
        .request_retrying(
            "textDocument/references",
            json!({
                "textDocument": {"uri": path_to_uri(file)},
                "position": {"line": line, "character": character},
                "context": {"includeDeclaration": include_declaration},
            }),
        )?
        .map_err(|e| session.server_error("textDocument/references", &e))?;

    let mut texts: BTreeMap<PathBuf, String> = BTreeMap::new();
    let mut out = Vec::new();
    for location in answer.as_array().into_iter().flatten() {
        let Some(uri) = location["uri"].as_str() else {
            continue;
        };
        let path = uri_to_path(uri)?;
        if !texts.contains_key(&path) {
            let source = std::fs::read_to_string(&path).map_err(|e| Error::io_at(&path, e))?;
            texts.insert(path.clone(), source);
        }
        let source = &texts[&path];
        let (start, end) = byte_range(source, &location["range"], session.encoding)?;
        let at = Located::at(source, start, end, Vec::new());
        let line_text = source
            .lines()
            .nth(at.line - 1)
            .unwrap_or("")
            .trim()
            .to_string();
        out.push(Reference {
            path,
            at,
            line_text,
        });
    }
    out.sort_by(|a, b| (&a.path, a.at.start).cmp(&(&b.path, b.at.start)));
    out.dedup_by(|a, b| a.path == b.path && a.at.start == b.at.start);
    Ok(out)
}

fn byte_range(text: &str, range: &Value, encoding: Encoding) -> Result<(usize, usize)> {
    let at = |field: &str| -> Result<usize> {
        let line = range
            .pointer(&format!("/{field}/line"))
            .and_then(Value::as_u64);
        let character = range
            .pointer(&format!("/{field}/character"))
            .and_then(Value::as_u64);
        match (line, character) {
            (Some(l), Some(c)) => offset_of(text, l as u32, c as u32, encoding),
            _ => Err(Error::Config(
                "language server sent a range without positions".into(),
            )),
        }
    };
    Ok((at("start")?, at("end")?))
}

/// Delete the declaration at `target` if nothing outside it refers to it.
///
/// The declaration's extent is the server's (`textDocument/documentSymbol`
/// — including doc comments and attributes where the server counts them);
/// references inside it (recursion) do not keep it alive. Deletion takes
/// whole lines, plus one blank line when the declaration stood between two.
pub fn safe_delete(server: &ServerCommand, target: &Target) -> Result<SafeDelete> {
    let (session, text) = start(server, target)?;
    let session = require(session, "documentSymbolProvider", "document symbols")?;
    let mut session = require(session, "referencesProvider", "references")?;
    let answer = session
        .request_retrying(
            "textDocument/documentSymbol",
            json!({"textDocument": {"uri": path_to_uri(&target.file)}}),
        )?
        .map_err(|e| session.server_error("textDocument/documentSymbol", &e))?;
    let encoding = session.encoding;
    let declaration = innermost_symbol(&answer, &text, target.start, encoding)?;
    let Some((symbol, (decl_start, decl_end))) = declaration else {
        let name = session.name().to_string();
        session.shutdown();
        return Err(Error::Config(format!(
            "`{name}` reports no declaration at {} byte {}; point --at at the symbol's name",
            target.file.display(),
            target.start
        )));
    };

    let references = references_in(&mut session, &target.file, &text, target.start, false)?;
    session.shutdown();
    let outside: Vec<Reference> = references
        .into_iter()
        .filter(|r| !(r.path == target.file && r.at.start >= decl_start && r.at.end <= decl_end))
        .collect();
    if !outside.is_empty() {
        return Ok(SafeDelete::Referenced {
            symbol,
            references: outside,
        });
    }

    let line_start = text[..decl_start].rfind('\n').map_or(0, |i| i + 1);
    let mut line_end = text[decl_end..]
        .find('\n')
        .map_or(text.len(), |i| decl_end + i + 1);
    let blank_before = line_start == 0 || text[..line_start].ends_with("\n\n");
    if blank_before && text[line_end..].starts_with('\n') {
        line_end += 1;
    }
    let mut after = String::with_capacity(text.len());
    after.push_str(&text[..line_start]);
    after.push_str(&text[line_end..]);
    Ok(SafeDelete::Unused {
        symbol,
        changes: ChangeSet::from_edits(vec![FileChange {
            path: target.file.clone(),
            before: text,
            after,
            rules_fired: vec![0],
        }]),
    })
}

/// The innermost symbol whose name range contains `offset`: its name and
/// its full byte range. Accepts both hierarchical `DocumentSymbol[]` and
/// flat `SymbolInformation[]` answers.
fn innermost_symbol(
    answer: &Value,
    text: &str,
    offset: usize,
    encoding: Encoding,
) -> Result<Option<(String, (usize, usize))>> {
    fn walk(
        symbols: &[Value],
        text: &str,
        offset: usize,
        encoding: Encoding,
        best: &mut Option<(String, (usize, usize))>,
    ) -> Result<()> {
        for symbol in symbols {
            let full = symbol
                .get("range")
                .or_else(|| symbol.pointer("/location/range"))
                .cloned()
                .unwrap_or(Value::Null);
            if full.is_null() {
                continue;
            }
            let name_range = symbol.get("selectionRange").unwrap_or(&full);
            let (ns, ne) = byte_range(text, name_range, encoding)?;
            let (fs, fe) = byte_range(text, &full, encoding)?;
            if ns <= offset && offset <= ne {
                let narrower = best.as_ref().is_none_or(|(_, (bs, be))| fe - fs <= be - bs);
                if narrower {
                    *best = Some((symbol["name"].as_str().unwrap_or("?").to_string(), (fs, fe)));
                }
            }
            if let Some(children) = symbol.get("children").and_then(Value::as_array) {
                walk(children, text, offset, encoding, best)?;
            }
        }
        Ok(())
    }
    let mut best = None;
    walk(
        answer.as_array().map(Vec::as_slice).unwrap_or(&[]),
        text,
        offset,
        encoding,
        &mut best,
    )?;
    Ok(best)
}

/// In-memory contents over the disk, for tidying text not yet written.
struct Overlay<'a>(&'a BTreeMap<PathBuf, String>);

impl Files for Overlay<'_> {
    fn read(&self, path: &Path) -> Result<String> {
        match self.0.get(path) {
            Some(text) => Ok(text.clone()),
            None => edit::Disk.read(path),
        }
    }
    fn exists(&self, path: &Path) -> bool {
        self.0.contains_key(path) || path.exists()
    }
    fn is_dir(&self, path: &Path) -> bool {
        path.is_dir()
    }
}

/// What tidying did to one file.
#[derive(Debug)]
pub struct Tidied {
    pub text: String,
    /// Steps the server could not take here, e.g. "no formatting".
    pub skipped: Vec<String>,
}

/// Organize imports (`source.organizeImports`) and format
/// (`textDocument/formatting`) each `(path, text)` — the *new* contents
/// of files a refactoring changed, which need not be on disk yet. Edits
/// the server makes to other files are ignored: tidying is per file.
pub fn tidy(
    server: &ServerCommand,
    root: &Path,
    files: &[(PathBuf, String)],
    timeout: Duration,
    settle: Duration,
) -> Result<Vec<Tidied>> {
    let mut session = Session::start(&server.program, &server.args, root, timeout, false)?;
    for (path, text) in files {
        session.open(path, text)?;
    }
    session.wait_until_quiet(settle)?;
    let can = |s: &Session, cap: &str| {
        !matches!(
            s.capabilities.get(cap),
            None | Some(Value::Null) | Some(Value::Bool(false))
        )
    };

    let mut out = Vec::new();
    for (path, text) in files {
        let mut current = text.clone();
        let mut skipped = Vec::new();
        if can(&session, "codeActionProvider") {
            let actions = request_actions(
                &mut session,
                path,
                &current,
                (0, current.len()),
                Some("source.organizeImports"),
            )?;
            let organize = actions.into_iter().find(|a| {
                let s = summarize(a);
                s.disabled.is_none() && kind_matches(s.kind.as_deref(), "source.organizeImports")
            });
            if let Some(action) = organize {
                let overlay: BTreeMap<PathBuf, String> =
                    [(path.clone(), current.clone())].into_iter().collect();
                let set = perform(&mut session, action, &Overlay(&overlay), false)?;
                if let Some(edit) = set.edits.into_iter().find(|e| e.path == *path) {
                    current = edit.after;
                    session.open(path, &current)?;
                }
            }
        } else {
            skipped.push("organize imports (no codeActionProvider)".to_string());
        }
        if can(&session, "documentFormattingProvider") {
            let answer = session
                .request_retrying(
                    "textDocument/formatting",
                    json!({
                        "textDocument": {"uri": path_to_uri(path)},
                        "options": {"tabSize": 4, "insertSpaces": true},
                    }),
                )?
                .map_err(|e| session.server_error("textDocument/formatting", &e))?;
            if let Some(edits) = answer.as_array().filter(|e| !e.is_empty()) {
                let overlay: BTreeMap<PathBuf, String> =
                    [(path.clone(), current.clone())].into_iter().collect();
                let changes = json!({"changes": {path_to_uri(path): edits}});
                let set =
                    edit::to_change_set(&changes, session.encoding, &Overlay(&overlay), false)?;
                if let Some(edit) = set.edits.into_iter().next() {
                    current = edit.after;
                    session.open(path, &current)?;
                }
            }
        } else {
            skipped.push("formatting (no documentFormattingProvider)".to_string());
        }
        out.push(Tidied {
            text: current,
            skipped,
        });
    }
    session.shutdown();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_match_by_dotted_prefix() {
        assert!(kind_matches(
            Some("refactor.extract.function"),
            "refactor.extract"
        ));
        assert!(kind_matches(Some("refactor.extract"), "refactor.extract"));
        assert!(!kind_matches(
            Some("refactor.extractor"),
            "refactor.extract"
        ));
        assert!(!kind_matches(None, "refactor"));
        assert!(kind_matches(None, ""));
    }

    #[test]
    fn the_innermost_symbol_around_the_cursor_wins() {
        let text = "impl A {\n    fn b() {}\n}\n";
        let r = |sl: u32, sc: u32, el: u32, ec: u32| json!({"start": {"line": sl, "character": sc}, "end": {"line": el, "character": ec}});
        let answer = json!([{
            "name": "A", "range": r(0, 0, 2, 1), "selectionRange": r(0, 5, 0, 6),
            "children": [{"name": "b", "range": r(1, 4, 1, 13), "selectionRange": r(1, 7, 1, 8)}]
        }]);
        let at_b = text.find("b()").unwrap();
        let (name, (s, e)) = innermost_symbol(&answer, text, at_b, Encoding::Utf16)
            .unwrap()
            .unwrap();
        assert_eq!(name, "b");
        assert_eq!(&text[s..e], "fn b() {}");
        assert!(
            innermost_symbol(&answer, text, 0, Encoding::Utf16)
                .unwrap()
                .is_none()
        );
    }
}
