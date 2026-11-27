//! C#, read by recursive descent into a tree of [`Item`]s: tree-sitter-
//! c-sharp's grammar (declarations, statements, the expression
//! precedences, patterns, queries, preprocessor conditionals), with kinds
//! and fields named as in `names.rs`.
//!
//! Where C# needs lookahead (a local declaration or an expression
//! statement; a cast, a lambda's parameters, a tuple or a parenthesized
//! expression; a generic name or a comparison) the parser scans the
//! tokens ahead with the `scan_*` functions, which build nothing, and
//! decides as tree-sitter's GLR parse does where the two differ from the
//! language (`(a) - b` is a cast). A construct it cannot read becomes an
//! `ERROR` node, and reading resumes at the next statement or member; a
//! token that should be there and is not is a missing one.
//!
//! The directives other than `#if` / `#elif` / `#else` / `#endif` are
//! extras, as comments are: [`Parser::new`] takes their lines out of the
//! token stream and builds their nodes, which are placed after parsing.

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
    /// `"ERROR"` for text the parser could not read.
    pub kind: &'static str,
    pub field: Option<&'static str>,
    pub items: Vec<Item>,
    /// An extra (a directive's node).
    pub extra: bool,
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
}

fn node(kind: &'static str, items: Vec<Item>) -> Item {
    Item::Node(Node {
        kind,
        field: None,
        items,
        extra: false,
    })
}

/// C#'s reserved keywords: never identifiers (unless written `@word`).
const KEYWORDS: &[&str] = &[
    "abstract",
    "as",
    "base",
    "bool",
    "break",
    "byte",
    "case",
    "catch",
    "char",
    "checked",
    "class",
    "const",
    "continue",
    "decimal",
    "default",
    "delegate",
    "do",
    "double",
    "else",
    "enum",
    "event",
    "explicit",
    "extern",
    "false",
    "finally",
    "fixed",
    "float",
    "for",
    "foreach",
    "goto",
    "if",
    "implicit",
    "in",
    "int",
    "interface",
    "internal",
    "is",
    "lock",
    "long",
    "namespace",
    "new",
    "null",
    "object",
    "operator",
    "out",
    "override",
    "params",
    "private",
    "protected",
    "public",
    "readonly",
    "ref",
    "return",
    "sbyte",
    "sealed",
    "short",
    "sizeof",
    "stackalloc",
    "static",
    "string",
    "struct",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "uint",
    "ulong",
    "unchecked",
    "unsafe",
    "ushort",
    "using",
    "virtual",
    "void",
    "volatile",
    "while",
    "nint",
    "nuint",
    "__makeref",
    "__reftype",
    "__refvalue",
];

/// tree-sitter-c-sharp's `_reserved_identifier`: words that are keywords
/// somewhere, read as an `identifier` holding the keyword's token.
const RESERVED_IDENTIFIERS: &[&str] = &[
    "alias",
    "ascending",
    "by",
    "descending",
    "equals",
    "field",
    "file",
    "from",
    "global",
    "group",
    "into",
    "join",
    "let",
    "method",
    "notnull",
    "on",
    "orderby",
    "param",
    "property",
    "scoped",
    "select",
    "type",
    "typevar",
    "unmanaged",
    "var",
    "when",
    "where",
    "yield",
];

pub(crate) const PREDEFINED: &[&str] = &[
    "bool", "byte", "char", "decimal", "double", "float", "int", "long", "object", "sbyte",
    "short", "string", "uint", "ulong", "ushort", "nint", "nuint", "void",
];

const MODIFIERS: &[&str] = &[
    "abstract",
    "async",
    "const",
    "extern",
    "file",
    "fixed",
    "internal",
    "new",
    "override",
    "partial",
    "private",
    "protected",
    "public",
    "readonly",
    "required",
    "sealed",
    "static",
    "unsafe",
    "virtual",
    "volatile",
];

const ASSIGNMENTS: &[&str] = &[
    "=", "+=", "-=", "*=", "/=", "%=", "&=", "^=", "|=", "<<=", ">>=", ">>>=", "??=",
];

pub(crate) struct Parser<'s> {
    src: &'s str,
    toks: Vec<Lx>,
    i: usize,
    pub errors: bool,
    /// The directives' nodes, extras placed after parsing.
    pub extras: Vec<Item>,
    /// A conditional's alternative was a conditional directive holding
    /// the rest of a `return` statement, its `;` included.
    tail_taken: bool,
    /// In a `when` clause: no lambda at its level.
    no_lambda: bool,
    /// Query expressions being read (their keywords end expressions).
    queries: usize,
}

impl<'s> Parser<'s> {
    pub fn new(src: &'s str, tokens: Vec<Lx>) -> Self {
        let mut p = Self {
            src,
            toks: Vec::with_capacity(tokens.len()),
            i: 0,
            errors: false,
            extras: Vec::new(),
            tail_taken: false,
            no_lambda: false,
            queries: 0,
        };
        let mut k = 0;
        while k < tokens.len() {
            let t = tokens[k];
            if let L::Directive(d) = t.k
                && !matches!(d, "#if" | "#elif" | "#else" | "#endif")
            {
                let mut end = k + 1;
                while end < tokens.len() && !matches!(tokens[end].k, L::LineEnd | L::Eof) {
                    end += 1;
                }
                if end < tokens.len() && tokens[end].k == L::LineEnd {
                    end += 1;
                }
                let line = p.directive_node(d, &tokens[k..end]);
                p.extras.push(line);
                k = end;
                continue;
            }
            p.toks.push(t);
            k += 1;
        }
        p
    }

