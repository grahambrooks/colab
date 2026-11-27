//! Java, read by recursive descent into a tree of [`Item`]s: tree-sitter-
//! java's grammar (declarations, statements, the expression precedences,
//! patterns, modules), with kinds and fields named as in `names.rs`.
//!
//! Where Java needs lookahead (a local variable declaration or an
//! expression statement; a cast or a parenthesized expression; a lambda's
//! parameters; a generic type before `::`) the parser scans the tokens
//! ahead with the `scan_*` functions, which build nothing. A construct it
//! cannot read becomes an `ERROR` node, and reading resumes at the next
//! statement; a token that should be there and is not is a missing one.

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
}

impl Item {
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

    fn field(&self) -> Option<&'static str> {
        match self {
            Item::Tok(t) => t.field,
            Item::Node(n) => n.field,
        }
    }

    fn without_field(mut self) -> Self {
        match &mut self {
            Item::Tok(t) => t.field = None,
            Item::Node(n) => n.field = None,
        }
        self
    }
}

fn node(kind: &'static str, items: Vec<Item>) -> Item {
    Item::Node(Node {
        kind,
        field: None,
        items,
    })
}

/// Words that are never identifiers.
const KEYWORDS: &[&str] = &[
    "abstract",
    "assert",
    "boolean",
    "break",
    "byte",
    "case",
    "catch",
    "char",
    "class",
    "const",
    "continue",
    "default",
    "do",
    "double",
    "else",
    "enum",
    "extends",
    "final",
    "finally",
    "float",
    "for",
    "goto",
    "if",
    "implements",
    "import",
    "instanceof",
    "int",
    "interface",
    "long",
    "native",
    "new",
    "package",
    "private",
    "protected",
    "public",
    "return",
    "short",
    "static",
    "strictfp",
    "super",
    "switch",
    "synchronized",
    "this",
    "throw",
    "throws",
    "transient",
    "try",
    "void",
    "volatile",
    "while",
    "true",
    "false",
    "null",
    "non-sealed",
];

const PRIMITIVES: &[&str] = &[
    "byte", "short", "int", "long", "char", "float", "double", "boolean", "void",
];

const MODIFIERS: &[&str] = &[
    "public",
    "protected",
    "private",
    "abstract",
    "static",
    "final",
    "strictfp",
    "default",
    "synchronized",
    "native",
    "transient",
    "volatile",
    "sealed",
    "non-sealed",
];

const ASSIGNMENTS: &[&str] = &[
    "=", "+=", "-=", "*=", "/=", "&=", "|=", "^=", "%=", "<<=", ">>=", ">>>=",
];

