//! Rust source as a [`colab_syntax::Tree`], parsed by rust-analyzer's
//! parser (`ra_ap_syntax`): step 3 of the move off tree-sitter, for Rust
//! first (docs/development-plan.md, "Building colab's own parsers").
//!
//! rust-analyzer's tree is already lossless (every byte in a token) and
//! recovers from errors with `ERROR` nodes, so the adapter only walks it:
//! nodes become nodes, tokens tokens, whitespace [`Kind::TRIVIA`],
//! comments named `extra` tokens, and an error the parser reports without
//! an `ERROR` node (a missing `;`) is kept with [`Builder::error_seen`].
//! Kinds are rust-analyzer's, lower-cased (`call_expr`, `ident`); a token
//! is named unless it is punctuation or a keyword. tree-sitter's names
//! are kept for rules written against them (`names.rs`): its kinds as
//! aliases (`call_expression` is `call_expr` and `method_call_expr`) and
//! its fields (`function`, `name`, `body`, …), read from rust-analyzer's
//! typed AST and put on the children.

mod names;

use std::sync::{Arc, OnceLock};

use colab_syntax::{BuildError, Builder, Field, Flags, Grammar, Kind, SourceParser, Tree};
use ra_ap_syntax::{Edition, NodeOrToken, SourceFile, SyntaxKind, WalkEvent};

/// The edition keywords are judged in. A file's edition is its crate's,
/// which its text does not say, so a file is parsed as 2021 (where `gen`
/// is a name, as in most code) and, failing that, as 2024 when that reads
/// it with fewer errors; keywords are the same set in both but for `gen`.
const EDITION: Edition = Edition::Edition2021;

/// rust-analyzer's parser for colab.
pub struct RustParser;

/// The one parser; it holds nothing but its grammar.
pub static PARSER: RustParser = RustParser;

/// A comment, which matching passes over: a plain one (a token), or a doc
/// comment (a `doc_comment` node holding an inner or outer token).
fn comment(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        SyntaxKind::COMMENT
            | SyntaxKind::DOC_COMMENT
            | SyntaxKind::INNER_DOC_COMMENT
            | SyntaxKind::OUTER_DOC_COMMENT
    )
}

fn named(kind: SyntaxKind) -> bool {
    !(kind.is_punct() || kind.is_keyword(EDITION) || kind == SyntaxKind::WHITESPACE)
}

fn grammar() -> Arc<Grammar> {
    static GRAMMAR: OnceLock<Arc<Grammar>> = OnceLock::new();
    GRAMMAR
        .get_or_init(|| {
            let last = SyntaxKind::__LAST as u16;
            let kinds = (0..last)
                .map(|id| {
                    let kind = SyntaxKind::from(id);
                    let name = format!("{kind:?}").to_lowercase();
                    (name.into_boxed_str(), named(kind))
                })
                .collect();
            let fields = names::FIELDS.iter().map(|f| (*f).into()).collect();
            let grammar = names::ALIASES
                .iter()
                .fold(Grammar::new("rust", kinds, fields), |g, (alias, kinds)| {
                    g.with_alias(alias, kinds)
                });
            Arc::new(grammar)
        })
        .clone()
}

/// The field id of `name` (in `names::FIELDS`).
fn field_id(name: &str) -> Option<Field> {
    let i = names::FIELDS.iter().position(|f| *f == name)?;
    u16::try_from(i).ok().map(Field)
}

fn kind_of(kind: SyntaxKind) -> Kind {
    if kind == SyntaxKind::ERROR {
        Kind::ERROR
    } else {
        Kind(kind as u16)
    }
}

/// How many syntax errors a parse has. `Parse::errors` adds
/// rust-analyzer's validation (an unknown escape, a raw `\r` in a CRLF
/// file's doc string), which is not a syntax error; the parser's own are
/// zero-width (where the token it expected is missing) and its recovery
/// leaves `ERROR` nodes. A lexer error with a span (an unterminated
/// string) is not counted, which tree-sitter would read as an error.
fn syntax_errors(parse: &ra_ap_syntax::Parse<SourceFile>) -> usize {
    let parser = parse
        .errors()
        .iter()
        .filter(|e| e.range().is_empty())
        .count();
    let nodes = parse
        .syntax_node()
        .descendants()
        .filter(|n| n.kind() == SyntaxKind::ERROR)
        .count();
    parser + nodes
}

/// rust-analyzer's own crate, for the Rust backend's modules that read
/// its typed AST (`ast::Use`, …) rather than colab's tree. Pinned with it.
pub use ra_ap_syntax as ra;

/// `text` parsed by rust-analyzer, as 2021 and, when that reads it with
/// errors, as 2024 if that reads it with fewer; with its syntax errors.
pub fn source_file(text: &str) -> (ra_ap_syntax::Parse<SourceFile>, usize) {
    let parse = SourceFile::parse(text, EDITION);
    let errors = syntax_errors(&parse);
    if errors > 0 {
        let newer = SourceFile::parse(text, Edition::Edition2024);
        let newer_errors = syntax_errors(&newer);
        if newer_errors < errors {
            return (newer, newer_errors);
        }
    }
    (parse, errors)
}

