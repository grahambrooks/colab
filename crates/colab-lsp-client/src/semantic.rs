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
//! - `implements "<path>.<name>"`: `textDocument/prepareTypeHierarchy`,
//!   then `typeHierarchy/supertypes` up to 200 types deep, which holds when
//!   a supertype's declaration resolves to the target as above.
//! - `calls` / `called_by "<path>.<name>"`: `textDocument/prepareCallHierarchy`,
//!   then `callHierarchy/outgoingCalls` / `incomingCalls`, which holds when
//!   a callee / caller resolves to the target.
//!
//! [`Oracle::ask_all`] answers a file's `type` / `resolves` questions with
//! their requests pipelined ([`Session::request_all`]), so a batch costs
//! about one round trip; the hierarchy questions are asked in turn.
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
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for (_, session) in state.sessions.drain() {
            if let Ok(session) = session {
                session.shutdown();
            }
        }
    }
}

/// A server ready to answer about one file: started, indexed, and sent
/// the file's text as the run has it.
struct Ready<'s> {
    session: &'s mut Session,
    /// The server's command line, for messages.
    key: String,
    file: PathBuf,
}

impl LspOracle {
    /// Start (once) the server for `path`, send it `source`, and run `f`
    /// with it.
    fn with_server<T>(
        &self,
        path: &Path,
        source: &str,
        query: &Query,
        f: impl FnOnce(&mut Ready<'_>) -> Result<T, String>,
    ) -> Result<T, String> {
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
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
        f(&mut Ready { session, key, file })
    }
}

impl Ready<'_> {
    fn failed(&self, query: &Query, e: &colab_core::Error) -> String {
        format!("`{query}`: `{}` {e}", self.key)
    }

    /// `textDocument/…` params at the binding's last identifier, and
    /// whether the binding is a call.
    fn position(&self, source: &str, span: Range<usize>) -> (Value, bool) {
        let (at, call) = asked_at(source, span);
        let (line, character) = position_of(source, at, self.session.encoding);
        (
            json!({
                "textDocument": {"uri": path_to_uri(&self.file)},
                "position": {"line": line, "character": character},
            }),
            call,
        )
    }

    fn request(&mut self, query: &Query, method: &str, params: Value) -> Result<Value, String> {
        let answer = self
            .session
            .request_retrying(method, params)
            .map_err(|e| self.failed(query, &e))?;
        answer.map_err(|e| {
            let e = self.session.server_error(method, &e);
            self.failed(query, &e)
        })
    }

    /// Several requests of one method, sent together.
    fn request_all(
        &mut self,
        query: &Query,
        method: &str,
        params: Vec<Value>,
    ) -> Result<Vec<Result<Value, String>>, String> {
        let answers = self
            .session
            .request_all(method, params)
            .map_err(|e| self.failed(query, &e))?;
        Ok(answers
            .into_iter()
            .map(|answer| {
                answer.map_err(|e| {
                    let e = self.session.server_error(method, &e);
                    self.failed(query, &e)
                })
            })
            .collect())
    }

    fn hover(&mut self, query: &Query, params: Value) -> Result<String, String> {
        Ok(hover_text(&self.request(
            query,
            "textDocument/hover",
            params,
        )?))
    }

    /// Whether a `definition` answer lands on a declaration that resolves
    /// to `target`; the hover at `params` settles a module the server
    /// cannot point into.
    fn resolves(
        &mut self,
        query: &Query,
        source: &str,
        target: &str,
        answer: &Value,
        params: Value,
    ) -> Result<bool, String> {
        let Some((uri, line, character)) = first_location(answer) else {
            // Nothing to resolve to (a local, a builtin the server has no
            // source for): it does not resolve there.
            return Ok(false);
        };
        let def = uri_to_path(&uri).map_err(|e| self.failed(query, &e))?;
        let text = if def == self.file {
            source.to_string()
        } else {
            std::fs::read_to_string(&def)
                .map_err(|e| format!("`{query}`: reading {}: {e}", def.display()))?
        };
        let name = offset_of(&text, line, character, self.session.encoding)
            .ok()
            .map_or_default(|offset| identifier_at(&text, offset));
        if resolves_to(target, &def, &name) {
            return Ok(true);
        }
        // A module that is not a file of its own (Rust's inline `mod`, a
        // Go package's import path): the hover names it.
        Ok(name_of(target) == name && named_in(target, &self.hover(query, params)?))
    }

    /// Whether a hierarchy item (`TypeHierarchyItem`, `CallHierarchyItem`)
    /// is a declaration that resolves to `target`.
    fn item_resolves(&self, target: &str, item: &Value) -> bool {
        let name = item["name"].as_str().unwrap_or("");
        // `Iterator<Item = T>`, `(*T).Read`: the identifier the item names.
        let name: String = name
            .rsplit(['.', ':'])
            .next()
            .unwrap_or(name)
            .chars()
            .take_while(|c| word(*c))
            .collect();
        item["uri"]
            .as_str()
            .and_then(|uri| uri_to_path(uri).ok())
            .is_some_and(|def| resolves_to(target, &def, &name))
    }

