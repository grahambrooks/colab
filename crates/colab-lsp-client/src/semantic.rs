//! The [`Oracle`] behind `where $X type "…"` and `where $F resolves "…"`:
//! each question asked of the language's own server.
//!
//! One server per command line, started in the run's root the first time
//! a file needs it, and waited on until it has indexed. Before each
//! question the file's text, as the run has it (earlier rules' edits
//! included), is sent with `didOpen` / `didChange`, so the answer is about
//! that text. Questions are asked at the binding's last identifier:
//!
//! - `type "<text>"`: `textDocument/hover` there, which holds when the
//!   hover's text contains `<text>` with no identifier character on either
//!   side (`io.Reader` matches `var r io.Reader`, not `io.ReaderAt`). The
//!   spelling is the server's: gopls writes `io.Reader`, rust-analyzer
//!   `Option<String>`, pyright `list[str]`.
//! - `resolves "<path>.<name>"` (also `::`): `textDocument/definition`
//!   there, which holds when the declaration it lands on is named `<name>`
//!   and its file's path holds each segment of `<path>`, in order, as a
//!   directory or the file's stem (a module cache's `@v1.2.3` ignored).
//!
//! Any failure (no server, a start-up error, indexing past the timeout, an
//! error answer) is the question's answer, and the run reports it.

use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use colab_core::semantic::{Oracle, Query};
use serde_json::{Value, json};

use crate::ServerCommand;
use crate::edit::{offset_of, path_to_uri, position_of, uri_to_path};
use crate::session::Session;

/// Answers semantic constraints through language servers.
pub struct LspOracle {
    root: PathBuf,
    /// `--lsp`: this server for every file, instead of each language's own.
    server: Option<ServerCommand>,
    timeout: Duration,
    settle: Duration,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    /// Started servers, by command line; `Err` remembers why one could not
    /// start, so each later question fails fast with the same reason.
    sessions: HashMap<String, Result<Session, String>>,
    /// The text each file was last sent with.
    sent: HashMap<PathBuf, String>,
}

impl LspOracle {
    pub fn new(root: &Path, server: Option<ServerCommand>, timeout: Duration) -> Self {
        Self {
            root: root.to_path_buf(),
            server,
            timeout,
            settle: Duration::from_millis(300),
            state: Mutex::new(State::default()),
        }
    }

    /// Stop every server started.
    pub fn shutdown(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        for (_, session) in state.sessions.drain() {
            if let Ok(session) = session {
                session.shutdown();
            }
        }
    }
}

impl Oracle for LspOracle {
    fn ask(
        &self,
        path: &Path,
        source: &str,
        span: Range<usize>,
        query: &Query,
    ) -> Result<bool, String> {
        let file = self.root.join(path.strip_prefix(".").unwrap_or(path));
        let server = match &self.server {
            Some(server) => server.clone(),
            None => ServerCommand::default_for(&file).ok_or_else(|| {
                format!(
                    "`{query}`: no language server is known for {}; name one with --lsp",
                    path.display()
                )
            })?,
        };
        let key = server.display();
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let state = &mut *state;
        let fresh = !state.sessions.contains_key(&key);
        let session = state.sessions.entry(key.clone()).or_insert_with(|| {
            Session::start(
                &server.program,
                &server.args,
                &self.root,
                self.timeout,
                false,
            )
            .map_err(|e| format!("`{query}` needs `{key}`: {e}"))
        });
        let session = session.as_mut().map_err(|e| e.clone())?;
        let failed = |e: colab_core::Error| format!("`{query}`: `{key}` {e}");
        if state.sent.get(&file).map(String::as_str) != Some(source) {
            session.open(&file, source).map_err(failed)?;
            state.sent.insert(file.clone(), source.to_string());
        }
        if fresh {
            // Its first answer is worth having only once it has indexed.
            session.wait_until_quiet(self.settle).map_err(failed)?;
        }
        let (at, call) = asked_at(source, span.clone());
        let (line, character) = position_of(source, at, session.encoding);
        let params = json!({
            "textDocument": {"uri": path_to_uri(&file)},
            "position": {"line": line, "character": character},
        });
        let hover = |session: &mut Session| -> Result<String, String> {
            session
                .request_retrying("textDocument/hover", params.clone())
                .map_err(|e| format!("`{query}`: `{key}` {e}"))?
                .map(|answer| hover_text(&answer))
                .map_err(|e| {
                    format!(
                        "`{query}`: `{key}` {}",
                        session.server_error("textDocument/hover", &e)
                    )
                })
        };
        match query {
            Query::Type(ty) => {
                let text = hover(session)?;
                // A call's type is what its callee returns.
                let text = if call { returned(&text) } else { text.as_str() };
                Ok(contains_token(text, ty))
            }
            Query::Resolves(target) => {
                let answer = session
                    .request_retrying("textDocument/definition", params.clone())
                    .map_err(failed)?
                    .map_err(|e| failed(session.server_error("textDocument/definition", &e)))?;
                let Some((uri, line, character)) = first_location(&answer) else {
                    // Nothing to resolve to (a local, a builtin the server
                    // has no source for): it does not resolve there.
                    return Ok(false);
                };
                let def = uri_to_path(&uri).map_err(failed)?;
                let text = if def == file {
                    source.to_string()
                } else {
                    std::fs::read_to_string(&def)
                        .map_err(|e| format!("`{query}`: reading {}: {e}", def.display()))?
                };
                let name = offset_of(&text, line, character, session.encoding)
                    .ok()
                    .map(|offset| identifier_at(&text, offset))
                    .unwrap_or_default();
                if resolves_to(target, &def, &name) {
                    return Ok(true);
                }
                // A module that is not a file of its own (Rust's inline
                // `mod`, a Go package's import path): the hover names it.
                Ok(name_of(target) == name && named_in(target, &hover(session)?))
            }
        }
    }
}

