//! PHP read by recursive descent into a tree of [`Item`]s: tree-sitter-php's
//! `php_only` grammar (statements here, declarations, types and parameters
//! in `parser/decl.rs`, expressions and strings in `parser/expr.rs`), kinds
//! and fields named as in `names.rs`.
//!
//! Keywords are matched whatever their case and named in lower case. A
//! construct the parser cannot read becomes an `ERROR` node and reading
//! resumes at the next statement; a token that should be there and is not
//! is a missing one. A node whose kind is `""` is transparent: its
//! children are emitted in its place.

mod decl;
mod expr;

use crate::lexer::{L, Lx};
use crate::names;

/// A node or a token of the tree being built.
#[derive(Debug, Clone)]
pub(crate) enum Item {
    Tok(Tok),
    Node(Node),
}

#[derive(Debug, Clone)]
pub(crate) struct Tok {
    pub kind: &'static str,
    pub named: bool,
    pub start: usize,
    pub end: usize,
    pub field: Option<&'static str>,
    /// Text inside the node it is in, no token of its own.
    pub hidden: bool,
    pub missing: bool,
    pub extra: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct Node {
    pub kind: &'static str,
    pub field: Option<&'static str>,
    pub items: Vec<Item>,
}

impl Item {
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Item::Tok(t) => t.kind,
            Item::Node(n) => n.kind,
        }
    }

    pub(crate) fn with(mut self, field: &'static str) -> Self {
        match &mut self {
            Item::Tok(t) => t.field = Some(field),
            Item::Node(n) => {
                if n.kind.is_empty() {
                    for i in &mut n.items {
                        let taken = std::mem::replace(i, Item::Tok(placeholder()));
                        *i = taken.with(field);
                    }
                } else {
                    n.field = Some(field);
                }
            }
        }
        self
    }
}

fn field_of(item: &Item) -> Option<&'static str> {
    match item {
        Item::Tok(t) => t.field,
        Item::Node(n) => n.field,
    }
}

/// The trailing `alternative`s of the `if` statement `item` ends in
/// (through loop bodies and `if`s without them), taken out of it.
fn take_tail_alternatives(item: &mut Item) -> Vec<Item> {
    let Item::Node(n) = item else {
        return Vec::new();
    };
    match n.kind {
        "if_statement" => {
            let alts = n
                .items
                .iter()
                .rev()
                .take_while(|i| field_of(i) == Some("alternative"))
                .count();
            if alts > 0 {
                let at = n.items.len() - alts;
                return n.items.split_off(at);
            }
        }
        "while_statement" | "for_statement" | "foreach_statement" => {}
        _ => return Vec::new(),
    }
    match n.items.last_mut() {
        Some(last) if field_of(last) == Some("body") => take_tail_alternatives(last),
        _ => Vec::new(),
    }
}

fn placeholder() -> Tok {
    Tok {
        kind: "",
        named: false,
        start: 0,
        end: 0,
        field: None,
        hidden: true,
        missing: false,
        extra: false,
    }
}

pub(crate) fn node(kind: &'static str, items: Vec<Item>) -> Item {
    Item::Node(Node {
        kind,
        field: None,
        items,
    })
}

/// tree-sitter-php's reserved words: never a plain `name`.
pub(crate) const RESERVED: &[&str] = &[
    "abstract",
    "and",
    "as",
    "break",
    "callable",
    "case",
    "catch",
    "class",
    "clone",
    "const",
    "continue",
    "declare",
    "default",
    "do",
    "echo",
    "else",
    "elseif",
    "enddeclare",
    "endfor",
    "endforeach",
    "endif",
    "endswitch",
    "endwhile",
    "extends",
    "final",
    "finally",
    "fn",
    "for",
    "foreach",
    "function",
    "global",
    "goto",
    "if",
    "implements",
    "include",
    "include_once",
    "instanceof",
    "insteadof",
    "interface",
    "match",
    "namespace",
    "new",
    "or",
    "print",
    "private",
    "protected",
    "public",
    "readonly",
    "require",
    "require_once",
    "return",
    "static",
    "switch",
    "throw",
    "trait",
    "try",
    "use",
    "var",
    "while",
    "xor",
    "yield",
];

/// What ends a run of statements in a colon block or a switch case.
const BLOCK_ENDS: &[&str] = &[
    "elseif",
    "else",
    "endif",
    "endwhile",
    "endfor",
    "endforeach",
    "endswitch",
    "enddeclare",
];

