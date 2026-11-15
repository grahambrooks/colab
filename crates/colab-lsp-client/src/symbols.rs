//! A file's declarations as the language server sees them
//! (`textDocument/documentSymbol`), for `colab edit --symbol`.
//!
//! Servers name things their own way; each name becomes the path segment
//! colab's tree-sitter declarations would use: rust-analyzer's
//! `impl Display for Config` is `Config`, and gopls's flat
//! `(*Config).Load` is `Load` under `Config`.

use std::path::Path;
use std::time::Duration;

use colab_core::symbols::Declaration;
use colab_core::{Error, Result};
use serde_json::{Value, json};

use crate::ServerCommand;
use crate::edit::{Encoding, lsp_u32, offset_of, path_to_uri};
use crate::session::Session;

/// The declarations in `file` (whose text is `text`), from `server`
/// started (or reached, if kept) in `root`.
pub fn document_symbols(
    server: &ServerCommand,
    root: &Path,
    file: &Path,
    text: &str,
    timeout: Duration,
) -> Result<Vec<Declaration>> {
    let mut session = Session::start(&server.program, &server.args, root, timeout, false)?;
    session.open(file, text)?;
    session.wait_until_quiet(Duration::from_millis(300))?;
    let answer = session
        .request_retrying(
            "textDocument/documentSymbol",
            json!({"textDocument": {"uri": path_to_uri(file)}}),
        )?
        .map_err(|e| session.server_error("textDocument/documentSymbol", &e))?;
    let encoding = session.encoding;
    session.shutdown();
    convert(&answer, text, encoding)
}

/// `answer` (`DocumentSymbol[]` or `SymbolInformation[]`) as declarations.
pub fn convert(answer: &Value, text: &str, encoding: Encoding) -> Result<Vec<Declaration>> {
    let mut top = Vec::new();
    for symbol in answer.as_array().into_iter().flatten() {
        if let Some(d) = one(symbol, text, encoding)? {
            top.push(d);
        }
    }
    // A flat `(*Config).Load`: under `Config`.
    let mut out: Vec<Declaration> = Vec::new();
    let mut methods = Vec::new();
    for (receiver, d) in top {
        match receiver {
            Some(r) => methods.push((r, d)),
            None => out.push(d),
        }
    }
    for (receiver, method) in methods {
        match out.iter_mut().find(|d| d.name == receiver) {
            Some(owner) => owner.children.push(method),
            None => out.push(Declaration {
                name: receiver,
                kind: "receiver".into(),
                range: method.range.clone(),
                children: vec![method],
            }),
        }
    }
    Ok(out)
}

fn last_word(text: &str) -> String {
    text.split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .rfind(|w| !w.is_empty())
        .unwrap_or("")
        .to_string()
}

/// A server's symbol name as a path segment, and the receiver type it
/// belongs to, if its name says so.
fn segment(name: &str) -> (String, Option<String>) {
    let name = name.trim();
    if let Some(rest) = name.strip_prefix("impl") {
        // Past `impl`'s own `<T>`, if it has one.
        let rest = rest.trim_start();
        let rest = match rest.strip_prefix('<') {
            Some(generic) => {
                let mut depth = 1;
                let close = generic
                    .char_indices()
                    .find(|&(_, c)| {
                        depth += match c {
                            '<' => 1,
                            '>' => -1,
                            _ => 0,
                        };
                        depth == 0
                    })
                    .map_or(generic.len(), |(i, _)| i + 1);
                &generic[close..]
            }
            None => rest,
        };
        let target = rest.rsplit_once(" for ").map_or(rest, |(_, t)| t);
        // `impl<T> Wrapper<T>`: the type, not its parameters.
        let target = target.split('<').next().unwrap_or(target);
        return (last_word(target), None);
    }
    if let Some((receiver, method)) = name.split_once(").") {
        return (method.to_string(), Some(last_word(receiver)));
    }
    (name.to_string(), None)
}

fn offset(text: &str, position: &Value, encoding: Encoding) -> Result<usize> {
    let line = position["line"].as_u64();
    let character = position["character"].as_u64();
    match (line, character) {
        (Some(l), Some(c)) => offset_of(text, lsp_u32(l), lsp_u32(c), encoding),
        _ => Err(Error::Config(
            "language server sent a symbol without a range".into(),
        )),
    }
}

fn one(
    symbol: &Value,
    text: &str,
    encoding: Encoding,
) -> Result<Option<(Option<String>, Declaration)>> {
    let Some(name) = symbol["name"].as_str() else {
        return Ok(None);
    };
    let range = symbol
        .get("range")
        .or_else(|| symbol.pointer("/location/range"))
        .unwrap_or(&Value::Null);
    if range.is_null() {
        return Ok(None);
    }
    let start = offset(text, &range["start"], encoding)?;
    let end = offset(text, &range["end"], encoding)?;
    let (name, receiver) = segment(name);
    let mut children = Vec::new();
    for child in symbol["children"].as_array().into_iter().flatten() {
        if let Some((_, d)) = one(child, text, encoding)? {
            children.push(d);
        }
    }
    Ok(Some((
        receiver,
        Declaration {
            name,
            kind: symbol["kind"]
                .as_u64()
                .map_or_else(|| "symbol".into(), |k| format!("lsp kind {k}")),
            range: start..end,
            children,
        },
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_names_become_path_segments() {
        assert_eq!(segment("impl Display for Config"), ("Config".into(), None));
        assert_eq!(segment("impl<T> Wrapper<T>"), ("Wrapper".into(), None));
        assert_eq!(
            segment("(*Config).Load"),
            ("Load".into(), Some("Config".into()))
        );
        assert_eq!(segment("load"), ("load".into(), None));
        let text = "type Config struct{}\nfunc (c *Config) Load() {}\n";
        let answer = json!([
            {"name": "Config", "kind": 23, "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 20}}},
            {"name": "(*Config).Load", "kind": 6, "range": {"start": {"line": 1, "character": 0}, "end": {"line": 1, "character": 26}}},
        ]);
        let decls = convert(&answer, text, Encoding::Utf16).unwrap();
        let found = colab_core::symbols::resolve(&decls, "Config/Load", text).unwrap();
        assert_eq!(
            &text[found.declaration.range.clone()],
            "func (c *Config) Load() {}"
        );
    }
}
