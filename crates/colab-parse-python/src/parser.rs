//! Python, read by recursive descent into a tree of [`Item`]s: tree-sitter-
//! python's grammar (statements, the expression precedences, patterns,
//! `match`), with kinds and fields named as in `names.rs`. A construct the
//! parser cannot read becomes an `ERROR` node holding the rest of its line,
//! and reading resumes at the next statement; a token that should be there
//! and is not is a missing one.

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
    /// Text that belongs to the node it is in but is no token of its own
    /// (a string's text, a zero-width statement end).
    pub hidden: bool,
    pub missing: bool,
    pub extra: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct Node {
    /// `"ERROR"` for text the parser could not read.
    pub kind: &'static str,
    pub named: bool,
    pub field: Option<&'static str>,
    pub items: Vec<Item>,
}

impl Item {
    pub fn start(&self) -> usize {
        match self {
            Item::Tok(t) => t.start,
            Item::Node(n) => n.items.first().map_or(0, Item::start),
        }
    }

    pub fn end(&self) -> usize {
        match self {
            Item::Tok(t) => t.end,
            Item::Node(n) => n.items.last().map_or(0, Item::end),
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            Item::Tok(t) => t.kind,
            Item::Node(n) => n.kind,
        }
    }

    fn with(mut self, field: &'static str) -> Self {
        match &mut self {
            Item::Tok(t) => t.field = Some(field),
            Item::Node(n) => n.field = Some(field),
        }
        self
    }

    fn renamed(mut self, kind: &'static str) -> Self {
        match &mut self {
            Item::Tok(t) => t.kind = kind,
            Item::Node(n) => n.kind = kind,
        }
        self
    }

    fn is_named(&self) -> bool {
        match self {
            Item::Tok(t) => t.named && !t.hidden,
            Item::Node(n) => n.named,
        }
    }
}

fn node(kind: &'static str, items: Vec<Item>) -> Item {
    Item::Node(Node {
        kind,
        named: true,
        field: None,
        items,
    })
}

/// Keywords that are never names.
const HARD: &[&str] = &[
    "and", "as", "assert", "break", "class", "continue", "def", "del", "elif", "else", "except",
    "finally", "for", "from", "global", "if", "import", "in", "is", "lambda", "nonlocal", "not",
    "or", "pass", "raise", "return", "try", "while", "with", "yield", "True", "False", "None",
];

const AUGMENTED: &[&str] = &[
    "+=", "-=", "*=", "/=", "@=", "//=", "%=", "**=", ">>=", "<<=", "&=", "^=", "|=",
];