pub(crate) struct Parser<'s> {
    src: &'s str,
    toks: Vec<Lx>,
    i: usize,
    pub errors: bool,
}

impl<'s> Parser<'s> {
    pub fn new(src: &'s str, toks: Vec<Lx>) -> Self {
        Self {
            src,
            toks,
            i: 0,
            errors: false,
        }
    }

    // ---- tokens ----------------------------------------------------------

    fn kind_at(&self, n: usize) -> L {
        self.toks.get(n).map_or(L::Eof, |t| t.k)
    }

    fn peek(&self) -> L {
        self.kind_at(self.i)
    }

    fn tk(&self, n: usize) -> Lx {
        self.toks.get(n).copied().unwrap_or(Lx {
            k: L::Eof,
            start: self.src.len(),
            end: self.src.len(),
        })
    }

    fn word_at(&self, n: usize) -> &'s str {
        match self.toks.get(n) {
            Some(t) if t.k == L::Name => &self.src[t.start..t.end],
            _ => "",
        }
    }

    fn word(&self) -> &'s str {
        self.word_at(self.i)
    }

    /// Whether token `n` is the keyword `w` (any case).
    fn kw_at(&self, n: usize, w: &str) -> bool {
        self.word_at(n).eq_ignore_ascii_case(w)
    }

    fn at_kw(&self, w: &str) -> bool {
        self.kw_at(self.i, w)
    }

    /// Whether token `n` is the operator `s`.
    fn is(&self, n: usize, s: &str) -> bool {
        matches!(self.kind_at(n), L::Op(o) if o == s)
    }

    fn at(&self, s: &str) -> bool {
        self.is(self.i, s)
    }

    /// Whether token `n` can be a plain `name` (not a reserved word).
    fn is_name(&self, n: usize) -> bool {
        self.kind_at(n) == L::Name
            && !RESERVED
                .iter()
                .any(|r| r.eq_ignore_ascii_case(self.word_at(n)))
    }

    fn eof(&self) -> bool {
        self.peek() == L::Eof
    }

    /// Whether tokens `n` and `n + 1` touch.
    fn adjacent(&self, n: usize) -> bool {
        match (self.toks.get(n), self.toks.get(n + 1)) {
            (Some(a), Some(b)) => a.end == b.start,
            _ => false,
        }
    }

    fn advance(&mut self) -> Lx {
        let t = self.tk(self.i);
        if t.k != L::Eof {
            self.i += 1;
        }
        t
    }

    fn tok(t: Lx, kind: &'static str, named: bool) -> Item {
        Item::Tok(Tok {
            kind,
            named,
            start: t.start,
            end: t.end,
            field: None,
            hidden: false,
            missing: false,
            extra: false,
        })
    }

    fn hidden_tok(t: Lx) -> Item {
        Item::Tok(Tok {
            kind: "",
            named: false,
            start: t.start,
            end: t.end,
            field: None,
            hidden: true,
            missing: false,
            extra: false,
        })
    }

    fn leaf(&mut self, kind: &'static str) -> Item {
        let t = self.advance();
        Self::tok(t, kind, true)
    }

    /// The current token, anonymous, named by its (lower-cased) text.
    fn punct(&mut self) -> Item {
        let t = self.advance();
        let kind = match t.k {
            L::Op(o) | L::StrOpen(o) | L::StrClose(o) => o,
            _ => names::anonymous(&self.src[t.start..t.end]).unwrap_or_else(|| {
                self.errors = true;
                "ERROR"
            }),
        };
        Self::tok(t, kind, false)
    }

    /// A keyword token wrapped in a node of `kind`.
    fn wrapped(&mut self, kind: &'static str) -> Item {
        let t = self.punct();
        node(kind, vec![t])
    }

    fn prev_end(&self) -> usize {
        if self.i == 0 {
            0
        } else {
            self.toks[self.i - 1].end
        }
    }

    fn missing(&mut self, kind: &'static str, named: bool) -> Item {
        self.errors = true;
        let at = self.prev_end();
        Item::Tok(Tok {
            kind,
            named,
            start: at,
            end: at,
            field: None,
            hidden: false,
            missing: true,
            extra: false,
        })
    }

    fn expect(&mut self, s: &'static str) -> Item {
        if self.at(s) {
            self.punct()
        } else {
            self.missing(s, false)
        }
    }

    fn expect_kw(&mut self, w: &'static str) -> Item {
        if self.at_kw(w) {
            self.punct()
        } else {
            self.missing(w, false)
        }
    }

    /// Any word as a `name` (where reserved words are names too).
    fn any_name(&mut self) -> Item {
        if self.peek() == L::Name {
            self.leaf("name")
        } else {
            self.missing("name", true)
        }
    }

    /// A plain `name`.
    fn name(&mut self) -> Item {
        if self.is_name(self.i) {
            self.leaf("name")
        } else {
            self.missing("name", true)
        }
    }

    fn error_token(&mut self) -> Item {
        self.errors = true;
        let t = self.advance();
        Self::tok(t, "ERROR", false)
    }

    /// The statement's `;`: there, or before `?>` (no token), or missing.
    fn semicolon(&mut self, items: &mut Vec<Item>) {
        if self.at(";") {
            items.push(self.punct());
        } else if self.peek() != L::EndTag {
            items.push(self.missing(";", false));
        }
    }

    /// The rest of the statement (to its `;`, or before a `}`) as an
    /// `ERROR` node; at least one token.
    fn error_statement(&mut self) -> Item {
        self.errors = true;
        let mut items = Vec::new();
        let mut depth = 0usize;
        while !self.eof() {
            if depth == 0 && (self.at("}") || (self.at("{") && !items.is_empty())) {
                break;
            }
            if self.at("(") || self.at("[") || self.at("{") {
                depth += 1;
            } else if (self.at(")") || self.at("]") || self.at("}")) && depth > 0 {
                depth -= 1;
            }
            let semi = depth == 0 && self.at(";");
            let t = self.advance();
            items.push(Self::tok(t, "ERROR", false));
            if semi {
                break;
            }
        }
        if items.is_empty() && !self.eof() {
            let t = self.advance();
            items.push(Self::tok(t, "ERROR", false));
        }
        node("ERROR", items)
    }

    /// `item (, item)*` until `close` (not taken), a trailing comma allowed
    /// when `trailing`.
    fn comma_list(
        &mut self,
        items: &mut Vec<Item>,
        close: &str,
        trailing: bool,
        mut item: impl FnMut(&mut Self) -> Item,
    ) {
        if self.at(close) {
            return;
        }
        loop {
            let before = self.i;
            items.push(item(self));
            if self.at(",") {
                items.push(self.punct());
                if trailing && self.at(close) {
                    return;
                }
                continue;
            }
            if self.i == before && !self.at(close) && !self.eof() {
                items.push(self.error_token());
                continue;
            }
            return;
        }
    }

    /// Past the bracket at `n` and its match.
    fn scan_balanced(&self, n: usize) -> Option<usize> {
        let mut depth = 0usize;
        let mut k = n;
        loop {
            match self.kind_at(k) {
                L::Eof => return None,
                L::Op("(" | "[" | "{" | "#[") => depth += 1,
                L::Op(")" | "]" | "}") => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return Some(k + 1);
                    }
                }
                _ => {}
            }
            k += 1;
        }
    }

    // ---- the file and statements -----------------------------------------

    pub(crate) fn program(&mut self) -> Item {
        let mut items = Vec::new();
        if self.peek() == L::Tag {
            items.push(self.leaf("php_tag"));
        }
        while !self.eof() {
            if self.peek() == L::EndTag {
                if self.kind_at(self.i + 1) == L::Eof {
                    items.push(self.leaf("php_end_tag"));
                } else {
                    items.push(self.error_token());
                }
                continue;
            }
            let before = self.i;
            items.push(self.statement());
            if self.i == before {
                items.push(self.error_token());
            }
        }
        node("program", items)
    }

    /// Statements until `}`, a block's end keyword or the end.
    fn statements_until(
        &mut self,
        items: &mut Vec<Item>,
        ends: &[&str],
        field: Option<&'static str>,
    ) {
        while !self.eof() && !self.at("}") && !ends.iter().any(|w| self.at_kw(w)) {
            if self.peek() == L::EndTag {
                break;
            }
            let before = self.i;
            let s = self.statement();
            items.push(match field {
                Some(f) => s.with(f),
                None => s,
            });
            if self.i == before {
                items.push(self.error_token());
            }
        }
    }

    pub(crate) fn compound(&mut self) -> Item {
        let mut items = vec![self.expect("{")];
        self.statements_until(&mut items, &[], None);
        items.push(self.expect("}"));
        node("compound_statement", items)
    }

    pub(crate) fn statement(&mut self) -> Item {
        match self.peek() {
            L::Op(";") => {
                let s = self.punct();
                return node("empty_statement", vec![s]);
            }
            L::Op("{") => return self.compound(),
            L::Op("#[") => {
                if let Some(d) = self.attributed_declaration() {
                    return d;
                }
            }
            L::Name => {
                if let Some(s) = self.keyword_statement() {
                    return s;
                }
            }
            L::Op(")" | "]") => return self.error_statement(),
            _ => {}
        }
        self.expression_statement()
    }

    fn expression_statement(&mut self) -> Item {
        let e = self.expression();
        let mut items = vec![e];
        self.semicolon(&mut items);
        node("expression_statement", items)
    }

    fn keyword_statement(&mut self) -> Option<Item> {
        let w = self.word().to_ascii_lowercase();
        let next = self.kind_at(self.i + 1);
        Some(match w.as_str() {
            "if" => self.if_statement(),
            "switch" => self.switch_statement(),
            "while" => self.while_statement(),
            "do" => self.do_statement(),
            "for" => self.for_statement(),
            "foreach" => self.foreach_statement(),
            "goto" => {
                let mut items = vec![self.punct(), self.name()];
                self.semicolon(&mut items);
                node("goto_statement", items)
            }
            "continue" | "break" | "return" => {
                let kind = match w.as_str() {
                    "continue" => "continue_statement",
                    "break" => "break_statement",
                    _ => "return_statement",
                };
                let mut items = vec![self.punct()];
                if !self.at(";") && self.peek() != L::EndTag && !self.at("}") {
                    items.push(self.expression());
                }
                self.semicolon(&mut items);
                node(kind, items)
            }
            "try" => self.try_statement(),
            "declare" if next == L::Op("(") => self.declare_statement(),
            "echo" => {
                let mut items = vec![self.punct(), self.expressions()];
                self.semicolon(&mut items);
                node("echo_statement", items)
            }
            "exit" => {
                let mut items = vec![self.punct()];
                if self.at("(") {
                    items.push(self.punct());
                    if !self.at(")") {
                        items.push(self.expression());
                    }
                    items.push(self.expect(")"));
                }
                self.semicolon(&mut items);
                node("exit_statement", items)
            }
            "unset" if next == L::Op("(") => {
                let mut items = vec![self.punct(), self.punct()];
                self.comma_list(&mut items, ")", true, Self::expression);
                items.push(self.expect(")"));
                self.semicolon(&mut items);
                node("unset_statement", items)
            }
            "const" => self.const_declaration(Vec::new(), false),
            "global" => {
                let mut items = vec![self.punct()];
                self.comma_list(&mut items, ";", false, Self::simple_variable);
                self.semicolon(&mut items);
                node("global_declaration", items)
            }
            "static" if next == L::Op("$") => {
                let mut items = vec![self.punct()];
                self.comma_list(&mut items, ";", false, |p| {
                    let mut v = vec![p.simple_variable().with("name")];
                    if p.at("=") {
                        v.push(p.punct());
                        v.push(p.expression().with("value"));
                    }
                    node("static_variable_declaration", v)
                });
                self.semicolon(&mut items);
                node("function_static_declaration", items)
            }
            "namespace" if next != L::Op("\\") => self.namespace_definition(),
            "use" => self.namespace_use_declaration(),
            "function"
                if self.kind_at(self.i + 1) == L::Name
                    || (next == L::Op("&") && self.kind_at(self.i + 2) == L::Name) =>
            {
                self.function_definition(Vec::new())
            }
            "class" | "interface" | "trait" => self.type_declaration(Vec::new()),
            "enum" if self.kind_at(self.i + 1) == L::Name => self.type_declaration(Vec::new()),
            "abstract" | "final" | "readonly" if self.class_after_modifiers() => {
                self.type_declaration(Vec::new())
            }
            _ if self.is_name(self.i) && next == L::Op(":") => {
                let items = vec![self.leaf("name"), self.punct()];
                node("named_label_statement", items)
            }
            _ => return None,
        })
    }

    /// At modifiers: whether `class` follows them.
    fn class_after_modifiers(&self) -> bool {
        let mut k = self.i;
        while matches!(
            self.word_at(k).to_ascii_lowercase().as_str(),
            "abstract" | "final" | "readonly"
        ) {
            k += 1;
        }
        self.kw_at(k, "class")
    }

    /// An expression, or several (`a, b`) as a `sequence_expression`.
    pub(crate) fn expressions(&mut self) -> Item {
        let first = self.expression();
        if !self.at(",") {
            return first;
        }
        let comma = self.punct();
        let rest = self.expressions();
        node("sequence_expression", vec![first, comma, rest])
    }

    fn parenthesized(&mut self) -> Item {
        let items = vec![self.expect("("), self.expression(), self.expect(")")];
        node("parenthesized_expression", items)
    }

    /// `: statements` up to one of `ends`.
    fn colon_block(&mut self, ends: &[&str]) -> Item {
        let mut items = vec![self.punct()];
        self.statements_until(&mut items, ends, None);
        node("colon_block", items)
    }

    fn if_statement(&mut self) -> Item {
        let mut items = vec![self.punct(), self.parenthesized().with("condition")];
        if self.at(":") {
            items.push(self.colon_block(BLOCK_ENDS).with("body"));
            loop {
                if self.at_kw("elseif") {
                    let v = vec![
                        self.punct(),
                        self.parenthesized().with("condition"),
                        self.colon_block_or_missing().with("body"),
                    ];
                    items.push(node("else_if_clause", v).with("alternative"));
                } else if self.at_kw("else") {
                    let v = vec![self.punct(), self.colon_block_or_missing().with("body")];
                    items.push(node("else_clause", v).with("alternative"));
                } else {
                    break;
                }
            }
            items.push(self.expect_kw("endif"));
            self.semicolon(&mut items);
            return node("if_statement", items);
        }
        items.push(self.statement().with("body"));
        while self.at_kw("elseif") {
            let v = vec![
                self.punct(),
                self.parenthesized().with("condition"),
                self.statement().with("body"),
            ];
            items.push(node("else_if_clause", v).with("alternative"));
        }
        if self.at_kw("else") {
            let v = vec![self.punct(), self.statement().with("body")];
            items.push(node("else_clause", v).with("alternative"));
        }
        // tree-sitter gives a dangling `else` (and `elseif`s) to the
        // outermost `if` without one: take those of the `if` this one's
        // last statement ends in.
        if !items.iter().any(|i| i.kind() == "else_clause")
            && let Some(last) = items.last_mut()
        {
            let stolen = match last {
                Item::Node(n) if n.kind == "else_if_clause" => {
                    n.items.last_mut().map_or_default(take_tail_alternatives)
                }
                _ => take_tail_alternatives(last),
            };
            items.extend(stolen);
        }
        node("if_statement", items)
    }

    fn colon_block_or_missing(&mut self) -> Item {
        if self.at(":") {
            self.colon_block(BLOCK_ENDS)
        } else {
            self.missing(":", false)
        }
    }

    fn switch_statement(&mut self) -> Item {
        let mut items = vec![self.punct(), self.parenthesized().with("condition")];
        let colon = self.at(":");
        let mut block = vec![if colon {
            self.punct()
        } else {
            self.expect("{")
        }];
        while !self.eof() && !self.at("}") && !self.at_kw("endswitch") {
            let before = self.i;
            if self.at_kw("case") {
                let mut v = vec![self.punct(), self.expression().with("value")];
                v.push(if self.at(";") {
                    self.punct()
                } else {
                    self.expect(":")
                });
                self.statements_until(&mut v, &["case", "default", "endswitch"], None);
                block.push(node("case_statement", v));
            } else if self.at_kw("default") {
                let mut v = vec![self.punct()];
                v.push(if self.at(";") {
                    self.punct()
                } else {
                    self.expect(":")
                });
                self.statements_until(&mut v, &["case", "default", "endswitch"], None);
                block.push(node("default_statement", v));
            } else {
                block.push(self.error_statement());
            }
            if self.i == before {
                block.push(self.error_token());
            }
        }
        if colon {
            block.push(self.expect_kw("endswitch"));
            self.semicolon(&mut block);
        } else {
            block.push(self.expect("}"));
        }
        items.push(node("switch_block", block).with("body"));
        node("switch_statement", items)
    }

    fn while_statement(&mut self) -> Item {
        let mut items = vec![self.punct(), self.parenthesized().with("condition")];
        if self.at(":") {
            items.push(self.colon_block(&["endwhile"]).with("body"));
            items.push(self.expect_kw("endwhile"));
            self.semicolon(&mut items);
        } else {
            items.push(self.statement().with("body"));
        }
        node("while_statement", items)
    }

    fn do_statement(&mut self) -> Item {
        let mut items = vec![
            self.punct(),
            self.statement().with("body"),
            self.expect_kw("while"),
            self.parenthesized().with("condition"),
        ];
        self.semicolon(&mut items);
        node("do_statement", items)
    }

    fn for_statement(&mut self) -> Item {
        let mut items = vec![self.punct(), self.expect("(")];
        for (field, end) in [("initialize", ";"), ("condition", ";"), ("update", ")")] {
            if !self.at(end) {
                items.push(self.expressions().with(field));
            }
            items.push(self.expect(if end == ";" { ";" } else { ")" }));
        }
        if self.at(":") {
            items.push(self.punct());
            self.statements_until(&mut items, &["endfor"], Some("body"));
            items.push(self.expect_kw("endfor"));
            self.semicolon(&mut items);
        } else if self.at(";") {
            items.push(self.punct());
        } else {
            items.push(self.statement().with("body"));
        }
        node("for_statement", items)
    }

    fn foreach_statement(&mut self) -> Item {
        let mut items = vec![self.punct(), self.expect("("), self.expression()];
        items.push(self.expect_kw("as"));
        let first = self.foreach_value();
        if self.at("=>") {
            let arrow = self.punct();
            let value = self.foreach_value();
            items.push(node("pair", vec![first, arrow, value]));
        } else {
            items.push(first);
        }
        items.push(self.expect(")"));
        if self.at(":") {
            items.push(self.colon_block(&["endforeach"]).with("body"));
            items.push(self.expect_kw("endforeach"));
            self.semicolon(&mut items);
        } else if self.at(";") {
            items.push(self.punct());
        } else {
            items.push(self.statement().with("body"));
        }
        node("foreach_statement", items)
    }

    /// A `foreach`'s key or value: `&$v`, a list, or an expression.
    fn foreach_value(&mut self) -> Item {
        if self.at("&") {
            return self.by_ref();
        }
        if self.at_kw("list") && self.is(self.i + 1, "(") {
            return self.list_literal();
        }
        if self.at("[") {
            let end = self.scan_balanced(self.i).unwrap_or(0);
            if self.is(end, ")") || self.is(end, "=>") {
                return self.list_literal();
            }
        }
        self.expression()
    }

    fn try_statement(&mut self) -> Item {
        let mut items = vec![self.punct(), self.compound().with("body")];
        loop {
            if self.at_kw("catch") {
                let mut v = vec![self.punct(), self.expect("(")];
                let mut types = Vec::new();
                loop {
                    types.push(self.named_type());
                    if !self.at("|") {
                        break;
                    }
                    types.push(self.punct());
                }
                v.push(node("type_list", types).with("type"));
                if self.at("$") {
                    v.push(self.simple_variable().with("name"));
                }
                v.push(self.expect(")"));
                v.push(self.compound().with("body"));
                items.push(node("catch_clause", v));
            } else if self.at_kw("finally") {
                let v = vec![self.punct(), self.compound().with("body")];
                items.push(node("finally_clause", v));
            } else {
                break;
            }
        }
        node("try_statement", items)
    }

    fn declare_statement(&mut self) -> Item {
        let mut items = vec![self.punct(), self.punct()];
        let mut d = Vec::new();
        if matches!(self.word(), "ticks" | "encoding" | "strict_types") {
            d.push(self.punct());
        } else {
            d.push(self.error_token());
        }
        d.push(self.expect("="));
        d.push(self.expression());
        items.push(node("declare_directive", d));
        items.push(self.expect(")"));
        if self.at(":") {
            items.push(self.punct());
            self.statements_until(&mut items, &["enddeclare"], None);
            items.push(self.expect_kw("enddeclare"));
            self.semicolon(&mut items);
        } else if self.at(";") || self.peek() == L::EndTag {
            self.semicolon(&mut items);
        } else {
            items.push(self.statement());
        }
        node("declare_statement", items)
    }

    fn namespace_definition(&mut self) -> Item {
        let mut items = vec![self.punct()];
        if self.peek() == L::Name {
            items.push(self.namespace_name().with("name"));
        }
        if self.at("{") {
            items.push(self.compound().with("body"));
        } else {
            self.semicolon(&mut items);
        }
        node("namespace_definition", items)
    }
}
