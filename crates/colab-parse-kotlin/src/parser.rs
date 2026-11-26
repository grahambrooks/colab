//! Kotlin read by recursive descent into a tree of [`Item`]s:
//! tree-sitter-kotlin-ng's grammar (the file, declarations and statements
//! here, expressions in `parser/expr.rs`, types in `parser/types.rs`),
//! kinds and fields named as in `names.rs`.
//!
//! A statement ends where tree-sitter-kotlin's scanner inserts a semicolon
//! ([`Parser::stops`]): at `;`, or at a line end unless the next line
//! continues the statement. Where the grammar is ambiguous the parser reads
//! as tree-sitter's GLR parse does. A construct it cannot read becomes an
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
    pub kind: &'static str,
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

    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Item::Tok(t) => t.kind,
            Item::Node(n) => n.kind,
        }
    }

    pub(crate) fn with(mut self, field: &'static str) -> Self {
        match &mut self {
            Item::Tok(t) => t.field = Some(field),
            Item::Node(n) => n.field = Some(field),
        }
        self
    }
}

pub(crate) fn node(kind: &'static str, items: Vec<Item>) -> Item {
    Item::Node(Node {
        kind,
        field: None,
        items,
    })
}

/// Words that never stand for a name in an expression.
pub(crate) const KEYWORDS: &[&str] = &[
    "as",
    "class",
    "do",
    "else",
    "for",
    "fun",
    "if",
    "in",
    "interface",
    "is",
    "object",
    "package",
    "return",
    "super",
    "this",
    "throw",
    "try",
    "typealias",
    "val",
    "var",
    "when",
    "while",
    "import",
    "by",
    "where",
    "catch",
    "finally",
];

/// Modifier words, by the node each is in.
pub(crate) const MODIFIERS: &[(&str, &str)] = &[
    ("enum", "class_modifier"),
    ("sealed", "class_modifier"),
    ("annotation", "class_modifier"),
    ("data", "class_modifier"),
    ("inner", "class_modifier"),
    ("value", "class_modifier"),
    ("tailrec", "function_modifier"),
    ("operator", "function_modifier"),
    ("infix", "function_modifier"),
    ("inline", "function_modifier"),
    ("external", "function_modifier"),
    ("suspend", "function_modifier"),
    ("const", "property_modifier"),
    ("public", "visibility_modifier"),
    ("private", "visibility_modifier"),
    ("protected", "visibility_modifier"),
    ("internal", "visibility_modifier"),
    ("abstract", "inheritance_modifier"),
    ("final", "inheritance_modifier"),
    ("open", "inheritance_modifier"),
    ("override", "member_modifier"),
    ("lateinit", "member_modifier"),
    ("vararg", "parameter_modifier"),
    ("noinline", "parameter_modifier"),
    ("crossinline", "parameter_modifier"),
    ("expect", "platform_modifier"),
    ("actual", "platform_modifier"),
];

/// The modifiers tree-sitter-kotlin's scanner reads past at a line start.
const SCANNER_MODIFIERS: &[&str] = &[
    "public",
    "private",
    "protected",
    "internal",
    "abstract",
    "final",
    "open",
    "override",
    "lateinit",
    "vararg",
    "noinline",
    "crossinline",
    "external",
    "suspend",
    "inline",
];

/// What a run of statements is the body of.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Ctx {
    /// The file or a block: statements, expressions among them.
    Statements,
    /// A class body: declarations only.
    Members,
}