const COMPARISONS: &[&str] = &["<", "<=", "==", "!=", ">=", ">", "<>"];

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

    fn peek(&self) -> L {
        self.toks[self.i].k
    }

    fn peek_at(&self, n: usize) -> L {
        self.toks.get(self.i + n).map_or(L::Eof, |t| t.k)
    }

    fn text_at(&self, n: usize) -> &'s str {
        self.toks
            .get(self.i + n)
            .map_or("", |t| &self.src[t.start..t.end])
    }

    fn at(&self, s: &str) -> bool {
        self.at_n(0, s)
    }

    /// Whether token `n` ahead is the operator or word `s`.
    fn at_n(&self, n: usize, s: &str) -> bool {
        match self.peek_at(n) {
            L::Op(o) => o == s,
            L::Name => self.text_at(n) == s,
            _ => false,
        }
    }

    fn at_any(&self, ops: &[&str]) -> bool {
        ops.iter().any(|o| self.at(o))
    }

    fn advance(&mut self) -> Lx {
        let t = self.toks[self.i];
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

    /// The current token, as a named leaf of `kind`.
    fn leaf(&mut self, kind: &'static str) -> Item {
        let t = self.advance();
        Self::tok(t, kind, true)
    }

    /// The current token, anonymous, named by its text.
    fn punct(&mut self) -> Item {
        let t = self.advance();
        let kind = match t.k {
            L::Op(o) => o,
            _ => names::anonymous(&self.src[t.start..t.end]).unwrap_or("ERROR"),
        };
        Self::tok(t, kind, false)
    }

    /// The current token, anonymous, as `kind`.
    fn punct_as(&mut self, kind: &'static str) -> Item {
        let t = self.advance();
        Self::tok(t, kind, false)
    }

    fn hidden_at(at: usize) -> Item {
        Item::Tok(Tok {
            kind: "",
            named: false,
            start: at,
            end: at,
            field: None,
            hidden: true,
            missing: false,
            extra: false,
        })
    }

    /// The current token as text inside its node (a string's text).
    fn hidden(&mut self) -> Item {
        let t = self.advance();
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

    /// `s` if it is next, else a missing `s`.
    fn expect(&mut self, s: &'static str) -> Item {
        if self.at(s) {
            self.punct()
        } else {
            self.missing(s, false)
        }
    }

    fn identifier(&mut self) -> Item {
        if self.peek() == L::Name && !HARD.contains(&self.text_at(0)) {
            self.leaf("identifier")
        } else {
            self.missing("identifier", true)
        }
    }

    /// A name after `.` or in an import: any word.
    fn name_any(&mut self) -> Item {
        if self.peek() == L::Name {
            self.leaf("identifier")
        } else {
            self.missing("identifier", true)
        }
    }

    /// Whether the current token can start an expression.
    fn starts_expression(&self) -> bool {
        self.starts_expression_at(0)
    }

    fn starts_expression_at(&self, n: usize) -> bool {
        match self.peek_at(n) {
            L::Name => {
                let w = self.text_at(n);
                !HARD.contains(&w) || matches!(w, "not" | "lambda" | "True" | "False" | "None")
            }
            L::Int | L::Float | L::StrStart => true,
            L::Op(o) => matches!(o, "(" | "[" | "{" | "-" | "+" | "~" | "*" | "**" | "..."),
            _ => false,
        }
    }

    /// The rest of the line, from here, as an `ERROR` node.
    fn error_line(&mut self) -> Item {
        self.errors = true;
        let mut items = Vec::new();
        while !matches!(self.peek(), L::Newline | L::Eof | L::Dedent | L::Indent) {
            let t = self.advance();
            items.push(Self::tok(t, "ERROR", false));
        }
        if items.is_empty() && !matches!(self.peek(), L::Eof) {
            let t = self.advance();
            items.push(Self::tok(t, "ERROR", false));
        }
        node("ERROR", items)
    }

    // ---- statements ------------------------------------------------------

    pub fn module(&mut self) -> Item {
        let mut items = Vec::new();
        loop {
            match self.peek() {
                L::Eof => break,
                L::Newline => {
                    let t = self.advance();
                    items.push(Self::hidden_at(t.start));
                }
                L::Indent | L::Dedent => {
                    self.advance();
                }
                _ => {
                    let before = self.i;
                    self.statement(&mut items);
                    if self.i == before {
                        items.push(self.error_line());
                    }
                }
            }
        }
        node("module", items)
    }

    fn statement(&mut self, out: &mut Vec<Item>) {
        let compound = match self.text_at(0) {
            _ if self.peek() != L::Name && !self.at("@") => None,
            "if" => Some(self.if_statement()),
            "for" | "while" | "try" | "with" | "def" | "class" => {
                Some(self.compound(self.text_at(0)))
            }
            "async" if matches!(self.text_at(1), "def" | "for" | "with") => {
                Some(self.compound(self.text_at(1)))
            }
            "@" => Some(self.decorated()),
            "match" if self.is_match_statement() => Some(self.match_statement()),
            _ => None,
        };
        match compound {
            Some(item) => out.push(item),
            None => self.simple_statements(out),
        }
    }

    fn compound(&mut self, word: &str) -> Item {
        match word {
            "for" => self.for_statement(),
            "while" => self.while_statement(),
            "try" => self.try_statement(),
            "with" => self.with_statement(),
            "def" => self.function_definition(),
            _ => self.class_definition(),
        }
    }

    fn simple_statements(&mut self, out: &mut Vec<Item>) {
        loop {
            let before = self.i;
            out.push(self.simple_statement());
            if self.i == before {
                break;
            }
            if self.at(";") {
                out.push(self.punct());
                if matches!(self.peek(), L::Newline | L::Eof | L::Dedent) {
                    break;
                }
                continue;
            }
            break;
        }
        match self.peek() {
            L::Newline => {
                let t = self.advance();
                out.push(Self::hidden_at(t.start));
            }
            L::Eof | L::Dedent => {}
            _ => out.push(self.error_line()),
        }
    }

    fn simple_statement(&mut self) -> Item {
        if self.peek() != L::Name {
            return self.expression_statement();
        }
        match self.text_at(0) {
            "import" => self.import_statement(),
            "from" => self.import_from(),
            "print" if self.is_print_statement() => self.print_statement(),
            "exec"
                if matches!(self.peek_at(1), L::StrStart)
                    || (self.peek_at(1) == L::Name && !HARD.contains(&self.text_at(1))) =>
            {
                self.exec_statement()
            }
            "type" if self.peek_at(1) == L::Name && (self.at_n(2, "=") || self.at_n(2, "[")) => {
                let kw = self.punct();
                let left = self.type_().with("left");
                let eq = self.expect("=");
                let right = self.type_().with("right");
                node("type_alias_statement", vec![kw, left, eq, right])
            }
            "assert" => {
                let mut items = vec![self.punct()];
                self.comma_list(&mut items, None);
                node("assert_statement", items)
            }
            "return" => {
                let mut items = vec![self.punct()];
                if self.starts_expression() {
                    items.push(self.expressions());
                }
                node("return_statement", items)
            }
            "del" => {
                let mut items = vec![self.punct()];
                items.push(self.expressions());
                node("delete_statement", items)
            }
            "raise" => {
                let mut items = vec![self.punct()];
                if self.starts_expression() {
                    items.push(self.expressions());
                }
                if self.at("from") {
                    items.push(self.punct());
                    items.push(self.expression().with("cause"));
                }
                node("raise_statement", items)
            }
            "pass" => node("pass_statement", vec![self.punct()]),
            "break" => node("break_statement", vec![self.punct()]),
            "continue" => node("continue_statement", vec![self.punct()]),
            w @ ("global" | "nonlocal") => {
                let kind = if w == "global" {
                    "global_statement"
                } else {
                    "nonlocal_statement"
                };
                let mut items = vec![self.punct()];
                loop {
                    items.push(self.identifier());
                    if !self.at(",") {
                        break;
                    }
                    items.push(self.punct());
                }
                node(kind, items)
            }
            _ => self.expression_statement(),
        }
    }

    /// `expression (, expression)*`, each with `field`, into `items`.
    fn comma_list(&mut self, items: &mut Vec<Item>, field: Option<&'static str>) {
        loop {
            let e = self.expression();
            items.push(match field {
                Some(f) => e.with(f),
                None => e,
            });
            if !self.at(",") {
                break;
            }
            items.push(self.punct());
            if !self.starts_expression() {
                break;
            }
        }
    }

    fn is_print_statement(&self) -> bool {
        if self.at_n(1, ">>") {
            return true;
        }
        match self.peek_at(1) {
            L::Name => !matches!(
                self.text_at(1),
                "and" | "or" | "in" | "is" | "if" | "else" | "for" | "as" | "not"
            ),
            L::StrStart | L::Int | L::Float => true,
            L::Op(o) => matches!(o, "{" | "~"),
            _ => false,
        }
    }

    fn print_statement(&mut self) -> Item {
        let mut items = vec![self.punct()];
        if self.at(">>") {
            let chevron = vec![self.punct(), self.expression()];
            items.push(node("chevron", chevron));
            while self.at(",") {
                items.push(self.punct());
                if !self.starts_expression() {
                    break;
                }
                items.push(self.expression().with("argument"));
            }
        } else {
            self.comma_list(&mut items, Some("argument"));
        }
        node("print_statement", items)
    }

    fn exec_statement(&mut self) -> Item {
        let mut items = vec![self.punct()];
        let code = if self.peek() == L::StrStart {
            self.string()
        } else {
            self.identifier()
        };
        items.push(code.with("code"));
        if self.at("in") {
            items.push(self.punct());
            self.comma_list(&mut items, None);
        }
        node("exec_statement", items)
    }

    fn import_statement(&mut self) -> Item {
        let mut items = vec![self.punct()];
        self.import_list(&mut items);
        node("import_statement", items)
    }

    fn import_list(&mut self, items: &mut Vec<Item>) {
        loop {
            let name = self.dotted_name();
            let item = if self.at("as") {
                let a = self.punct();
                let alias = self.identifier().with("alias");
                node("aliased_import", vec![name.with("name"), a, alias])
            } else {
                name
            };
            items.push(item.with("name"));
            if !self.at(",") {
                break;
            }
            items.push(self.punct());
            if self.peek() != L::Name {
                break;
            }
        }
    }

    fn dotted_name(&mut self) -> Item {
        let mut items = vec![self.name_any()];
        while self.at(".") && self.peek_at(1) == L::Name {
            items.push(self.punct());
            items.push(self.name_any());
        }
        node("dotted_name", items)
    }

    fn import_from(&mut self) -> Item {
        let from = self.punct();
        if self.at("__future__") && self.at_n(1, "import") {
            let mut items = vec![from, self.punct(), self.punct()];
            let paren = self.at("(");
            if paren {
                items.push(self.punct());
            }
            self.import_list(&mut items);
            if paren {
                items.push(self.expect(")"));
            }
            return node("future_import_statement", items);
        }
        let mut items = vec![from];
        let module = if self.at(".") || self.at("...") {
            let mut dots = Vec::new();
            while self.at(".") || self.at("...") {
                let t = self.advance();
                for k in t.start..t.end {
                    dots.push(Item::Tok(Tok {
                        kind: ".",
                        named: false,
                        start: k,
                        end: k + 1,
                        field: None,
                        hidden: false,
                        missing: false,
                        extra: false,
                    }));
                }
            }
            let mut parts = vec![node("import_prefix", dots)];
            if self.peek() == L::Name && !self.at("import") {
                parts.push(self.dotted_name());
            }
            node("relative_import", parts)
        } else {
            self.dotted_name()
        };
        items.push(module.with("module_name"));
        items.push(self.expect("import"));
        if self.at("*") {
            items.push(node("wildcard_import", vec![self.punct()]));
        } else if self.at("(") {
            items.push(self.punct());
            self.import_list(&mut items);
            items.push(self.expect(")"));
        } else {
            self.import_list(&mut items);
        }
        node("import_from_statement", items)
    }

    fn expression_statement(&mut self) -> Item {
        if self.at("yield") {
            let y = self.yield_();
            return node("expression_statement", vec![y]);
        }
        let first = self.expression();
        if self.at(",") {
            let mut elems = vec![first];
            while self.at(",") {
                elems.push(self.punct());
                if !self.starts_expression() {
                    break;
                }
                elems.push(self.expression());
            }
            if self.at("=") || self.at_any(AUGMENTED) {
                let left = node("pattern_list", elems.into_iter().map(to_pattern).collect());
                let a = self.assignment(left);
                return node("expression_statement", vec![a]);
            }
            return node("expression_statement", elems);
        }
        if self.at("=") || self.at(":") || self.at_any(AUGMENTED) {
            let a = self.assignment(to_pattern(first));
            return node("expression_statement", vec![a]);
        }
        node("expression_statement", vec![first])
    }

    /// An assignment of `left` (already a pattern) at its `=`, `:` or
    /// augmented operator.
    fn assignment(&mut self, left: Item) -> Item {
        let mut items = vec![left.with("left")];
        if self.at_any(AUGMENTED) {
            items.push(self.punct().with("operator"));
            items.push(self.right_hand_side().with("right"));
            return node("augmented_assignment", items);
        }
        if self.at(":") {
            items.push(self.punct());
            items.push(self.type_().with("type"));
            if !self.at("=") {
                return node("assignment", items);
            }
        }
        items.push(self.expect("="));
        items.push(self.right_hand_side().with("right"));
        node("assignment", items)
    }

    fn right_hand_side(&mut self) -> Item {
        if self.at("yield") {
            return self.yield_();
        }
        let e = self.expressions();
        if self.at("=") || self.at_any(AUGMENTED) {
            return self.assignment(to_pattern(e));
        }
        e
    }

    /// An expression, or several as an `expression_list`.
    fn expressions(&mut self) -> Item {
        let first = self.expression();
        if !self.at(",") {
            return first;
        }
        let mut items = vec![first];
        while self.at(",") {
            items.push(self.punct());
            if !self.starts_expression() {
                break;
            }
            items.push(self.expression());
        }
        node("expression_list", items)
    }

    fn yield_(&mut self) -> Item {
        let mut items = vec![self.punct()];
        if self.at("from") {
            items.push(self.punct());
            items.push(self.expression());
        } else if self.starts_expression() {
            items.push(self.expressions());
        }
        node("yield", items)
    }

    // ---- compound statements --------------------------------------------

    /// After a `:`: the indented block, the statements on the same line,
    /// or (a newline and no indent) an empty block.
    fn suite(&mut self) -> Item {
        if self.peek() == L::Newline {
            let nl = self.advance();
            if self.peek() == L::Indent {
                self.advance();
                return self.block(Vec::new(), None);
            }
            return node("block", vec![Self::hidden_at(nl.start)]);
        }
        let mut items = Vec::new();
        self.simple_statements(&mut items);
        node("block", items)
    }

    /// Statements up to the block's dedent (with `field` on each, for a
    /// `match` block's cases).
    fn block(&mut self, mut items: Vec<Item>, cases: Option<()>) -> Item {
        loop {
            match self.peek() {
                L::Dedent => {
                    let d = self.advance();
                    items.push(Self::hidden_at(d.start));
                    break;
                }
                L::Eof => break,
                L::Newline => {
                    let t = self.advance();
                    items.push(Self::hidden_at(t.start));
                }
                L::Indent => {
                    self.errors = true;
                    self.advance();
                }
                _ => {
                    let before = self.i;
                    if cases.is_some() && self.at("case") {
                        items.push(self.case_clause().with("alternative"));
                    } else {
                        self.statement(&mut items);
                    }
                    if self.i == before {
                        items.push(self.error_line());
                    }
                }
            }
        }
        node("block", items)
    }

    fn if_statement(&mut self) -> Item {
        let mut items = vec![self.punct()];
        items.push(self.expression().with("condition"));
        items.push(self.expect(":"));
        items.push(self.suite().with("consequence"));
        while self.at("elif") {
            let mut e = vec![self.punct()];
            e.push(self.expression().with("condition"));
            e.push(self.expect(":"));
            e.push(self.suite().with("consequence"));
            items.push(node("elif_clause", e).with("alternative"));
        }
        if self.at("else") {
            items.push(self.else_clause().with("alternative"));
        }
        node("if_statement", items)
    }

    fn else_clause(&mut self) -> Item {
        let items = vec![self.punct(), self.expect(":"), self.suite().with("body")];
        node("else_clause", items)
    }

    fn for_statement(&mut self) -> Item {
        let mut items = Vec::new();
        if self.at("async") {
            items.push(self.punct());
        }
        items.push(self.punct());
        items.push(self.targets().with("left"));
        items.push(self.expect("in"));
        items.push(self.expressions().with("right"));
        items.push(self.expect(":"));
        items.push(self.suite().with("body"));
        if self.at("else") {
            items.push(self.else_clause().with("alternative"));
        }
        node("for_statement", items)
    }

    fn while_statement(&mut self) -> Item {
        let mut items = vec![self.punct()];
        items.push(self.expression().with("condition"));
        items.push(self.expect(":"));
        items.push(self.suite().with("body"));
        if self.at("else") {
            items.push(self.else_clause().with("alternative"));
        }
        node("while_statement", items)
    }

    fn try_statement(&mut self) -> Item {
        let mut items = vec![self.punct(), self.expect(":"), self.suite().with("body")];
        while self.at("except") {
            let mut e = vec![self.punct()];
            if self.at("*") {
                e.push(self.punct());
            }
            if !self.at(":") {
                // `except E as e` reads as an `as_pattern`; Python 2's
                // `except E, e` as two values.
                self.comma_list(&mut e, Some("value"));
            }
            e.push(self.expect(":"));
            e.push(self.suite());
            items.push(node("except_clause", e));
        }
        if self.at("else") {
            items.push(self.else_clause());
        }
        if self.at("finally") {
            let f = vec![self.punct(), self.expect(":"), self.suite()];
            items.push(node("finally_clause", f));
        }
        node("try_statement", items)
    }

    fn with_statement(&mut self) -> Item {
        let mut items = Vec::new();
        if self.at("async") {
            items.push(self.punct());
        }
        items.push(self.punct());
        let mut clause = Vec::new();
        let parenthesized = self.at("(") && self.with_items_in_parens();
        if parenthesized {
            clause.push(self.punct());
        }
        loop {
            let item = node("with_item", vec![self.expression().with("value")]);
            clause.push(item);
            if !self.at(",") {
                break;
            }
            clause.push(self.punct());
            if !self.starts_expression() {
                break;
            }
        }
        if parenthesized {
            clause.push(self.expect(")"));
        }
        items.push(node("with_clause", clause));
        items.push(self.expect(":"));
        items.push(self.suite().with("body"));
        node("with_statement", items)
    }

    /// At `with (`: whether the parentheses hold the items (`with (a as b,
    /// c):`) rather than begin the first one's expression.
    fn with_items_in_parens(&self) -> bool {
        let mut depth = 0usize;
        let mut list = false;
        let mut n = 0;
        loop {
            match self.peek_at(n) {
                L::Op("(" | "[" | "{") => depth += 1,
                L::Op(")" | "]" | "}") => {
                    depth -= 1;
                    if depth == 0 {
                        return list && self.at_n(n + 1, ":");
                    }
                }
                // A comma between two items; `(a as b)` and `(a as b,)`
                // are one item's expression.
                L::Op(",") if depth == 1 && !self.at_n(n + 1, ")") => list = true,
                L::Newline | L::Eof => return false,
                _ => {}
            }
            n += 1;
        }
    }

    fn function_definition(&mut self) -> Item {
        let mut items = Vec::new();
        if self.at("async") {
            items.push(self.punct());
        }
        items.push(self.punct());
        items.push(self.identifier().with("name"));
        if self.at("[") {
            items.push(self.type_parameter().with("type_parameters"));
        }
        let mut params = vec![self.expect("(")];
        self.parameters(&mut params, false);
        params.push(self.expect(")"));
        items.push(node("parameters", params).with("parameters"));
        if self.at("->") {
            items.push(self.punct());
            items.push(self.type_().with("return_type"));
        }
        items.push(self.expect(":"));
        items.push(self.suite().with("body"));
        node("function_definition", items)
    }

    fn class_definition(&mut self) -> Item {
        let mut items = vec![self.punct()];
        items.push(self.identifier().with("name"));
        if self.at("[") {
            items.push(self.type_parameter().with("type_parameters"));
        }
        if self.at("(") {
            items.push(self.argument_list().with("superclasses"));
        }
        items.push(self.expect(":"));
        items.push(self.suite().with("body"));
        node("class_definition", items)
    }

    fn decorated(&mut self) -> Item {
        let mut items = Vec::new();
        while self.at("@") {
            let mut d = vec![self.punct(), self.expression()];
            if self.peek() == L::Newline {
                let t = self.advance();
                d.push(Self::hidden_at(t.start));
            }
            items.push(node("decorator", d));
        }
        let def = match (self.text_at(0), self.text_at(1)) {
            ("class", _) => self.class_definition(),
            ("def", _) | ("async", "def") => self.function_definition(),
            _ => self.error_line(),
        };
        items.push(def.with("definition"));
        node("decorated_definition", items)
    }

    /// Parameters up to `)` (or, `lambda`, up to `:`).
    fn parameters(&mut self, items: &mut Vec<Item>, lambda: bool) {
        loop {
            if self.at(")") || (lambda && self.at(":")) || !self.starts_parameter() {
                break;
            }
            items.push(self.parameter(lambda));
            if !self.at(",") {
                break;
            }
            items.push(self.punct());
        }
    }

    fn starts_parameter(&self) -> bool {
        match self.peek() {
            L::Name => !HARD.contains(&self.text_at(0)),
            L::Op(o) => matches!(o, "*" | "**" | "/" | "("),
            _ => false,
        }
    }

    fn parameter(&mut self, lambda: bool) -> Item {
        if self.at("/") {
            return node("positional_separator", vec![self.punct()]);
        }
        if self.at("*") || self.at("**") {
            let star = self.at("*");
            if star && (self.at_n(1, ",") || self.at_n(1, ")") || self.at_n(1, ":")) {
                return node("keyword_separator", vec![self.punct()]);
            }
            let op = self.punct();
            let kind = if star {
                "list_splat_pattern"
            } else {
                "dictionary_splat_pattern"
            };
            let splat = node(kind, vec![op, self.identifier()]);
            if !lambda && self.at(":") {
                let colon = self.punct();
                let ty = self.type_().with("type");
                return node("typed_parameter", vec![splat, colon, ty]);
            }
            return splat;
        }
        let name = if self.at("(") {
            to_pattern(self.atom())
        } else {
            self.identifier()
        };
        if !lambda && self.at(":") {
            let colon = self.punct();
            let ty = self.type_().with("type");
            if self.at("=") {
                let eq = self.punct();
                let value = self.expression().with("value");
                return node(
                    "typed_default_parameter",
                    vec![name.with("name"), colon, ty, eq, value],
                );
            }
            return node("typed_parameter", vec![name, colon, ty]);
        }
        if self.at("=") {
            let eq = self.punct();
            let value = self.expression().with("value");
            return node("default_parameter", vec![name.with("name"), eq, value]);
        }
        name
    }

    // ---- match -----------------------------------------------------------

    /// At `match`: whether it begins a `match` statement (its line ends in
    /// `:` and an indented `case` follows) rather than naming something.
    fn is_match_statement(&self) -> bool {
        if !self.starts_expression_at(1) {
            return false;
        }
        let mut n = 1;
        while !matches!(self.peek_at(n), L::Newline | L::Eof) {
            n += 1;
        }
        self.at_n(n - 1, ":") && self.peek_at(n + 1) == L::Indent && self.at_n(n + 2, "case")
    }

    fn match_statement(&mut self) -> Item {
        let mut items = vec![self.punct()];
        loop {
            items.push(self.expression().with("subject"));
            if !self.at(",") {
                break;
            }
            items.push(self.punct());
            if self.at(":") {
                break;
            }
        }
        items.push(self.expect(":"));
        let body = if self.peek() == L::Newline && self.peek_at(1) == L::Indent {
            self.advance();
            let indent = self.advance();
            self.block(vec![Self::hidden_at(indent.start)], Some(()))
        } else {
            self.suite()
        };
        items.push(body.with("body"));
        node("match_statement", items)
    }

    fn case_clause(&mut self) -> Item {
        let mut items = vec![self.punct()];
        loop {
            items.push(self.case_pattern());
            if !self.at(",") {
                break;
            }
            items.push(self.punct());
            if self.at(":") || self.at("if") {
                break;
            }
        }
        if self.at("if") {
            let guard = vec![self.punct(), self.expression()];
            items.push(node("if_clause", guard).with("guard"));
        }
        items.push(self.expect(":"));
        items.push(self.suite().with("consequence"));
        node("case_clause", items)
    }

    fn case_pattern(&mut self) -> Item {
        let inner = if self.peek() == L::Name && self.at_n(1, "=") {
            let mut items = vec![self.identifier(), self.punct()];
            items.extend(self.simple_pattern());
            vec![node("keyword_pattern", items)]
        } else {
            let first = self.simple_pattern();
            if self.at("|") {
                let mut items = first;
                while self.at("|") {
                    items.push(self.punct());
                    items.extend(self.simple_pattern());
                }
                vec![node("union_pattern", items)]
            } else {
                first
            }
        };
        let mut cp = node("case_pattern", inner);
        while self.at("as") {
            let a = self.punct();
            let name = self.identifier();
            cp = node("case_pattern", vec![node("as_pattern", vec![cp, a, name])]);
        }
        cp
    }

    /// One simple pattern: its items (a negative number is two).
    fn simple_pattern(&mut self) -> Vec<Item> {
        match self.peek() {
            L::Op("[") => {
                let mut items = vec![self.punct()];
                self.case_patterns(&mut items, "]");
                items.push(self.expect("]"));
                vec![node("list_pattern", items)]
            }
            L::Op("(") => {
                let mut items = vec![self.punct()];
                self.case_patterns(&mut items, ")");
                items.push(self.expect(")"));
                vec![node("tuple_pattern", items)]
            }
            L::Op("{") => vec![self.dict_pattern()],
            L::Op("*" | "**") => vec![self.splat_pattern()],
            L::Op("-") | L::Int | L::Float => self.number_pattern(),
            L::StrStart => vec![self.strings()],
            L::Name => match self.text_at(0) {
                "True" => vec![self.leaf("true")],
                "False" => vec![self.leaf("false")],
                "None" => vec![self.leaf("none")],
                "_" if !self.at_n(1, ".") && !self.at_n(1, "(") => vec![self.punct()],
                _ => {
                    let name = self.dotted_name();
                    if self.at("(") {
                        let mut items = vec![name, self.punct()];
                        self.case_patterns(&mut items, ")");
                        items.push(self.expect(")"));
                        vec![node("class_pattern", items)]
                    } else {
                        vec![name]
                    }
                }
            },
            _ => vec![self.missing("identifier", true)],
        }
    }

    fn case_patterns(&mut self, items: &mut Vec<Item>, close: &str) {
        while !self.at(close) && !matches!(self.peek(), L::Newline | L::Eof) {
            let before = self.i;
            items.push(self.case_pattern());
            if self.i == before || !self.at(",") {
                break;
            }
            items.push(self.punct());
        }
    }

    fn dict_pattern(&mut self) -> Item {
        let mut items = vec![self.punct()];
        while !self.at("}") && !matches!(self.peek(), L::Newline | L::Eof) {
            let before = self.i;
            if self.at("*") || self.at("**") {
                items.push(self.splat_pattern());
            } else {
                for k in self.simple_pattern() {
                    items.push(k.with("key"));
                }
                items.push(self.expect(":"));
                items.push(self.case_pattern().with("value"));
            }
            if self.i == before || !self.at(",") {
                break;
            }
            items.push(self.punct());
        }
        items.push(self.expect("}"));
        node("dict_pattern", items)
    }

    fn splat_pattern(&mut self) -> Item {
        let op = self.punct();
        let name = if self.at("_") {
            self.punct()
        } else {
            self.identifier()
        };
        node("splat_pattern", vec![op, name])
    }

    fn number_pattern(&mut self) -> Vec<Item> {
        let mut items = Vec::new();
        if self.at("-") {
            items.push(self.punct());
        }
        items.push(self.number());
        if (self.at("+") || self.at("-")) && matches!(self.peek_at(1), L::Int | L::Float) {
            items.push(self.punct());
            items.push(self.number());
            return vec![node("complex_pattern", items)];
        }
        items
    }

    fn number(&mut self) -> Item {
        match self.peek() {
            L::Int => self.leaf("integer"),
            L::Float => self.leaf("float"),
            _ => self.missing("integer", true),
        }
    }

    // ---- types -----------------------------------------------------------

    /// A `type`: a generic (`list[int]`), a splat (`*Ts`), or an
    /// expression; `|` between a generic and another type is a union.
    fn type_(&mut self) -> Item {
        let mut t = self.type_operand();
        loop {
            if self.at("|") {
                let bar = self.punct();
                let right = self.type_operand();
                t = node("type", vec![node("union_type", vec![t, bar, right])]);
            } else if self.at(".") && self.peek_at(1) == L::Name {
                let dot = self.punct();
                let name = self.identifier();
                t = node("type", vec![node("member_type", vec![t, dot, name])]);
            } else {
                break;
            }
        }
        t
    }

    /// One type, no `|` or `.` after it.
    fn type_operand(&mut self) -> Item {
        let inner = if (self.at("*") || self.at("**"))
            && self.peek_at(1) == L::Name
            && !self.at_n(2, "[")
            && !self.at_n(2, ".")
        {
            let op = self.punct();
            node("splat_type", vec![op, self.identifier()])
        } else if self.peek() == L::Name && self.at_n(1, "[") && (!HARD.contains(&self.text_at(0)))
        {
            let name = self.leaf("identifier");
            node("generic_type", vec![name, self.type_parameter()])
        } else {
            self.expression()
        };
        node("type", vec![inner])
    }

    fn type_parameter(&mut self) -> Item {
        let mut items = vec![self.punct()];
        while !self.at("]") && !matches!(self.peek(), L::Newline | L::Eof) {
            let before = self.i;
            let mut t = self.type_();
            if self.at(":") {
                let colon = self.punct();
                let c = self.type_();
                t = node("type", vec![node("constrained_type", vec![t, colon, c])]);
            }
            items.push(t);
            if self.i == before || !self.at(",") {
                break;
            }
            items.push(self.punct());
        }
        items.push(self.expect("]"));
        node("type_parameter", items)
    }

    // ---- expressions -----------------------------------------------------

    fn expression(&mut self) -> Item {
        self.expression_with(false)
    }

    /// An expression; `no_cond` where an `if` after it is a clause (a
    /// comprehension's), not a conditional expression.
    fn expression_with(&mut self, no_cond: bool) -> Item {
        let mut e = self.expression_no_as(no_cond);
        while self.at("as") {
            let a = self.punct();
            let target = node("as_pattern_target", vec![self.expression_no_as(no_cond)]);
            e = node("as_pattern", vec![e, a, target.with("alias")]);
        }
        e
    }

    fn expression_no_as(&mut self, no_cond: bool) -> Item {
        if self.at("lambda") {
            return self.lambda(no_cond);
        }
        if self.peek() == L::Name && self.at_n(1, ":=") && !HARD.contains(&self.text_at(0)) {
            let name = self.leaf("identifier").with("name");
            let op = self.punct();
            let value = self.expression().with("value");
            return node("named_expression", vec![name, op, value]);
        }
        let e = self.disjunction();
        if !no_cond && self.at("if") {
            let mut items = vec![e, self.punct(), self.disjunction()];
            items.push(self.expect("else"));
            items.push(self.expression());
            return node("conditional_expression", items);
        }
        e
    }

    fn lambda(&mut self, no_cond: bool) -> Item {
        let mut items = vec![self.punct()];
        if !self.at(":") {
            let mut params = Vec::new();
            self.parameters(&mut params, true);
            items.push(node("lambda_parameters", params).with("parameters"));
        }
        items.push(self.expect(":"));
        items.push(self.expression_with(no_cond).with("body"));
        node("lambda", items)
    }

    fn disjunction(&mut self) -> Item {
        let mut left = self.conjunction();
        while self.at("or") {
            let op = self.punct().with("operator");
            let right = self.conjunction().with("right");
            left = node("boolean_operator", vec![left.with("left"), op, right]);
        }
        left
    }

    fn conjunction(&mut self) -> Item {
        let mut left = self.inversion();
        while self.at("and") {
            let op = self.punct().with("operator");
            let right = self.inversion().with("right");
            left = node("boolean_operator", vec![left.with("left"), op, right]);
        }
        left
    }

    fn inversion(&mut self) -> Item {
        // tree-sitter takes `x and a := b` as `x and (a := b)`.
        if self.peek() == L::Name && self.at_n(1, ":=") && !HARD.contains(&self.text_at(0)) {
            let name = self.leaf("identifier").with("name");
            let op = self.punct();
            let value = self.expression().with("value");
            return node("named_expression", vec![name, op, value]);
        }
        if self.at("not") {
            let op = self.punct();
            let arg = self.inversion().with("argument");
            return node("not_operator", vec![op, arg]);
        }
        self.comparison()
    }

    fn comparison(&mut self) -> Item {
        let first = self.binary(0);
        let mut items = vec![first];
        loop {
            let op = if self.at_any(COMPARISONS) || self.at("in") {
                self.punct()
            } else if self.at("not") && self.at_n(1, "in") {
                let pair = vec![self.punct(), self.punct()];
                Item::Node(Node {
                    kind: "not in",
                    named: false,
                    field: None,
                    items: pair,
                })
            } else if self.at("is") {
                if self.at_n(1, "not") {
                    let pair = vec![self.punct(), self.punct()];
                    Item::Node(Node {
                        kind: "is not",
                        named: false,
                        field: None,
                        items: pair,
                    })
                } else {
                    self.punct()
                }
            } else {
                break;
            };
            items.push(op.with("operators"));
            items.push(self.binary(0));
        }
        if items.len() == 1 {
            return items.pop().unwrap_or_else(|| unreachable!());
        }
        node("comparison_operator", items)
    }

    fn binop(&self) -> Option<(u8, bool)> {
        let L::Op(o) = self.peek() else {
            return None;
        };
        Some(match o {
            "|" => (14, false),
            "&" => (15, false),
            "^" => (16, false),
            "<<" | ">>" => (17, false),
            "+" | "-" => (18, false),
            "*" | "@" | "/" | "%" | "//" => (19, false),
            "**" => (21, true),
            _ => return None,
        })
    }

    /// Binary operators of precedence `min` and above.
    fn binary(&mut self, min: u8) -> Item {
        let mut left = self.unary();
        while let Some((prec, right_assoc)) = self.binop() {
            if prec < min {
                break;
            }
            let op = self.punct().with("operator");
            let right = self.binary(if right_assoc { prec } else { prec + 1 });
            left = node(
                "binary_operator",
                vec![left.with("left"), op, right.with("right")],
            );
        }
        left
    }

    fn unary(&mut self) -> Item {
        if self.at("-") || self.at("+") || self.at("~") {
            let op = self.punct().with("operator");
            let arg = self.binary(21).with("argument");
            return node("unary_operator", vec![op, arg]);
        }
        if self.at("await") && self.starts_expression_at(1) && !self.at_n(1, "*") {
            let kw = self.punct();
            let arg = self.binary(21);
            return node("await", vec![kw, arg]);
        }
        self.primary()
    }

    fn primary(&mut self) -> Item {
        let mut e = self.atom();
        loop {
            if self.at(".") && self.peek_at(1) == L::Name {
                let dot = self.punct();
                let name = self.name_any().with("attribute");
                e = node("attribute", vec![e.with("object"), dot, name]);
            } else if self.at("(") {
                let args = self.call_arguments();
                e = node("call", vec![e.with("function"), args.with("arguments")]);
            } else if self.at("[") {
                e = self.subscript(e);
            } else {
                return e;
            }
        }
    }

    fn atom(&mut self) -> Item {
        match self.peek() {
            L::Name => match self.text_at(0) {
                "True" => self.leaf("true"),
                "False" => self.leaf("false"),
                "None" => self.leaf("none"),
                "lambda" => self.lambda(false),
                "yield" => self.yield_(),
                "not" => self.inversion(),
                w if HARD.contains(&w) => self.missing("identifier", true),
                _ => self.leaf("identifier"),
            },
            L::Int => self.leaf("integer"),
            L::Float => self.leaf("float"),
            L::StrStart => self.strings(),
            L::Op("...") => self.leaf("ellipsis"),
            L::Op("(") => self.parenthesized(),
            L::Op("[") => self.list(),
            L::Op("{") => self.braces(),
            L::Op("*") => {
                let star = self.punct();
                let operand = self.primary();
                node("list_splat", vec![star, operand])
            }
            L::Op("-" | "+" | "~") => self.unary(),
            _ => self.missing("identifier", true),
        }
    }

    /// An element of a list, set, tuple or argument list.
    fn star_item(&mut self) -> Item {
        if self.at("*") {
            let star = self.punct();
            let e = self.expression();
            return node("list_splat", vec![star, e]);
        }
        if self.at("yield") {
            return self.yield_();
        }
        let e = self.expression();
        // `(*x)` as an element is a `parenthesized_list_splat`.
        match &e {
            Item::Node(n)
                if n.kind == "parenthesized_expression"
                    && n.items.get(1).is_some_and(|i| i.kind() == "list_splat") =>
            {
                e.renamed("parenthesized_list_splat")
            }
            _ => e,
        }
    }

    /// Elements after the first up to `close`, comma-separated.
    fn elements(&mut self, items: &mut Vec<Item>, close: &str) {
        while self.at(",") {
            items.push(self.punct());
            if self.at(close) || !(self.starts_expression() || self.at("yield")) {
                break;
            }
            items.push(self.star_item());
        }
    }

    fn parenthesized(&mut self) -> Item {
        let open = self.punct();
        if self.at(")") {
            return node("tuple", vec![open, self.punct()]);
        }
        let first = self.star_item();
        let is_splat = first.kind() == "list_splat";
        if self.at("for") || (self.at("async") && self.at_n(1, "for")) {
            let mut items = vec![open, first.with("body")];
            self.comprehension(&mut items);
            items.push(self.expect(")"));
            return node("generator_expression", items);
        }
        if self.at(")") {
            let close = self.punct();
            let _ = is_splat;
            return node("parenthesized_expression", vec![open, first, close]);
        }
        let mut items = vec![open, first];
        self.elements(&mut items, ")");
        items.push(self.expect(")"));
        node("tuple", items)
    }

    fn list(&mut self) -> Item {
        let open = self.punct();
        if self.at("]") {
            return node("list", vec![open, self.punct()]);
        }
        let first = self.star_item();
        if self.at("for") || (self.at("async") && self.at_n(1, "for")) {
            let mut items = vec![open, first.with("body")];
            self.comprehension(&mut items);
            items.push(self.expect("]"));
            return node("list_comprehension", items);
        }
        let mut items = vec![open, first];
        self.elements(&mut items, "]");
        items.push(self.expect("]"));
        node("list", items)
    }

    fn braces(&mut self) -> Item {
        let open = self.punct();
        if self.at("}") {
            return node("dictionary", vec![open, self.punct()]);
        }
        let first = self.dict_or_set_item();
        let is_dict = matches!(first.kind(), "pair" | "dictionary_splat");
        if self.at("for") || (self.at("async") && self.at_n(1, "for")) {
            let kind = if is_dict {
                "dictionary_comprehension"
            } else {
                "set_comprehension"
            };
            let mut items = vec![open, first.with("body")];
            self.comprehension(&mut items);
            items.push(self.expect("}"));
            return node(kind, items);
        }
        let mut items = vec![open, first];
        while self.at(",") {
            items.push(self.punct());
            if self.at("}") || !(self.starts_expression() || self.at("yield")) {
                break;
            }
            items.push(self.dict_or_set_item());
        }
        items.push(self.expect("}"));
        node(if is_dict { "dictionary" } else { "set" }, items)
    }

    fn dict_or_set_item(&mut self) -> Item {
        if self.at("**") {
            let op = self.punct();
            let e = self.expression();
            return node("dictionary_splat", vec![op, e]);
        }
        let first = self.star_item();
        if self.at(":") {
            let colon = self.punct();
            let value = self.expression().with("value");
            return node("pair", vec![first.with("key"), colon, value]);
        }
        first
    }

    /// `for … in …` and `if …` clauses after a comprehension's body.
    fn comprehension(&mut self, items: &mut Vec<Item>) {
        loop {
            if self.at("for") || (self.at("async") && self.at_n(1, "for")) {
                let mut c = Vec::new();
                if self.at("async") {
                    c.push(self.punct());
                }
                c.push(self.punct());
                c.push(self.targets().with("left"));
                c.push(self.expect("in"));
                loop {
                    c.push(self.expression_with(true).with("right"));
                    if !self.at(",") {
                        break;
                    }
                    c.push(self.punct());
                    if !self.starts_expression() {
                        break;
                    }
                }
                items.push(node("for_in_clause", c));
            } else if self.at("if") {
                let c = vec![self.punct(), self.expression_with(true)];
                items.push(node("if_clause", c));
            } else {
                return;
            }
        }
    }

    /// The targets of a `for`: a pattern, or several as a `pattern_list`.
    fn targets(&mut self) -> Item {
        let first = self.binary(0);
        if !self.at(",") {
            return to_pattern(first);
        }
        let mut items = vec![to_pattern(first)];
        while self.at(",") {
            items.push(self.punct());
            if self.at("in") || !self.starts_expression() {
                break;
            }
            items.push(to_pattern(self.binary(0)));
        }
        node("pattern_list", items)
    }

    /// A call's arguments at its `(`: an argument list, or a generator.
    fn call_arguments(&mut self) -> Item {
        let save = self.i;
        let open = self.punct();
        if !self.at(")") && !self.at("*") && !self.at("**") && !self.is_keyword_argument() {
            let first = self.expression();
            if self.at("for") || (self.at("async") && self.at_n(1, "for")) {
                let mut items = vec![open, first.with("body")];
                self.comprehension(&mut items);
                items.push(self.expect(")"));
                return node("generator_expression", items);
            }
        }
        self.i = save;
        self.argument_list()
    }

    fn is_keyword_argument(&self) -> bool {
        self.peek() == L::Name && self.at_n(1, "=") && !HARD.contains(&self.text_at(0))
    }

    fn argument_list(&mut self) -> Item {
        let mut items = vec![self.punct()];
        while !self.at(")") && !matches!(self.peek(), L::Eof) {
            let before = self.i;
            let arg = if self.at("*") {
                let star = self.punct();
                node("list_splat", vec![star, self.expression()])
            } else if self.at("**") {
                let op = self.punct();
                node("dictionary_splat", vec![op, self.expression()])
            } else if self.is_keyword_argument() {
                let name = self.leaf("identifier").with("name");
                let eq = self.punct();
                let value = self.expression().with("value");
                node("keyword_argument", vec![name, eq, value])
            } else {
                self.expression()
            };
            items.push(arg);
            if self.i == before || !self.at(",") {
                break;
            }
            items.push(self.punct());
        }
        items.push(self.expect(")"));
        node("argument_list", items)
    }

    fn subscript(&mut self, value: Item) -> Item {
        let mut items = vec![value.with("value"), self.punct()];
        loop {
            let before = self.i;
            items.push(self.subscript_item().with("subscript"));
            if self.i == before || !self.at(",") {
                break;
            }
            items.push(self.punct());
            if self.at("]") {
                break;
            }
        }
        items.push(self.expect("]"));
        node("subscript", items)
    }

    fn subscript_item(&mut self) -> Item {
        let mut items = Vec::new();
        if !self.at(":") {
            let e = self.star_item();
            if !self.at(":") {
                return e;
            }
            items.push(e);
        }
        items.push(self.punct());
        if !self.at(":") && !self.at(",") && !self.at("]") {
            items.push(self.expression());
        }
        if self.at(":") {
            items.push(self.punct());
            if !self.at(",") && !self.at("]") {
                items.push(self.expression());
            }
        }
        node("slice", items)
    }

    // ---- strings ---------------------------------------------------------

    /// A string, or several side by side as a `concatenated_string`.
    fn strings(&mut self) -> Item {
        let first = self.string();
        if self.peek() != L::StrStart {
            return first;
        }
        let mut items = vec![first];
        while self.peek() == L::StrStart {
            items.push(self.string());
        }
        node("concatenated_string", items)
    }

    fn string(&mut self) -> Item {
        let mut items = vec![self.leaf("string_start")];
        let mut content: Vec<Item> = Vec::new();
        let flush = |content: &mut Vec<Item>, items: &mut Vec<Item>| {
            if !content.is_empty() {
                items.push(node("string_content", std::mem::take(content)));
            }
        };
        loop {
            match self.peek() {
                L::StrText => content.push(self.hidden()),
                L::NotEscape => content.push(self.punct_as("\\")),
                L::Escape => content.push(self.leaf("escape_sequence")),
                L::EscInterp => content.push(self.leaf("escape_interpolation")),
                L::Op("{") => {
                    flush(&mut content, &mut items);
                    items.push(self.interpolation("interpolation"));
                }
                L::Bad => {
                    flush(&mut content, &mut items);
                    self.errors = true;
                    let t = self.advance();
                    items.push(node("ERROR", vec![Self::tok(t, "ERROR", false)]));
                }
                L::StrEnd => {
                    flush(&mut content, &mut items);
                    items.push(self.leaf("string_end"));
                    break;
                }
                _ => {
                    flush(&mut content, &mut items);
                    items.push(self.missing("string_end", true));
                    break;
                }
            }
        }
        node("string", items)
    }

    /// An interpolation at its `{` (`format_expression` in a format
    /// specifier).
    fn interpolation(&mut self, kind: &'static str) -> Item {
        let mut items = vec![self.punct()];
        let e = if self.at("yield") {
            self.yield_()
        } else {
            self.expressions()
        };
        items.push(e.with("expression"));
        if self.at("=") {
            items.push(self.punct());
        }
        if self.peek() == L::Conversion {
            items.push(self.leaf("type_conversion").with("type_conversion"));
        }
        if self.at(":") {
            let mut spec = vec![self.punct()];
            loop {
                match self.peek() {
                    L::FmtText => spec.push(self.hidden()),
                    L::Op("{") => spec.push(self.interpolation("format_expression")),
                    _ => break,
                }
            }
            items.push(node("format_specifier", spec).with("format_specifier"));
        }
        items.push(self.expect("}"));
        node(kind, items)
    }
}

/// An expression read where a pattern belongs (an assignment's or a
/// `for`'s target), as that pattern.
fn to_pattern(item: Item) -> Item {
    let kind = match item.kind() {
        "tuple" | "parenthesized_expression" => "tuple_pattern",
        "list" => "list_pattern",
        "list_splat" => "list_splat_pattern",
        "expression_list" => "pattern_list",
        _ => return item,
    };
    let Item::Node(n) = item else {
        return item;
    };
    let splat = kind == "list_splat_pattern";
    let items = n
        .items
        .into_iter()
        .map(|i| {
            if !splat && i.is_named() {
                to_pattern(i)
            } else {
                i
            }
        })
        .collect();
    Item::Node(Node {
        kind,
        named: true,
        field: n.field,
        items,
    })
    .renamed(kind)
}
