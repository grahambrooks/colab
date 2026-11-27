//! Swift read by recursive descent into a tree of [`Item`]s:
//! tree-sitter-swift's grammar (the file and statements here, declarations
//! in `parser/decl.rs`, expressions in `parser/expr.rs`, types and patterns
//! in `parser/types.rs`), kinds and fields named as in `names.rs`.
//!
//! A statement ends where tree-sitter-swift's scanner inserts a semicolon
//! ([`Parser::stops`]): at `;`, or at a line end unless the next line
//! starts with an operator or keyword that continues it. Its zero-width
//! semicolon is a statement list's last child, so the list reaches to it
//! ([`Parser::semi_item`]). A construct the parser cannot read becomes an
//! `ERROR` node and reading resumes at the next statement; a token that
//! should be there and is not is a missing one.

mod decl;
mod expr;
mod types;

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
    /// `""` for a hidden rule, whose children are its parent's.
    pub kind: &'static str,
    pub field: Option<&'static str>,
    pub items: Vec<Item>,
    /// An anonymous node (tree-sitter's alias of a rule to a string: an
    /// optional's `?`).
    pub anonymous: bool,
}

impl Item {
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Item::Tok(t) => t.kind,
            Item::Node(n) => n.kind,
        }
    }

    /// The item with `field`: a hidden rule's children each take it, unless
    /// they have their own.
    pub(crate) fn with(mut self, field: &'static str) -> Self {
        match &mut self {
            Item::Tok(t) => {
                if t.field.is_none() {
                    t.field = Some(field);
                }
            }
            Item::Node(n) if n.kind.is_empty() => {
                for i in &mut n.items {
                    let taken = std::mem::replace(i, Item::Tok(placeholder()));
                    *i = taken.with(field);
                }
            }
            Item::Node(n) => {
                if n.field.is_none() {
                    n.field = Some(field);
                }
            }
        }
        self
    }

    /// The item with `field`, replacing a field it has.
    pub(crate) fn field_over(mut self, field: &'static str) -> Self {
        match &mut self {
            Item::Tok(t) => t.field = Some(field),
            Item::Node(n) if n.kind.is_empty() => {
                for i in &mut n.items {
                    let taken = std::mem::replace(i, Item::Tok(placeholder()));
                    *i = taken.field_over(field);
                }
            }
            Item::Node(n) => n.field = Some(field),
        }
        self
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
        anonymous: false,
    })
}

/// A hidden rule's items, which its parent takes in its place.
pub(crate) fn hidden(items: Vec<Item>) -> Item {
    node("", items)
}

pub(crate) struct Parser<'s> {
    src: &'s str,
    toks: Vec<Lx>,
    i: usize,
    pub errors: bool,
    /// Inside parentheses or brackets, where line ends end nothing.
    nl_blind: usize,
    /// A `{` here does not start a trailing closure (an `if`'s condition).
    no_closure: bool,
    /// Comments' (start, end), in order.
    comments: Vec<(usize, usize)>,
}