    /// `implements`: a supertype, at any depth, that resolves to `target`.
    fn implements(&mut self, query: &Query, target: &str, params: Value) -> Result<bool, String> {
        let prepared = self.request(query, "textDocument/prepareTypeHierarchy", params)?;
        let mut queue: Vec<Value> = prepared.as_array().cloned().unwrap_or_default();
        let mut seen = std::collections::HashSet::new();
        // Bounded: a hierarchy is shallow, a cycle is not.
        while let Some(item) = queue.pop() {
            if seen.len() >= HIERARCHY_LIMIT || !seen.insert(item.to_string()) {
                continue;
            }
            let supers = self.request(query, "typeHierarchy/supertypes", json!({"item": item}))?;
            for parent in supers.as_array().into_iter().flatten() {
                if self.item_resolves(target, parent) {
                    return Ok(true);
                }
                queue.push(parent.clone());
            }
        }
        Ok(false)
    }

    /// `calls` / `called_by`: a callee (`outgoingCalls`, under `to`) or a
    /// caller (`incomingCalls`, under `from`) that resolves to `target`.
    fn call_edge(
        &mut self,
        query: &Query,
        target: &str,
        params: Value,
        outgoing: bool,
    ) -> Result<bool, String> {
        let prepared = self.request(query, "textDocument/prepareCallHierarchy", params)?;
        let (method, end) = if outgoing {
            ("callHierarchy/outgoingCalls", "to")
        } else {
            ("callHierarchy/incomingCalls", "from")
        };
        for item in prepared.as_array().into_iter().flatten() {
            let calls = self.request(query, method, json!({"item": item}))?;
            if calls
                .as_array()
                .into_iter()
                .flatten()
                .any(|call| self.item_resolves(target, &call[end]))
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// One question about the binding at `span`.
    fn answer(&mut self, source: &str, span: Range<usize>, query: &Query) -> Result<bool, String> {
        let (params, call) = self.position(source, span);
        match query {
            Query::Type(ty) => {
                let text = self.hover(query, params)?;
                // A call's type is what its callee returns.
                let text = if call { returned(&text) } else { text.as_str() };
                Ok(contains_token(text, ty))
            }
            Query::Resolves(target) => {
                let answer = self.request(query, "textDocument/definition", params.clone())?;
                self.resolves(query, source, target, &answer, params)
            }
            Query::Implements(target) => self.implements(query, target, params),
            Query::Calls(target) => self.call_edge(query, target, params, true),
            Query::CalledBy(target) => self.call_edge(query, target, params, false),
        }
    }
}

/// How many types `implements` walks before it stops looking.
const HIERARCHY_LIMIT: usize = 200;

impl Oracle for LspOracle {
    fn ask(
        &self,
        path: &Path,
        source: &str,
        span: Range<usize>,
        query: &Query,
    ) -> Result<bool, String> {
        self.with_server(path, source, query, |ready| {
            ready.answer(source, span, query)
        })
    }

    /// `type` and `resolves` about a file's bindings, each method's
    /// requests sent together; the hierarchy questions, which take more
    /// than one step each, are asked in turn.
    fn ask_all(
        &self,
        path: &Path,
        source: &str,
        spans: &[Range<usize>],
        query: &Query,
    ) -> Vec<Result<bool, String>> {
        let batch = self.with_server(path, source, query, |ready| {
            let asked: Vec<(Value, bool)> = spans
                .iter()
                .map(|s| ready.position(source, s.clone()))
                .collect();
            let params = || asked.iter().map(|(p, _)| p.clone()).collect::<Vec<_>>();
            Ok(match query {
                Query::Type(ty) => ready
                    .request_all(query, "textDocument/hover", params())?
                    .into_iter()
                    .zip(&asked)
                    .map(|(answer, (_, call))| {
                        let text = hover_text(&answer?);
                        let text = if *call {
                            returned(&text)
                        } else {
                            text.as_str()
                        };
                        Ok(contains_token(text, ty))
                    })
                    .collect(),
                Query::Resolves(target) => {
                    let answers = ready.request_all(query, "textDocument/definition", params())?;
                    answers
                        .into_iter()
                        .zip(&asked)
                        .map(|(answer, (p, _))| {
                            ready.resolves(query, source, target, &answer?, p.clone())
                        })
                        .collect()
                }
                Query::Implements(_) | Query::Calls(_) | Query::CalledBy(_) => spans
                    .iter()
                    .map(|span| ready.answer(source, span.clone(), query))
                    .collect(),
            })
        });
        match batch {
            Ok(answers) => answers,
            // The server could not be reached at all: every question fails
            // with the same reason.
            Err(why) => spans.iter().map(|_| Err(why.clone())).collect(),
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
        u32::try_from(range.pointer("/start/line")?.as_u64()?).ok()?,
        u32::try_from(range.pointer("/start/character")?.as_u64()?).ok()?,
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