pub(crate) struct Parser<'s> {
    src: &'s str,
    toks: Vec<Lx>,
    i: usize,
    pub errors: bool,
    /// Inside parentheses or brackets, where line ends do not end
    /// statements.
    nl_blind: usize,
    /// A class body's members are being read (the scanner's
    /// `_class_member_semi`).
    members: bool,
    /// A property may take a getter or setter on the next line.
    accessor_ok: bool,
    /// A `when` entry's condition is being read (`in` on the next line
    /// starts a range test).
    in_when: bool,
    /// A `{` here does not start a trailing lambda (a delegation's target).
    no_lambda: bool,
    /// The type being read is a function type's result.
    no_nullable: bool,
    /// The file's own statements are being read (no braces around).
    top: bool,
    /// An operator's right operand is being read: a generic call there is
    /// left to the operator (tree-sitter's parse calls the operation).
    rhs: bool,
    /// Comments' (start, end), in order, for where a semicolon lands.
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
            members: false,
            accessor_ok: false,
            in_when: false,
            no_lambda: false,
            no_nullable: false,
            top: true,
            rhs: false,
            comments,
        }
    }

    /// The end of the last comment between the previous token and the
    /// current one, else the previous token's end: where tree-sitter's
    /// zero-width semicolon lands.
    fn semi_at(&self) -> usize {
        let prev = self.prev_end();
        let next = self.tk(self.i).start;
        let k = self.comments.partition_point(|c| c.1 <= next);
        match k.checked_sub(1).map(|k| self.comments[k]) {
            Some((start, end)) if start >= prev => end,
            _ => prev,
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

    /// Whether token `n` is a name (no keyword).
    fn is_ident(&self, n: usize) -> bool {
        self.kind_at(n) == L::Word && !KEYWORDS.contains(&self.word_at(n))
    }

    fn eof(&self) -> bool {
        self.peek() == L::Eof
    }

    /// Whether a line ends before token `n`.
    fn nl_at(&self, n: usize) -> bool {
        self.tk(n).nl
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

    /// The current token, anonymous, named by its text.
    fn punct(&mut self) -> Item {
        let t = self.advance();
        let kind = match t.k {
            L::Op(o) | L::StrOpen(o) | L::StrClose(o) => o,
            L::InterpOpen => "${",
            L::Dollar => "$",
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

    /// A name as `identifier` (any word: keywords where a name must be).
    fn identifier(&mut self) -> Item {
        if self.peek() == L::Word {
            self.leaf("identifier")
        } else {
            self.missing("identifier", true)
        }
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
                L::Op("(" | "[" | "{") | L::InterpOpen => depth += 1,
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

    /// Run `f` with line ends ignored (inside brackets).
    fn blind<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> T {
        self.nl_blind += 1;
        let r = f(self);
        self.nl_blind -= 1;
        r
    }

    /// Run `f` with line ends ending statements again (inside braces).
    fn sighted<T>(&mut self, members: bool, f: impl FnOnce(&mut Self) -> T) -> T {
        let saved = (
            self.nl_blind,
            self.members,
            self.accessor_ok,
            self.in_when,
            self.top,
        );
        self.nl_blind = 0;
        self.members = members;
        self.accessor_ok = false;
        self.in_when = false;
        self.top = false;
        let r = f(self);
        (
            self.nl_blind,
            self.members,
            self.accessor_ok,
            self.in_when,
            self.top,
        ) = saved;
        r
    }

    /// Whether tree-sitter-kotlin's scanner ends the statement before the
    /// current token: at the end, or after a line end when the next line
    /// does not continue the statement.
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
        let next = |k: usize| text.as_bytes().get(k).copied().unwrap_or(0);
        match t.k {
            L::Op(o) => match o.as_bytes()[0] {
                b',' | b'.' | b':' | b'*' | b'%' | b'>' | b'<' | b'=' | b'{' | b'[' | b'|'
                | b'&' | b'/' => false,
                b'+' | b'-' => next(1) == next(0) || next(1).is_ascii_digit(),
                b'!' => {
                    if o == "!is" {
                        return true;
                    }
                    next(1) != b'='
                }
                // `?.` and `?:` continue; the scanner ends nothing at `?`.
                b'?' => false,
                // `as?`: the scanner reads the word `as`.
                b'a' => false,
                _ => true,
            },
            L::Word => {
                // Modifiers first, then the word after them.
                let mut k = n;
                while SCANNER_MODIFIERS.contains(&self.word_at(k)) {
                    k += 1;
                }
                let w = self.word_at(k);
                match w {
                    "else" => self.is(k + 1, "->"),
                    "in" => self.in_when,
                    "get" | "set" => !self.accessor_ok || self.is(k + 1, "["),
                    "constructor" => self.members || self.is(k + 1, ".") || self.is(k + 1, "="),
                    "instanceof" | "by" | "as" | "where" => false,
                    _ => true,
                }
            }
            _ => true,
        }
    }

    /// The statement's `;` (no token of its own), if there; else, at a line
    /// end, the scanner's zero-width one after any comments (which the
    /// node then holds).
    fn semi(&mut self, items: &mut Vec<Item>) {
        if self.at(";") {
            let t = self.advance();
            items.push(Self::hidden_tok(t));
        } else if self.stops() {
            let at = self.semi_at();
            items.push(Self::hidden_tok(Lx {
                k: L::Op(";"),
                start: at,
                end: at,
                nl: false,
            }));
        }
    }

    // ---- the file and statements -----------------------------------------

    pub(crate) fn source_file(&mut self) -> Item {
        let mut items = Vec::new();
        if self.peek() == L::Shebang {
            items.push(self.leaf("shebang"));
        }
        while self.at("@") && self.text_at(self.i + 1) == "file" && self.is(self.i + 2, ":") {
            items.push(self.file_annotation());
        }
        if self.at("package") {
            let mut p = vec![self.punct(), self.qualified_identifier()];
            if self.at(";") {
                p.push(self.punct());
            }
            items.push(node("package_header", p));
        }
        while self.at("import") {
            let mut p = vec![self.punct(), self.qualified_identifier()];
            if self.at(".") && self.is(self.i + 1, "*") {
                p.push(self.punct());
                p.push(self.punct());
            } else if self.at("as") {
                p.push(self.punct());
                p.push(self.identifier());
            }
            if self.at(";") {
                p.push(self.punct());
            }
            items.push(node("import", p));
        }
        self.statements(&mut items, Ctx::Statements, |_| false);
        node("source_file", items)
    }

    fn file_annotation(&mut self) -> Item {
        let mut items = vec![self.punct(), self.punct(), self.punct()];
        if self.at("[") {
            items.push(self.punct());
            while !self.eof() && !self.at("]") {
                let before = self.i;
                items.push(self.unescaped_annotation());
                if self.i == before {
                    items.push(self.error_token());
                }
            }
            items.push(self.expect("]"));
        } else {
            items.push(self.unescaped_annotation());
        }
        self.semi(&mut items);
        node("file_annotation", items)
    }

    pub(crate) fn qualified_identifier(&mut self) -> Item {
        let mut items = vec![self.identifier()];
        while self.at(".") && self.kind_at(self.i + 1) == L::Word {
            items.push(self.punct());
            items.push(self.identifier());
        }
        node("qualified_identifier", items)
    }

    /// Statements (or members) until `}` or `end` says so.
    pub(crate) fn statements(
        &mut self,
        items: &mut Vec<Item>,
        ctx: Ctx,
        end: impl Fn(&Self) -> bool,
    ) {
        loop {
            while self.at(";") {
                let t = self.advance();
                items.push(Self::hidden_tok(t));
            }
            if self.eof() || self.at("}") || end(self) {
                return;
            }
            let before = self.i;
            let s = match ctx {
                Ctx::Statements => self.statement(),
                Ctx::Members => self.member(),
            };
            items.push(s);
            if self.i == before {
                items.push(self.error_token());
                continue;
            }
            // What is left on the line before the next statement.
            if !self.stops() && !self.at("}") && !end(self) {
                self.errors = true;
                if !self.eof() {
                    items.push(self.error_token());
                }
            }
        }
    }

    pub(crate) fn block(&mut self) -> Item {
        self.sighted(false, |p| {
            let mut items = vec![p.expect("{")];
            p.statements(&mut items, Ctx::Statements, |_| false);
            items.push(p.expect("}"));
            node("block", items)
        })
    }

    pub(crate) fn statement(&mut self) -> Item {
        if let Some(d) = self.declaration_here() {
            return d;
        }
        let w = self.word();
        let label = self.peek() == L::Label
            && !matches!(self.text_at(self.i), "this@" | "super@" | "return@")
            && matches!(self.word_at(self.i + 1), "for" | "while" | "do");
        if label || matches!(w, "for" | "while" | "do") {
            return self.loop_statement();
        }
        let e = self.expression();
        let op = match self.peek() {
            L::Op(o @ ("=" | "+=" | "-=" | "*=" | "/=" | "%=")) if !self.stops() => o,
            _ => return e,
        };
        let _ = op;
        let items = vec![
            e.with("left"),
            self.punct().with("operator"),
            self.expression().with("right"),
        ];
        node("assignment", items)
    }

    fn loop_statement(&mut self) -> Item {
        let mut items = Vec::new();
        if self.peek() == L::Label {
            items.push(self.leaf("label"));
        }
        match self.word() {
            "for" => {
                items.push(self.punct());
                items.push(self.expect("("));
                self.blind(|p| {
                    while p.at("@") {
                        items.push(p.annotation());
                    }
                    items.push(if p.at("(") {
                        p.multi_variable_declaration()
                    } else {
                        p.variable_declaration()
                    });
                    items.push(p.expect("in"));
                    items.push(p.expression());
                });
                items.push(self.expect(")"));
                if let Some(b) = self.loop_body() {
                    items.push(b);
                }
                node("for_statement", items)
            }
            "while" => {
                items.push(self.punct());
                items.push(self.expect("("));
                items.push(self.blind(Self::expression).with("condition"));
                items.push(self.expect(")"));
                if self.at(";") && !self.stops_after_semicolon() {
                    items.push(self.punct());
                } else if let Some(b) = self.loop_body() {
                    items.push(b);
                }
                node("while_statement", items)
            }
            _ => {
                items.push(self.punct());
                if !self.at("while") {
                    if self.at(";") {
                        items.push(self.punct());
                    } else if let Some(b) = self.loop_body() {
                        items.push(b);
                    }
                }
                items.push(self.expect("while"));
                items.push(self.expect("("));
                items.push(self.blind(Self::expression).with("condition"));
                items.push(self.expect(")"));
                node("do_while_statement", items)
            }
        }
    }

    /// A `;` after `while (…)`: the loop's own (a `;` is the statement's
    /// end otherwise).
    fn stops_after_semicolon(&self) -> bool {
        false
    }

    /// A loop's body: a block or a statement, when one follows on the line.
    fn loop_body(&mut self) -> Option<Item> {
        if self.at("{") {
            return Some(self.block());
        }
        if self.stops() || self.at("}") || self.at(")") {
            return None;
        }
        Some(self.statement())
    }
}