fn word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Where to ask about `source[span]`, and whether it is a call: a binding
/// that ends in `(…)` is asked at its callee (`create_dir_all` in
/// `fs::create_dir_all(p)`, `d` in `a.b(c).d(e)`), anything else at its
/// last identifier.
fn asked_at(source: &str, span: Range<usize>) -> (usize, bool) {
    let text = &source[span.clone()];
    if text.trim_end().ends_with(')') {
        let mut depth = 0i32;
        for (i, c) in text.char_indices().rev() {
            match c {
                ')' => depth += 1,
                '(' => {
                    depth -= 1;
                    if depth == 0 {
                        let callee = span.start..span.start + i;
                        if text[..i].chars().any(word) {
                            return (last_identifier(source, callee), true);
                        }
                        break;
                    }
                }
                _ => {}
            }
        }
    }
    (last_identifier(source, span), false)
}

/// What a signature returns: in the hover's first code block (the
/// signature; the docs follow it), the text after its last `->`, up to a
/// `where` clause. Without `->`, the whole first block.
fn returned(hover: &str) -> &str {
    let block = match hover.find("```") {
        Some(open) => {
            let body = &hover[open + 3..];
            // Past the fence's language tag.
            let body = body.split_once('\n').map_or(body, |(_, rest)| rest);
            body.find("```").map_or(body, |close| &body[..close])
        }
        None => hover,
    };
    // A `where` clause's bounds have arrows of their own.
    let signature = block.find("\nwhere").map_or(block, |w| &block[..w]);
    match signature.rfind("->") {
        Some(at) => &signature[at + 2..],
        None => signature,
    }
}

/// The start of the last identifier in `source[span]`.
fn last_identifier(source: &str, span: Range<usize>) -> usize {
    let text = &source[span.clone()];
    let Some((end, _)) = text.char_indices().rev().find(|(_, c)| word(*c)) else {
        return span.start;
    };
    let start = text[..end]
        .char_indices()
        .rev()
        .take_while(|(_, c)| word(*c))
        .last()
        .map_or(end, |(i, _)| i);
    span.start + start
}

/// The identifier around byte `offset` of `text`.
fn identifier_at(text: &str, offset: usize) -> String {
    let offset = offset.min(text.len());
    let start = text[..offset]
        .char_indices()
        .rev()
        .take_while(|(_, c)| word(*c))
        .last()
        .map_or(offset, |(i, _)| i);
    text[start..].chars().take_while(|c| word(*c)).collect()
}

/// A hover's text, whatever shape the server sent it in.
fn hover_text(answer: &Value) -> String {
    fn flatten(v: &Value, out: &mut String) {
        match v {
            Value::String(s) => out.push_str(s),
            Value::Array(items) => {
                for item in items {
                    flatten(item, out);
                    out.push('\n');
                }
            }
            Value::Object(o) => {
                if let Some(value) = o.get("value") {
                    flatten(value, out);
                }
            }
            _ => {}
        }
    }
    let mut out = String::new();
    flatten(&answer["contents"], &mut out);
    out
}

/// `true` when `needle` occurs in `hay` with no identifier character just
/// before or after it.
fn contains_token(hay: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    hay.match_indices(needle).any(|(at, _)| {
        let before = hay[..at].chars().next_back();
        let after = hay[at + needle.len()..].chars().next();
        !before.is_some_and(word) && !after.is_some_and(word)
    })
}

/// The first `(uri, line, character)` of a definition answer: a
/// `Location`, a list of them, or a list of `LocationLink`s.
fn first_location(answer: &Value) -> Option<(String, u32, u32)> {
    let first = match answer {
        Value::Array(items) => items.first()?,
        Value::Null => return None,
        other => other,
    };
    let (uri, range) = match first.get("targetUri") {
        Some(uri) => (uri, first.get("targetSelectionRange")?),
        None => (first.get("uri")?, first.get("range")?),
    };
    Some((
        uri.as_str()?.to_string(),
        range.pointer("/start/line")?.as_u64()? as u32,
        range.pointer("/start/character")?.as_u64()? as u32,
    ))
}

/// `target`'s last segment, after `.`, `/` or `::`.
fn name_of(target: &str) -> &str {
    target.rsplit(['.', '/', ':']).next().unwrap_or(target)
}