impl<'s> Parser<'s> {
    pub fn new(src: &'s str, toks: Vec<Lx>, comments: Vec<(usize, usize)>) -> Self {
        Self {
            src,
            toks,
            i: 0,
            errors: false,
            nl_blind: 0,
            no_closure: false,
            comments,
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
            nl: true,
        })
    }

    fn text_at(&self, n: usize) -> &'s str {
        let t = self.tk(n);
        &self.src[t.start..t.end]
    }

    fn word_at(&self, n: usize) -> &'s str {
        match self.toks.get(n) {
            Some(t) if t.k == L::Word => &self.src[t.start..t.end],
            _ => "",
        }
    }

    fn word(&self) -> &'s str {
        self.word_at(self.i)
    }

    fn is(&self, n: usize, s: &str) -> bool {
        match self.kind_at(n) {
            L::Op(o) => o == s,
            L::Word => self.word_at(n) == s,
            _ => false,
        }
    }

    fn at(&self, s: &str) -> bool {
        self.is(self.i, s)
    }

    fn eof(&self) -> bool {
        self.peek() == L::Eof
    }

    fn nl_at(&self, n: usize) -> bool {
        self.tk(n).nl
    }

    /// Whether token `n` touches the one before it.
    fn touches(&self, n: usize) -> bool {
        n > 0 && self.tk(n - 1).end == self.tk(n).start
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

    fn hidden_tok(start: usize, end: usize) -> Item {
        Item::Tok(Tok {
            kind: "",
            named: false,
            start,
            end,
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

    /// The current token, anonymous, named by its text.
    fn punct(&mut self) -> Item {
        let t = self.advance();
        let kind = match t.k {
            L::Op(o) | L::StrOpen(o) | L::StrClose(o) | L::Directive(o) => o,
            L::InterpOpen => "\\(",
            _ => names::anonymous(&self.src[t.start..t.end]).unwrap_or_else(|| {
                self.errors = true;
                "ERROR"
            }),
        };
        Self::tok(t, kind, false)
    }

    fn wrapped(&mut self, kind: &'static str) -> Item {
        let t = self.punct();
        node(kind, vec![t])
    }

    /// An anonymous node `kind` around an anonymous token (an optional's
    /// `?`).
    fn anon_node(&mut self, kind: &'static str) -> Item {
        let t = self.punct();
        Item::Node(Node {
            kind,
            field: None,
            items: vec![t],
            anonymous: true,
        })
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

    /// A name as `simple_identifier` (any word where a name must be).
    fn simple_identifier(&mut self) -> Item {
        if self.peek() == L::Word {
            self.name_leaf("simple_identifier")
        } else {
            self.missing("simple_identifier", true)
        }
    }

    /// A name `kind`; a contextual keyword (`package`, `async`) is the
    /// keyword's token inside it, as tree-sitter reads one.
    fn name_leaf(&mut self, kind: &'static str) -> Item {
        const CONTEXTUAL: &[&str] = &[
            "actor",
            "async",
            "consume",
            "copy",
            "discard",
            "each",
            "lazy",
            "repeat",
            "package",
            "unsafe",
            "borrowing",
            "consuming",
            "sending",
            "isolated",
        ];
        if let Some(k) = CONTEXTUAL.iter().find(|w| **w == self.word()) {
            let t = self.advance();
            return node(kind, vec![Self::tok(t, k, false)]);
        }
        self.leaf(kind)
    }

    fn error_token(&mut self) -> Item {
        self.errors = true;
        let t = self.advance();
        Self::tok(t, "ERROR", false)
    }

    /// Past the bracket at `n` and its match.
    fn scan_balanced(&self, n: usize) -> Option<usize> {
        let mut depth = 0usize;
        let mut k = n;
        loop {
            match self.kind_at(k) {
                L::Eof => return None,
                L::Op("(" | "[" | "{") | L::InterpOpen | L::RawInterp => depth += 1,
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

    fn blind<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> T {
        self.nl_blind += 1;
        let saved = std::mem::replace(&mut self.no_closure, false);
        let r = f(self);
        self.no_closure = saved;
        self.nl_blind -= 1;
        r
    }

    fn sighted<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> T {
        let saved = (self.nl_blind, self.no_closure);
        self.nl_blind = 0;
        self.no_closure = false;
        let r = f(self);
        (self.nl_blind, self.no_closure) = saved;
        r
    }

    /// Whether tree-sitter-swift's scanner ends the statement before the
    /// current token.
    pub(crate) fn stops(&self) -> bool {
        self.stops_at(self.i)
    }

    fn stops_at(&self, n: usize) -> bool {
        let t = self.tk(n);
        if t.k == L::Eof || t.k == L::Op(";") {
            return true;
        }
        if self.nl_blind > 0 || !t.nl {
            return false;
        }
        let text = &self.src[t.start..];
        let after = |k: usize| text.as_bytes().get(k).copied().unwrap_or(b' ');
        match t.k {
            L::Op(o) => match o.as_bytes()[0] {
                // Characters that keep the statement going.
                b'?' | b':' | b'{' | b'&' | b'|' | b'^' | b'<' | b'>' => false,
                b'.' => o != "." || after(1) == b'.',
                b'=' => false,
                b'+' | b'-' => !(o.len() == 1 && after(1).is_ascii_whitespace()) && o != "->",
                _ => true,
            },
            L::Custom => false,
            L::Word => !matches!(
                self.word_at(n),
                "throws" | "rethrows" | "default" | "where" | "else" | "catch" | "as"
            ),
            _ => true,
        }
    }

    /// Where the scanner's zero-width semicolon after the previous token
    /// lands: past the comments after it, at the next token.
    fn semi_pos(&self) -> usize {
        let prev = self.prev_end();
        let next = self.tk(self.i).start;
        let bytes = self.src.as_bytes();
        let mut i = prev;
        let mut c = self.comments.partition_point(|c| c.0 < prev);
        while i < next {
            match bytes[i] {
                b if b.is_ascii_whitespace() => i += 1,
                _ => match self.comments.get(c) {
                    Some(&(s, e)) if s == i => {
                        i = e;
                        c += 1;
                    }
                    _ => break,
                },
            }
        }
        i
    }

    /// The statement separator here: `;` (no token of its own), or at a line
    /// end the zero-width semicolon.
    fn semi_item(&mut self) -> Option<Item> {
        if self.at(";") {
            let t = self.advance();
            return Some(Self::hidden_tok(t.start, t.end));
        }
        if self.stops() {
            let at = self.semi_pos();
            return Some(Self::hidden_tok(at, at));
        }
        None
    }

    // ---- the file and statements -----------------------------------------

    pub(crate) fn source_file(&mut self) -> Item {
        let mut items = Vec::new();
        if self.peek() == L::Shebang {
            items.push(self.leaf("shebang_line"));
        }
        self.statement_list(&mut items, true);
        node("source_file", items)
    }

    /// Statements and their separators until `}` or the end.
    fn statement_list(&mut self, items: &mut Vec<Item>, top: bool) {
        loop {
            while self.at(";") {
                let t = self.advance();
                items.push(Self::hidden_tok(t.start, t.end));
            }
            if self.eof() || self.at("}") || self.switch_entry_here() {
                return;
            }
            let before = self.i;
            items.push(self.statement(top));
            if self.i == before {
                items.push(self.error_token());
                continue;
            }
            if self.at("}") && !self.nl_at(self.i) {
                return;
            }
            match self.semi_item() {
                Some(s) => items.push(s),
                None if self.eof() || self.at("}") || self.switch_entry_here() => return,
                None => {
                    self.errors = true;
                    items.push(self.error_token());
                }
            }
        }
    }

    /// At `case` / `default` in a switch.
    fn switch_entry_here(&self) -> bool {
        (self.at("case") || (self.at("default") && self.is(self.i + 1, ":")))
            || (self.at("@") && self.switch_attrs_then_case())
    }

    fn switch_attrs_then_case(&self) -> bool {
        let mut k = self.i;
        while self.is(k, "@") {
            k += 2;
            if self.is(k, "(") {
                match self.scan_balanced(k) {
                    Some(e) => k = e,
                    None => return false,
                }
            }
        }
        self.is(k, "case") || self.is(k, "default")
    }

    /// A `{ statements }` block's items: braces and statements.
    pub(crate) fn block_items(&mut self) -> Vec<Item> {
        self.sighted(|p| {
            let mut items = vec![p.expect("{")];
            let mut body = Vec::new();
            p.statement_list(&mut body, false);
            if !body.is_empty() {
                items.push(node("statements", body));
            }
            items.push(p.expect("}"));
            items
        })
    }

    pub(crate) fn statement(&mut self, top: bool) -> Item {
        if let Some(d) = self.declaration_here(!top) {
            return d;
        }
        // `label: for …`.
        if self.peek() == L::Word
            && self.is(self.i + 1, ":")
            && self.touches(self.i + 1)
            && matches!(
                self.word_at(self.i + 2),
                "for" | "while" | "repeat" | "do" | "if" | "guard" | "switch"
            )
        {
            let a = self.advance();
            let b = self.advance();
            let label = Self::tok(Lx { end: b.end, ..a }, "statement_label", true);
            let s = self.labeled();
            return hidden(vec![label, s]);
        }
        match self.word() {
            "for" | "while" | "repeat" | "do" | "guard" => return self.labeled(),
            "return" | "continue" | "break" | "yield" if !top || self.word() == "return" => {
                let mut items = vec![self.punct()];
                if !self.stops() && !self.at("}") && self.expression_starts(self.i) {
                    items.push(self.expression().with("result"));
                }
                return node("control_transfer_statement", items);
            }
            "throw" => {
                let items = vec![self.leaf("throw_keyword"), self.expression()];
                return if top {
                    hidden(items)
                } else {
                    node("control_transfer_statement", items)
                };
            }
            _ => {}
        }
        self.expression()
    }

    fn labeled(&mut self) -> Item {
        match self.word() {
            "for" => self.for_statement(),
            "while" => {
                let mut items = vec![self.punct()];
                self.conditions(&mut items);
                items.extend(self.block_items());
                node("while_statement", items)
            }
            "repeat" => {
                let mut items = vec![self.punct()];
                items.extend(self.block_items());
                items.push(self.expect("while"));
                self.conditions(&mut items);
                node("repeat_while_statement", items)
            }
            "do" => {
                let mut items = vec![self.punct()];
                if matches!(self.word(), "throws" | "rethrows") {
                    items.push(self.throws());
                }
                items.extend(self.block_items());
                while self.at("catch") {
                    let mut c = vec![self.leaf("catch_keyword")];
                    if !self.at("{") && !self.at("where") {
                        let saved = std::mem::replace(&mut self.no_closure, true);
                        c.push(self.pattern(false).field_over("error"));
                        self.no_closure = saved;
                    }
                    if self.at("where") {
                        c.push(self.where_clause());
                    }
                    c.extend(self.block_items());
                    items.push(node("catch_block", c));
                }
                node("do_statement", items)
            }
            "guard" => {
                let mut items = vec![self.punct()];
                self.conditions(&mut items);
                if self.at("else") {
                    items.push(self.leaf("else"));
                } else {
                    items.push(self.missing("else", true));
                }
                items.extend(self.block_items());
                node("guard_statement", items)
            }
            "if" => self.if_statement(),
            _ => self.switch_statement(),
        }
    }

    fn for_statement(&mut self) -> Item {
        let mut items = vec![self.punct()];
        if self.at("try") {
            items.push(self.try_operator());
        }
        for w in ["await", "unsafe"] {
            if self.at(w) && !self.is(self.i + 1, "in") {
                items.push(self.punct());
            }
        }
        items.push(self.pattern(false).field_over("item"));
        if self.at(":") {
            items.push(self.type_annotation());
        }
        items.push(self.expect("in"));
        let saved = std::mem::replace(&mut self.no_closure, true);
        items.push(self.expression().with("collection"));
        if self.at("where") {
            items.push(self.where_clause());
        }
        self.no_closure = saved;
        items.extend(self.block_items());
        node("for_statement", items)
    }

    pub(crate) fn where_clause(&mut self) -> Item {
        let items = vec![self.leaf("where_keyword"), self.expression()];
        node("where_clause", items)
    }

    /// An `if` / `guard` / `while`'s conditions, `,`-separated.
    pub(crate) fn conditions(&mut self, items: &mut Vec<Item>) {
        let saved = std::mem::replace(&mut self.no_closure, true);
        loop {
            items.push(self.condition().with("condition"));
            if self.at(",") {
                items.push(self.punct());
                continue;
            }
            break;
        }
        self.no_closure = saved;
    }

    fn condition(&mut self) -> Item {
        if self.at("let") || self.at("var") || self.at("case") {
            // `let x = y`, `case .a(let b) = c`: the binding's parts are the
            // `if`'s own.
            let mut items = Vec::new();
            if self.at("case") {
                items.push(self.punct());
                // The pattern's parts are the condition's own.
                match self.pattern(true) {
                    Item::Node(n) if n.kind == "pattern" => items.push(hidden(n.items)),
                    p => items.push(p),
                }
            } else {
                items.push(self.value_binding_pattern());
                // A name is the condition's own; a tuple or `_` a pattern.
                if self.peek() == L::Word && !self.at("_") {
                    items.push(self.simple_identifier().with("bound_identifier"));
                } else {
                    match self.bound_pattern() {
                        Item::Node(n) if n.kind == "pattern" => items.push(hidden(n.items)),
                        p => items.push(p),
                    }
                }
            }
            if self.at(":") {
                items.push(self.type_annotation());
            }
            if self.at("=") {
                items.push(self.punct());
                items.push(self.expression());
            }
            if self.at("where") {
                items.push(self.where_clause());
            }
            return hidden(items);
        }
        if self.at("#") && matches!(self.word_at(self.i + 1), "available" | "unavailable") {
            return self.availability_condition();
        }
        self.expression()
    }

    fn availability_condition(&mut self) -> Item {
        let mut items = vec![self.punct(), self.punct(), self.expect("(")];
        self.blind(|p| {
            loop {
                if p.at("*") {
                    items.push(p.punct());
                } else {
                    let mut id = vec![p.simple_identifier()];
                    while p.at(".") && p.kind_at(p.i + 1) == L::Word {
                        id.push(p.punct());
                        id.push(p.simple_identifier());
                    }
                    items.push(node("identifier", id));
                    // `14.0` lexes as one real; the grammar's integers.
                    while matches!(p.peek(), L::Int | L::Real) || p.at(".") {
                        if p.peek() == L::Real {
                            let t = p.advance();
                            let text = &p.src[t.start..t.end];
                            let mut at = t.start;
                            for (k, part) in text.split('.').enumerate() {
                                if k > 0 {
                                    items.push(Self::tok(
                                        Lx {
                                            start: at,
                                            end: at + 1,
                                            ..t
                                        },
                                        ".",
                                        false,
                                    ));
                                    at += 1;
                                }
                                items.push(Self::tok(
                                    Lx {
                                        start: at,
                                        end: at + part.len(),
                                        ..t
                                    },
                                    "integer_literal",
                                    true,
                                ));
                                at += part.len();
                            }
                        } else if p.at(".") {
                            items.push(p.punct());
                        } else {
                            items.push(p.leaf("integer_literal"));
                        }
                    }
                }
                if p.at(",") {
                    items.push(p.punct());
                    continue;
                }
                break;
            }
        });
        items.push(self.expect(")"));
        node("availability_condition", items)
    }

    pub(crate) fn if_statement(&mut self) -> Item {
        let mut items = vec![self.punct()];
        self.conditions(&mut items);
        items.extend(self.block_items());
        if self.at("else") {
            items.push(self.leaf("else"));
            if self.at("if") {
                items.push(self.if_statement());
            } else {
                items.extend(self.block_items());
            }
        }
        node("if_statement", items)
    }

    pub(crate) fn switch_statement(&mut self) -> Item {
        let mut items = vec![self.punct()];
        let saved = std::mem::replace(&mut self.no_closure, true);
        items.push(self.expression().with("expr"));
        self.no_closure = saved;
        items.push(self.expect("{"));
        self.sighted(|p| {
            while !p.eof() && !p.at("}") {
                let before = p.i;
                if p.peek() == L::Directive("#if") || matches!(p.peek(), L::Directive(_)) {
                    items.push(p.directive());
                    continue;
                }
                items.push(p.switch_entry());
                if p.i == before {
                    items.push(p.error_token());
                }
            }
        });
        items.push(self.expect("}"));
        node("switch_statement", items)
    }

    fn switch_entry(&mut self) -> Item {
        let mut items = Vec::new();
        if self.at("@") {
            items.push(self.modifiers());
        }
        if self.at("default") {
            items.push(self.leaf("default_keyword"));
        } else {
            items.push(self.expect("case"));
            self.blind(|p| {
                loop {
                    let pat = p.pattern(true);
                    items.push(node("switch_pattern", vec![pat]));
                    if p.at("where") {
                        items.push(p.leaf("where_keyword"));
                        items.push(p.expression());
                    }
                    if p.at(",") {
                        items.push(p.punct());
                        continue;
                    }
                    break;
                }
            });
        }
        items.push(self.expect(":"));
        let mut body = Vec::new();
        self.statement_list(&mut body, false);
        if !body.is_empty() {
            items.push(node("statements", body));
        }
        node("switch_entry", items)
    }

    /// `#if cond`, `#elseif cond`, `#else`, `#endif`.
    pub(crate) fn directive(&mut self) -> Item {
        let d = self.peek();
        let mut items = vec![self.punct()];
        if matches!(d, L::Directive("#if" | "#elseif")) {
            self.compilation_condition(&mut items);
        }
        node("directive", items)
    }

    fn compilation_condition(&mut self, items: &mut Vec<Item>) {
        // One line: names, `!`, `&&`, `||`, parentheses, `os(…)` and the
        // like.
        let line_start = self.i;
        while !self.eof() && (self.i == line_start || !self.nl_at(self.i)) {
            match self.peek() {
                L::Word if matches!(self.word(), "true" | "false") => {
                    items.push(self.wrapped("boolean_literal"));
                }
                L::Word
                    if self.is(self.i + 1, "(")
                        && matches!(
                            self.word(),
                            "os" | "arch"
                                | "swift"
                                | "compiler"
                                | "canImport"
                                | "targetEnvironment"
                        ) =>
                {
                    items.push(self.punct());
                    items.push(self.punct());
                    while !self.eof() && !self.at(")") {
                        items.push(match self.peek() {
                            L::Word => self.simple_identifier(),
                            L::Int | L::Real => {
                                let mut v = Vec::new();
                                self.version_numbers(&mut v);
                                hidden(v)
                            }
                            _ => self.punct(),
                        });
                    }
                    items.push(self.expect(")"));
                }
                L::Word => items.push(self.simple_identifier()),
                L::Op("!" | "(" | ")" | "&&" | "||") => items.push(self.punct()),
                _ => break,
            }
        }
    }
}