/// `text` as a colab tree.
pub fn parse(text: &str) -> Result<Tree, BuildError> {
    let (parse, errors) = source_file(text);
    let root = parse.syntax_node();
    let mut b = Builder::new(text, grammar());
    if errors > 0 {
        b.error_seen();
    }
    // A parser error with no node (an expected token that is not there)
    // becomes a zero-width missing token where it was expected, as
    // tree-sitter's `MISSING`, so a pattern that parses only with one is
    // not taken as clean.
    let mut missing: Vec<usize> = parse
        .errors()
        .iter()
        .filter(|e| e.range().is_empty())
        .map(|e| usize::from(e.range().start()))
        .collect();
    missing.sort_unstable();
    missing.dedup();
    let mut next_missing = 0usize;
    let missing_flags = Flags {
        missing: true,
        ..Flags::default()
    };
    let fields = names::fields(&root);
    let mut depth = 0usize;
    for event in root.preorder_with_tokens() {
        if let WalkEvent::Enter(NodeOrToken::Token(token)) = &event {
            let at = usize::from(token.text_range().start());
            while missing.get(next_missing).is_some_and(|&m| m <= at) {
                b.token(Kind::ERROR, None, missing_flags, 0)?;
                next_missing += 1;
            }
        }
        match event {
            WalkEvent::Enter(NodeOrToken::Node(node)) => {
                let kind = node.kind();
                let r = node.text_range();
                let field = fields
                    .get(&(kind, r.start().into(), r.end().into()))
                    .and_then(|f| field_id(f));
                b.start_node(
                    kind_of(kind),
                    field,
                    Flags {
                        named: true,
                        extra: comment(kind),
                        ..Flags::default()
                    },
                )?;
                depth += 1;
            }
            WalkEvent::Leave(NodeOrToken::Node(_)) => {
                depth -= 1;
                // The root is closed by `finish`, after any missing token
                // expected at the end of the text.
                if depth > 0 {
                    b.finish_node()?;
                } else {
                    for _ in next_missing..missing.len() {
                        b.token(Kind::ERROR, None, missing_flags, 0)?;
                    }
                }
            }
            WalkEvent::Enter(NodeOrToken::Token(token)) => {
                let kind = token.kind();
                let len = usize::from(token.text_range().len());
                if kind == SyntaxKind::WHITESPACE {
                    b.trivia_to(b.pos() + len)?;
                } else {
                    let flags = Flags {
                        named: named(kind),
                        extra: comment(kind),
                        ..Flags::default()
                    };
                    b.token(kind_of(kind), None, flags, len)?;
                }
            }
            WalkEvent::Leave(NodeOrToken::Token(_)) => {}
        }
    }
    b.finish()
}

impl SourceParser for RustParser {
    fn grammar(&self) -> Arc<Grammar> {
        grammar()
    }

    fn parse(&self, text: &str) -> Tree {
        // The adapter only fails on a tree that is not lossless, which
        // rust-analyzer's always is; an empty tree is the safe answer.
        parse(text).unwrap_or_else(|_| {
            let mut b = Builder::new(text, grammar());
            let _ = b.start_node(Kind(SyntaxKind::SOURCE_FILE as u16), None, Flags::default());
            b.error_seen();
            b.finish()
                .unwrap_or_else(|_| unreachable!("a root over the text always builds"))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "// leading\nuse std::fmt;\n\n/// Doc.\npub struct S { a: u32 }\n\nfn broken(a: i32) -> i32 {\n    a + ( let = ;\n}\n\n/* block */ fn last() -> usize { f(1, \"é\") }\n";

    #[test]
    fn a_parse_is_lossless_and_recovers() {
        let tree = parse(SRC).unwrap();
        let text: String = tree.tokens().map(|t| t.text()).collect();
        assert_eq!(text, SRC);
        assert!(tree.has_error());
        let fns: Vec<&str> = tree
            .nodes()
            .filter(|n| n.kind_name() == "fn")
            .map(|n| {
                n.children()
                    .find(|c| c.kind_name() == "name")
                    .map_or("", |c| c.text())
            })
            .collect();
        assert_eq!(
            fns,
            ["broken", "last"],
            "the function after the error is whole"
        );
    }

    #[test]
    fn comments_are_extras_and_whitespace_is_trivia() {
        let tree = parse("fn f() { /* c */ g(); }\n").unwrap();
        let comment = tree.nodes().find(|n| n.kind_name() == "comment").unwrap();
        assert!(comment.is_extra() && comment.is_named());
        assert!(tree.nodes().any(|n| n.is_trivia()));
        let semi = tree.nodes().find(|n| n.text() == ";").unwrap();
        assert!(!semi.is_named(), "punctuation is anonymous");
        let fn_kw = tree.nodes().find(|n| n.text() == "fn").unwrap();
        assert!(!fn_kw.is_named(), "keywords are anonymous");
        // An identifier is a token under single-child nodes
        // (`path_expr` > `path` > `path_segment` > `name_ref` > `ident`).
        let ident = tree
            .nodes()
            .find(|n| n.is_token() && n.text() == "g")
            .unwrap();
        assert!(ident.is_named() && ident.kind_name() == "ident");
    }

    #[test]
    fn a_missing_token_is_still_an_error() {
        // No `ERROR` node, only a reported error: the missing `;`.
        let tree = parse("fn f() { let x = 1 }\n").unwrap();
        assert!(tree.has_error());
        assert!(!parse("fn f() { let x = 1; }\n").unwrap().has_error());
    }
}