/// Whether `hover` names `target`'s qualifier, as written or with its
/// separators swapped (`spike::mine` for `spike.mine`).
fn named_in(target: &str, hover: &str) -> bool {
    let qualifier = &target[..target.len() - name_of(target).len()];
    let qualifier = qualifier.trim_end_matches(['.', '/', ':']);
    if qualifier.is_empty() {
        return true;
    }
    [
        qualifier.to_string(),
        qualifier.replace('.', "::"),
        qualifier.replace("::", "."),
    ]
    .iter()
    .any(|q| contains_token(hover, q))
}

/// Whether a declaration named `name` in the file `def` is `target`.
fn resolves_to(target: &str, def: &Path, name: &str) -> bool {
    // `std::fs::read` and `github.com/pkg/errors.Wrap`: the name follows
    // the last `.`, `/` or `::`.
    let target = target.replace("::", "/");
    let at = target.rfind(['.', '/']);
    let (qualifier, wanted) = match at {
        Some(i) => (&target[..i], &target[i + 1..]),
        None => ("", target.as_str()),
    };
    if wanted != name {
        return false;
    }
    // Each component of the file's path, a version suffix dropped, and
    // the file's stem last.
    let mut components: Vec<String> = def
        .parent()
        .into_iter()
        .flat_map(|p| p.components())
        .map(|c| {
            let c = c.as_os_str().to_string_lossy();
            c.split('@').next().unwrap_or(&c).to_string()
        })
        .collect();
    if let Some(stem) = def.file_stem() {
        components.push(stem.to_string_lossy().into_owned());
    }
    // Segments in order: a `/` segment whole (`github.com`), else split
    // at its dots (`os.path`).
    let mut at = 0;
    let mut find = |segment: &str| match components[at..].iter().position(|c| c == segment) {
        Some(i) => {
            at += i + 1;
            true
        }
        None => false,
    };
    qualifier
        .split('/')
        .filter(|s| !s.is_empty())
        .all(|segment| find(segment) || segment.split('.').all(&mut find))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_question_is_asked_at_the_last_identifier() {
        let src = "x := pkg.Func(a)";
        assert_eq!(last_identifier(src, 5..13), 9, "Func in pkg.Func");
        assert_eq!(last_identifier(src, 5..16), 14, "a, the argument");
        assert_eq!(asked_at(src, 5..16), (9, true), "a call: its callee");
        assert_eq!(asked_at("a.b(c).d(e)", 0..11), (7, true));
        assert_eq!(asked_at("x.y", 0..3), (2, false));
        assert_eq!(
            returned("pub fn f(o: Option<u8>) -> io::Result<()>"),
            " io::Result<()>"
        );
        let hover = "```rust\npub fn w<F>(v: F) -> Result<Walk>\nwhere\n    F: FnMut() -> Option<()>,\n```\n\nDocs: `a` -> `b`";
        assert_eq!(returned(hover), " Result<Walk>");
        assert_eq!(identifier_at("func Wrap(err)", 7), "Wrap");
    }

    #[test]
    fn hover_text_matches_whole_types() {
        let hover =
            json!({"contents": {"kind": "markdown", "value": "```go\nvar r io.Reader\n```"}});
        let text = hover_text(&hover);
        assert!(contains_token(&text, "io.Reader"));
        assert!(!contains_token(&text, "io.Read"));
        let marked =
            json!({"contents": [{"language": "rust", "value": "let x: Option<String>"}, "docs"]});
        assert!(contains_token(&hover_text(&marked), "Option<String>"));
    }

    #[test]
    fn a_definition_resolves_by_name_and_path_segments() {
        let cache = Path::new("/home/u/go/pkg/mod/github.com/pkg/errors@v0.9.1/errors.go");
        assert!(resolves_to("github.com/pkg/errors.Wrap", cache, "Wrap"));
        assert!(!resolves_to("github.com/pkg/errors.Wrap", cache, "New"));
        assert!(!resolves_to("golang.org/x/xerrors.Wrap", cache, "Wrap"));
        let std = Path::new("/rust/library/std/src/fs.rs");
        assert!(resolves_to("std::fs::read", std, "read"));
        assert!(!resolves_to("tokio::fs::read", std, "read"));
        assert!(named_in(
            "spike::mine::read",
            "spike::mine\n```rust\npub fn read()```"
        ));
        assert!(!named_in(
            "tokio::fs::read",
            "std::fs\n```rust\npub fn read()```"
        ));
        assert!(named_in(
            "github.com/pkg/errors.Wrap",
            "[docs](https://pkg.go.dev/github.com/pkg/errors#Wrap)"
        ));
        let py = Path::new("/usr/lib/python3.12/os/path.py");
        assert!(resolves_to("os.path.join", py, "join"));
        let link = json!([{"targetUri": "file:///a.go", "targetSelectionRange": {"start": {"line": 3, "character": 5}}}]);
        assert_eq!(first_location(&link), Some(("file:///a.go".into(), 3, 5)));
        assert_eq!(first_location(&Value::Null), None);
    }
}