    /// An extra directive's line as its node.
    fn directive_node(&mut self, d: &'static str, toks: &[Lx]) -> Item {
        let kind = match d {
            "#region" => "preproc_region",
            "#endregion" => "preproc_endregion",
            "#pragma" => "preproc_pragma",
            "#nullable" => "preproc_nullable",
            "#line" => "preproc_line",
            "#error" => "preproc_error",
            "#warning" => "preproc_warning",
            "#define" => "preproc_define",
            _ => "preproc_undef",
        };
        let mut items = Vec::new();
        let mut words = 0;
        let mut j = 0;
        let ended = toks.last().is_some_and(|t| t.k == L::LineEnd);
        while j < toks.len() {
            let t = toks[j];
            j += 1;
            match t.k {
                L::Directive(d) => items.push(Self::tok(t, d, false)),
                L::PreprocArg => {
                    let arg = Self::tok(t, "preproc_arg", true);
                    items.push(if matches!(kind, "preproc_region" | "preproc_endregion") {
                        arg.with("content")
                    } else {
                        arg
                    });
                }
                L::LineEnd => items.push(Self::hidden(t.start, t.end)),
                L::Name => {
                    let w = &self.src[t.start..t.end];
                    let keyword = match kind {
                        "preproc_pragma" => {
                            words < 2 && matches!(w, "warning" | "disable" | "restore" | "checksum")
                        }
                        "preproc_nullable" => matches!(
                            w,
                            "enable" | "disable" | "restore" | "annotations" | "warnings"
                        ),
                        "preproc_line" => matches!(w, "default" | "hidden"),
                        _ => false,
                    };
                    words += 1;
                    if keyword {
                        items.push(Self::tok(t, names::anonymous(w).unwrap_or("ERROR"), false));
                    } else {
                        items.push(Self::tok(t, "identifier", true));
                    }
                }
                L::Int => items.push(Self::tok(t, "integer_literal", true)),
                L::Op(o) => items.push(Self::tok(t, o, false)),
                L::Quote => {
                    let mut s = vec![Self::tok(t, "\"", false)];
                    while j < toks.len() {
                        let u = toks[j];
                        j += 1;
                        match u.k {
                            L::StrContent => s.push(Self::tok(u, "string_literal_content", true)),
                            L::Escape => s.push(Self::tok(u, "escape_sequence", true)),
                            L::Quote => {
                                s.push(Self::tok(u, "\"", false));
                                break;
                            }
                            _ => {
                                j -= 1;
                                break;
                            }
                        }
                    }
                    items.push(node("string_literal", s));
                }
                _ => {
                    self.errors = true;
                    items.push(Self::tok(t, "ERROR", false));
                }
            }
        }
        if !ended {
            self.errors = true;
        }
        Item::Node(Node {
            kind,
            field: None,
            items,
            extra: true,
        })
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

    /// Whether token `n` is the operator or word `s`.
    fn is(&self, n: usize, s: &str) -> bool {
        match self.kind_at(n) {
            L::Op(o) => o == s,
            L::Name => self.word_at(n) == s,
            L::Directive(d) => d == s,
            _ => false,
        }
    }

    fn at(&self, s: &str) -> bool {
        self.is(self.i, s)
    }

    /// Whether token `n` can be an identifier.
    fn is_ident(&self, n: usize) -> bool {
        self.kind_at(n) == L::Name && {
            let w = self.word_at(n);
            w.starts_with('@') || !KEYWORDS.contains(&w)
        }
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

    fn hidden(start: usize, end: usize) -> Item {
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
            L::Op(o) => o,
            L::Directive(d) => d,
            _ => names::anonymous(&self.src[t.start..t.end]).unwrap_or_else(|| {
                self.errors = true;
                "ERROR"
            }),
        };
        Self::tok(t, kind, false)
    }

    /// Tokens `self.i .. self.i + n`, which touch, as one anonymous token
    /// of `kind` (`>>`, `>=`).
    fn joined(&mut self, n: usize, kind: &'static str) -> Item {
        let start = self.tk(self.i).start;
        let mut end = start;
        for _ in 0..n {
            end = self.advance().end;
        }
        Item::Tok(Tok {
            kind,
            named: false,
            start,
            end,
            field: None,
            hidden: false,
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

    fn expect(&mut self, s: &'static str) -> Item {
        if self.at(s) {
            self.punct()
        } else {
            self.missing(s, false)
        }
    }

    /// An identifier: a leaf, or (a contextual keyword) the keyword's
    /// token in an `identifier` node.
    fn identifier(&mut self) -> Item {
        if !self.is_ident(self.i) {
            return self.missing("identifier", true);
        }
        let w = self.word();
        if RESERVED_IDENTIFIERS.contains(&w) {
            let t = self.punct();
            node("identifier", vec![t])
        } else {
            self.leaf("identifier")
        }
    }

    fn error_token(&mut self) -> Item {
        self.errors = true;
        let t = self.advance();
        Self::tok(t, "ERROR", false)
    }

    /// The rest of the statement (to its `;`, or before a `}` or `{` that
    /// closes or opens a block) as an `ERROR` node; at least one token.
    fn error_statement(&mut self) -> Item {
        self.errors = true;
        let mut items = Vec::new();
        let mut depth = 0usize;
        while !self.eof() {
            if depth == 0
                && (self.at("}") || (self.at("{") && !items.is_empty()) || self.at_directive_end())
            {
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

    /// Commas and items: `item (, item)*`, until `close` (not taken).
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

    // ---- the file --------------------------------------------------------

    pub fn program(&mut self) -> Item {
        let mut items = Vec::new();
        if self.peek() == L::Shebang {
            items.push(self.leaf("shebang_directive"));
        }
        while !self.eof() {
            let before = self.i;
            let item = self.top_level_item();
            items.push(item);
            if self.i == before {
                items.push(self.error_token());
            }
        }
        node("compilation_unit", items)
    }

    fn top_level_item(&mut self) -> Item {
        if self.at("#if") {
            return self.preproc_if(&mut Self::top_level_item);
        }
        if self.at_directive_end() {
            return self.error_token();
        }
        if self.at("extern") && self.is(self.i + 1, "alias") {
            let items = vec![
                self.punct(),
                self.punct(),
                self.identifier().with("name"),
                self.expect(";"),
            ];
            return node("extern_alias_directive", items);
        }
        if self.using_directive_at(self.i) {
            return self.using_directive();
        }
        if self.at("[")
            && (self.is(self.i + 1, "assembly") || self.is(self.i + 1, "module"))
            && self.is(self.i + 2, ":")
        {
            return self.global_attribute();
        }
        if self.at("namespace") {
            return self.namespace_declaration();
        }
        if self.type_declaration_at() {
            return self.declaration();
        }
        node("global_statement", vec![self.statement()])
    }

    fn using_directive_at(&self, n: usize) -> bool {
        let n = if self.is(n, "global") && self.is(n + 1, "using") {
            n + 1
        } else {
            n
        };
        self.is(n, "using") && !self.is(n + 1, "(") && !self.is(n + 1, "var") && {
            // `using X;`, `using X = …;`, `using static X;`; not
            // `using T x = …;` (a declaration).
            let mut k = n + 1;
            while self.is(k, "static") || self.is(k, "unsafe") {
                k += 1;
            }
            match self.scan_name(k) {
                Some(e) => self.is(e, ";") || (self.is(e, "=") && self.is_ident(k)),
                None => false,
            }
        }
    }

    fn using_directive(&mut self) -> Item {
        let mut items = Vec::new();
        if self.at("global") {
            items.push(self.punct());
        }
        items.push(self.punct());
        if self.at("unsafe") && self.is_ident(self.i + 1) && self.is(self.i + 2, "=") {
            items.push(self.punct());
        }
        if self.is_ident(self.i) && self.is(self.i + 1, "=") {
            items.push(self.identifier().with("name"));
            items.push(self.punct());
            items.push(self.ty());
        } else {
            while self.at("static") || self.at("unsafe") {
                items.push(self.punct());
            }
            items.push(self.name());
        }
        items.push(self.expect(";"));
        node("using_directive", items)
    }

    fn global_attribute(&mut self) -> Item {
        let mut items = vec![self.punct(), self.punct(), self.punct()];
        self.comma_list(&mut items, "]", true, Self::attribute);
        items.push(self.expect("]"));
        node("global_attribute", items)
    }

    fn namespace_declaration(&mut self) -> Item {
        let mut items = vec![self.punct(), self.name().with("name")];
        if self.at(";") {
            items.push(self.punct());
            return node("file_scoped_namespace_declaration", items);
        }
        items.push(self.declaration_list().with("body"));
        self.optional_semi(&mut items);
        node("namespace_declaration", items)
    }

    /// `;` right after a body's `}` belongs to the declaration, as no node.
    fn optional_semi(&mut self, items: &mut Vec<Item>) {
        if self.at(";") && self.i > 0 && self.toks[self.i - 1].end == self.tk(self.i).start {
            let t = self.advance();
            items.push(Self::hidden(t.start, t.end));
        }
    }

    // ---- preprocessor conditionals ---------------------------------------

    /// At `#elif`, `#else` or `#endif`: the end of a conditional's branch.
    fn at_directive_end(&self) -> bool {
        matches!(self.peek(), L::Directive("#elif" | "#else" | "#endif"))
    }

    /// `#if` … `#endif`, its branches read by `content` until a directive.
    fn preproc_if(&mut self, content: &mut dyn FnMut(&mut Self) -> Item) -> Item {
        let mut items = vec![self.punct()];
        items.push(self.preproc_condition().with("condition"));
        self.line_end(&mut items);
        self.branch(&mut items, content);
        if let Some(alt) = self.preproc_alternative(content) {
            items.push(alt.with("alternative"));
        }
        items.push(self.expect("#endif"));
        node("preproc_if", items)
    }

    /// `#if` … `#endif` whose branches each hold at most one `content`
    /// (which pushes its items), aliased `preproc_if`.
    pub(crate) fn preproc_if_once(
        &mut self,
        content: &mut dyn FnMut(&mut Self, &mut Vec<Item>),
    ) -> Item {
        let mut items = vec![self.punct()];
        items.push(self.preproc_condition().with("condition"));
        self.line_end(&mut items);
        if !self.at_directive_end() {
            content(self, &mut items);
        }
        if let Some(alt) = self.preproc_alternative_once(content) {
            items.push(alt.with("alternative"));
        }
        items.push(self.expect("#endif"));
        node("preproc_if", items)
    }

    fn preproc_alternative_once(
        &mut self,
        content: &mut dyn FnMut(&mut Self, &mut Vec<Item>),
    ) -> Option<Item> {
        if self.at("#else") {
            let mut items = vec![self.punct()];
            if !self.at_directive_end() {
                content(self, &mut items);
            }
            return Some(node("preproc_else", items));
        }
        if self.at("#elif") {
            let mut items = vec![self.punct()];
            items.push(self.preproc_condition().with("condition"));
            self.line_end(&mut items);
            if !self.at_directive_end() {
                content(self, &mut items);
            }
            if let Some(alt) = self.preproc_alternative_once(content) {
                items.push(alt.with("alternative"));
            }
            return Some(node("preproc_elif", items));
        }
        None
    }

    /// `#if` at `n` whose first content (after the condition's line) is
    /// `else`.
    fn else_conditional_at(&self, n: usize) -> bool {
        if !self.is(n, "#if") {
            return false;
        }
        let mut k = n + 1;
        while !matches!(self.kind_at(k), L::LineEnd | L::Eof) {
            k += 1;
        }
        self.is(k + 1, "else")
    }

    fn line_end(&mut self, items: &mut Vec<Item>) {
        if self.peek() == L::LineEnd {
            let t = self.advance();
            items.push(Self::hidden(t.start, t.end));
        } else {
            self.errors = true;
        }
    }

    fn branch(&mut self, items: &mut Vec<Item>, content: &mut dyn FnMut(&mut Self) -> Item) {
        while !self.eof() && !self.at_directive_end() {
            let before = self.i;
            items.push(content(self));
            if self.i == before {
                if self.at_directive_end() {
                    break;
                }
                items.push(self.error_token());
            }
        }
    }

    fn preproc_alternative(&mut self, content: &mut dyn FnMut(&mut Self) -> Item) -> Option<Item> {
        if self.at("#else") {
            let mut items = vec![self.punct()];
            self.branch(&mut items, content);
            return Some(node("preproc_else", items));
        }
        if self.at("#elif") {
            let mut items = vec![self.punct()];
            items.push(self.preproc_condition().with("condition"));
            self.line_end(&mut items);
            self.branch(&mut items, content);
            if let Some(alt) = self.preproc_alternative(content) {
                items.push(alt.with("alternative"));
            }
            return Some(node("preproc_elif", items));
        }
        None
    }

    /// A directive's condition: `||` below `&&` below `==` / `!=`.
    fn preproc_condition(&mut self) -> Item {
        self.preproc_binary(0)
    }

    fn preproc_binary(&mut self, min: u8) -> Item {
        let mut left = self.preproc_unary();
        loop {
            let prec = match self.peek() {
                L::Op("||") => 1,
                L::Op("&&") => 2,
                L::Op("==" | "!=") => 3,
                _ => break,
            };
            if prec < min {
                break;
            }
            let op = self.punct().with("operator");
            let right = self.preproc_binary(prec + 1).with("right");
            left = node("binary_expression", vec![left.with("left"), op, right]);
        }
        left
    }

    fn preproc_unary(&mut self) -> Item {
        match self.peek() {
            L::Op("!") => {
                let op = self.punct().with("operator");
                let arg = self.preproc_unary().with("argument");
                node("unary_expression", vec![op, arg])
            }
            L::Op("(") => {
                let open = self.punct();
                let inner = self.preproc_binary(0);
                let close = self.expect(")");
                node("parenthesized_expression", vec![open, inner, close])
            }
            L::Int => self.leaf("integer_literal"),
            L::Name if self.at("true") || self.at("false") => {
                let t = self.punct();
                node("boolean_literal", vec![t])
            }
            L::Name => self.identifier_any(),
            _ => self.missing("identifier", true),
        }
    }

    /// Any word as an identifier (a directive's symbol may be a keyword).
    fn identifier_any(&mut self) -> Item {
        let w = self.word();
        if RESERVED_IDENTIFIERS.contains(&w) {
            let t = self.punct();
            node("identifier", vec![t])
        } else {
            self.leaf("identifier")
        }
    }

    // ---- attributes and modifiers ----------------------------------------

    fn attribute_lists(&mut self, items: &mut Vec<Item>) {
        loop {
            if self.at("[") {
                items.push(self.attribute_list());
            } else if self.attribute_conditional_at(self.i) {
                items.push(self.preproc_if_attributes());
            } else {
                return;
            }
        }
    }

    /// `#if …` whose branches hold one attribute list each (or none),
    /// before more of a declaration: `preproc_if_in_attribute_list`.
    fn attribute_conditional_at(&self, n: usize) -> bool {
        self.attribute_conditional_end(n)
            .is_some_and(|k| !self.is(k, "}") && self.kind_at(k) != L::Eof)
    }

    /// Past such a conditional's `#endif`.
    fn attribute_conditional_end(&self, n: usize) -> Option<usize> {
        if !self.is(n, "#if") {
            return None;
        }
        let mut k = n + 1;
        loop {
            // The condition's line.
            while !matches!(self.kind_at(k), L::LineEnd | L::Eof) {
                k += 1;
            }
            k += 1;
            if self.is(k, "[") {
                k = self.scan_balanced(k)?;
            }
            match self.kind_at(k) {
                L::Directive("#endif") => return Some(k + 1),
                L::Directive("#elif") => k += 1,
                L::Directive("#else") => {
                    k += 1;
                    if self.is(k, "[") {
                        k = self.scan_balanced(k)?;
                    }
                    return self.is(k, "#endif").then_some(k + 1);
                }
                _ => return None,
            }
        }
    }

    fn preproc_if_attributes(&mut self) -> Item {
        let mut items = vec![self.punct()];
        items.push(self.preproc_condition().with("condition"));
        self.line_end(&mut items);
        if self.at("[") {
            items.push(self.attribute_list());
        }
        if let Some(alt) = self.attribute_alternative() {
            items.push(alt.with("alternative"));
        }
        items.push(self.expect("#endif"));
        node("preproc_if_in_attribute_list", items)
    }

    fn attribute_alternative(&mut self) -> Option<Item> {
        if self.at("#else") {
            let mut items = vec![self.punct()];
            if self.at("[") {
                items.push(self.attribute_list());
            }
            return Some(node("preproc_else", items));
        }
        if self.at("#elif") {
            let mut items = vec![self.punct()];
            items.push(self.preproc_condition().with("condition"));
            self.line_end(&mut items);
            if self.at("[") {
                items.push(self.attribute_list());
            }
            if let Some(alt) = self.attribute_alternative() {
                items.push(alt.with("alternative"));
            }
            return Some(node("preproc_elif", items));
        }
        None
    }

    fn attribute_list(&mut self) -> Item {
        let mut items = vec![self.punct()];
        if self.kind_at(self.i) == L::Name
            && matches!(
                self.word(),
                "field" | "event" | "method" | "param" | "property" | "return" | "type" | "typevar"
            )
            && self.is(self.i + 1, ":")
        {
            let target = vec![self.punct(), self.punct()];
            items.push(node("attribute_target_specifier", target));
        }
        self.comma_list(&mut items, "]", true, Self::attribute);
        items.push(self.expect("]"));
        node("attribute_list", items)
    }

    fn attribute(&mut self) -> Item {
        let mut items = vec![self.name().with("name")];
        if self.at("(") {
            let mut args = vec![self.punct()];
            self.comma_list(&mut args, ")", false, |p| {
                let mut a = Vec::new();
                if p.is_ident(p.i)
                    && (p.is(p.i + 1, ":") || (p.is(p.i + 1, "=") && !p.is(p.i + 2, "=")))
                {
                    a.push(p.identifier().with("name"));
                    a.push(p.punct());
                }
                a.push(p.expression());
                node("attribute_argument", a)
            });
            args.push(self.expect(")"));
            items.push(node("attribute_argument_list", args));
        }
        node("attribute", items)
    }

    fn is_modifier_at(&self, n: usize) -> bool {
        let w = self.word_at(n);
        if !MODIFIERS.contains(&w) {
            return false;
        }
        match w {
            // Contextual: a modifier only before more of a declaration.
            "async" | "partial" | "file" | "required" => {
                self.is_ident(n + 1)
                    || self.kind_at(n + 1) == L::Name
                    || self.is(n + 1, "(")
                    || self.is(n + 1, "[")
            }
            _ => true,
        }
    }

    fn modifiers(&mut self, items: &mut Vec<Item>) {
        while self.kind_at(self.i) == L::Name && self.is_modifier_at(self.i) {
            // `partial` before a return type is the method's.
            let t = self.punct();
            items.push(node("modifier", vec![t]));
        }
    }

    fn scan_modifiers(&self, mut n: usize) -> usize {
        while self.kind_at(n) == L::Name && self.is_modifier_at(n) {
            n += 1;
        }
        n
    }

    fn scan_attributes(&self, mut n: usize) -> usize {
        loop {
            let next = if self.is(n, "[") {
                self.scan_balanced(n)
            } else {
                self.attribute_conditional_end(n)
            };
            match next {
                Some(k) => n = k,
                None => return n,
            }
        }
    }

    // ---- declarations ----------------------------------------------------

    /// Whether a type declaration starts here (after attributes and
    /// modifiers): `class`, `struct`, `interface`, `enum`, `record`,
    /// `delegate`.
    fn type_declaration_at(&self) -> bool {
        let n = self.scan_modifiers(self.scan_attributes(self.i));
        let n = if self.is(n, "ref") { n + 1 } else { n };
        let n = if self.is(n, "partial") { n + 1 } else { n };
        match self.word_at(n) {
            "class" | "struct" | "interface" | "enum" => true,
            "delegate" => !self.is(n + 1, "*") && !self.is(n + 1, "(") && !self.is(n + 1, "{"),
            "record" => self.is(n + 1, "class") || self.is(n + 1, "struct") || self.is_ident(n + 1),
            _ => false,
        }
    }

    fn declaration_list(&mut self) -> Item {
        let mut items = vec![self.expect("{")];
        while !self.eof() && !self.at("}") {
            let before = self.i;
            if self.at_directive_end() {
                break;
            }
            items.push(self.declaration());
            if self.i == before {
                items.push(self.error_token());
            }
        }
        items.push(self.expect("}"));
        node("declaration_list", items)
    }

    /// One member of a namespace, type or file.
    fn declaration(&mut self) -> Item {
        if self.at("#if") && !self.attribute_conditional_at(self.i) {
            return self.preproc_if(&mut Self::declaration);
        }
        if self.at_directive_end() {
            return self.error_token();
        }
        if self.at("namespace") {
            return self.namespace_declaration();
        }
        if self.using_directive_at(self.i) {
            return self.using_directive();
        }
        let start = self.i;
        let mut items = Vec::new();
        self.attribute_lists(&mut items);
        self.modifiers(&mut items);
        let w = self.word();
        match w {
            "class" | "struct" | "interface" | "enum" => return self.type_declaration(items),
            "ref" | "partial"
                if self.is(self.i + 1, "struct")
                    || (self.is(self.i + 1, "partial") && self.is(self.i + 2, "struct")) =>
            {
                return self.type_declaration(items);
            }
            "record"
                if self.is(self.i + 1, "class")
                    || self.is(self.i + 1, "struct")
                    || (self.is_ident(self.i + 1)
                        && matches!(
                            self.kind_at(self.i + 2),
                            L::Op("(" | "<" | "{" | ":" | ";")
                        )) =>
            {
                return self.record_declaration(items);
            }
            "delegate" if !self.is(self.i + 1, "*") => return self.delegate_declaration(items),
            "event" => return self.event_declaration(items),
            "implicit" | "explicit" => return self.conversion_operator(items),
            "extension" if self.is(self.i + 1, "(") || self.is(self.i + 1, "<") => {
                return self.extension_declaration(items);
            }
            _ => {}
        }
        if self.at("~") || (self.at("extern") && self.is(self.i + 1, "~")) {
            if self.at("extern") {
                items.push(self.punct());
            }
            items.push(self.punct());
            items.push(self.identifier().with("name"));
            items.push(self.parameter_list().with("parameters"));
            self.function_body(&mut items);
            return node("destructor_declaration", items);
        }
        if self.is_ident(self.i) && self.is(self.i + 1, "(") {
            items.push(self.identifier().with("name"));
            items.push(self.parameter_list().with("parameters"));
            if self.at(":") && (self.is(self.i + 1, "base") || self.is(self.i + 1, "this")) {
                let init = vec![self.punct(), self.punct(), self.argument_list()];
                items.push(node("constructor_initializer", init));
            }
            self.function_body(&mut items);
            return node("constructor_declaration", items);
        }
        if self.scan_type(self.i).is_none() {
            if self.i == start {
                return self.error_member();
            }
            items.push(self.error_member());
            return node("ERROR", items);
        }
        let ty = self.ty();
        if self.at("operator") {
            return self.operator_declaration(items, ty);
        }
        let interface = self.explicit_interface();
        if self.at("this") && self.is(self.i + 1, "[") {
            items.push(ty.with("type"));
            items.extend(interface);
            items.push(self.punct());
            items.push(self.bracketed_parameter_list().with("parameters"));
            if self.at("=>") {
                items.push(self.arrow_clause().with("value"));
                items.push(self.expect(";"));
            } else {
                items.push(self.accessor_list().with("accessors"));
            }
            return node("indexer_declaration", items);
        }
        if interface.is_some() && self.at("operator") {
            return self.operator_declaration_after(items, ty, interface);
        }
        if self.is_ident(self.i) && (self.is(self.i + 1, "(") || self.is(self.i + 1, "<")) {
            items.push(ty.with("returns"));
            items.extend(interface);
            items.push(self.identifier().with("name"));
            if self.at("<") {
                items.push(self.type_parameter_list().with("type_parameters"));
            }
            items.push(self.parameter_list().with("parameters"));
            self.constraints(&mut items);
            self.function_body(&mut items);
            return node("method_declaration", items);
        }
        if self.is_ident(self.i) && (self.is(self.i + 1, "{") || self.is(self.i + 1, "=>")) {
            items.push(ty.with("type"));
            items.extend(interface);
            items.push(self.identifier().with("name"));
            if self.at("=>") {
                items.push(self.arrow_clause().with("value"));
                items.push(self.expect(";"));
            } else {
                items.push(self.accessor_list().with("accessors"));
                if self.at("=") {
                    items.push(self.punct());
                    items.push(self.expression().with("value"));
                    items.push(self.expect(";"));
                }
            }
            return node("property_declaration", items);
        }
        if interface.is_some() {
            items.push(ty.with("type"));
            items.extend(interface);
            items.push(self.error_member());
            return node("ERROR", items);
        }
        let decl = self.variable_declaration_after(ty);
        items.push(decl);
        items.push(self.expect(";"));
        node("field_declaration", items)
    }

    /// A member that cannot be read: to its `;` or before its body.
    fn error_member(&mut self) -> Item {
        self.errors = true;
        let mut items = Vec::new();
        let mut depth = 0usize;
        while !self.eof() {
            if depth == 0 && (self.at("}") || self.at_directive_end()) {
                break;
            }
            if self.at("(") || self.at("[") || self.at("{") {
                depth += 1;
            } else if (self.at(")") || self.at("]") || self.at("}")) && depth > 0 {
                depth -= 1;
                if depth == 0 && self.at("}") {
                    let t = self.advance();
                    items.push(Self::tok(t, "ERROR", false));
                    break;
                }
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

    /// `I.` / `A.B<T>.` before a member's name, `this` or `operator`.
    fn explicit_interface(&mut self) -> Option<Item> {
        // Segments of a name, each followed by `.`; the interface's name
        // ends before the dot whose next segment is the member (a name
        // then `(`, `<`, `{`, `=>`, …), `this` or `operator`.
        let start = self.i;
        let mut k = start;
        if !self.is_ident(k) {
            return None;
        }
        k += 1;
        if self.is(k, "::") && self.is_ident(k + 1) {
            k += 2;
        }
        let end = loop {
            if self.is(k, "<") {
                k = self.scan_type_arguments(k)?;
            }
            if !self.is(k, ".") {
                return None;
            }
            let next = k + 1;
            if self.is(next, "this") || self.is(next, "operator") {
                break k;
            }
            if !self.is_ident(next) {
                return None;
            }
            let after = if self.is(next + 1, "<") {
                self.scan_type_arguments(next + 1)?
            } else {
                next + 1
            };
            if matches!(self.kind_at(after), L::Op("(" | "{" | "=>" | ";" | "=")) {
                break k;
            }
            k = next + 1;
        };
        let name = self.name_until(end);
        let dot = self.punct();
        Some(node("explicit_interface_specifier", vec![name, dot]))
    }

    fn type_declaration(&mut self, mut items: Vec<Item>) -> Item {
        let kind = loop {
            match self.word() {
                "ref" | "partial" => items.push(self.punct()),
                "class" => break "class_declaration",
                "struct" => break "struct_declaration",
                "interface" => break "interface_declaration",
                _ => break "enum_declaration",
            }
        };
        items.push(self.punct());
        items.push(self.identifier().with("name"));
        if kind == "enum_declaration" {
            if self.at(":") {
                items.push(self.base_list());
            }
            if self.at(";") {
                items.push(self.punct());
            } else {
                items.push(self.enum_member_list().with("body"));
                self.optional_semi(&mut items);
            }
            return node(kind, items);
        }
        if kind == "interface_declaration" {
            if self.at("<") {
                items.push(self.type_parameter_list().with("type_parameters"));
            }
            if self.at(":") {
                items.push(self.base_list());
            }
        } else {
            loop {
                if self.at("<") {
                    items.push(self.type_parameter_list());
                } else if self.at("(") {
                    items.push(self.parameter_list());
                } else if self.at(":") {
                    items.push(self.base_list());
                } else {
                    break;
                }
            }
        }
        self.constraints(&mut items);
        self.declaration_body(&mut items);
        node(kind, items)
    }

    fn declaration_body(&mut self, items: &mut Vec<Item>) {
        if self.at(";") {
            items.push(self.punct());
        } else {
            items.push(self.declaration_list().with("body"));
            self.optional_semi(items);
        }
    }

    fn record_declaration(&mut self, mut items: Vec<Item>) -> Item {
        items.push(self.punct());
        if self.at("class") || self.at("struct") {
            items.push(self.punct());
        }
        items.push(self.identifier().with("name"));
        loop {
            if self.at("<") {
                items.push(self.type_parameter_list());
            } else if self.at("(") {
                items.push(self.parameter_list());
            } else {
                break;
            }
        }
        if self.at(":") {
            let mut base = vec![self.punct()];
            let n = self.scan_name(self.i);
            if n.is_some_and(|n| self.is(n, "(")) {
                let ty = self.name().with("type");
                let args = self.argument_list();
                base.push(node("primary_constructor_base_type", vec![ty, args]));
                if self.at(",") {
                    base.push(self.punct());
                    self.comma_list(&mut base, "{", false, Self::name);
                }
            } else {
                self.comma_list(&mut base, "{", false, Self::name);
            }
            items.push(node("base_list", base));
        }
        self.constraints(&mut items);
        self.declaration_body(&mut items);
        node("record_declaration", items)
    }

    fn delegate_declaration(&mut self, mut items: Vec<Item>) -> Item {
        items.push(self.punct());
        items.push(self.ty().with("type"));
        items.push(self.identifier().with("name"));
        if self.at("<") {
            items.push(self.type_parameter_list().with("type_parameters"));
        }
        items.push(self.parameter_list().with("parameters"));
        self.constraints(&mut items);
        items.push(self.expect(";"));
        node("delegate_declaration", items)
    }

    fn event_declaration(&mut self, mut items: Vec<Item>) -> Item {
        items.push(self.punct());
        let ty = self.ty();
        let interface = self.explicit_interface();
        if interface.is_some() || (self.is_ident(self.i) && self.is(self.i + 1, "{")) {
            items.push(ty.with("type"));
            items.extend(interface);
            items.push(self.identifier().with("name"));
            if self.at("{") {
                items.push(self.accessor_list().with("accessors"));
            } else {
                items.push(self.expect(";"));
            }
            return node("event_declaration", items);
        }
        items.push(self.variable_declaration_after(ty));
        items.push(self.expect(";"));
        node("event_field_declaration", items)
    }

    fn conversion_operator(&mut self, mut items: Vec<Item>) -> Item {
        items.push(self.punct());
        loop {
            if self.at("operator") || self.at("checked") {
                items.push(self.punct());
            } else if let Some(spec) = self.explicit_interface() {
                items.push(spec);
            } else {
                break;
            }
        }
        items.push(self.ty().with("type"));
        items.push(self.parameter_list().with("parameters"));
        self.function_body(&mut items);
        node("conversion_operator_declaration", items)
    }

    fn operator_declaration(&mut self, items: Vec<Item>, ty: Item) -> Item {
        self.operator_declaration_after(items, ty, None)
    }

    fn operator_declaration_after(
        &mut self,
        mut items: Vec<Item>,
        ty: Item,
        interface: Option<Item>,
    ) -> Item {
        items.push(ty.with("type"));
        items.extend(interface);
        items.push(self.punct());
        if self.at("checked") {
            items.push(self.punct());
        }
        let op = if self.at(">") {
            let (n, kind) = self.joined_greater();
            self.joined(n, kind)
        } else {
            self.punct()
        };
        items.push(op.with("operator"));
        items.push(self.parameter_list().with("parameters"));
        self.function_body(&mut items);
        node("operator_declaration", items)
    }

    fn extension_declaration(&mut self, mut items: Vec<Item>) -> Item {
        items.push(self.punct());
        if self.at("<") {
            items.push(self.type_parameter_list());
        }
        items.push(self.expect("("));
        let mut recv = Vec::new();
        self.attribute_lists(&mut recv);
        self.parameter_modifiers(&mut recv);
        recv.push(self.ty().with("type"));
        if self.is_ident(self.i) {
            recv.push(self.identifier().with("name"));
        }
        items.push(node("receiver_parameter", recv));
        items.push(self.expect(")"));
        self.constraints(&mut items);
        let mut body = vec![self.expect("{")];
        while !self.eof() && !self.at("}") {
            let before = self.i;
            body.push(self.declaration());
            if self.i == before {
                body.push(self.error_token());
            }
        }
        body.push(self.expect("}"));
        if self.at(";") {
            body.push(self.punct());
        }
        items.push(node("extension_body", body));
        node("extension_declaration", items)
    }

    fn enum_member_list(&mut self) -> Item {
        let mut items = vec![self.expect("{")];
        loop {
            if self.at("}") || self.eof() {
                break;
            }
            let before = self.i;
            if self.at("#if") {
                items.push(self.preproc_if(&mut Self::enum_member));
            } else if self.at_directive_end() {
                break;
            } else {
                items.push(self.enum_member());
            }
            if self.at(",") {
                items.push(self.punct());
                continue;
            }
            if self.at("#if") {
                continue;
            }
            if self.i == before {
                items.push(self.error_token());
                continue;
            }
            if !self.at("}") && !self.eof() && !self.at_directive_end() {
                items.push(self.error_token());
                continue;
            }
            break;
        }
        items.push(self.expect("}"));
        node("enum_member_declaration_list", items)
    }

    fn enum_member(&mut self) -> Item {
        let mut items = Vec::new();
        self.attribute_lists(&mut items);
        items.push(self.identifier().with("name"));
        if self.at("=") {
            items.push(self.punct());
            items.push(self.expression().with("value"));
        }
        // A trailing `,` within a conditional belongs to its branch.
        if self.at(",") && matches!(self.kind_at(self.i + 1), L::Directive(_)) {
            let m = node("enum_member_declaration", items);
            return m;
        }
        node("enum_member_declaration", items)
    }

    fn base_list(&mut self) -> Item {
        let mut items = vec![self.punct()];
        self.comma_list(&mut items, "{", false, Self::ty);
        // `class C(int x) : B(x)`: arguments after the base type.
        let mut out = Vec::with_capacity(items.len());
        for it in items {
            out.push(it);
        }
        if self.at("(") {
            out.push(self.argument_list());
            while self.at(",") {
                out.push(self.punct());
                out.push(self.ty());
            }
        }
        node("base_list", out)
    }

    fn type_parameter_list(&mut self) -> Item {
        let mut items = vec![self.punct()];
        self.comma_list(&mut items, ">", false, |p| {
            let mut t = Vec::new();
            p.attribute_lists(&mut t);
            if p.at("in") || p.at("out") {
                t.push(p.punct());
            }
            t.push(p.identifier().with("name"));
            node("type_parameter", t)
        });
        items.push(self.expect(">"));
        node("type_parameter_list", items)
    }

    fn constraints(&mut self, items: &mut Vec<Item>) {
        while self.at("where") && self.is_ident(self.i + 1) && self.is(self.i + 2, ":") {
            let mut c = vec![self.punct(), self.identifier(), self.punct()];
            self.comma_list(&mut c, "{", false, |p| {
                let mut k = Vec::new();
                if p.at("class") {
                    k.push(p.punct());
                    if p.at("?") {
                        k.push(p.punct());
                    }
                } else if p.at("struct")
                    || ((p.at("notnull") || p.at("unmanaged"))
                        && !p.is(p.i + 1, ".")
                        && !p.is(p.i + 1, "<"))
                {
                    k.push(p.punct());
                } else if p.at("new") && p.is(p.i + 1, "(") {
                    let n = vec![p.punct(), p.punct(), p.expect(")")];
                    k.push(node("constructor_constraint", n));
                } else if p.at("default") {
                    // Not in tree-sitter's grammar: the word as a type name.
                    k.push(p.leaf("identifier").with("type"));
                } else {
                    k.push(p.ty().with("type"));
                }
                node("type_parameter_constraint", k)
            });
            items.push(node("type_parameter_constraints_clause", c));
        }
    }

    fn accessor_list(&mut self) -> Item {
        let mut items = vec![self.expect("{")];
        while !self.eof() && !self.at("}") {
            let before = self.i;
            if self.at("#if") && !self.attribute_conditional_at(self.i) {
                items.push(self.preproc_if(&mut Self::accessor));
            } else if self.at_directive_end() {
                break;
            } else {
                items.push(self.accessor());
            }
            if self.i == before {
                items.push(self.error_token());
            }
        }
        items.push(self.expect("}"));
        node("accessor_list", items)
    }

    fn accessor(&mut self) -> Item {
        let mut items = Vec::new();
        self.attribute_lists(&mut items);
        self.modifiers(&mut items);
        match self.word() {
            "get" | "set" | "add" | "remove" | "init" => items.push(self.punct().with("name")),
            _ if self.is_ident(self.i) => items.push(self.identifier().with("name")),
            _ => return self.error_member(),
        }
        self.function_body(&mut items);
        node("accessor_declaration", items)
    }

    /// `{ … }`, `=> e;` or `;`.
    fn function_body(&mut self, items: &mut Vec<Item>) {
        if self.at("{") {
            items.push(self.block().with("body"));
        } else if self.at("=>") {
            items.push(self.arrow_clause().with("body"));
            items.push(self.expect(";"));
        } else if self.at("#if") {
            items.push(self.preproc_if(&mut |p| {
                let mut b = Vec::new();
                p.constraints(&mut b);
                p.function_body(&mut b);
                match b.len() {
                    1 => b.pop().unwrap_or_else(|| p.missing(";", false)),
                    _ => node("ERROR", b),
                }
            }));
        } else {
            items.push(self.expect(";"));
        }
    }

    fn arrow_clause(&mut self) -> Item {
        let arrow = self.punct();
        let e = self.expression();
        node("arrow_expression_clause", vec![arrow, e])
    }

    fn parameter_list(&mut self) -> Item {
        let mut items = vec![self.expect("(")];
        self.parameters(&mut items, ")");
        items.push(self.expect(")"));
        node("parameter_list", items)
    }

    fn bracketed_parameter_list(&mut self) -> Item {
        let mut items = vec![self.expect("[")];
        self.parameters(&mut items, "]");
        items.push(self.expect("]"));
        node("bracketed_parameter_list", items)
    }

    fn parameters(&mut self, items: &mut Vec<Item>, close: &str) {
        if self.at(close) {
            return;
        }
        loop {
            let before = self.i;
            self.parameter(items);
            if self.at(",") {
                items.push(self.punct());
                continue;
            }
            if self.i == before && !self.at(close) && !self.eof() {
                items.push(self.error_token());
                continue;
            }
            return;
        }
    }

    /// One parameter; `params T x` is its tokens, in the list itself.
    fn parameter(&mut self, items: &mut Vec<Item>) {
        let mut p = Vec::new();
        self.attribute_lists(&mut p);
        if self.at("params") {
            items.extend(p);
            items.push(self.punct());
            items.push(self.ty().with("type"));
            items.push(self.identifier().with("name"));
            return;
        }
        self.parameter_modifiers(&mut p);
        // A type, unless only a name is left (a lambda's parameter).
        if !(self.is_ident(self.i)
            && matches!(self.kind_at(self.i + 1), L::Op("," | ")" | "=" | "]")))
        {
            p.push(self.ty().with("type"));
        }
        p.push(self.identifier().with("name"));
        if self.at("=") {
            p.push(self.punct());
            p.push(self.expression());
        }
        items.push(node("parameter", p));
    }

    fn parameter_modifiers(&mut self, items: &mut Vec<Item>) {
        loop {
            match self.word() {
                "this" | "ref" | "out" | "in" | "readonly" => {
                    items.push(self.leaf("modifier"));
                }
                // `scoped` before a type and a name.
                "scoped"
                    if self.scan_type(self.i + 1).is_some_and(|k| self.is_ident(k))
                        || self.is(self.i + 1, "ref") =>
                {
                    items.push(self.leaf("modifier"));
                }
                _ => return,
            }
        }
    }

    fn variable_declaration_after(&mut self, ty: Item) -> Item {
        let mut items = vec![ty.with("type")];
        self.comma_list(&mut items, ";", false, Self::variable_declarator);
        node("variable_declaration", items)
    }

    fn variable_declaration(&mut self) -> Item {
        let ty = self.ty();
        self.variable_declaration_after(ty)
    }

    fn variable_declarator(&mut self) -> Item {
        let mut items = Vec::new();
        if self.at("(") {
            items.push(self.tuple_pattern());
        } else {
            items.push(self.identifier().with("name"));
        }
        if self.at("[") {
            items.push(self.bracketed_argument_list());
        }
        if self.at("=") {
            items.push(self.punct());
            items.push(self.expression());
        }
        node("variable_declarator", items)
    }

    fn tuple_pattern(&mut self) -> Item {
        let mut items = vec![self.punct()];
        self.comma_list(&mut items, ")", false, |p| {
            if p.at("(") {
                p.tuple_pattern()
            } else if p.at("_") {
                p.leaf("discard")
            } else {
                p.identifier().with("name")
            }
        });
        items.push(self.expect(")"));
        node("tuple_pattern", items)
    }

    // ---- statements ------------------------------------------------------

    pub(crate) fn block(&mut self) -> Item {
        let mut items = vec![self.expect("{")];
        self.statements(&mut items);
        items.push(self.expect("}"));
        node("block", items)
    }

    fn statements(&mut self, items: &mut Vec<Item>) {
        while !self.eof() && !self.at("}") && !self.at_directive_end() {
            let before = self.i;
            items.push(self.statement());
            if self.i == before {
                items.push(self.error_token());
            }
        }
    }

    pub(crate) fn statement(&mut self) -> Item {
        let w = self.word();
        match self.peek() {
            L::Op("{") => return self.block(),
            L::Op(";") => return node("empty_statement", vec![self.punct()]),
            L::Directive("#if") => {
                return self.preproc_if(&mut |p| {
                    if p.type_declaration_at() || p.at("namespace") {
                        p.declaration()
                    } else {
                        p.statement()
                    }
                });
            }
            L::Directive(_) => return self.error_token(),
            L::Name => {}
            // `(A, B) x = …`, `[Attr] void F() {}`.
            L::Op("(" | "[") => {
                if let Some(s) = self.declaration_statement() {
                    return s;
                }
                return self.expression_statement();
            }
            _ => return self.expression_statement(),
        }
        match w {
            "if" => return self.if_statement(),
            "while" => {
                let items = vec![
                    self.punct(),
                    self.expect("("),
                    self.expression().with("condition"),
                    self.expect(")"),
                    self.statement().with("body"),
                ];
                return node("while_statement", items);
            }
            "do" => {
                let items = vec![
                    self.punct(),
                    self.statement().with("body"),
                    self.expect("while"),
                    self.expect("("),
                    self.expression().with("condition"),
                    self.expect(")"),
                    self.expect(";"),
                ];
                return node("do_statement", items);
            }
            "for" => return self.for_statement(),
            "foreach" => return self.foreach_statement(Vec::new()),
            "switch" => return self.switch_statement(),
            "try" => return self.try_statement(),
            "return" => {
                let mut items = vec![self.punct()];
                if !self.at(";") {
                    self.tail_taken = false;
                    items.push(self.expression());
                }
                // `return c ? a :` / `#if X` / `b;` …: the conditional's
                // alternative took the statement's `;`.
                if !std::mem::take(&mut self.tail_taken) {
                    items.push(self.expect(";"));
                }
                return node("return_statement", items);
            }
            "throw" => {
                let mut items = vec![self.punct()];
                if !self.at(";") {
                    items.push(self.expression());
                }
                items.push(self.expect(";"));
                return node("throw_statement", items);
            }
            "break" => {
                let items = vec![self.punct(), self.expect(";")];
                return node("break_statement", items);
            }
            "continue" => {
                let items = vec![self.punct(), self.expect(";")];
                return node("continue_statement", items);
            }
            "goto" => {
                let mut items = vec![self.punct()];
                if self.at("case") || self.at("default") {
                    items.push(self.punct());
                }
                if !self.at(";") {
                    items.push(self.expression());
                }
                items.push(self.expect(";"));
                return node("goto_statement", items);
            }
            "yield" if self.is(self.i + 1, "return") || self.is(self.i + 1, "break") => {
                let mut items = vec![self.punct()];
                if self.at("return") {
                    items.push(self.punct());
                    items.push(self.expression());
                } else {
                    items.push(self.punct());
                }
                items.push(self.expect(";"));
                return node("yield_statement", items);
            }
            "lock" => {
                let items = vec![
                    self.punct(),
                    self.expect("("),
                    self.expression(),
                    self.expect(")"),
                    self.statement(),
                ];
                return node("lock_statement", items);
            }
            "fixed" if self.is(self.i + 1, "(") => {
                let items = vec![
                    self.punct(),
                    self.punct(),
                    self.variable_declaration(),
                    self.expect(")"),
                    self.statement(),
                ];
                return node("fixed_statement", items);
            }
            "unsafe" if self.is(self.i + 1, "{") => {
                let items = vec![self.punct(), self.block()];
                return node("unsafe_statement", items);
            }
            "checked" | "unchecked" if self.is(self.i + 1, "{") => {
                let items = vec![self.punct(), self.block()];
                return node("checked_statement", items);
            }
            "using" if self.is(self.i + 1, "(") => return self.using_statement(Vec::new()),
            "await" if self.is(self.i + 1, "using") && self.is(self.i + 2, "(") => {
                let aw = vec![self.punct()];
                return self.using_statement(aw);
            }
            "await" if self.is(self.i + 1, "foreach") => {
                let aw = vec![self.punct()];
                return self.foreach_statement(aw);
            }
            _ => {}
        }
        // `label:`
        if self.is_ident(self.i) && self.is(self.i + 1, ":") {
            let items = vec![self.identifier(), self.punct(), self.statement()];
            return node("labeled_statement", items);
        }
        if let Some(s) = self.declaration_statement() {
            return s;
        }
        self.expression_statement()
    }

    fn expression_statement(&mut self) -> Item {
        if !self.expression_start(self.i) {
            return self.error_statement();
        }
        let e = self.expression();
        let semi = self.expect(";");
        node("expression_statement", vec![e, semi])
    }

    /// A local declaration or a local function, if one starts here.
    fn declaration_statement(&mut self) -> Option<Item> {
        let mut n = self.i;
        let mut awaits = false;
        if self.is(n, "await") && self.is(n + 1, "using") {
            n += 1;
            awaits = true;
        }
        let using = self.is(n, "using");
        if using {
            n += 1;
        }
        let attrs = self.scan_attributes(n);
        let m = self.scan_local_modifiers(attrs);
        // `await f();` awaits; it declares nothing.
        if self.is(m, "await") && !using && self.await_operand_at(m + 1) {
            return None;
        }
        let t = self.scan_type(m)?;
        // `var (a, b) = …`, and `A.B(x) = …`, which tree-sitter reads as
        // a declaration of a tuple pattern too.
        let tuple_declarator = self.is(t, "(")
            && self.scan_name(m) == Some(t)
            && self.scan_balanced(t).is_some_and(|k| {
                self.is(k, "=")
                    && !self.is(k + 1, "=")
                    && (t + 1..k - 1).all(|j| self.is_ident(j) || self.is(j, ","))
            });
        if (self.var_deconstruction_at(m) || tuple_declarator) && attrs == n {
            let mut items = Vec::new();
            if awaits {
                items.push(self.punct());
            }
            if using {
                items.push(self.punct());
            }
            self.local_modifiers(&mut items);
            items.push(self.variable_declaration());
            items.push(self.expect(";"));
            return Some(node("local_declaration_statement", items));
        }
        if !self.is_ident(t) {
            return None;
        }
        let local_function = !using
            && (self.is(t + 1, "(")
                || (self.is(t + 1, "<")
                    && self
                        .scan_type_arguments(t + 1)
                        .is_some_and(|k| self.is(k, "("))));
        if !local_function {
            if attrs != n {
                return None;
            }
            if !matches!(self.kind_at(t + 1), L::Op("=" | ";" | "," | "[")) {
                return None;
            }
            // `a < b > c = …` and `a * b;` are declarations to tree-sitter
            // too; `a = …` is not reached (no name after the type).
        }
        let mut items = Vec::new();
        if local_function {
            self.attribute_lists(&mut items);
            self.local_modifiers(&mut items);
            items.push(self.ty().with("type"));
            items.push(self.identifier().with("name"));
            if self.at("<") {
                items.push(self.type_parameter_list().with("type_parameters"));
            }
            items.push(self.parameter_list().with("parameters"));
            self.constraints(&mut items);
            self.function_body(&mut items);
            return Some(node("local_function_statement", items));
        }
        if awaits {
            items.push(self.punct());
        }
        if using {
            items.push(self.punct());
        }
        self.local_modifiers(&mut items);
        items.push(self.variable_declaration());
        items.push(self.expect(";"));
        Some(node("local_declaration_statement", items))
    }

    /// `var (a, b) = …` at `n`: a declaration of a tuple pattern.
    fn var_deconstruction_at(&self, n: usize) -> bool {
        self.is(n, "var")
            && self.is(n + 1, "(")
            && self
                .scan_balanced(n + 1)
                .is_some_and(|k| self.is(k, "=") || self.is(k, "in"))
    }

    fn scan_local_modifiers(&self, mut n: usize) -> usize {
        // Any modifier (tree-sitter takes `private` on a top-level local
        // function too); `async` only before more words.
        while self.kind_at(n) == L::Name
            && MODIFIERS.contains(&self.word_at(n))
            && !matches!(self.word_at(n), "new" | "partial" | "file" | "required")
            && (self.word_at(n) != "async" || self.kind_at(n + 1) == L::Name)
        {
            n += 1;
        }
        n
    }

    fn local_modifiers(&mut self, items: &mut Vec<Item>) {
        let end = self.scan_local_modifiers(self.i);
        while self.i < end {
            let t = self.punct();
            items.push(node("modifier", vec![t]));
        }
    }

    fn if_statement(&mut self) -> Item {
        let mut items = vec![
            self.punct(),
            self.expect("("),
            self.expression().with("condition"),
            self.expect(")"),
            self.statement().with("consequence"),
        ];
        // `#if X` / `else …` / `#endif`: the conditional holds the else.
        if self.else_conditional_at(self.i) {
            items.push(self.preproc_if_once(&mut |p, b| {
                if p.at("else") {
                    b.push(p.punct());
                    b.push(p.statement());
                }
            }));
        }
        if self.at("else") {
            items.push(self.punct());
            items.push(self.statement().with("alternative"));
        }
        node("if_statement", items)
    }

    fn for_statement(&mut self) -> Item {
        let mut items = vec![self.punct(), self.expect("(")];
        if !self.at(";") {
            let decl = self.var_deconstruction_at(self.i)
                || self
                    .scan_type(self.i)
                    .is_some_and(|t| self.is_ident(t) && !self.is(t, "in"));
            if decl {
                items.push(self.variable_declaration().with("initializer"));
            } else {
                self.expression_list(&mut items, "initializer");
            }
        }
        items.push(self.expect(";"));
        if !self.at(";") {
            items.push(self.expression().with("condition"));
        }
        items.push(self.expect(";"));
        if !self.at(")") {
            self.expression_list(&mut items, "update");
        }
        items.push(self.expect(")"));
        items.push(self.statement().with("body"));
        node("for_statement", items)
    }

    /// `e, e, …`, each expression and comma in `field`.
    fn expression_list(&mut self, items: &mut Vec<Item>, field: &'static str) {
        loop {
            items.push(self.expression().with(field));
            if self.at(",") {
                items.push(self.punct().with(field));
                continue;
            }
            return;
        }
    }

    fn foreach_statement(&mut self, mut items: Vec<Item>) -> Item {
        items.push(self.punct());
        items.push(self.expect("("));
        let typed = self.scan_type(self.i).is_some_and(|t| {
            (self.is_ident(t) && self.is(t + 1, "in"))
                || (self.is(t, "(") && self.scan_balanced(t).is_some_and(|k| self.is(k, "in")))
        });
        if typed {
            items.push(self.ty().with("type"));
            if self.at("(") {
                items.push(self.tuple_pattern().with("left"));
            } else {
                items.push(self.identifier().with("left"));
            }
        } else {
            items.push(self.expression().with("left"));
        }
        items.push(self.expect("in"));
        items.push(self.expression().with("right"));
        items.push(self.expect(")"));
        items.push(self.statement().with("body"));
        node("foreach_statement", items)
    }

    fn using_statement(&mut self, mut items: Vec<Item>) -> Item {
        items.push(self.punct());
        items.push(self.expect("("));
        let decl = self
            .scan_type(self.i)
            .is_some_and(|t| self.is_ident(t) && matches!(self.kind_at(t + 1), L::Op("=" | ",")));
        if decl {
            let ty = self.ty();
            let mut d = vec![ty.with("type")];
            self.comma_list(&mut d, ")", false, |p| {
                let mut v = vec![p.identifier().with("name")];
                if p.at("=") {
                    v.push(p.punct());
                    v.push(p.expression());
                }
                node("variable_declarator", v)
            });
            items.push(node("variable_declaration", d));
        } else {
            items.push(self.expression());
        }
        items.push(self.expect(")"));
        items.push(self.statement().with("body"));
        node("using_statement", items)
    }

    fn switch_statement(&mut self) -> Item {
        let mut items = vec![self.punct()];
        // `switch (a, b)` is a tuple; `switch (x)` its parentheses.
        let tuple = self.at("(") && self.paren_has_comma(self.i);
        if tuple {
            items.push(self.expression().with("value"));
        } else {
            items.push(self.expect("("));
            items.push(self.expression().with("value"));
            items.push(self.expect(")"));
        }
        let mut body = vec![self.expect("{")];
        while !self.eof() && !self.at("}") {
            let before = self.i;
            if self.at("#if") {
                body.push(self.preproc_if(&mut Self::switch_section));
            } else if self.at_directive_end() {
                break;
            } else {
                body.push(self.switch_section());
            }
            if self.i == before {
                body.push(self.error_token());
            }
        }
        body.push(self.expect("}"));
        items.push(node("switch_body", body).with("body"));
        node("switch_statement", items)
    }

    /// Whether the parentheses at `n` hold a top-level comma.
    fn paren_has_comma(&self, n: usize) -> bool {
        let mut depth = 0usize;
        let mut k = n;
        loop {
            match self.kind_at(k) {
                L::Op("(" | "[" | "{") => depth += 1,
                L::Op(")" | "]" | "}") => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return false;
                    }
                }
                L::Op(",") if depth == 1 => return true,
                // `F<A, B>(…)`: the type arguments' commas are not the
                // parentheses'.
                L::Op("<") if self.kind_at(k - 1) == L::Name => {
                    if let Some(e) = self.scan_type_arguments(k)
                        && self.generic_follows(e)
                    {
                        k = e;
                        continue;
                    }
                }
                L::Eof => return false,
                _ => {}
            }
            k += 1;
        }
    }

    fn switch_section(&mut self) -> Item {
        let mut items = Vec::new();
        if self.at("case") {
            items.push(self.punct());
            items.push(self.pattern_in(expr::PatternCtx::Case));
            if self.at("when") {
                items.push(self.when_clause());
            }
        } else if self.at("default") {
            items.push(self.punct());
        } else {
            return self.error_statement();
        }
        items.push(self.expect(":"));
        while !self.eof()
            && !self.at("}")
            && !self.at("case")
            && !(self.at("default") && self.is(self.i + 1, ":"))
            && !self.at_directive_end()
        {
            let before = self.i;
            items.push(self.statement());
            if self.i == before {
                items.push(self.error_token());
            }
        }
        node("switch_section", items)
    }

    /// `when e`: in a switch arm `when x => y` is the arm's `=>`, so no
    /// lambda starts at the clause's own level.
    fn when_clause(&mut self) -> Item {
        let kw = self.punct();
        let saved = std::mem::replace(&mut self.no_lambda, true);
        let e = self.expression();
        self.no_lambda = saved;
        node("when_clause", vec![kw, e])
    }

    fn try_statement(&mut self) -> Item {
        let mut items = vec![self.punct(), self.block().with("body")];
        while self.at("catch") {
            let mut c = vec![self.punct()];
            loop {
                if self.at("(") {
                    let mut d = vec![self.punct(), self.ty().with("type")];
                    if self.is_ident(self.i) {
                        d.push(self.identifier().with("name"));
                    }
                    d.push(self.expect(")"));
                    c.push(node("catch_declaration", d));
                } else if self.at("when") {
                    let f = vec![
                        self.punct(),
                        self.expect("("),
                        self.expression(),
                        self.expect(")"),
                    ];
                    c.push(node("catch_filter_clause", f));
                } else {
                    break;
                }
            }
            c.push(self.block().with("body"));
            items.push(node("catch_clause", c));
        }
        if self.at("finally") {
            let f = vec![self.punct(), self.block()];
            items.push(node("finally_clause", f));
        }
        node("try_statement", items)
    }
}
