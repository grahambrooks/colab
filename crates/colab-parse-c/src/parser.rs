//! C++ (and, as a dialect, C) read by recursive descent into a tree of
//! [`Item`]s: tree-sitter-cpp's grammar (declarations and their
//! declarators, classes and templates, statements, the expression
//! precedences, the preprocessor's lines), kinds and fields named as in
//! `names.rs`.
//!
//! Where C++ needs lookahead (a declaration or an expression statement; a
//! template's arguments or a comparison; a declarator's parameters or an
//! initializer's arguments) the parser scans the tokens ahead with the
//! `scan_*` functions, which build nothing, and decides as tree-sitter's
//! GLR parse does. A construct it cannot read becomes an `ERROR` node and
//! reading resumes at the next statement or member; a token that should
//! be there and is not is a missing one.
//!
//! A node whose kind is `""` is transparent: its children are emitted in
//! its place (tree-sitter's hidden rules, such as an empty declaration's).

mod decl;
mod expr;

use crate::lexer::{L, Lx};
use crate::names;

/// The C dialect or C++.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dialect {
    C,
    Cpp,
}

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
    /// `"ERROR"` for text the parser could not read; `""` for a hidden
    /// rule, whose children are its parent's.
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
                    // A hidden rule's field goes to its children.
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

    pub(crate) fn field(&self) -> Option<&'static str> {
        match self {
            Item::Tok(t) => t.field,
            Item::Node(n) => n.field,
        }
    }

    pub(crate) fn without_field(mut self) -> Self {
        match &mut self {
            Item::Tok(t) => t.field = None,
            Item::Node(n) => n.field = None,
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
    })
}

/// Words that are never identifiers.
pub(crate) const KEYWORDS: &[&str] = &[
    "alignas",
    "alignof",
    "asm",
    "auto",
    "bool",
    "break",
    "case",
    "catch",
    "char",
    "class",
    "co_await",
    "co_return",
    "co_yield",
    "const",
    "consteval",
    "constexpr",
    "constinit",
    "continue",
    "decltype",
    "default",
    "delete",
    "do",
    "double",
    "else",
    "enum",
    "explicit",
    "extern",
    "false",
    "float",
    "for",
    "friend",
    "goto",
    "if",
    "inline",
    "int",
    "long",
    "mutable",
    "namespace",
    "new",
    "noexcept",
    "nullptr",
    "operator",
    "private",
    "protected",
    "public",
    "register",
    "requires",
    "return",
    "short",
    "signed",
    "sizeof",
    "static",
    "static_assert",
    "struct",
    "switch",
    "template",
    "this",
    "thread_local",
    "throw",
    "true",
    "try",
    "typedef",
    "typename",
    "union",
    "unsigned",
    "using",
    "virtual",
    "void",
    "volatile",
    "while",
    "and",
    "and_eq",
    "bitand",
    "bitor",
    "compl",
    "not",
    "not_eq",
    "or",
    "or_eq",
    "xor",
    "xor_eq",
];

/// C's words that are never identifiers.
pub(crate) const C_KEYWORDS: &[&str] = &[
    "auto",
    "break",
    "case",
    "char",
    "const",
    "continue",
    "default",
    "do",
    "double",
    "else",
    "enum",
    "extern",
    "float",
    "for",
    "goto",
    "if",
    "inline",
    "int",
    "long",
    "register",
    "restrict",
    "return",
    "short",
    "signed",
    "sizeof",
    "static",
    "struct",
    "switch",
    "typedef",
    "union",
    "unsigned",
    "void",
    "volatile",
    "while",
    "bool",
    "true",
    "false",
    "NULL",
    "nullptr",
    "_Atomic",
    "_Noreturn",
    "_Alignas",
    "_Alignof",
    "_Generic",
    "alignas",
    "alignof",
    "constexpr",
    "thread_local",
    "offsetof",
    "__attribute__",
    "__attribute",
    "__declspec",
    "__extension__",
];