/// Where a run of declarations is: what a member that is not a type or
/// method declares.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Body {
    Class,
    Interface,
    Annotation,
}

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
            _ => false,
        }
    }

    fn at(&self, s: &str) -> bool {
        self.is(self.i, s)
    }

    fn is_ident(&self, n: usize) -> bool {
        self.kind_at(n) == L::Name && !KEYWORDS.contains(&self.word_at(n))
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

    /// Tokens `self.i .. self.i + n`, which touch, as one anonymous token
    /// of `kind` (`>>`, `@interface`).
    fn joined(&mut self, n: usize, kind: &'static str) -> Item {
        let start = self.toks[self.i].start;
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

    fn missing(&mut self, kind: &'static str, named: bool) -> Item {
        self.errors = true;
        let at = if self.i == 0 {
            0
        } else {
            self.toks[self.i - 1].end
        };
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

    /// The rest of the statement (to its `;`, or before a `}` or `{` that
    /// closes or opens a block) as an `ERROR` node; at least one token.
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

    // ---- scanning ahead --------------------------------------------------

    /// An annotation at `n`: the token after it.
    fn scan_annotation(&self, n: usize) -> Option<usize> {
        if !self.is(n, "@") || self.is(n + 1, "interface") {
            return None;
        }
        let mut n = n + 1;
        if self.kind_at(n) != L::Name {
            return None;
        }
        n += 1;
        while self.is(n, ".") && self.kind_at(n + 1) == L::Name {
            n += 2;
        }
        if self.is(n, "(") {
            n = self.scan_balanced(n)?;
        }
        Some(n)
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

    fn scan_annotations(&self, mut n: usize) -> usize {
        while let Some(k) = self.scan_annotation(n) {
            n = k;
        }
        n
    }

    /// A type at `n` (annotations, a primitive or a name with type
    /// arguments, array dimensions): the token after it.
    fn scan_type(&self, n: usize) -> Option<usize> {
        let mut n = self.scan_annotations(n);
        if PRIMITIVES.contains(&self.word_at(n)) {
            n += 1;
        } else if self.is_ident(n) {
            n += 1;
            loop {
                if self.is(n, "<") {
                    n = self.scan_type_arguments(n)?;
                }
                if self.is(n, ".") {
                    let k = self.scan_annotations(n + 1);
                    if self.is_ident(k) {
                        n = k + 1;
                        continue;
                    }
                }
                break;
            }
        } else {
            return None;
        }
        Some(self.scan_dimensions(n))
    }

    fn scan_dimensions(&self, mut n: usize) -> usize {
        loop {
            let k = self.scan_annotations(n);
            if self.is(k, "[") && self.is(k + 1, "]") {
                n = k + 2;
            } else {
                return n;
            }
        }
    }

    /// Type arguments at `n` (a `<`): the token after their `>`.
    fn scan_type_arguments(&self, n: usize) -> Option<usize> {
        let mut n = n + 1;
        if self.is(n, ">") {
            return Some(n + 1);
        }
        loop {
            n = self.scan_annotations(n);
            if self.is(n, "?") {
                n += 1;
                if self.is(n, "extends") || self.is(n, "super") {
                    n = self.scan_type(n + 1)?;
                }
            } else {
                n = self.scan_type(n)?;
            }
            while self.is(n, "&") {
                n = self.scan_type(n + 1)?;
            }
            if self.is(n, ",") {
                n += 1;
                continue;
            }
            return self.is(n, ">").then_some(n + 1);
        }
    }

    /// Modifiers at `n` (keywords and annotations): the token after them.
    fn scan_modifiers(&self, mut n: usize) -> usize {
        loop {
            if let Some(k) = self.scan_annotation(n) {
                n = k;
            } else if self.is_modifier_at(n) {
                n += 1;
            } else {
                return n;
            }
        }
    }

    fn is_modifier_at(&self, n: usize) -> bool {
        let w = self.word_at(n);
        match w {
            // Contextual: a modifier only before what modifiers precede.
            "sealed" => {
                let next = self.word_at(n + 1);
                MODIFIERS.contains(&next)
                    || matches!(next, "class" | "interface" | "record" | "enum")
                    || self.is(n + 1, "@")
            }
            "synchronized" => !self.is(n + 1, "("),
            "default" => !self.is(n + 1, ":") && !self.is(n + 1, "->"),
            _ => MODIFIERS.contains(&w),
        }
    }

    /// Whether a local variable declaration starts at `n`: modifiers, a
    /// type, a name, and what follows a declarator's name.
    fn declaration_at(&self, n: usize) -> bool {
        let n = self.scan_modifiers(n);
        let Some(k) = self.scan_type(n) else {
            return false;
        };
        (self.is_ident(k) || self.word_at(k) == "_")
            && matches!(
                self.kind_at(k + 1),
                L::Op("=" | ";" | "," | "[" | ":" | ")" | "@")
            )
    }

    /// At `(`: whether a lambda's parameters start here (the matching `)`
    /// is followed by `->`).
    fn lambda_at(&self, n: usize) -> bool {
        if self.is_ident(n) || self.word_at(n) == "_" {
            return self.is(n + 1, "->");
        }
        if !self.is(n, "(") {
            return false;
        }
        self.scan_balanced(n).is_some_and(|k| self.is(k, "->"))
    }

    // ---- the file ----------------------------------------------------------

    pub fn program(&mut self) -> Item {
        let mut items = Vec::new();
        while !self.eof() {
            let before = self.i;
            if self.at("}") {
                items.push(self.error_statement());
                continue;
            }
            items.push(self.toplevel());
            if self.i == before {
                items.push(self.error_statement());
            }
        }
        node("program", items)
    }

    /// A statement, or a method declaration (the file may be a fragment).
    fn toplevel(&mut self) -> Item {
        let save = self.i;
        let n = self.scan_modifiers(self.i);
        let k = if self.is(n, "<") {
            self.scan_type_parameters(n).unwrap_or(n)
        } else {
            n
        };
        if let Some(t) = self.scan_type(k)
            && self.is_ident(t)
            && self.is(t + 1, "(")
            && !self.is(k, "new")
        {
            self.i = save;
            return self.member(Body::Class);
        }
        self.statement()
    }

    fn scan_type_parameters(&self, n: usize) -> Option<usize> {
        let mut depth = 0usize;
        let mut k = n;
        loop {
            match self.kind_at(k) {
                L::Op("<") => depth += 1,
                L::Op(">") => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(k + 1);
                    }
                }
                L::Op(";" | "{" | "(") | L::Eof => return None,
                _ => {}
            }
            k += 1;
        }
    }

    // ---- declarations ------------------------------------------------------

    fn modifiers(&mut self) -> Option<Item> {
        let mut items = Vec::new();
        loop {
            if self.scan_annotation(self.i).is_some() {
                items.push(self.annotation());
            } else if self.is_modifier_at(self.i) {
                items.push(self.punct());
            } else {
                break;
            }
        }
        (!items.is_empty()).then(|| node("modifiers", items))
    }

    fn annotation(&mut self) -> Item {
        let at = self.punct();
        let name = self.name().with("name");
        if self.at("(") {
            let args = self.annotation_arguments().with("arguments");
            return node("annotation", vec![at, name, args]);
        }
        node("marker_annotation", vec![at, name])
    }

    fn annotations(&mut self, items: &mut Vec<Item>) {
        while self.scan_annotation(self.i).is_some() {
            items.push(self.annotation());
        }
    }

    fn annotation_arguments(&mut self) -> Item {
        let mut items = vec![self.punct()];
        if self.is_ident(self.i) && self.is(self.i + 1, "=") {
            loop {
                let key = self.leaf("identifier").with("key");
                let eq = self.punct();
                let value = self.element_value().with("value");
                items.push(node("element_value_pair", vec![key, eq, value]));
                if !self.at(",") {
                    break;
                }
                items.push(self.punct());
            }
        } else if !self.at(")") {
            items.push(self.element_value());
        }
        items.push(self.expect(")"));
        node("annotation_argument_list", items)
    }

    fn element_value(&mut self) -> Item {
        if self.at("{") {
            let mut items = vec![self.punct()];
            while !self.at("}") && !self.eof() {
                let before = self.i;
                items.push(self.element_value());
                if self.i == before || !self.at(",") {
                    break;
                }
                items.push(self.punct());
            }
            items.push(self.expect("}"));
            return node("element_value_array_initializer", items);
        }
        if self.scan_annotation(self.i).is_some() {
            return self.annotation();
        }
        self.expression()
    }

    /// `a`, or `a.b.c` as nested `scoped_identifier`s.
    fn name(&mut self) -> Item {
        let mut n = if self.kind_at(self.i) == L::Name {
            self.leaf("identifier")
        } else {
            self.missing("identifier", true)
        };
        while self.at(".") && self.kind_at(self.i + 1) == L::Name {
            let dot = self.punct();
            let name = self.leaf("identifier").with("name");
            n = node("scoped_identifier", vec![n.with("scope"), dot, name]);
        }
        n
    }

    /// A declaration that starts with modifiers (or a type keyword), in a
    /// block or at the top: a type declaration or a local variable.
    fn type_declaration(&mut self, mods: Option<Item>) -> Option<Item> {
        let kind = match self.word() {
            "class" => "class",
            "interface" => "interface",
            "enum" => "enum",
            "record"
                if self.is_ident(self.i + 1)
                    && (self.is(self.i + 2, "(") || self.is(self.i + 2, "<")) =>
            {
                "record"
            }
            _ if self.at("@") && self.is(self.i + 1, "interface") => "@interface",
            _ => return None,
        };
        let mut items: Vec<Item> = mods.into_iter().collect();
        match kind {
            "class" => {
                items.push(self.punct());
                items.push(self.identifier().with("name"));
                if self.at("<") {
                    items.push(self.type_parameters().with("type_parameters"));
                }
                if self.at("extends") {
                    let e = vec![self.punct(), self.ty()];
                    items.push(node("superclass", e).with("superclass"));
                }
                if self.at("implements") {
                    let e = vec![self.punct(), self.type_list()];
                    items.push(node("super_interfaces", e).with("interfaces"));
                }
                if self.at("permits") {
                    let e = vec![self.punct(), self.type_list()];
                    items.push(node("permits", e).with("permits"));
                }
                items.push(self.class_body().with("body"));
                Some(node("class_declaration", items))
            }
            "interface" => {
                items.push(self.punct());
                items.push(self.identifier().with("name"));
                if self.at("<") {
                    items.push(self.type_parameters().with("type_parameters"));
                }
                if self.at("extends") {
                    let e = vec![self.punct(), self.type_list()];
                    items.push(node("extends_interfaces", e));
                }
                if self.at("permits") {
                    let e = vec![self.punct(), self.type_list()];
                    items.push(node("permits", e).with("permits"));
                }
                items.push(self.body(Body::Interface, "interface_body").with("body"));
                Some(node("interface_declaration", items))
            }
            "enum" => {
                items.push(self.punct());
                items.push(self.identifier().with("name"));
                if self.at("implements") {
                    let e = vec![self.punct(), self.type_list()];
                    items.push(node("super_interfaces", e).with("interfaces"));
                }
                items.push(self.enum_body().with("body"));
                Some(node("enum_declaration", items))
            }
            "record" => {
                items.push(self.punct());
                items.push(self.identifier().with("name"));
                if self.at("<") {
                    items.push(self.type_parameters().with("type_parameters"));
                }
                items.push(self.formal_parameters().with("parameters"));
                if self.at("implements") {
                    let e = vec![self.punct(), self.type_list()];
                    items.push(node("super_interfaces", e).with("interfaces"));
                }
                items.push(self.class_body().with("body"));
                Some(node("record_declaration", items))
            }
            _ => {
                items.push(self.joined(2, "@interface"));
                items.push(self.identifier().with("name"));
                items.push(
                    self.body(Body::Annotation, "annotation_type_body")
                        .with("body"),
                );
                Some(node("annotation_type_declaration", items))
            }
        }
    }

    fn type_list(&mut self) -> Item {
        let mut items = vec![self.ty()];
        while self.at(",") {
            items.push(self.punct());
            items.push(self.ty());
        }
        node("type_list", items)
    }

    fn type_parameters(&mut self) -> Item {
        let mut items = vec![self.punct()];
        loop {
            let mut p = Vec::new();
            self.annotations(&mut p);
            p.push(self.type_identifier());
            if self.at("extends") {
                let mut b = vec![self.punct(), self.ty()];
                while self.at("&") {
                    b.push(self.punct());
                    b.push(self.ty());
                }
                p.push(node("type_bound", b));
            }
            items.push(node("type_parameter", p));
            if !self.at(",") {
                break;
            }
            items.push(self.punct());
        }
        items.push(self.close_angle());
        node("type_parameters", items)
    }

    fn type_identifier(&mut self) -> Item {
        if self.is_ident(self.i) {
            self.leaf("type_identifier")
        } else {
            self.missing("type_identifier", true)
        }
    }

    fn class_body(&mut self) -> Item {
        self.body(Body::Class, "class_body")
    }

    fn body(&mut self, body: Body, kind: &'static str) -> Item {
        let mut items = vec![self.expect("{")];
        while !self.at("}") && !self.eof() {
            let before = self.i;
            items.push(self.member(body));
            if self.i == before {
                items.push(self.error_statement());
            }
        }
        items.push(self.expect("}"));
        node(kind, items)
    }

    fn enum_body(&mut self) -> Item {
        let mut items = vec![self.expect("{")];
        while !self.at("}") && !self.at(";") && !self.eof() {
            let before = self.i;
            let mut c: Vec<Item> = self.modifiers().into_iter().collect();
            c.push(self.identifier().with("name"));
            if self.at("(") {
                c.push(self.arguments().with("arguments"));
            }
            if self.at("{") {
                c.push(self.class_body().with("body"));
            }
            items.push(node("enum_constant", c));
            if self.i == before {
                items.push(self.error_statement());
                break;
            }
            if !self.at(",") {
                break;
            }
            items.push(self.punct());
        }
        if self.at(";") {
            let mut d = vec![self.punct()];
            while !self.at("}") && !self.eof() {
                let before = self.i;
                d.push(self.member(Body::Class));
                if self.i == before {
                    d.push(self.error_statement());
                }
            }
            items.push(node("enum_body_declarations", d));
        }
        items.push(self.expect("}"));
        node("enum_body", items)
    }

    /// One member of a class, interface or annotation body.
    fn member(&mut self, body: Body) -> Item {
        if self.at(";") {
            return self.punct();
        }
        if self.at("{") {
            return self.block();
        }
        if self.at("static") && self.is(self.i + 1, "{") {
            let s = vec![self.punct(), self.block()];
            return node("static_initializer", s);
        }
        let mods = self.modifiers();
        if let Some(d) = self.type_declaration(mods.clone()) {
            return d;
        }
        let mut items: Vec<Item> = mods.into_iter().collect();
        let mut type_params = None;
        if self.at("<") {
            type_params = Some(self.type_parameters().with("type_parameters"));
        }
        // A constructor: the name, then its parameters (or, a record's
        // compact constructor, its body).
        if self.is_ident(self.i) && self.is(self.i + 1, "(") {
            items.extend(type_params);
            items.push(self.identifier().with("name"));
            items.push(self.formal_parameters().with("parameters"));
            if self.at("throws") {
                items.push(self.throws());
            }
            items.push(self.constructor_body().with("body"));
            return node("constructor_declaration", items);
        }
        if type_params.is_none() && self.is_ident(self.i) && self.is(self.i + 1, "{") {
            items.push(self.identifier().with("name"));
            items.push(self.block().with("body"));
            return node("compact_constructor_declaration", items);
        }
        if let Some(tp) = type_params {
            items.push(tp);
            self.annotations(&mut items);
        }
        let ty = self.unannotated_type().with("type");
        items.push(ty);
        let name_ok = self.is_ident(self.i) || self.word() == "_";
        if name_ok && self.is(self.i + 1, "(") {
            items.push(self.leaf("identifier").with("name"));
            if body == Body::Annotation {
                items.push(self.punct());
                items.push(self.expect(")"));
                if self.at("[") || self.at("@") {
                    items.push(self.dimensions().with("dimensions"));
                }
                if self.at("default") {
                    items.push(self.punct());
                    items.push(self.element_value().with("value"));
                }
                items.push(self.expect(";"));
                return node("annotation_type_element_declaration", items);
            }
            items.push(self.formal_parameters().with("parameters"));
            if self.at("[") || (self.at("@") && self.is(self.scan_annotations(self.i), "[")) {
                items.push(self.dimensions().with("dimensions"));
            }
            if self.at("throws") {
                items.push(self.throws());
            }
            if self.at("{") {
                items.push(self.block().with("body"));
            } else {
                items.push(self.expect(";"));
            }
            return node("method_declaration", items);
        }
        self.declarators(&mut items);
        items.push(self.expect(";"));
        let kind = match body {
            Body::Class => "field_declaration",
            Body::Interface | Body::Annotation => "constant_declaration",
        };
        node(kind, items)
    }

    fn throws(&mut self) -> Item {
        let mut items = vec![self.punct(), self.ty()];
        while self.at(",") {
            items.push(self.punct());
            items.push(self.ty());
        }
        node("throws", items)
    }

    fn constructor_body(&mut self) -> Item {
        let mut items = vec![self.expect("{")];
        if let Some(inv) = self.explicit_constructor_invocation() {
            items.push(inv);
        }
        self.statements(&mut items);
        items.push(self.expect("}"));
        node("constructor_body", items)
    }

    /// `this(…);`, `super(…);`, `<T>super(…);`, `outer.super(…);`.
    fn explicit_constructor_invocation(&mut self) -> Option<Item> {
        let save = self.i;
        let mut items = Vec::new();
        if self.at("<") {
            self.scan_type_arguments(self.i)?;
            items.push(self.type_arguments().with("type_arguments"));
        }
        if (self.at("this") || self.at("super")) && self.is(self.i + 1, "(") {
            let kind = if self.at("this") { "this" } else { "super" };
            items.push(self.leaf(kind).with("constructor"));
        } else if items.is_empty() && self.qualified_super_at() {
            let object = self.primary_postfix_until_super();
            items.push(object.with("object"));
            items.push(self.punct());
            if self.at("<") {
                items.push(self.type_arguments().with("type_arguments"));
            }
            items.push(self.leaf("super").with("constructor"));
        } else {
            self.i = save;
            return None;
        }
        items.push(self.arguments().with("arguments"));
        items.push(self.expect(";"));
        Some(node("explicit_constructor_invocation", items))
    }

    /// Whether `a.b.super(` starts here.
    fn qualified_super_at(&self) -> bool {
        let mut n = self.i;
        if !self.is_ident(n) && !self.is(n, "this") {
            return false;
        }
        n += 1;
        while self.is(n, ".") && self.is_ident(n + 1) {
            n += 2;
        }
        self.is(n, ".") && self.is(n + 1, "super") && self.is(n + 2, "(")
    }

    /// The object of `a.b.super(…)`: up to its last `.`.
    fn primary_postfix_until_super(&mut self) -> Item {
        let mut e = if self.at("this") {
            self.leaf("this")
        } else {
            self.leaf("identifier")
        };
        while self.at(".") && self.is_ident(self.i + 1) {
            let dot = self.punct();
            let field = self.leaf("identifier").with("field");
            e = node("field_access", vec![e.with("object"), dot, field]);
        }
        e
    }

    fn formal_parameters(&mut self) -> Item {
        let mut items = vec![self.expect("(")];
        while !self.at(")") && !self.eof() {
            let before = self.i;
            items.push(self.formal_parameter());
            if self.i == before || !self.at(",") {
                break;
            }
            items.push(self.punct());
        }
        items.push(self.expect(")"));
        node("formal_parameters", items)
    }

    fn formal_parameter(&mut self) -> Item {
        let mut items: Vec<Item> = self.modifiers().into_iter().collect();
        let ty = self.unannotated_type();
        if self.at("...") {
            items.push(ty);
            items.push(self.punct());
            self.annotations(&mut items);
            let mut d = vec![self.variable_name()];
            if self.at("[") {
                d.push(self.dimensions().with("dimensions"));
            }
            items.push(node("variable_declarator", d));
            return node("spread_parameter", items);
        }
        // A receiver parameter: `Outer.this`.
        if self.at("this") || (self.is_ident(self.i) && self.is(self.i + 1, ".")) {
            let mut r = items;
            r.push(ty);
            while self.is_ident(self.i) && self.is(self.i + 1, ".") {
                r.push(self.leaf("identifier"));
                r.push(self.punct());
            }
            r.push(if self.at("this") {
                self.leaf("this")
            } else {
                self.missing("this", true)
            });
            return node("receiver_parameter", r);
        }
        items.push(ty.with("type"));
        items.push(self.variable_name());
        if self.at("[") || (self.at("@") && self.is(self.scan_annotations(self.i), "[")) {
            items.push(self.dimensions().with("dimensions"));
        }
        node("formal_parameter", items)
    }

    /// `a = 1, b[] = {2}` into `items`, each a `declarator`.
    fn declarators(&mut self, items: &mut Vec<Item>) {
        loop {
            let mut d = vec![self.variable_name()];
            if self.at("[") || (self.at("@") && self.is(self.scan_annotations(self.i), "[")) {
                d.push(self.dimensions().with("dimensions"));
            }
            if self.at("=") {
                d.push(self.punct());
                d.push(self.variable_initializer().with("value"));
            }
            items.push(node("variable_declarator", d).with("declarator"));
            if !self.at(",") {
                break;
            }
            items.push(self.punct());
        }
    }

    fn variable_name(&mut self) -> Item {
        if self.word() == "_" {
            return self.leaf("underscore_pattern").with("name");
        }
        self.identifier().with("name")
    }

    fn variable_initializer(&mut self) -> Item {
        if self.at("{") {
            return self.array_initializer();
        }
        self.expression()
    }

    fn array_initializer(&mut self) -> Item {
        let mut items = vec![self.punct()];
        while !self.at("}") && !self.eof() {
            let before = self.i;
            items.push(self.variable_initializer());
            if self.i == before || !self.at(",") {
                break;
            }
            items.push(self.punct());
        }
        items.push(self.expect("}"));
        node("array_initializer", items)
    }

    // ---- types -------------------------------------------------------------

    /// A `_type`: annotated or not.
    fn ty(&mut self) -> Item {
        if self.scan_annotation(self.i).is_some() {
            let mut items = Vec::new();
            self.annotations(&mut items);
            items.push(self.unannotated_type());
            return node("annotated_type", items);
        }
        self.unannotated_type()
    }

    fn unannotated_type(&mut self) -> Item {
        let t = self.simple_type();
        self.array_type_of(t)
    }

    /// `t` with the dimensions after it, if any, as an `array_type`.
    fn array_type_of(&mut self, t: Item) -> Item {
        let k = self.scan_annotations(self.i);
        if self.is(k, "[") && self.is(k + 1, "]") {
            let dims = self.dimensions().with("dimensions");
            return node("array_type", vec![t.with("element"), dims]);
        }
        t
    }

    fn dimensions(&mut self) -> Item {
        let mut items = Vec::new();
        loop {
            let k = self.scan_annotations(self.i);
            if !(self.is(k, "[") && self.is(k + 1, "]")) {
                break;
            }
            self.annotations(&mut items);
            items.push(self.punct());
            items.push(self.punct());
        }
        node("dimensions", items)
    }

    fn simple_type(&mut self) -> Item {
        match self.word() {
            "void" => return self.leaf("void_type"),
            "boolean" => return self.leaf("boolean_type"),
            "byte" | "short" | "int" | "long" | "char" => {
                return node("integral_type", vec![self.punct()]);
            }
            "float" | "double" => return node("floating_point_type", vec![self.punct()]),
            _ => {}
        }
        let mut t = self.type_identifier();
        loop {
            if self.at("<") {
                let args = self.type_arguments();
                t = node("generic_type", vec![t, args]);
            }
            if self.at(".") {
                let k = self.scan_annotations(self.i + 1);
                if self.is_ident(k) {
                    let mut items = vec![t, self.punct()];
                    self.annotations(&mut items);
                    items.push(self.leaf("type_identifier"));
                    t = node("scoped_type_identifier", items);
                    continue;
                }
            }
            return t;
        }
    }

    fn type_arguments(&mut self) -> Item {
        let mut items = vec![self.punct()];
        while !self.at(">") && !self.eof() {
            let before = self.i;
            let k = self.scan_annotations(self.i);
            if self.is(k, "?") {
                let mut w = Vec::new();
                self.annotations(&mut w);
                w.push(self.punct());
                if self.at("extends") {
                    w.push(self.punct());
                    w.push(self.ty());
                } else if self.at("super") {
                    w.push(self.leaf("super"));
                    w.push(self.ty());
                }
                items.push(node("wildcard", w));
            } else {
                items.push(self.ty());
            }
            if self.i == before || !self.at(",") {
                break;
            }
            items.push(self.punct());
        }
        items.push(self.close_angle());
        node("type_arguments", items)
    }

    fn close_angle(&mut self) -> Item {
        self.expect(">")
    }

    // ---- statements --------------------------------------------------------

    fn block(&mut self) -> Item {
        let mut items = vec![self.expect("{")];
        self.statements(&mut items);
        items.push(self.expect("}"));
        node("block", items)
    }

    /// Statements up to a `}`.
    fn statements(&mut self, items: &mut Vec<Item>) {
        while !self.at("}") && !self.eof() {
            let before = self.i;
            items.push(self.statement());
            if self.i == before {
                items.push(self.error_statement());
            }
        }
    }

    fn statement(&mut self) -> Item {
        if self.peek() == L::Name {
            match self.word() {
                "if" => return self.if_statement(),
                "while" => {
                    let items = vec![
                        self.punct(),
                        self.parenthesized().with("condition"),
                        self.statement().with("body"),
                    ];
                    return node("while_statement", items);
                }
                "do" => {
                    let items = vec![
                        self.punct(),
                        self.statement().with("body"),
                        self.expect("while"),
                        self.parenthesized().with("condition"),
                        self.expect(";"),
                    ];
                    return node("do_statement", items);
                }
                "for" => return self.for_statement(),
                "try" => return self.try_statement(),
                "switch" => return self.switch_expression(),
                "return" => {
                    let mut items = vec![self.punct()];
                    if !self.at(";") {
                        items.push(self.expression());
                    }
                    items.push(self.expect(";"));
                    return node("return_statement", items);
                }
                w @ ("break" | "continue") => {
                    let kind = if w == "break" {
                        "break_statement"
                    } else {
                        "continue_statement"
                    };
                    let mut items = vec![self.punct()];
                    if self.is_ident(self.i) {
                        items.push(self.leaf("identifier"));
                    }
                    items.push(self.expect(";"));
                    return node(kind, items);
                }
                "throw" => {
                    let items = vec![self.punct(), self.expression(), self.expect(";")];
                    return node("throw_statement", items);
                }
                "assert" => {
                    let mut items = vec![self.punct(), self.expression()];
                    if self.at(":") {
                        items.push(self.punct());
                        items.push(self.expression());
                    }
                    items.push(self.expect(";"));
                    return node("assert_statement", items);
                }
                "synchronized" if self.is(self.i + 1, "(") => {
                    let items = vec![
                        self.punct(),
                        self.parenthesized(),
                        self.block().with("body"),
                    ];
                    return node("synchronized_statement", items);
                }
                "yield" if self.yield_at() => {
                    let items = vec![self.punct(), self.expression(), self.expect(";")];
                    return node("yield_statement", items);
                }
                "package" => return self.package_declaration(Vec::new()),
                "import" => return self.import_declaration(),
                "module" | "open" if self.module_at(self.i) => {
                    return self.module_declaration(Vec::new());
                }
                _ => {}
            }
            if self.is_ident(self.i) && self.is(self.i + 1, ":") {
                let items = vec![self.leaf("identifier"), self.punct(), self.statement()];
                return node("labeled_statement", items);
            }
        }
        if self.at("{") {
            return self.block();
        }
        if self.at(";") {
            return self.punct();
        }
        // Declarations: types, or (modifiers, a type and a name) a local
        // variable.
        let mods_end = self.scan_modifiers(self.i);
        if mods_end > self.i || self.is_type_keyword(self.i) {
            let save = self.i;
            if self.is(mods_end, "package") {
                let mut annotations = Vec::new();
                self.annotations(&mut annotations);
                return self.package_declaration(annotations);
            }
            if self.module_at(mods_end) {
                let mut annotations = Vec::new();
                self.annotations(&mut annotations);
                return self.module_declaration(annotations);
            }
            let mods = self.modifiers();
            if let Some(d) = self.type_declaration(mods) {
                return d;
            }
            self.i = save;
        }
        if self.declaration_at(self.i) {
            return self.local_variable_declaration();
        }
        let e = self.expression();
        let semi = self.expect(";");
        node("expression_statement", vec![e, semi])
    }

    fn is_type_keyword(&self, n: usize) -> bool {
        matches!(self.word_at(n), "class" | "interface" | "enum")
            || (self.word_at(n) == "record"
                && self.is_ident(n + 1)
                && (self.is(n + 2, "(") || self.is(n + 2, "<")))
            || (self.is(n, "@") && self.is(n + 1, "interface"))
    }

    /// At `yield`: a yield statement, not a name being assigned or called.
    fn yield_at(&self) -> bool {
        let n = self.i + 1;
        !matches!(
            self.kind_at(n),
            L::Op("=" | "." | "[" | ";" | "++" | "--" | "+=" | "-=" | "*=" | "/=" | ":" | "::")
        ) && self.kind_at(n) != L::Eof
    }

    fn module_at(&self, n: usize) -> bool {
        let n = if self.is(n, "open") { n + 1 } else { n };
        self.is(n, "module") && self.kind_at(n + 1) == L::Name && {
            let mut k = n + 2;
            while self.is(k, ".") && self.kind_at(k + 1) == L::Name {
                k += 2;
            }
            self.is(k, "{")
        }
    }

    fn local_variable_declaration(&mut self) -> Item {
        let mut items: Vec<Item> = self.modifiers().into_iter().collect();
        items.push(self.unannotated_type().with("type"));
        self.declarators(&mut items);
        items.push(self.expect(";"));
        node("local_variable_declaration", items)
    }

    fn package_declaration(&mut self, mut items: Vec<Item>) -> Item {
        items.push(self.punct());
        items.push(self.name());
        items.push(self.expect(";"));
        node("package_declaration", items)
    }

    fn import_declaration(&mut self) -> Item {
        let mut items = vec![self.punct()];
        if self.at("static") {
            items.push(self.punct());
        }
        items.push(self.name());
        if self.at(".") && self.is(self.i + 1, "*") {
            items.push(self.punct());
            items.push(node("asterisk", vec![self.punct()]));
        }
        items.push(self.expect(";"));
        node("import_declaration", items)
    }

    fn module_declaration(&mut self, mut items: Vec<Item>) -> Item {
        if self.at("open") {
            items.push(self.punct());
        }
        items.push(self.punct());
        items.push(self.name().with("name"));
        let mut body = vec![self.expect("{")];
        while !self.at("}") && !self.eof() {
            let before = self.i;
            let d = match self.word() {
                "requires" => {
                    let mut d = vec![self.punct()];
                    while matches!(self.word(), "transitive" | "static")
                        && self.kind_at(self.i + 1) == L::Name
                    {
                        d.push(node("requires_modifier", vec![self.punct()]).with("modifiers"));
                    }
                    d.push(self.name().with("module"));
                    d.push(self.expect(";"));
                    node("requires_module_directive", d)
                }
                w @ ("exports" | "opens") => {
                    let kind = if w == "exports" {
                        "exports_module_directive"
                    } else {
                        "opens_module_directive"
                    };
                    let mut d = vec![self.punct(), self.name().with("package")];
                    if self.at("to") {
                        d.push(self.punct());
                        d.push(self.name().with("modules"));
                        while self.at(",") {
                            d.push(self.punct());
                            d.push(self.name().with("modules"));
                        }
                    }
                    d.push(self.expect(";"));
                    node(kind, d)
                }
                "uses" => {
                    let d = vec![self.punct(), self.name().with("type"), self.expect(";")];
                    node("uses_module_directive", d)
                }
                "provides" => {
                    let mut d = vec![self.punct(), self.name().with("provided")];
                    d.push(self.expect("with"));
                    d.push(self.name());
                    while self.at(",") {
                        d.push(self.punct());
                        d.push(self.name().with("provider"));
                    }
                    d.push(self.expect(";"));
                    node("provides_module_directive", d)
                }
                _ => self.error_statement(),
            };
            body.push(d);
            if self.i == before {
                break;
            }
        }
        body.push(self.expect("}"));
        items.push(node("module_body", body).with("body"));
        node("module_declaration", items)
    }

    fn parenthesized(&mut self) -> Item {
        let items = vec![self.expect("("), self.expression(), self.expect(")")];
        node("parenthesized_expression", items)
    }

    fn if_statement(&mut self) -> Item {
        let mut items = vec![
            self.punct(),
            self.parenthesized().with("condition"),
            self.statement().with("consequence"),
        ];
        if self.at("else") {
            items.push(self.punct());
            items.push(self.statement().with("alternative"));
        }
        node("if_statement", items)
    }

    fn for_statement(&mut self) -> Item {
        let for_kw = self.punct();
        let open = self.expect("(");
        // `for (T x : xs)`
        let n = self.scan_modifiers(self.i);
        if let Some(k) = self.scan_type(n)
            && (self.is_ident(k) || self.word_at(k) == "_")
            && {
                let d = self.scan_dimensions(k + 1);
                self.is(d, ":")
            }
        {
            let mut items = vec![for_kw, open];
            items.extend(self.modifiers());
            items.push(self.unannotated_type().with("type"));
            items.push(self.variable_name());
            if self.at("[") {
                items.push(self.dimensions().with("dimensions"));
            }
            items.push(self.punct());
            items.push(self.expression().with("value"));
            items.push(self.expect(")"));
            items.push(self.statement().with("body"));
            return node("enhanced_for_statement", items);
        }
        let mut items = vec![for_kw, open];
        if self.declaration_at(self.i) {
            items.push(self.local_variable_declaration().with("init"));
        } else {
            while !self.at(";") && !self.eof() {
                let before = self.i;
                items.push(self.expression().with("init"));
                if self.i == before || !self.at(",") {
                    break;
                }
                items.push(self.punct());
            }
            items.push(self.expect(";"));
        }
        if !self.at(";") {
            items.push(self.expression().with("condition"));
        }
        items.push(self.expect(";"));
        while !self.at(")") && !self.eof() {
            let before = self.i;
            items.push(self.expression().with("update"));
            if self.i == before || !self.at(",") {
                break;
            }
            items.push(self.punct());
        }
        items.push(self.expect(")"));
        items.push(self.statement().with("body"));
        node("for_statement", items)
    }

    fn try_statement(&mut self) -> Item {
        let try_kw = self.punct();
        if self.at("(") {
            let mut res = vec![self.punct()];
            while !self.at(")") && !self.eof() {
                let before = self.i;
                res.push(self.resource());
                if self.i == before || !self.at(";") {
                    break;
                }
                res.push(self.punct());
            }
            res.push(self.expect(")"));
            let mut items = vec![
                try_kw,
                node("resource_specification", res).with("resources"),
                self.block().with("body"),
            ];
            self.catches(&mut items);
            return node("try_with_resources_statement", items);
        }
        let mut items = vec![try_kw, self.block().with("body")];
        self.catches(&mut items);
        node("try_statement", items)
    }

    fn resource(&mut self) -> Item {
        if self.declaration_at(self.i) {
            let mut items: Vec<Item> = self.modifiers().into_iter().collect();
            items.push(self.unannotated_type().with("type"));
            items.push(self.variable_name());
            if self.at("[") {
                items.push(self.dimensions().with("dimensions"));
            }
            items.push(self.expect("="));
            items.push(self.expression().with("value"));
            return node("resource", items);
        }
        let e = self.expression();
        node("resource", vec![e.without_field()])
    }

    fn catches(&mut self, items: &mut Vec<Item>) {
        while self.at("catch") {
            let mut c = vec![self.punct(), self.expect("(")];
            let mut p: Vec<Item> = self.modifiers().into_iter().collect();
            let mut types = vec![self.unannotated_type()];
            while self.at("|") {
                types.push(self.punct());
                types.push(self.unannotated_type());
            }
            p.push(node("catch_type", types));
            p.push(self.variable_name());
            if self.at("[") {
                p.push(self.dimensions().with("dimensions"));
            }
            c.push(node("catch_formal_parameter", p));
            c.push(self.expect(")"));
            c.push(self.block().with("body"));
            items.push(node("catch_clause", c));
        }
        if self.at("finally") {
            let f = vec![self.punct(), self.block()];
            items.push(node("finally_clause", f));
        }
    }

    fn switch_expression(&mut self) -> Item {
        let kw = self.punct();
        let cond = self.parenthesized().with("condition");
        let mut body = vec![self.expect("{")];
        let rules = self.switch_uses_rules();
        while !self.at("}") && !self.eof() {
            let before = self.i;
            if rules {
                let mut r = vec![self.switch_label(), self.expect("->")];
                r.push(if self.at("{") {
                    self.block()
                } else if self.at("throw") {
                    self.statement()
                } else {
                    let e = self.expression();
                    let semi = self.expect(";");
                    node("expression_statement", vec![e, semi])
                });
                body.push(node("switch_rule", r));
            } else {
                // A label with no statements after it is a group of its own,
                // as tree-sitter-java reads `case A: case B: f();`.
                let mut g = Vec::new();
                if self.at("case") || self.at("default") {
                    g.push(self.switch_label());
                    g.push(self.expect(":"));
                }
                if g.is_empty() {
                    body.push(self.error_statement());
                    continue;
                }
                while !self.at("}")
                    && !self.eof()
                    && !self.at("case")
                    && !(self.at("default")
                        && (self.is(self.i + 1, ":") || self.is(self.i + 1, "->")))
                {
                    let b = self.i;
                    g.push(self.statement());
                    if self.i == b {
                        g.push(self.error_statement());
                    }
                }
                body.push(node("switch_block_statement_group", g));
            }
            if self.i == before {
                body.push(self.error_statement());
            }
        }
        body.push(self.expect("}"));
        let items = vec![kw, cond, node("switch_block", body).with("body")];
        node("switch_expression", items)
    }

    /// Whether the switch block's first label ends in `->`.
    fn switch_uses_rules(&self) -> bool {
        let mut n = self.i;
        let mut depth = 0usize;
        loop {
            match self.kind_at(n) {
                L::Op("(" | "[" | "{") => depth += 1,
                L::Op(")" | "]" | "}") => {
                    if depth == 0 {
                        return false;
                    }
                    depth -= 1;
                }
                L::Op("->") if depth == 0 => return true,
                L::Op(":") if depth == 0 => return false,
                L::Eof => return false,
                _ => {}
            }
            n += 1;
        }
    }

    fn switch_label(&mut self) -> Item {
        if self.at("default") {
            return node("switch_label", vec![self.punct()]);
        }
        let mut items = vec![self.expect("case")];
        if self.pattern_at() {
            items.push(self.pattern());
        } else {
            loop {
                let before = self.i;
                if self.at("default") {
                    items.push(self.punct());
                } else {
                    items.push(self.expression_no_lambda());
                }
                if self.i == before || !self.at(",") {
                    break;
                }
                items.push(self.punct());
            }
        }
        if self.at("when") {
            let g = vec![self.punct(), self.expression()];
            items.push(node("guard", g));
        }
        node("switch_label", items)
    }

    /// After `case`: a type pattern (`String s`) or a record pattern
    /// (`Point(int x, var y)`).
    fn pattern_at(&self) -> bool {
        let Some(k) = self.scan_type(self.i) else {
            return false;
        };
        (self.is_ident(k) || self.word_at(k) == "_") && !self.is(k, "when")
            || (self.is(k, "(") && self.is_ident(self.i))
    }

    fn pattern(&mut self) -> Item {
        let p = self.pattern_inner();
        node("pattern", vec![p])
    }

    fn pattern_inner(&mut self) -> Item {
        let ty = self.unannotated_type();
        if self.at("(") {
            let body = self.record_pattern_body();
            return record_pattern(ty, body);
        }
        let name = if self.word() == "_" {
            self.leaf("identifier")
        } else {
            self.identifier()
        };
        node("type_pattern", vec![ty, name])
    }

    fn record_pattern_body(&mut self) -> Item {
        let mut items = vec![self.punct()];
        while !self.at(")") && !self.eof() {
            let before = self.i;
            if self.word() == "_" && (self.is(self.i + 1, ",") || self.is(self.i + 1, ")")) {
                let u = self.leaf("underscore_pattern");
                items.push(node("record_pattern_component", vec![u]));
            } else {
                let ty = self.unannotated_type();
                if self.at("(") {
                    let body = self.record_pattern_body();
                    items.push(record_pattern(ty, body));
                } else {
                    let name = self.identifier();
                    items.push(node("record_pattern_component", vec![ty, name]));
                }
            }
            if self.i == before || !self.at(",") {
                break;
            }
            items.push(self.punct());
        }
        items.push(self.expect(")"));
        node("record_pattern_body", items)
    }

    // ---- expressions -------------------------------------------------------

    pub fn expression(&mut self) -> Item {
        if self.lambda_at(self.i) {
            return self.lambda();
        }
        let left = self.ternary();
        if let Some((n, op)) = self.assignment_op() {
            let op = self.joined(n, op).with("operator");
            let right = self.expression().with("right");
            return node("assignment_expression", vec![left.with("left"), op, right]);
        }
        left
    }

    /// An expression that is not a lambda (a `case` label's, where `->`
    /// ends the label).
    fn expression_no_lambda(&mut self) -> Item {
        self.ternary()
    }

    /// The assignment operator here, and how many tokens it takes (`>>=`
    /// is `>` and `>=`).
    fn assignment_op(&self) -> Option<(usize, &'static str)> {
        let n = self.i;
        if self.is(n, ">") && self.adjacent(n) {
            if self.is(n + 1, ">=") {
                return Some((2, ">>="));
            }
            if self.is(n + 1, ">") && self.adjacent(n + 1) && self.is(n + 2, ">=") {
                return Some((3, ">>>="));
            }
            return None;
        }
        match self.kind_at(n) {
            L::Op(o) if ASSIGNMENTS.contains(&o) => Some((1, o)),
            _ => None,
        }
    }

    fn lambda(&mut self) -> Item {
        let params = if self.at("(") {
            let end = self.scan_balanced(self.i).unwrap_or(self.i);
            // All plain names: inferred parameters.
            let mut k = self.i + 1;
            let mut inferred = k < end - 1;
            while k < end - 1 {
                if !(self.kind_at(k) == L::Name && !KEYWORDS.contains(&self.word_at(k))) {
                    inferred = false;
                    break;
                }
                k += 1;
                if k < end - 1 {
                    if !self.is(k, ",") {
                        inferred = false;
                        break;
                    }
                    k += 1;
                }
            }
            if inferred {
                let mut items = vec![self.punct()];
                while !self.at(")") {
                    items.push(self.leaf("identifier"));
                    if self.at(",") {
                        items.push(self.punct());
                    }
                }
                items.push(self.punct());
                node("inferred_parameters", items)
            } else {
                self.formal_parameters()
            }
        } else {
            self.leaf("identifier")
        };
        let arrow = self.expect("->");
        let body = if self.at("{") {
            self.block()
        } else {
            self.expression()
        };
        node(
            "lambda_expression",
            vec![params.with("parameters"), arrow, body.with("body")],
        )
    }

    fn ternary(&mut self) -> Item {
        let cond = self.binary(4);
        if !self.at("?") {
            return cond;
        }
        let q = self.punct();
        let then = self.expression().with("consequence");
        let colon = self.expect(":");
        let alt = if self.lambda_at(self.i) {
            self.lambda()
        } else {
            self.ternary()
        };
        node(
            "ternary_expression",
            vec![
                cond.with("condition"),
                q,
                then,
                colon,
                alt.with("alternative"),
            ],
        )
    }

    /// The binary operator here: its precedence, its spelling and how
    /// many tokens it takes (`>>` is two `>`).
    fn binop(&self) -> Option<(u8, &'static str, usize)> {
        let n = self.i;
        if self.is(n, ">") {
            if self.adjacent(n) && self.is(n + 1, ">") {
                if self.adjacent(n + 1) && self.is(n + 2, ">") {
                    return Some((11, ">>>", 3));
                }
                if self.adjacent(n + 1) && self.is(n + 2, ">=") {
                    return None;
                }
                return Some((11, ">>", 2));
            }
            if self.adjacent(n) && self.is(n + 1, ">=") {
                return None;
            }
            return Some((10, ">", 1));
        }
        if self.is(n, "instanceof") {
            return Some((10, "instanceof", 1));
        }
        let L::Op(o) = self.kind_at(n) else {
            return None;
        };
        let prec = match o {
            "||" => 4,
            "&&" => 5,
            "|" => 6,
            "^" => 7,
            "&" => 8,
            "==" | "!=" => 9,
            "<" | "<=" | ">=" => 10,
            "<<" => 11,
            "+" | "-" => 12,
            "*" | "/" | "%" => 13,
            _ => return None,
        };
        Some((prec, o, 1))
    }

    fn binary(&mut self, min: u8) -> Item {
        let mut left = self.unary();
        while let Some((prec, op, n)) = self.binop() {
            if prec < min {
                break;
            }
            if op == "instanceof" {
                left = self.instanceof(left);
                continue;
            }
            let op = self.joined(n, op).with("operator");
            let right = self.binary(prec + 1).with("right");
            left = node("binary_expression", vec![left.with("left"), op, right]);
        }
        left
    }

    fn instanceof(&mut self, left: Item) -> Item {
        let mut items = vec![left.with("left"), self.punct()];
        if self.at("final") {
            items.push(self.punct());
        }
        let ty = self.ty();
        if self.at("(") {
            let body = self.record_pattern_body();
            items.push(record_pattern(ty, body).with("pattern"));
            return node("instanceof_expression", items);
        }
        items.push(ty.with("right"));
        if self.is_ident(self.i) && !matches!(self.word(), "instanceof") {
            items.push(self.leaf("identifier").with("name"));
        }
        node("instanceof_expression", items)
    }

    fn unary(&mut self) -> Item {
        if self.at("+") || self.at("-") || self.at("!") || self.at("~") {
            let op = self.punct().with("operator");
            let operand = self.unary().with("operand");
            return node("unary_expression", vec![op, operand]);
        }
        if self.at("++") || self.at("--") {
            let op = self.punct();
            let operand = self.unary();
            return node("update_expression", vec![op, operand]);
        }
        if self.at("(")
            && let Some(cast) = self.cast()
        {
            return cast;
        }
        let mut e = self.postfix();
        while self.at("++") || self.at("--") {
            let op = self.punct();
            e = node("update_expression", vec![e, op]);
        }
        e
    }

    /// A cast at `(`, if one starts here.
    fn cast(&mut self) -> Option<Item> {
        let n = self.i + 1;
        let mut k = self.scan_type(n)?;
        while self.is(k, "&") {
            k = self.scan_type(k + 1)?;
        }
        if !self.is(k, ")") {
            return None;
        }
        let after = k + 1;
        let primitive = PRIMITIVES.contains(&self.word_at(self.scan_annotations(n)))
            && self.scan_dimensions(n + 1) == k;
        let starts = match self.kind_at(after) {
            L::Name => {
                let w = self.word_at(after);
                !matches!(w, "instanceof")
            }
            L::Num(_) | L::Char | L::StrOpen => true,
            L::Op(o) => {
                matches!(o, "(" | "!" | "~") || (primitive && matches!(o, "+" | "-" | "++" | "--"))
            }
            _ => false,
        };
        if !starts {
            return None;
        }
        let mut items = vec![self.punct(), self.ty().with("type")];
        while self.at("&") {
            items.push(self.punct());
            items.push(self.ty().with("type"));
        }
        items.push(self.expect(")"));
        let value = if self.lambda_at(self.i) {
            self.lambda()
        } else {
            self.unary()
        };
        items.push(value.with("value"));
        Some(node("cast_expression", items))
    }

    fn postfix(&mut self) -> Item {
        let mut e = self.primary();
        loop {
            if self.at(".") {
                let n = self.i + 1;
                if self.is(n, "new") {
                    e = self.object_creation(Some(e));
                } else if self.is(n, "class") {
                    let t = to_type(e);
                    let dot = self.punct();
                    let kw = self.punct();
                    e = node("class_literal", vec![t, dot, kw]);
                } else if self.is(n, "this") {
                    let dot = self.punct();
                    let this = self.leaf("this").with("field");
                    e = node("field_access", vec![e.with("object"), dot, this]);
                } else if self.is(n, "super") {
                    // `X.super.m()` / `X.super.f`.
                    let mut items = vec![e.with("object"), self.punct(), self.leaf("super")];
                    items.push(self.expect("."));
                    if self.at("<") {
                        items.push(self.type_arguments().with("type_arguments"));
                    }
                    let name = self.identifier().with("name");
                    if self.at("(") {
                        items.push(name);
                        items.push(self.arguments().with("arguments"));
                        e = node("method_invocation", items);
                    } else {
                        items.push(name.without_field().with("field"));
                        e = node("field_access", items);
                    }
                } else if self.is(n, "<") {
                    let mut items = vec![e.with("object"), self.punct()];
                    items.push(self.type_arguments().with("type_arguments"));
                    items.push(self.identifier().with("name"));
                    items.push(self.arguments().with("arguments"));
                    e = node("method_invocation", items);
                } else if self.kind_at(n) == L::StrOpen {
                    let dot = self.punct();
                    let s = self.string_literal().with("template_argument");
                    e = node(
                        "template_expression",
                        vec![e.with("template_processor"), dot, s],
                    );
                } else if self.kind_at(n) == L::Name {
                    let dot = self.punct();
                    let name = self.leaf("identifier");
                    if self.at("(") {
                        let args = self.arguments().with("arguments");
                        e = node(
                            "method_invocation",
                            vec![e.with("object"), dot, name.with("name"), args],
                        );
                    } else {
                        e = node(
                            "field_access",
                            vec![e.with("object"), dot, name.with("field")],
                        );
                    }
                } else {
                    return e;
                }
            } else if self.at("[") {
                if self.is(self.i + 1, "]") {
                    // `String[]::new`, `int[].class`: a type.
                    let t = to_type(e);
                    e = self.array_type_of(t);
                    continue;
                }
                let open = self.punct();
                let index = self.expression().with("index");
                let close = self.expect("]");
                e = node("array_access", vec![e.with("array"), open, index, close]);
            } else if self.at("::") {
                let mut items = vec![e.without_field(), self.punct()];
                if self.at("<") {
                    items.push(self.type_arguments());
                }
                items.push(if self.at("new") {
                    self.punct()
                } else {
                    self.identifier()
                });
                e = node("method_reference", items);
            } else if self.at("<") && self.generic_reference_at() {
                // `List<String>::new`
                let t = to_type(e);
                let args = self.type_arguments();
                e = node("generic_type", vec![t, args]);
                e = self.array_type_of(e);
            } else {
                return e;
            }
        }
    }

    /// At `<` after a name: type arguments followed by `::`.
    fn generic_reference_at(&self) -> bool {
        self.scan_type_arguments(self.i)
            .is_some_and(|k| self.is(self.scan_dimensions(k), "::"))
    }

    fn primary(&mut self) -> Item {
        match self.peek() {
            L::Num(kind) => self.leaf(kind),
            L::Char => self.leaf("character_literal"),
            L::StrOpen => self.string_literal(),
            L::Op("(") => self.parenthesized(),
            // An annotated type before `::` or `.class`.
            L::Op("@") => self.ty(),
            L::Name => match self.word() {
                "true" => self.leaf("true"),
                "false" => self.leaf("false"),
                "null" => self.leaf("null_literal"),
                "this" => self.leaf("this"),
                "super" => self.leaf("super"),
                "new" => self.object_creation(None),
                "switch" => self.switch_expression(),
                w if PRIMITIVES.contains(&w) => {
                    // `int.class`, `int[]::new`.
                    let t = self.simple_type();
                    self.array_type_of(t)
                }
                _ if self.is_ident(self.i) => {
                    let name = self.leaf("identifier");
                    if self.at("(") {
                        let args = self.arguments().with("arguments");
                        return node("method_invocation", vec![name.with("name"), args]);
                    }
                    name
                }
                _ => self.missing("identifier", true),
            },
            _ => self.missing("identifier", true),
        }
    }

    /// `new …` (after `outer.`, when `outer` is given).
    fn object_creation(&mut self, outer: Option<Item>) -> Item {
        let mut items = Vec::new();
        if let Some(o) = outer {
            items.push(o.without_field());
            items.push(self.punct());
        }
        items.push(self.punct());
        let mut ann = Vec::new();
        self.annotations(&mut ann);
        if self.at("<") && items.len() <= 2 {
            items.append(&mut ann);
            items.push(self.type_arguments().with("type_arguments"));
            self.annotations(&mut items);
        }
        let ty = self.simple_type().with("type");
        let k = self.scan_annotations(self.i);
        if self.is(k, "[") {
            // An array.
            let mut a = vec![items.remove(items.len() - 1)];
            a.extend(ann);
            a.push(ty);
            let mut has_exprs = false;
            loop {
                let k = self.scan_annotations(self.i);
                if !self.is(k, "[") || self.is(k + 1, "]") {
                    break;
                }
                let mut d = Vec::new();
                self.annotations(&mut d);
                d.push(self.punct());
                d.push(self.expression());
                d.push(self.expect("]"));
                a.push(node("dimensions_expr", d).with("dimensions"));
                has_exprs = true;
            }
            let k = self.scan_annotations(self.i);
            if self.is(k, "[") && self.is(k + 1, "]") {
                a.push(self.dimensions().with("dimensions"));
            }
            if !has_exprs && self.at("{") {
                a.push(self.array_initializer().with("value"));
            }
            let mut out: Vec<Item> = items;
            out.extend(a);
            return node("array_creation_expression", out);
        }
        items.extend(ann);
        items.push(ty);
        items.push(self.arguments().with("arguments"));
        if self.at("{") {
            items.push(self.class_body());
        }
        node("object_creation_expression", items)
    }

    fn arguments(&mut self) -> Item {
        let mut items = vec![self.expect("(")];
        while !self.at(")") && !self.eof() {
            let before = self.i;
            items.push(self.expression());
            if self.i == before || !self.at(",") {
                break;
            }
            items.push(self.punct());
        }
        self.close(&mut items, ")");
        node("argument_list", items)
    }

    /// A list's closing `close`; tokens before it on the way, at the same
    /// nesting, are an `ERROR` inside the list.
    fn close(&mut self, items: &mut Vec<Item>, close: &'static str) {
        if !self.at(close) {
            let mut depth = 0usize;
            let mut j = self.i;
            let found = loop {
                match self.kind_at(j) {
                    L::Eof | L::Op(";" | "{" | "}") if depth == 0 => break false,
                    L::Op(o) if o == close && depth == 0 => break true,
                    L::Op("(" | "[") => depth += 1,
                    L::Op(")" | "]") if depth == 0 => break false,
                    L::Op(")" | "]") => depth -= 1,
                    L::Eof => break false,
                    _ => {}
                }
                j += 1;
            };
            if found {
                self.errors = true;
                let mut bad = Vec::new();
                while self.i < j {
                    let t = self.advance();
                    bad.push(Self::tok(t, "ERROR", false));
                }
                items.push(node("ERROR", bad));
            }
        }
        items.push(self.expect(close));
    }

    fn string_literal(&mut self) -> Item {
        let open = self.advance();
        let text_block = open.end - open.start == 3;
        let mut items = vec![Self::tok(
            open,
            if text_block { "\"\"\"" } else { "\"" },
            false,
        )];
        loop {
            match self.peek() {
                L::Frag(kind) => items.push(self.leaf(kind)),
                L::Escape => items.push(self.leaf("escape_sequence")),
                L::HiddenEscape => {
                    let t = self.advance();
                    items.push(Item::Tok(Tok {
                        kind: "",
                        named: false,
                        start: t.start,
                        end: t.end,
                        field: None,
                        hidden: true,
                        missing: false,
                        extra: false,
                    }));
                }
                L::Op("\\{") => {
                    let mut s = vec![self.punct(), self.expression()];
                    s.push(self.expect("}"));
                    items.push(node("string_interpolation", s));
                }
                L::StrClose => {
                    let t = self.advance();
                    items.push(Self::tok(
                        t,
                        if text_block { "\"\"\"" } else { "\"" },
                        false,
                    ));
                    break;
                }
                _ => {
                    items.push(self.missing(if text_block { "\"\"\"" } else { "\"" }, false));
                    break;
                }
            }
        }
        node("string_literal", items)
    }
}

/// A record pattern: its type is named by an `identifier` unless generic
/// (`identifier | generic_type` in tree-sitter-java's grammar).
fn record_pattern(ty: Item, body: Item) -> Item {
    let ty = if ty.kind() == "type_identifier" {
        ty.renamed("identifier")
    } else {
        ty
    };
    node("record_pattern", vec![ty, body])
}

/// An expression read where a type turns out to be (`a.b.C` before
/// `.class` or `[]`): identifiers as type identifiers, field accesses as
/// scoped type identifiers.
fn to_type(e: Item) -> Item {
    match e {
        Item::Tok(t) if t.kind == "identifier" => Item::Tok(Tok {
            kind: "type_identifier",
            field: None,
            ..t
        }),
        Item::Node(n) if n.kind == "field_access" => {
            let items = n
                .items
                .into_iter()
                .map(|i| match i.field() {
                    Some("object") => to_type(i.without_field()),
                    Some("field") => to_type(i.without_field()),
                    _ => i,
                })
                .collect();
            node("scoped_type_identifier", items)
        }
        other => other.without_field(),
    }
}