pub(crate) const PRIMITIVES: &[&str] = &[
    "bool",
    "char",
    "int",
    "float",
    "double",
    "void",
    "size_t",
    "ssize_t",
    "ptrdiff_t",
    "intptr_t",
    "uintptr_t",
    "charptr_t",
    "nullptr_t",
    "max_align_t",
    "int8_t",
    "int16_t",
    "int32_t",
    "int64_t",
    "uint8_t",
    "uint16_t",
    "uint32_t",
    "uint64_t",
    "char8_t",
    "char16_t",
    "char32_t",
    "char64_t",
];

pub(crate) const STORAGE: &[&str] = &[
    "extern",
    "static",
    "register",
    "inline",
    "__inline",
    "__inline__",
    "__forceinline",
    "thread_local",
    "__thread",
];

pub(crate) const QUALIFIERS: &[&str] = &[
    "const",
    "constexpr",
    "volatile",
    "restrict",
    "__restrict__",
    "__extension__",
    "_Atomic",
    "_Noreturn",
    "noreturn",
    "_Nonnull",
    "mutable",
    "constinit",
    "consteval",
];

pub(crate) const SIZED: &[&str] = &["signed", "unsigned", "long", "short"];

/// What a run of items is the body of.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Ctx {
    /// The file, a namespace's or a linkage's declaration list.
    Top,
    /// A compound statement.
    Block,
    /// A class's field declaration list.
    Field,
}

pub(crate) struct Parser<'s> {
    src: &'s str,
    toks: Vec<Lx>,
    i: usize,
    pub errors: bool,
    pub dialect: Dialect,
    /// Template-argument lists being read: a `>` there closes, it does
    /// not compare.
    no_gt: usize,
}

impl<'s> Parser<'s> {
    pub fn new(src: &'s str, toks: Vec<Lx>, dialect: Dialect) -> Self {
        Self {
            src,
            toks,
            i: 0,
            errors: false,
            dialect,
            no_gt: 0,
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

    /// Whether token `n` is the operator, word or directive `s`.
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

    /// Whether token `n` can be an identifier (C++'s keywords are C's
    /// identifiers: `int class = 1;`).
    fn is_ident(&self, n: usize) -> bool {
        let keywords = if self.cpp() { KEYWORDS } else { C_KEYWORDS };
        self.kind_at(n) == L::Name && !keywords.contains(&self.word_at(n))
    }

    pub(crate) fn cpp(&self) -> bool {
        self.dialect == Dialect::Cpp
    }

    /// A storage class's word (C's `auto` among them).
    pub(crate) fn storage(&self, w: &str) -> bool {
        STORAGE.contains(&w) || (w == "auto" && !self.cpp())
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
            L::Close(c) | L::StrOpen(c) | L::CharOpen(c) | L::RawOpen(c) => c,
            _ => names::anonymous(&self.src[t.start..t.end]).unwrap_or_else(|| {
                self.errors = true;
                "ERROR"
            }),
        };
        Self::tok(t, kind, false)
    }

    /// A keyword token wrapped in a node of `kind` (`storage_class_specifier`
    /// over `static`).
    fn wrapped(&mut self, kind: &'static str) -> Item {
        let t = self.punct();
        node(kind, vec![t])
    }

    /// Tokens `self.i .. self.i + n`, which touch, as one anonymous token
    /// of `kind` (`>>`, `[[`).
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

    fn identifier(&mut self) -> Item {
        if self.is_ident(self.i) {
            self.leaf("identifier")
        } else {
            self.missing("identifier", true)
        }
    }

    /// An identifier read as `kind` (`type_identifier`, `field_identifier`).
    fn ident_as(&mut self, kind: &'static str) -> Item {
        if self.is_ident(self.i) {
            self.leaf(kind)
        } else {
            self.missing(kind, true)
        }
    }

    fn error_token(&mut self) -> Item {
        self.errors = true;
        let t = self.advance();
        Self::tok(t, "ERROR", false)
    }

    /// At `[` `[` touching: an attribute's opening.
    fn at_attribute(&self) -> bool {
        self.at("[") && self.is(self.i + 1, "[") && self.adjacent(self.i)
    }

    /// The rest of the statement (to its `;`, or before a `}` or a `{`
    /// that closes or opens a block) as an `ERROR` node; at least one
    /// token.
    fn error_statement(&mut self) -> Item {
        self.errors = true;
        let mut items = Vec::new();
        let mut depth = 0usize;
        while !self.eof() {
            if depth == 0
                && (self.at("}")
                    || (self.at("{") && !items.is_empty())
                    || matches!(self.peek(), L::Directive(_) | L::OtherDirective))
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

    /// Past the bracket at `n` and its match.
    fn scan_balanced(&self, n: usize) -> Option<usize> {
        let mut depth = 0usize;
        let mut k = n;
        loop {
            match self.kind_at(k) {
                L::Op("(" | "[" | "{") => depth += 1,
                L::Op(")" | "]" | "}") => {
                    depth = depth.checked_sub(1)?;
                    if depth == 0 {
                        return Some(k + 1);
                    }
                }
                L::Eof => return None,
                _ => {}
            }
            k += 1;
        }
    }

    // ---- the file --------------------------------------------------------

    pub fn program(&mut self) -> Item {
        let mut items = Vec::new();
        while !self.eof() {
            let before = self.i;
            let item = self.item(Ctx::Top);
            items.push(item);
            if self.i == before {
                items.push(self.error_token());
            }
        }
        node("translation_unit", items)
    }

    /// At `#elif`, `#else`, `#endif` (or their `def` forms): the end of a
    /// conditional's branch.
    fn at_directive_end(&self) -> bool {
        matches!(
            self.peek(),
            L::Directive("#elif" | "#elifdef" | "#elifndef" | "#else" | "#endif")
        )
    }

    /// One item of a file, a block or a class body.
    pub(crate) fn item(&mut self, ctx: Ctx) -> Item {
        match self.peek() {
            L::Directive("#if" | "#ifdef" | "#ifndef") => {
                return self.preproc_conditional(&mut |p| p.item(ctx));
            }
            L::Directive("#include") => return self.preproc_include(),
            L::Directive("#define") => return self.preproc_define(),
            L::OtherDirective => return self.preproc_call(),
            L::Directive(_) => return self.error_token(),
            _ => {}
        }
        if ctx == Ctx::Field {
            return self.field_item();
        }
        match self.word() {
            "typedef" => return self.type_definition(),
            "__extension__" if self.is(self.i + 1, "typedef") => return self.type_definition(),
            "extern" if matches!(self.kind_at(self.i + 1), L::StrOpen(_)) => {
                return self.linkage();
            }
            _ if !self.cpp() => {}
            "namespace" => return self.namespace(),
            "inline" if self.is(self.i + 1, "namespace") => return self.namespace(),
            "using" => return self.using(),
            "template" => return self.template(),
            "static_assert" => return self.static_assert(),
            "concept" => return self.concept(),
            _ => {}
        }
        if self.declaration_at(ctx) {
            return self.declaration(ctx);
        }
        self.statement_in(ctx)
    }

    // ---- the preprocessor --------------------------------------------------

    fn line_end(&mut self, items: &mut Vec<Item>) {
        if self.peek() == L::LineEnd {
            let t = self.advance();
            items.push(Self::hidden(t.start, t.end));
        } else if !self.eof() {
            self.errors = true;
        }
    }

    fn preproc_include(&mut self) -> Item {
        let mut items = vec![self.punct()];
        let path = match self.peek() {
            L::SysLib => self.leaf("system_lib_string"),
            L::StrOpen(_) => self.string_literal(),
            L::Name if self.is(self.i + 1, "(") => {
                let f = self.leaf("identifier").with("function");
                let args = self.preproc_arguments().with("arguments");
                node("call_expression", vec![f, args])
            }
            L::Name => self.leaf("identifier"),
            _ => self.missing("string_literal", true),
        };
        items.push(path.with("path"));
        self.line_end(&mut items);
        node("preproc_include", items)
    }

    fn preproc_define(&mut self) -> Item {
        let mut items = vec![self.punct()];
        if self.peek() != L::Name {
            items.push(self.missing("identifier", true));
            self.line_end(&mut items);
            return node("preproc_def", items);
        }
        items.push(self.leaf("identifier").with("name"));
        let function = self.at("(") && self.adjacent(self.i - 1);
        if function {
            let mut params = vec![self.punct()];
            loop {
                match self.peek() {
                    L::Op(")") => break,
                    L::Op(",") => params.push(self.punct()),
                    L::Op("...") => params.push(self.punct()),
                    L::Name => params.push(self.leaf("identifier")),
                    _ => break,
                }
            }
            params.push(self.expect(")"));
            items.push(node("preproc_params", params).with("parameters"));
        }
        if self.peek() == L::PreprocArg {
            items.push(self.leaf("preproc_arg").with("value"));
        }
        self.line_end(&mut items);
        let kind = if function {
            "preproc_function_def"
        } else {
            "preproc_def"
        };
        node(kind, items)
    }

    fn preproc_call(&mut self) -> Item {
        let mut items = vec![self.leaf("preproc_directive").with("directive")];
        if self.peek() == L::PreprocArg {
            items.push(self.leaf("preproc_arg").with("argument"));
        }
        self.line_end(&mut items);
        node("preproc_call", items)
    }

    /// `#if` / `#ifdef` / `#ifndef` … `#endif`, its branches read by
    /// `content` until a directive ends them.
    pub(crate) fn preproc_conditional(
        &mut self,
        content: &mut dyn FnMut(&mut Self) -> Item,
    ) -> Item {
        let ifdef = !self.at("#if");
        let mut items = vec![self.punct()];
        if ifdef {
            items.push(self.preproc_name().with("name"));
        } else {
            items.push(self.preproc_condition().with("condition"));
            items.push(self.newline());
        }
        self.branch(&mut items, content);
        if let Some(alt) = self.preproc_alternative(content) {
            items.push(alt.with("alternative"));
        }
        items.push(self.expect("#endif"));
        node(if ifdef { "preproc_ifdef" } else { "preproc_if" }, items)
    }

    fn preproc_name(&mut self) -> Item {
        if self.peek() == L::Name {
            self.leaf("identifier")
        } else {
            self.missing("identifier", true)
        }
    }

    fn newline(&mut self) -> Item {
        if self.peek() == L::Newline {
            let t = self.advance();
            Self::tok(t, "\n", false)
        } else {
            self.missing("\n", false)
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
        match self.peek() {
            L::Directive("#else") => {
                let mut items = vec![self.punct()];
                self.branch(&mut items, content);
                Some(node("preproc_else", items))
            }
            L::Directive("#elif") => {
                let mut items = vec![self.punct()];
                items.push(self.preproc_condition().with("condition"));
                items.push(self.newline());
                self.branch(&mut items, content);
                if let Some(alt) = self.preproc_alternative(content) {
                    items.push(alt.with("alternative"));
                }
                Some(node("preproc_elif", items))
            }
            L::Directive("#elifdef" | "#elifndef") => {
                let mut items = vec![self.punct()];
                items.push(self.preproc_name().with("name"));
                self.branch(&mut items, content);
                if let Some(alt) = self.preproc_alternative(content) {
                    items.push(alt.with("alternative"));
                }
                Some(node("preproc_elifdef", items))
            }
            _ => None,
        }
    }

    /// A directive's condition, by C's precedences.
    fn preproc_condition(&mut self) -> Item {
        self.preproc_binary(0)
    }

    fn preproc_binary(&mut self, min: u8) -> Item {
        let mut left = self.preproc_unary();
        loop {
            let prec = match self.peek() {
                L::Op("||") => 1,
                L::Op("&&") => 2,
                L::Op("|") => 3,
                L::Op("^") => 4,
                L::Op("&") => 5,
                L::Op("==" | "!=") => 6,
                L::Op("<" | "<=") => 7,
                L::Op(">") => 7,
                L::Op("<<") => 9,
                L::Op("+" | "-") => 10,
                L::Op("*" | "/" | "%") => 11,
                _ => break,
            };
            if prec < min {
                break;
            }
            let op = if self.at(">") {
                let (n, kind) = self.joined_greater();
                let n = if matches!(kind, ">=" | ">>") { n } else { 1 };
                let kind = if n == 1 { ">" } else { kind };
                let prec2 = if kind == ">>" { 9 } else { 7 };
                if prec2 < min {
                    break;
                }
                self.joined(n, kind)
            } else {
                self.punct()
            };
            let p = match op.kind() {
                ">>" => 9,
                _ => prec,
            };
            let right = self.preproc_binary(p + 1).with("right");
            left = node(
                "binary_expression",
                vec![left.with("left"), op.with("operator"), right],
            );
        }
        left
    }

    fn preproc_unary(&mut self) -> Item {
        match self.peek() {
            L::Op("!" | "~" | "-" | "+") => {
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
            L::Number => self.leaf("number_literal"),
            L::CharOpen(_) => self.char_literal(),
            L::Name if self.at("defined") => {
                let kw = self.punct();
                if self.at("(") {
                    let open = self.punct();
                    let id = self.leaf("identifier");
                    let close = self.expect(")");
                    node("preproc_defined", vec![kw, open, id, close])
                } else {
                    let id = self.preproc_name();
                    node("preproc_defined", vec![kw, id])
                }
            }
            L::Name if self.is(self.i + 1, "(") => {
                let f = self.leaf("identifier").with("function");
                let args = self.preproc_arguments().with("arguments");
                node("call_expression", vec![f, args])
            }
            L::Name => self.leaf("identifier"),
            _ => self.missing("identifier", true),
        }
    }

    fn preproc_arguments(&mut self) -> Item {
        let mut items = vec![self.punct()];
        self.comma_list(&mut items, ")", false, |p| p.preproc_binary(0));
        items.push(self.expect(")"));
        node("argument_list", items)
    }

    // ---- statements --------------------------------------------------------

    pub(crate) fn compound_statement(&mut self) -> Item {
        let mut items = vec![self.expect("{")];
        while !self.eof() && !self.at("}") && !self.at_directive_end() {
            let before = self.i;
            items.push(self.item(Ctx::Block));
            if self.i == before {
                items.push(self.error_token());
            }
        }
        items.push(self.expect("}"));
        node("compound_statement", items)
    }

    pub(crate) fn statement(&mut self) -> Item {
        self.statement_in(Ctx::Block)
    }

    fn statement_in(&mut self, ctx: Ctx) -> Item {
        match self.peek() {
            L::Op("{") => return self.compound_statement(),
            L::Op(";") => return node("expression_statement", vec![self.punct()]),
            L::Op("[") if self.at_attribute() => {
                let mut items = Vec::new();
                while self.at_attribute() {
                    items.push(self.attribute_declaration());
                }
                if self.declaration_at(ctx) {
                    // Attributes before a declaration are its own.
                    let mut d = self.declaration(ctx);
                    if let Item::Node(n) = &mut d {
                        let mut all = items;
                        all.append(&mut n.items);
                        n.items = all;
                    }
                    return d;
                }
                items.push(self.statement_in(ctx));
                return node("attributed_statement", items);
            }
            L::Directive(_) | L::OtherDirective => return self.item(ctx),
            L::Name => {}
            _ => return self.expression_statement(),
        }
        let w = self.word();
        // C++'s statement keywords are C's identifiers.
        let w = if !self.cpp() && matches!(w, "try" | "throw" | "co_return" | "co_yield") {
            ""
        } else {
            w
        };
        match w {
            "if" => self.if_statement(),
            "while" => {
                let items = vec![
                    self.punct(),
                    self.condition_clause().with("condition"),
                    self.statement().with("body"),
                ];
                node("while_statement", items)
            }
            "do" => {
                let items = vec![
                    self.punct(),
                    self.statement().with("body"),
                    self.expect("while"),
                    self.parenthesized_expression().with("condition"),
                    self.expect(";"),
                ];
                node("do_statement", items)
            }
            "for" => self.for_statement(),
            "switch" => {
                let items = vec![
                    self.punct(),
                    self.condition_clause().with("condition"),
                    self.compound_statement().with("body"),
                ];
                node("switch_statement", items)
            }
            "case" | "default" if w == "case" || self.is(self.i + 1, ":") => self.case_statement(),
            "return" => {
                let mut items = vec![self.punct()];
                if self.at("{") && self.cpp() {
                    items.push(self.initializer_list());
                } else if !self.at(";") {
                    items.push(self.comma_expression());
                }
                items.push(self.expect(";"));
                node("return_statement", items)
            }
            "break" | "continue" => {
                let kind = if w == "break" {
                    "break_statement"
                } else {
                    "continue_statement"
                };
                let items = vec![self.punct(), self.expect(";")];
                node(kind, items)
            }
            "goto" => {
                let items = vec![
                    self.punct(),
                    self.ident_as("statement_identifier").with("label"),
                    self.expect(";"),
                ];
                node("goto_statement", items)
            }
            "try" => self.try_statement(),
            "throw" => {
                let mut items = vec![self.punct()];
                if !self.at(";") {
                    items.push(self.expression());
                }
                items.push(self.expect(";"));
                node("throw_statement", items)
            }
            "co_return" => {
                let mut items = vec![self.punct()];
                if !self.at(";") {
                    items.push(self.expression());
                }
                items.push(self.expect(";"));
                node("co_return_statement", items)
            }
            "co_yield" => {
                let items = vec![self.punct(), self.expression(), self.expect(";")];
                node("co_yield_statement", items)
            }
            "__try" => {
                let mut items = vec![self.punct(), self.compound_statement().with("body")];
                if self.at("__except") {
                    let e = vec![
                        self.punct(),
                        self.parenthesized_expression().with("filter"),
                        self.compound_statement().with("body"),
                    ];
                    items.push(node("seh_except_clause", e));
                } else if self.at("__finally") {
                    let f = vec![self.punct(), self.compound_statement().with("body")];
                    items.push(node("seh_finally_clause", f));
                }
                node("seh_try_statement", items)
            }
            "__leave" => {
                let items = vec![self.punct(), self.expect(";")];
                node("seh_leave_statement", items)
            }
            // `private:` in a block is a label to tree-sitter.
            _ if (self.is_ident(self.i) || matches!(w, "public" | "private" | "protected"))
                && self.is(self.i + 1, ":")
                && !self.is(self.i + 2, ":") =>
            {
                let label = self.leaf("statement_identifier").with("label");
                let colon = self.punct();
                let body = if self.declaration_at(Ctx::Block) {
                    self.declaration(Ctx::Block)
                } else {
                    self.statement()
                };
                node("labeled_statement", vec![label, colon, body])
            }
            _ => self.expression_statement(),
        }
    }

    fn expression_statement(&mut self) -> Item {
        let mut items = Vec::new();
        if !self.at(";") {
            if !self.expression_start(self.i) {
                return self.error_statement();
            }
            items.push(self.comma_expression());
        }
        items.push(self.expect(";"));
        node("expression_statement", items)
    }

    fn if_statement(&mut self) -> Item {
        let mut items = vec![self.punct()];
        if self.at("constexpr") {
            items.push(self.punct());
        }
        items.push(self.condition_clause().with("condition"));
        items.push(self.statement().with("consequence"));
        if self.at("else") {
            let e = vec![self.punct(), self.statement()];
            items.push(node("else_clause", e).with("alternative"));
        }
        node("if_statement", items)
    }

    /// `( [init;] value )`, the value a declaration where one starts.
    pub(crate) fn condition_clause(&mut self) -> Item {
        if self.dialect == Dialect::C {
            return self.parenthesized_expression();
        }
        let mut items = vec![self.expect("(")];
        if self.paren_has_semicolon(self.i - 1) {
            items.push(self.init_statement().with("initializer"));
        }
        // A condition's declaration has a value (`if (a && b)` compares).
        if self.declaration_at(Ctx::Block) && self.paren_has_initializer(self.i) {
            let mut d = Vec::new();
            self.decl_specifiers(&mut d, Ctx::Block);
            d.push(self.declarator(decl::Dk::Normal).with("declarator"));
            if self.at("=") {
                d.push(self.punct());
                d.push(self.expression().with("value"));
            } else if self.at("{") {
                d.push(self.initializer_list().with("value"));
            }
            items.push(node("declaration", d).with("value"));
        } else {
            items.push(self.comma_expression().with("value"));
        }
        items.push(self.expect(")"));
        node("condition_clause", items)
    }

    /// Whether the rest of the parentheses (from token `n`, inside them)
    /// hold a top-level `=` or `{`: a condition declaration's value.
    fn paren_has_initializer(&self, n: usize) -> bool {
        let mut depth = 1usize;
        let mut k = n;
        loop {
            match self.kind_at(k) {
                L::Op("(" | "[") => depth += 1,
                L::Op("{") if depth == 1 => return true,
                L::Op("{") => depth += 1,
                L::Op(")" | "]" | "}") => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return false;
                    }
                }
                L::Op("=") if depth == 1 => return true,
                L::Eof => return false,
                _ => {}
            }
            k += 1;
        }
    }

    /// Whether the parentheses at `n` hold a top-level `;`.
    fn paren_has_semicolon(&self, n: usize) -> bool {
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
                L::Op(";") if depth == 1 => return true,
                L::Eof => return false,
                _ => {}
            }
            k += 1;
        }
    }

    /// An `if`'s or a range `for`'s initializer: a declaration, an alias,
    /// a type definition or an expression statement (each with its `;`).
    fn init_statement(&mut self) -> Item {
        let inner = if self.at("using") {
            self.using()
        } else if self.at("typedef") {
            self.type_definition()
        } else if self.declaration_at(Ctx::Block) {
            self.declaration(Ctx::Block)
        } else {
            self.expression_statement()
        };
        node("init_statement", vec![inner])
    }

    fn for_statement(&mut self) -> Item {
        let mut items = vec![self.punct(), self.expect("(")];
        // `for (T x : xs)`: a top-level `:` before the `)`.
        if self.dialect == Dialect::Cpp && self.range_for_at(self.i - 1) {
            if self.paren_has_semicolon(self.i - 1) {
                items.push(self.init_statement().with("initializer"));
            }
            self.decl_specifiers(&mut items, Ctx::Block);
            items.push(self.declarator(decl::Dk::Normal).with("declarator"));
            items.push(self.expect(":"));
            let right = if self.at("{") {
                self.initializer_list()
            } else {
                self.expression()
            };
            items.push(right.with("right"));
            items.push(self.expect(")"));
            items.push(self.statement().with("body"));
            return node("for_range_loop", items);
        }
        if self.declaration_at(Ctx::Block) {
            items.push(self.declaration(Ctx::Block).with("initializer"));
        } else {
            if !self.at(";") {
                items.push(self.comma_expression().with("initializer"));
            }
            items.push(self.expect(";"));
        }
        if !self.at(";") {
            items.push(self.comma_expression().with("condition"));
        }
        items.push(self.expect(";"));
        if !self.at(")") {
            items.push(self.comma_expression().with("update"));
        }
        items.push(self.expect(")"));
        items.push(self.statement().with("body"));
        node("for_statement", items)
    }

    /// Whether the parentheses at `n` hold a top-level `:` (not `::`, not a
    /// conditional's).
    fn range_for_at(&self, n: usize) -> bool {
        let mut depth = 0usize;
        let mut questions = 0usize;
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
                L::Op("?") if depth == 1 => questions += 1,
                L::Op(":") if depth == 1 => {
                    if questions == 0 {
                        return true;
                    }
                    questions -= 1;
                }
                L::Op(";") if depth == 1 => {}
                L::Eof => return false,
                _ => {}
            }
            k += 1;
        }
    }

    fn case_statement(&mut self) -> Item {
        let mut items = vec![];
        if self.at("case") {
            items.push(self.punct());
            items.push(self.expression().with("value"));
        } else {
            items.push(self.punct());
        }
        items.push(self.expect(":"));
        // A case holds statements, declarations and type definitions only:
        // a directive, `using`, `template` and the like end it.
        while !self.eof()
            && !self.at("}")
            && !self.at("case")
            && !(self.at("default") && self.is(self.i + 1, ":"))
            && !matches!(self.peek(), L::Directive(_) | L::OtherDirective)
            && !(self.cpp()
                && matches!(
                    self.word(),
                    "using" | "template" | "static_assert" | "namespace" | "concept"
                ))
        {
            let before = self.i;
            items.push(self.item(Ctx::Block));
            if self.i == before {
                items.push(self.error_token());
            }
        }
        node("case_statement", items)
    }

    fn try_statement(&mut self) -> Item {
        let mut items = vec![self.punct(), self.compound_statement().with("body")];
        while self.at("catch") {
            let c = vec![
                self.punct(),
                self.parameter_list().with("parameters"),
                self.compound_statement().with("body"),
            ];
            items.push(node("catch_clause", c));
        }
        node("try_statement", items)
    }
}
