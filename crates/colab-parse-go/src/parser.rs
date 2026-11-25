//! Go, read by recursive descent into a tree of [`Item`]s: the grammar of
//! the Go specification, with kinds and fields named as in `names.rs`.
//! A construct the parser cannot read becomes an `ERROR` node holding its
//! tokens, and reading resumes at the next statement or declaration; a
//! token that should be there and is not is a missing one.

use crate::lexer::{L, Lx};

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
    /// Whitespace that belongs to the node it is in (a statement's
    /// newline).
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

    fn items(&self) -> &[Item] {
        match self {
            Item::Node(n) => &n.items,
            Item::Tok(_) => &[],
        }
    }
}

fn node(kind: &'static str, items: Vec<Item>) -> Item {
    Item::Node(Node {
        kind,
        field: None,
        items,
    })
}

/// Kinds that are types, read where an expression was expected.
const TYPE_KINDS: &[&str] = &[
    "slice_type",
    "array_type",
    "implicit_length_array_type",
    "map_type",
    "channel_type",
    "function_type",
    "struct_type",
    "interface_type",
    "pointer_type",
    "generic_type",
    "qualified_type",
    "parenthesized_type",
    "negated_type",
    "type_identifier",
];

pub(crate) struct Parser<'s> {
    src: &'s str,
    toks: Vec<Lx>,
    i: usize,
    pub errors: bool,
    /// In a statement header (`if`, `for`, `switch`), where `T {` is the
    /// header ending, not a composite literal.
    no_literal: bool,
}

impl<'s> Parser<'s> {
    pub fn new(src: &'s str, toks: Vec<Lx>) -> Self {
        Self {
            src,
            toks,
            i: 0,
            errors: false,
            no_literal: false,
        }
    }

    fn peek(&self) -> L {
        self.toks[self.i].k
    }

    fn peek_at(&self, n: usize) -> L {
        self.toks.get(self.i + n).map_or(L::Eof, |t| t.k)
    }

    fn at(&self, s: &str) -> bool {
        matches!(self.peek(), L::Op(o) | L::Kw(o) if o == s)
    }

    fn at_n(&self, n: usize, s: &str) -> bool {
        matches!(self.peek_at(n), L::Op(o) | L::Kw(o) if o == s)
    }

    fn eof(&self) -> bool {
        self.peek() == L::Eof
    }

    fn is_term(&self) -> bool {
        self.peek() == L::Term || self.at(";")
    }

    fn text(&self) -> &'s str {
        let t = self.toks[self.i];
        &self.src[t.start..t.end]
    }

    fn tok(&mut self, kind: &'static str, named: bool) -> Item {
        let t = self.toks[self.i];
        if t.k != L::Eof {
            self.i += 1;
        }
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

    /// The current token as the parser names it by default.
    fn take(&mut self) -> Item {
        let t = self.toks[self.i];
        match t.k {
            L::Ident => self.tok("identifier", true),
            L::Kw(w) if matches!(w, "nil" | "true" | "false" | "iota") => self.tok(w, true),
            L::Kw(w) | L::Op(w) => self.tok(w, false),
            L::Int => self.tok("int_literal", true),
            L::Float => self.tok("float_literal", true),
            L::Imag => self.tok("imaginary_literal", true),
            L::Rune => self.tok("rune_literal", true),
            L::Str { closed } => self.string(t, closed),
            L::Raw { closed } => self.raw_string(t, closed),
            L::Term => {
                self.i += 1;
                Item::Tok(Tok {
                    kind: "\n",
                    named: false,
                    start: t.start,
                    end: t.end,
                    field: None,
                    hidden: true,
                    missing: false,
                    extra: false,
                })
            }
            L::Bad => {
                self.errors = true;
                let tok = self.tok("ERROR", true);
                node("ERROR", vec![tok])
            }
            L::Eof => self.missing("identifier", true),
        }
    }

    /// A token that should be here and is not.
    fn missing(&mut self, kind: &'static str, named: bool) -> Item {
        self.errors = true;
        let at = self.toks[self.i].start;
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

    /// The keyword or punctuation `s`, or a missing one.
    fn expect(&mut self, s: &'static str) -> Item {
        if self.at(s) {
            self.take()
        } else {
            self.missing(s, false)
        }
    }

    /// Whether the current token can be a name: an identifier, or one of
    /// the predeclared names Go lets a program redeclare (`nil` as a
    /// field).
    fn at_name(&self) -> bool {
        matches!(
            self.peek(),
            L::Ident | L::Kw("nil" | "true" | "false" | "iota")
        )
    }

    /// An identifier, named `kind`, or a missing one.
    fn ident(&mut self, kind: &'static str) -> Item {
        if self.at_name() {
            self.tok(kind, true)
        } else {
            self.missing(kind, true)
        }
    }

    fn string(&mut self, t: Lx, closed: bool) -> Item {
        self.i += 1;
        let tok = |kind: &'static str, named: bool, start: usize, end: usize| {
            Item::Tok(Tok {
                kind,
                named,
                start,
                end,
                field: None,
                hidden: false,
                missing: false,
                extra: false,
            })
        };
        let b = self.src.as_bytes();
        let mut items = vec![tok("\"", false, t.start, t.start + 1)];
        let body_end = if closed { t.end - 1 } else { t.end };
        let mut j = t.start + 1;
        while j < body_end {
            if b[j] == b'\\' {
                let end = escape(b, j).min(body_end);
                items.push(tok("escape_sequence", true, j, end));
                j = end;
            } else {
                let mut k = j;
                while k < body_end && b[k] != b'\\' {
                    k += 1;
                }
                items.push(tok("interpreted_string_literal_content", true, j, k));
                j = k;
            }
        }
        if closed {
            items.push(tok("\"", false, t.end - 1, t.end));
        } else {
            self.errors = true;
            items.push(Item::Tok(Tok {
                kind: "\"",
                named: false,
                start: t.end,
                end: t.end,
                field: None,
                hidden: false,
                missing: true,
                extra: false,
            }));
        }
        node("interpreted_string_literal", items)
    }

    fn raw_string(&mut self, t: Lx, closed: bool) -> Item {
        self.i += 1;
        let tok = |kind: &'static str, named: bool, start: usize, end: usize, missing: bool| {
            Item::Tok(Tok {
                kind,
                named,
                start,
                end,
                field: None,
                hidden: false,
                missing,
                extra: false,
            })
        };
        let body_end = if closed { t.end - 1 } else { t.end };
        let mut items = vec![tok("`", false, t.start, t.start + 1, false)];
        // The content is there even when empty: `[^`]*` matches nothing.
        items.push(tok(
            "raw_string_literal_content",
            true,
            t.start + 1,
            body_end,
            false,
        ));
        if closed {
            items.push(tok("`", false, t.end - 1, t.end, false));
        } else {
            self.errors = true;
            items.push(tok("`", false, t.end, t.end, true));
        }
        node("raw_string_literal", items)
    }

    /// A newline before the current token, taken as the terminator a list
    /// needs where Go's rule inserted none (after `L: ;`): the last one
    /// outside comments in the gap before the token.
    fn implicit_term(&mut self) -> Option<Item> {
        if self.i == 0 {
            return None;
        }
        let (from, to) = (self.toks[self.i - 1].end, self.toks[self.i].start);
        let gap = &self.src.as_bytes()[from..to];
        let mut last = None;
        let mut k = 0;
        while k < gap.len() {
            if gap[k..].starts_with(b"//") {
                while k < gap.len() && gap[k] != b'\n' {
                    k += 1;
                }
                continue;
            }
            if gap[k..].starts_with(b"/*") {
                k += 2;
                while k < gap.len() && !gap[k..].starts_with(b"*/") {
                    k += 1;
                }
                k += 2;
                continue;
            }
            if gap[k] == b'\n' {
                last = Some(from + k);
            }
            k += 1;
        }
        let at = last?;
        Some(Item::Tok(Tok {
            kind: "\n",
            named: false,
            start: at,
            end: at + 1,
            field: None,
            hidden: true,
            missing: false,
            extra: false,
        }))
    }

    /// The tokens up to the next statement or declaration, as an error.
    fn recover(&mut self, stop: &[&str]) -> Option<Item> {
        let mut items = Vec::new();
        let mut depth = 0usize;
        while !self.eof() {
            if depth == 0 && (self.is_term() || stop.iter().any(|s| self.at(s))) {
                break;
            }
            if self.at("{") || self.at("(") || self.at("[") {
                depth += 1;
            } else if (self.at("}") || self.at(")") || self.at("]")) && depth > 0 {
                depth -= 1;
            } else if self.at("}") || self.at(")") || self.at("]") {
                break;
            }
            items.push(self.take());
        }
        if items.is_empty() {
            return None;
        }
        self.errors = true;
        Some(node("ERROR", items))
    }

    // ----- the file ------------------------------------------------------

    pub fn source_file(&mut self) -> Item {
        let mut items = Vec::new();
        while !self.eof() {
            if self.is_term() {
                items.push(self.take());
                continue;
            }
            let before = self.i;
            let item = match self.peek() {
                L::Kw("package") => {
                    let kw = self.take();
                    let name = self.ident("package_identifier");
                    node("package_clause", vec![kw, name])
                }
                L::Kw("import") => self.import_declaration(),
                L::Kw("func") if self.at_n(1, "(") => self.method_declaration(),
                L::Kw("func")
                    if matches!(
                        self.peek_at(1),
                        L::Ident | L::Kw("nil" | "true" | "false" | "iota")
                    ) =>
                {
                    self.function_declaration()
                }
                _ => self.statement(),
            };
            items.push(item);
            if !self.eof() && !self.is_term() {
                if self.i > before
                    && let Some(nl) = self.implicit_term()
                {
                    items.push(nl);
                    continue;
                }
                if let Some(error) = self.recover(&[]) {
                    items.push(error);
                }
                if !self.eof() && !self.is_term() && self.i == before {
                    let bad = self.take();
                    items.push(node("ERROR", vec![bad]));
                    self.errors = true;
                }
                if !self.eof() && !self.is_term() {
                    let bad = self.take();
                    items.push(node("ERROR", vec![bad]));
                    self.errors = true;
                }
            }
        }
        node("source_file", items)
    }

    fn import_declaration(&mut self) -> Item {
        let mut items = vec![self.take()];
        if self.at("(") {
            let mut list = vec![self.take()];
            while !self.eof() && !self.at(")") {
                if self.is_term() {
                    list.push(self.take());
                    continue;
                }
                let before = self.i;
                list.push(self.import_spec());
                if self.i == before {
                    if let Some(e) = self.recover(&[")"]) {
                        list.push(e);
                    } else {
                        break;
                    }
                }
            }
            list.push(self.expect(")"));
            items.push(node("import_spec_list", list));
        } else {
            items.push(self.import_spec());
        }
        node("import_declaration", items)
    }

    fn import_spec(&mut self) -> Item {
        let mut items = Vec::new();
        match self.peek() {
            L::Op(".") => {
                let dot = self.take();
                items.push(node("dot", vec![dot]).with("name"));
            }
            L::Ident if self.text() == "_" => {
                items.push(self.tok("blank_identifier", true).with("name"));
            }
            L::Ident => items.push(self.tok("package_identifier", true).with("name")),
            _ => {}
        }
        items.push(self.string_literal().with("path"));
        node("import_spec", items)
    }

    fn string_literal(&mut self) -> Item {
        match self.peek() {
            L::Str { .. } | L::Raw { .. } => self.take(),
            _ => self.missing("interpreted_string_literal", true),
        }
    }

    fn function_declaration(&mut self) -> Item {
        let mut items = vec![self.take(), self.ident("identifier").with("name")];
        if self.at("[") {
            items.push(self.type_parameter_list().with("type_parameters"));
        }
        items.push(self.parameter_list().with("parameters"));
        if let Some(result) = self.result() {
            items.push(result);
        }
        if self.at("{") {
            items.push(self.block().with("body"));
        }
        node("function_declaration", items)
    }

    fn method_declaration(&mut self) -> Item {
        let mut items = vec![
            self.take(),
            self.parameter_list().with("receiver"),
            self.ident("field_identifier").with("name"),
        ];
        items.push(self.parameter_list().with("parameters"));
        if let Some(result) = self.result() {
            items.push(result);
        }
        if self.at("{") {
            items.push(self.block().with("body"));
        }
        node("method_declaration", items)
    }

    /// A function's result: a parameter list, or a type.
    fn result(&mut self) -> Option<Item> {
        if self.at("(") {
            return Some(self.parameter_list().with("result"));
        }
        if self.type_starts() && !self.at("(") {
            return Some(self.ty().with("result"));
        }
        None
    }

    fn type_starts(&self) -> bool {
        self.type_starts_at(0)
    }

    fn type_starts_at(&self, n: usize) -> bool {
        match self.peek_at(n) {
            L::Ident => true,
            L::Op(o) => matches!(o, "*" | "[" | "(" | "<-" | "~"),
            L::Kw(k) => matches!(k, "map" | "chan" | "func" | "struct" | "interface"),
            _ => false,
        }
    }

    /// The index of the token after the `]` matching the `[` at `n`.
    fn after_brackets(&self, n: usize) -> usize {
        let mut depth = 0usize;
        let mut k = n;
        loop {
            match self.peek_at(k) {
                L::Op("[") => depth += 1,
                L::Op("]") => {
                    depth -= 1;
                    if depth == 0 {
                        return k + 1;
                    }
                }
                L::Eof => return k,
                _ => {}
            }
            k += 1;
        }
    }

    // ----- parameters ----------------------------------------------------

    fn parameter_list(&mut self) -> Item {
        enum Elem {
            Bare(Item),
            Named(Item, Item),
            Type(Item),
            Variadic(Option<Item>, Item, Item),
        }
        let open = self.expect("(");
        let mut elems: Vec<(Elem, Option<Item>)> = Vec::new();
        while !self.eof() && !self.at(")") {
            let before = self.i;
            let elem = if self.at("...") {
                let dots = self.take();
                Elem::Variadic(None, dots, self.ty())
            } else if self.at_name() {
                let next = self.peek_at(1);
                if matches!(next, L::Op("," | ")")) {
                    Elem::Bare(self.tok("identifier", true))
                } else if next == L::Op(".") {
                    Elem::Type(self.ty())
                } else if next == L::Op("...") {
                    let name = self.tok("identifier", true);
                    let dots = self.take();
                    Elem::Variadic(Some(name), dots, self.ty())
                } else if next == L::Op("[") {
                    let after = self.after_brackets(1);
                    let named = self.at_n(2, "]") || self.type_starts_at(after);
                    if named {
                        let name = self.tok("identifier", true);
                        Elem::Named(name, self.ty())
                    } else {
                        Elem::Type(self.ty())
                    }
                } else if self.type_starts_at(1) {
                    let name = self.tok("identifier", true);
                    Elem::Named(name, self.ty())
                } else {
                    Elem::Bare(self.tok("identifier", true))
                }
            } else {
                Elem::Type(self.ty())
            };
            let comma = self.at(",").then(|| self.take());
            let done = comma.is_none();
            elems.push((elem, comma));
            if self.i == before {
                break;
            }
            if done {
                break;
            }
        }
        let named_mode = elems
            .iter()
            .any(|(e, _)| matches!(e, Elem::Named(..) | Elem::Variadic(Some(_), _, _)));
        let mut items = vec![open];
        let mut pending: Vec<Item> = Vec::new();
        for (elem, comma) in elems {
            match elem {
                Elem::Bare(id) if named_mode => {
                    pending.push(id.with("name"));
                    if let Some(c) = comma {
                        pending.push(c);
                    }
                    continue;
                }
                Elem::Bare(id) => {
                    let ty = id.renamed("type_identifier").with("type");
                    items.push(node("parameter_declaration", vec![ty]));
                }
                Elem::Named(name, ty) => {
                    let mut decl = std::mem::take(&mut pending);
                    decl.push(name.with("name"));
                    decl.push(ty.with("type"));
                    items.push(node("parameter_declaration", decl));
                }
                Elem::Type(ty) => {
                    let mut decl = std::mem::take(&mut pending);
                    decl.push(ty.with("type"));
                    items.push(node("parameter_declaration", decl));
                }
                Elem::Variadic(name, dots, ty) => {
                    let mut decl = Vec::new();
                    if let Some(n) = name {
                        decl.push(n.with("name"));
                    }
                    decl.push(dots);
                    decl.push(ty.with("type"));
                    items.extend(std::mem::take(&mut pending));
                    items.push(node("variadic_parameter_declaration", decl));
                }
            }
            if let Some(c) = comma {
                items.push(c);
            }
        }
        // Names with no type after them (`(a, b)` where a type was meant).
        for p in pending {
            items.push(p);
        }
        items.push(self.expect(")"));
        node("parameter_list", items)
    }

    fn type_parameter_list(&mut self) -> Item {
        let mut items = vec![self.expect("[")];
        let mut names: Vec<Item> = Vec::new();
        while !self.eof() && !self.at("]") {
            let before = self.i;
            names.push(self.ident("identifier").with("name"));
            if self.at(",") {
                names.push(self.take());
                continue;
            }
            let constraint = self.type_elem().renamed("type_constraint").with("type");
            let mut decl = std::mem::take(&mut names);
            decl.push(constraint);
            items.push(node("type_parameter_declaration", decl));
            if self.at(",") {
                items.push(self.take());
            } else {
                break;
            }
            if self.i == before {
                break;
            }
        }
        items.extend(names);
        items.push(self.expect("]"));
        node("type_parameter_list", items)
    }

    // ----- types ---------------------------------------------------------

    fn ty(&mut self) -> Item {
        match self.peek() {
            L::Ident => {
                let named = if self.at_n(1, ".") && self.peek_at(2) == L::Ident {
                    let pkg = self.tok("package_identifier", true).with("package");
                    let dot = self.take();
                    let name = self.tok("type_identifier", true).with("name");
                    node("qualified_type", vec![pkg, dot, name])
                } else {
                    self.tok("type_identifier", true)
                };
                if self.at("[") && !self.at_n(1, "]") {
                    let args = self.type_arguments().with("type_arguments");
                    node("generic_type", vec![named.with("type"), args])
                } else {
                    named
                }
            }
            L::Op("*") => {
                let star = self.take();
                node("pointer_type", vec![star, self.ty()])
            }
            L::Op("~") => {
                let tilde = self.take();
                node("negated_type", vec![tilde, self.ty()])
            }
            L::Op("(") => {
                let open = self.take();
                let inner = self.ty();
                node("parenthesized_type", vec![open, inner, self.expect(")")])
            }
            L::Op("[") => self.bracket_type(),
            L::Op("<-") => {
                let arrow = self.take();
                let chan = self.expect("chan");
                node("channel_type", vec![arrow, chan, self.ty().with("value")])
            }
            L::Kw("map") => {
                let kw = self.take();
                let open = self.expect("[");
                let key = self.ty().with("key");
                let close = self.expect("]");
                node(
                    "map_type",
                    vec![kw, open, key, close, self.ty().with("value")],
                )
            }
            L::Kw("chan") => {
                let mut items = vec![self.take()];
                if self.at("<-") {
                    items.push(self.take());
                }
                items.push(self.ty().with("value"));
                node("channel_type", items)
            }
            L::Kw("func") => {
                let mut items = vec![self.take(), self.parameter_list().with("parameters")];
                if let Some(r) = self.result() {
                    items.push(r);
                }
                node("function_type", items)
            }
            L::Kw("struct") => {
                let kw = self.take();
                node("struct_type", vec![kw, self.field_declaration_list()])
            }
            L::Kw("interface") => self.interface_type(),
            // `case nil:` in a type switch: a type named `nil`.
            L::Kw("nil" | "true" | "false" | "iota") => self.tok("type_identifier", true),
            _ => self.missing("type_identifier", true),
        }
    }

    fn bracket_type(&mut self) -> Item {
        let open = self.take();
        if self.at("]") {
            let close = self.take();
            return node("slice_type", vec![open, close, self.ty().with("element")]);
        }
        if self.at("...") {
            let dots = self.take();
            let close = self.expect("]");
            return node(
                "implicit_length_array_type",
                vec![open, dots, close, self.ty().with("element")],
            );
        }
        let saved = self.no_literal;
        self.no_literal = false;
        let length = self.expr().with("length");
        self.no_literal = saved;
        let close = self.expect("]");
        node(
            "array_type",
            vec![open, length, close, self.ty().with("element")],
        )
    }

    fn type_arguments(&mut self) -> Item {
        let mut items = vec![self.expect("[")];
        while !self.eof() && !self.at("]") {
            let before = self.i;
            items.push(self.type_elem());
            if self.at(",") {
                items.push(self.take());
            } else {
                break;
            }
            if self.i == before {
                break;
            }
        }
        items.push(self.expect("]"));
        node("type_arguments", items)
    }

    fn type_elem(&mut self) -> Item {
        let mut items = vec![self.ty()];
        while self.at("|") {
            items.push(self.take());
            items.push(self.ty());
        }
        node("type_elem", items)
    }

    fn field_declaration_list(&mut self) -> Item {
        let mut items = vec![self.expect("{")];
        while !self.eof() && !self.at("}") {
            if self.is_term() {
                items.push(self.take());
                continue;
            }
            let before = self.i;
            items.push(self.field_declaration());
            if self.i == before || (!self.is_term() && !self.at("}")) {
                match self.recover(&["}"]) {
                    Some(e) => items.push(e),
                    None if self.i == before => break,
                    None => {}
                }
            }
        }
        items.push(self.expect("}"));
        node("field_declaration_list", items)
    }

    fn field_declaration(&mut self) -> Item {
        let mut items = Vec::new();
        let embedded = match self.peek() {
            L::Op("*") => true,
            L::Ident | L::Kw("nil" | "true" | "false" | "iota") => match self.peek_at(1) {
                L::Op(".") => true,
                L::Op("[") => {
                    let after = self.after_brackets(1);
                    !(self.at_n(2, "]") || self.type_starts_at(after))
                }
                L::Op(",") => false,
                _ => !self.type_starts_at(1),
            },
            _ => false,
        };
        if embedded {
            if self.at("*") {
                items.push(self.take());
            }
            items.push(self.ty().with("type"));
        } else {
            items.push(self.ident("field_identifier").with("name"));
            while self.at(",") {
                items.push(self.take());
                items.push(self.ident("field_identifier").with("name"));
            }
            items.push(self.ty().with("type"));
        }
        if matches!(self.peek(), L::Str { .. } | L::Raw { .. }) {
            items.push(self.take().with("tag"));
        }
        node("field_declaration", items)
    }

    fn interface_type(&mut self) -> Item {
        let mut items = vec![self.take(), self.expect("{")];
        while !self.eof() && !self.at("}") {
            if self.is_term() {
                items.push(self.take());
                continue;
            }
            let before = self.i;
            if self.peek() == L::Ident && self.at_n(1, "(") {
                let mut elem = vec![
                    self.tok("field_identifier", true).with("name"),
                    self.parameter_list().with("parameters"),
                ];
                if let Some(r) = self.result() {
                    elem.push(r);
                }
                items.push(node("method_elem", elem));
            } else {
                items.push(self.type_elem());
            }
            if self.i == before {
                match self.recover(&["}"]) {
                    Some(e) => items.push(e),
                    None => break,
                }
            }
        }
        items.push(self.expect("}"));
        node("interface_type", items)
    }

    // ----- declarations --------------------------------------------------

    fn const_declaration(&mut self) -> Item {
        let mut items = vec![self.take()];
        if self.at("(") {
            items.push(self.take());
            while !self.eof() && !self.at(")") {
                if self.is_term() {
                    items.push(self.take());
                    continue;
                }
                let before = self.i;
                items.push(self.const_spec());
                if self.i == before {
                    match self.recover(&[")"]) {
                        Some(e) => items.push(e),
                        None => break,
                    }
                }
            }
            items.push(self.expect(")"));
        } else {
            items.push(self.const_spec());
        }
        node("const_declaration", items)
    }

    fn const_spec(&mut self) -> Item {
        // `field('name', commaSep1(…))`: the commas are in the field too.
        let mut items = vec![self.ident("identifier").with("name")];
        while self.at(",") {
            items.push(self.take().with("name"));
            items.push(self.ident("identifier").with("name"));
        }
        if !self.is_term() && !self.at(")") && !self.at("=") && self.type_starts() {
            items.push(self.ty().with("type"));
        }
        if self.at("=") {
            items.push(self.take());
            items.push(self.expression_list().with("value"));
        }
        node("const_spec", items)
    }

    fn var_declaration(&mut self) -> Item {
        let mut items = vec![self.take()];
        if self.at("(") {
            let mut list = vec![self.take()];
            while !self.eof() && !self.at(")") {
                if self.is_term() {
                    list.push(self.take());
                    continue;
                }
                let before = self.i;
                list.push(self.var_spec());
                if self.i == before {
                    match self.recover(&[")"]) {
                        Some(e) => list.push(e),
                        None => break,
                    }
                }
            }
            list.push(self.expect(")"));
            items.push(node("var_spec_list", list));
        } else {
            items.push(self.var_spec());
        }
        node("var_declaration", items)
    }

    fn var_spec(&mut self) -> Item {
        let mut items = vec![self.ident("identifier").with("name")];
        while self.at(",") {
            items.push(self.take());
            items.push(self.ident("identifier").with("name"));
        }
        if !self.at("=") {
            items.push(self.ty().with("type"));
        }
        if self.at("=") {
            items.push(self.take());
            items.push(self.expression_list().with("value"));
        }
        node("var_spec", items)
    }

    fn type_declaration(&mut self) -> Item {
        let mut items = vec![self.take()];
        if self.at("(") {
            items.push(self.take());
            while !self.eof() && !self.at(")") {
                if self.is_term() {
                    items.push(self.take());
                    continue;
                }
                let before = self.i;
                items.push(self.type_spec());
                if self.i == before {
                    match self.recover(&[")"]) {
                        Some(e) => items.push(e),
                        None => break,
                    }
                }
            }
            items.push(self.expect(")"));
        } else {
            items.push(self.type_spec());
        }
        node("type_declaration", items)
    }

    fn type_spec(&mut self) -> Item {
        let mut items = vec![self.ident("type_identifier").with("name")];
        // `type T[P any] …`: a parameter list, where `type A [N]int` is an
        // array: a name then something other than `]`.
        if self.at("[")
            && self.peek_at(1) == L::Ident
            && !self.at_n(2, "]")
            && (matches!(
                self.peek_at(2),
                L::Ident
                    | L::Op("," | "~" | "[")
                    | L::Kw("interface" | "func" | "map" | "chan" | "struct")
            ))
        {
            items.push(self.type_parameter_list().with("type_parameters"));
        }
        if self.at("=") {
            items.push(self.take());
            items.push(self.ty().with("type"));
            return node("type_alias", items);
        }
        items.push(self.ty().with("type"));
        node("type_spec", items)
    }

    // ----- statements ----------------------------------------------------

    fn block(&mut self) -> Item {
        let saved = self.no_literal;
        self.no_literal = false;
        let mut items = vec![self.expect("{")];
        if let Some(list) = self.statement_list() {
            items.push(list);
        }
        items.push(self.expect("}"));
        self.no_literal = saved;
        node("block", items)
    }

    fn statement_list(&mut self) -> Option<Item> {
        let mut items = Vec::new();
        loop {
            if self.eof() || self.at("}") || self.at("case") || self.at("default") {
                break;
            }
            if self.is_term() {
                items.push(self.take());
                continue;
            }
            let before = self.i;
            items.push(self.statement());
            if !self.eof()
                && !self.is_term()
                && !self.at("}")
                && !self.at("case")
                && !self.at("default")
            {
                if let Some(nl) = (self.i > before).then(|| self.implicit_term()).flatten() {
                    items.push(nl);
                } else if let Some(e) = self.recover(&["}", "case", "default"]) {
                    items.push(e);
                }
            }
            if self.i == before {
                let bad = self.take();
                self.errors = true;
                items.push(node("ERROR", vec![bad]));
            }
        }
        (!items.is_empty()).then(|| node("statement_list", items))
    }

    fn statement(&mut self) -> Item {
        match self.peek() {
            L::Kw("const") => self.const_declaration(),
            L::Kw("var") => self.var_declaration(),
            L::Kw("type") => self.type_declaration(),
            L::Kw("return") => {
                let mut items = vec![self.take()];
                if !self.statement_ends() {
                    items.push(self.expression_list());
                }
                node("return_statement", items)
            }
            L::Kw("go") => {
                let kw = self.take();
                node("go_statement", vec![kw, self.expr()])
            }
            L::Kw("defer") => {
                let kw = self.take();
                node("defer_statement", vec![kw, self.expr()])
            }
            L::Kw("if") => self.if_statement(),
            L::Kw("for") => self.for_statement(),
            L::Kw("switch") => self.switch_statement(),
            L::Kw("select") => self.select_statement(),
            L::Kw("fallthrough") => {
                let kw = self.take();
                node("fallthrough_statement", vec![kw])
            }
            L::Kw(k @ ("break" | "continue")) => {
                let mut items = vec![self.take()];
                if self.peek() == L::Ident {
                    items.push(self.tok("label_name", true));
                }
                node(
                    if k == "break" {
                        "break_statement"
                    } else {
                        "continue_statement"
                    },
                    items,
                )
            }
            L::Kw("goto") => {
                let kw = self.take();
                node("goto_statement", vec![kw, self.ident("label_name")])
            }
            L::Op("{") => self.block(),
            L::Op(";") => {
                let semi = self.take();
                node("empty_statement", vec![semi])
            }
            L::Ident if self.at_n(1, ":") => {
                let label = self.tok("label_name", true).with("label");
                let colon = self.take();
                let mut items = vec![label, colon];
                // `L: ;` labels an empty statement.
                if self.at(";") || !self.statement_ends() {
                    items.push(self.statement());
                }
                node("labeled_statement", items)
            }
            _ => self.simple_statement(false),
        }
    }

    fn statement_ends(&self) -> bool {
        self.eof() || self.is_term() || self.at("}") || self.at("case") || self.at("default")
    }

    /// A simple statement; with `range_ok`, `k, v := range x` comes back as
    /// a `range_clause`.
    fn simple_statement(&mut self, range_ok: bool) -> Item {
        if range_ok && self.at("range") {
            let kw = self.take();
            return node("range_clause", vec![kw, self.expr().with("right")]);
        }
        let mut list = self.expr_items();
        let single = list.len() == 1;
        match self.peek() {
            L::Op(":=") | L::Op("=") if range_ok && self.at_n(1, "range") => {
                let op = self.take();
                let kw = self.take();
                let right = self.expr().with("right");
                let left = node("expression_list", list).with("left");
                node("range_clause", vec![left, op, kw, right])
            }
            L::Op(":=") => {
                let op = self.take();
                let right = self.expression_list().with("right");
                let left = node("expression_list", list).with("left");
                node("short_var_declaration", vec![left, op, right])
            }
            L::Op(o) if is_assignment(o) => {
                let op = self.take().with("operator");
                let right = self.expression_list().with("right");
                let left = node("expression_list", list).with("left");
                node("assignment_statement", vec![left, op, right])
            }
            L::Op(o @ ("++" | "--")) if single => {
                let op = self.take();
                let e = list.remove(0);
                node(
                    if o == "++" {
                        "inc_statement"
                    } else {
                        "dec_statement"
                    },
                    vec![e, op],
                )
            }
            L::Op("<-") if single => {
                let op = self.take();
                let value = self.expr().with("value");
                let channel = list.remove(0).with("channel");
                node("send_statement", vec![channel, op, value])
            }
            _ if single => {
                let e = list.remove(0);
                if e.kind() == "__guard" {
                    return e;
                }
                node("expression_statement", vec![e])
            }
            _ => {
                self.errors = true;
                node("ERROR", list)
            }
        }
    }

    fn if_statement(&mut self) -> Item {
        let mut items = vec![self.take()];
        let saved = self.no_literal;
        self.no_literal = true;
        let first = self.simple_statement(false);
        let condition = if self.at(";") {
            items.push(first.with("initializer"));
            items.push(self.take());
            self.expr()
        } else {
            unwrap_expression(first)
        };
        items.push(condition.with("condition"));
        self.no_literal = saved;
        items.push(self.block().with("consequence"));
        if self.at("else") {
            items.push(self.take());
            let alt = if self.at("if") {
                self.if_statement()
            } else {
                self.block()
            };
            items.push(alt.with("alternative"));
        }
        node("if_statement", items)
    }

    fn for_statement(&mut self) -> Item {
        let mut items = vec![self.take()];
        let saved = self.no_literal;
        self.no_literal = true;
        if !self.at("{") {
            let first = if self.at(";") {
                None
            } else {
                Some(self.simple_statement(true))
            };
            if self.at(";") {
                let mut clause = Vec::new();
                if let Some(init) = first {
                    clause.push(init.with("initializer"));
                }
                clause.push(self.take());
                if !self.at(";") {
                    clause.push(self.expr().with("condition"));
                }
                clause.push(self.expect(";"));
                if !self.at("{") {
                    clause.push(self.simple_statement(false).with("update"));
                }
                items.push(node("for_clause", clause));
            } else if let Some(first) = first {
                if first.kind() == "range_clause" {
                    items.push(first);
                } else {
                    items.push(unwrap_expression(first));
                }
            }
        }
        self.no_literal = saved;
        items.push(self.block().with("body"));
        node("for_statement", items)
    }

    fn switch_statement(&mut self) -> Item {
        let mut items = vec![self.take()];
        let saved = self.no_literal;
        self.no_literal = true;
        let mut type_switch = false;
        if !self.at("{") {
            let mut first = self.simple_statement(false);
            if self.at(";") {
                items.push(first.with("initializer"));
                items.push(self.take());
                first = if self.at("{") {
                    node("__none", Vec::new())
                } else {
                    self.simple_statement(false)
                };
            }
            match first.kind() {
                "__none" => {}
                "__guard" => {
                    type_switch = true;
                    if let Item::Node(n) = first {
                        items.extend(n.items);
                    }
                }
                "short_var_declaration" if guard_in(&first) => {
                    type_switch = true;
                    if let Item::Node(n) = first {
                        let mut parts = n.items.into_iter();
                        if let Some(left) = parts.next() {
                            items.push(left.with("alias"));
                        }
                        if let Some(op) = parts.next() {
                            items.push(op);
                        }
                        if let Some(Item::Node(right)) = parts.next()
                            && let Some(Item::Node(guard)) = right.items.into_iter().next()
                        {
                            items.extend(guard.items);
                        }
                    }
                }
                _ => items.push(unwrap_expression(first).with("value")),
            }
        }
        self.no_literal = saved;
        items.push(self.expect("{"));
        while !self.eof() && !self.at("}") {
            let before = self.i;
            if self.at("case") {
                let kw = self.take();
                let mut case = vec![kw];
                if type_switch {
                    case.push(self.ty().with("type"));
                    while self.at(",") {
                        case.push(self.take().with("type"));
                        case.push(self.ty().with("type"));
                    }
                } else {
                    case.push(self.expression_list().with("value"));
                }
                case.push(self.expect(":"));
                if let Some(list) = self.statement_list() {
                    case.push(list);
                }
                items.push(node(
                    if type_switch {
                        "type_case"
                    } else {
                        "expression_case"
                    },
                    case,
                ));
            } else if self.at("default") {
                items.push(self.default_case());
            } else if self.is_term() {
                items.push(self.take());
            } else {
                match self.recover(&["}", "case", "default"]) {
                    Some(e) => items.push(e),
                    None => break,
                }
            }
            if self.i == before {
                break;
            }
        }
        items.push(self.expect("}"));
        node(
            if type_switch {
                "type_switch_statement"
            } else {
                "expression_switch_statement"
            },
            items,
        )
    }

    fn default_case(&mut self) -> Item {
        let mut case = vec![self.take(), self.expect(":")];
        if let Some(list) = self.statement_list() {
            case.push(list);
        }
        node("default_case", case)
    }

    fn select_statement(&mut self) -> Item {
        let mut items = vec![self.take(), self.expect("{")];
        while !self.eof() && !self.at("}") {
            let before = self.i;
            if self.at("case") {
                let kw = self.take();
                let comm = self.simple_statement(false);
                let comm = match comm.kind() {
                    "send_statement" => comm,
                    "short_var_declaration" | "assignment_statement" => {
                        let mut parts = match comm {
                            Item::Node(n) => n.items.into_iter(),
                            Item::Tok(_) => Vec::new().into_iter(),
                        };
                        let mut recv = Vec::new();
                        if let Some(left) = parts.next() {
                            recv.push(left.with("left"));
                        }
                        if let Some(Item::Tok(mut op)) = parts.next() {
                            op.field = None;
                            recv.push(Item::Tok(op));
                        }
                        if let Some(right) = parts.next() {
                            let inner = match right {
                                Item::Node(n) if n.items.len() == 1 => {
                                    n.items.into_iter().next().unwrap_or_else(|| unreachable!())
                                }
                                other => other,
                            };
                            recv.push(inner.with("right"));
                        }
                        node("receive_statement", recv)
                    }
                    _ => node(
                        "receive_statement",
                        vec![unwrap_expression(comm).with("right")],
                    ),
                };
                let colon = self.expect(":");
                let mut case = vec![kw, comm.with("communication"), colon];
                if let Some(list) = self.statement_list() {
                    case.push(list);
                }
                items.push(node("communication_case", case));
            } else if self.at("default") {
                items.push(self.default_case());
            } else if self.is_term() {
                items.push(self.take());
            } else {
                match self.recover(&["}", "case", "default"]) {
                    Some(e) => items.push(e),
                    None => break,
                }
            }
            if self.i == before {
                break;
            }
        }
        items.push(self.expect("}"));
        node("select_statement", items)
    }

    // ----- expressions ---------------------------------------------------

    fn expr_items(&mut self) -> Vec<Item> {
        let mut items = vec![self.expr()];
        while self.at(",") {
            items.push(self.take());
            items.push(self.expr());
        }
        items
    }

    fn expression_list(&mut self) -> Item {
        let items = self.expr_items();
        node("expression_list", items)
    }

    pub fn expr(&mut self) -> Item {
        self.binary(1)
    }

    fn binary(&mut self, min: u8) -> Item {
        let mut left = self.unary();
        loop {
            let prec = match self.peek() {
                L::Op(o) => binary_precedence(o),
                _ => 0,
            };
            if prec == 0 || prec < min {
                return left;
            }
            let op = self.take().with("operator");
            let right = self.binary(prec + 1).with("right");
            left = node("binary_expression", vec![left.with("left"), op, right]);
        }
    }

    fn unary(&mut self) -> Item {
        if let L::Op(o @ ("+" | "-" | "!" | "^" | "*" | "&" | "<-")) = self.peek() {
            // `<-chan T` is a type.
            if o == "<-" && self.at_n(1, "chan") {
                return self.primary();
            }
            let op = self.take().with("operator");
            let operand = self.unary().with("operand");
            return node("unary_expression", vec![op, operand]);
        }
        self.primary()
    }

    fn operand(&mut self) -> Item {
        match self.peek() {
            L::Ident
            | L::Int
            | L::Float
            | L::Imag
            | L::Rune
            | L::Str { .. }
            | L::Raw { .. }
            | L::Kw("nil" | "true" | "false" | "iota") => self.take(),
            L::Op("(") => {
                let saved = self.no_literal;
                self.no_literal = false;
                let open = self.take();
                let inner = self.expr();
                let close = self.expect(")");
                self.no_literal = saved;
                // `(*[2]byte)`, `(*interface{})`: only a type can be this.
                if only_a_type(&inner) {
                    let ty = typeify(clear_field(inner));
                    return node("parenthesized_type", vec![open, ty, close]);
                }
                node("parenthesized_expression", vec![open, inner, close])
            }
            L::Kw("func") => {
                let kw = self.take();
                let mut items = vec![kw, self.parameter_list().with("parameters")];
                if let Some(r) = self.result() {
                    items.push(r);
                }
                if self.at("{") {
                    items.push(self.block().with("body"));
                    node("func_literal", items)
                } else {
                    node("function_type", items)
                }
            }
            L::Op("[") | L::Kw("map" | "chan" | "struct" | "interface") | L::Op("<-") => self.ty(),
            _ => {
                self.errors = true;
                self.missing("identifier", true)
            }
        }
    }

    fn primary(&mut self) -> Item {
        let mut x = self.operand();
        loop {
            match self.peek() {
                L::Op(".") => {
                    if self.at_n(1, "(") {
                        let dot = self.take();
                        let open = self.take();
                        if self.at("type") {
                            let kw = self.take();
                            let close = self.expect(")");
                            return node("__guard", vec![x.with("value"), dot, open, kw, close]);
                        }
                        let ty = self.ty().with("type");
                        let close = self.expect(")");
                        x = node(
                            "type_assertion_expression",
                            vec![x.with("operand"), dot, open, ty, close],
                        );
                    } else {
                        let dot = self.take();
                        let field = self.ident("field_identifier").with("field");
                        x = node("selector_expression", vec![x.with("operand"), dot, field]);
                    }
                }
                L::Op("[") => x = self.bracket_suffix(x),
                L::Op("(") => {
                    if TYPE_KINDS.contains(&x.kind()) && x.kind() != "type_identifier" {
                        x = self.conversion(x);
                    } else {
                        let special = matches!(&x, Item::Tok(t) if t.kind == "identifier" && matches!(&self.src[t.start..t.end], "new" | "make"));
                        let args = if special {
                            self.special_arguments()
                        } else {
                            self.arguments()
                        };
                        x = node(
                            "call_expression",
                            vec![x.with("function"), args.with("arguments")],
                        );
                    }
                }
                L::Op("{") if self.literal_allowed(&x) => {
                    let ty = typeify(x).with("type");
                    let body = self.literal_value().with("body");
                    x = node("composite_literal", vec![ty, body]);
                }
                _ => return x,
            }
        }
    }

    /// Whether `x {` is a composite literal here.
    fn literal_allowed(&self, x: &Item) -> bool {
        let kind = x.kind();
        if matches!(
            kind,
            "slice_type"
                | "array_type"
                | "implicit_length_array_type"
                | "map_type"
                | "struct_type"
                | "generic_type"
                | "qualified_type"
        ) {
            return true;
        }
        if self.no_literal {
            return false;
        }
        match kind {
            "identifier" => true,
            "selector_expression" => {
                matches!(x.items().first(), Some(Item::Tok(t)) if t.kind == "identifier")
            }
            "index_expression" => {
                matches!(x.items().first(), Some(first) if matches!(first.kind(), "identifier" | "selector_expression"))
            }
            // Its type already read as one (`Pair[T1, T2]{…}`).
            "type_instantiation_expression" => {
                matches!(x.items().first(), Some(first) if matches!(first.kind(), "type_identifier" | "qualified_type"))
            }
            _ => false,
        }
    }

    fn conversion(&mut self, ty: Item) -> Item {
        let saved = self.no_literal;
        self.no_literal = false;
        let open = self.take();
        let mut items = vec![ty.with("type"), open, self.expr().with("operand")];
        if self.at(",") {
            items.push(self.take());
        }
        items.push(self.expect(")"));
        self.no_literal = saved;
        node("type_conversion_expression", items)
    }

    /// `x[…]`: an index, a slice, an instantiation, or a call's type
    /// arguments.
    fn bracket_suffix(&mut self, x: Item) -> Item {
        let saved = self.no_literal;
        self.no_literal = false;
        let open = self.take();
        let result = if self.at(":") {
            self.slice_rest(x, open, None)
        } else {
            let first = self.expr();
            let types_only = self.at(",") || TYPE_KINDS.contains(&first.kind());
            if self.at(":") {
                self.slice_rest(x, open, Some(first))
            } else if types_only || (self.called_after_brackets() && typeable(&first)) {
                // `X[T…](…)` without type information is a call, a call's
                // type arguments or a conversion. As tree-sitter-go chooses
                // (its dynamic precedences): one argument converts to the
                // generic type; otherwise a call with type arguments,
                // unless the brackets hold one name, which is an index.
                let one_argument = self.one_argument_after_brackets();
                let index_wins = !types_only
                    && type_score(&first) <= -1
                    && !(one_argument && type_name_like(&x));
                if index_wins {
                    let close = self.expect("]");
                    node(
                        "index_expression",
                        vec![x.with("operand"), open, first.with("index"), close],
                    )
                } else {
                    let mut types = vec![typeify(first)];
                    while self.at(",") {
                        types.push(self.take());
                        if self.at("]") {
                            break;
                        }
                        types.push(self.ty());
                    }
                    let close = self.expect("]");
                    if self.at("(") {
                        let mut args = vec![open];
                        for t in types {
                            if matches!(&t, Item::Tok(tok) if tok.kind == ",") {
                                args.push(t);
                            } else {
                                args.push(node("type_elem", vec![t]));
                            }
                        }
                        args.push(close);
                        let type_args = node("type_arguments", args).with("type_arguments");
                        if one_argument && type_name_like(&x) {
                            let ty = node("generic_type", vec![typeify(x).with("type"), type_args]);
                            self.no_literal = saved;
                            return self.conversion(ty);
                        }
                        let call_args = self.arguments();
                        self.no_literal = saved;
                        return node(
                            "call_expression",
                            vec![x.with("function"), type_args, call_args.with("arguments")],
                        );
                    }
                    let mut items = vec![typeify(x).with("type"), open];
                    items.extend(types);
                    items.push(close);
                    node("type_instantiation_expression", items)
                }
            } else {
                let close = self.expect("]");
                node(
                    "index_expression",
                    vec![x.with("operand"), open, first.with("index"), close],
                )
            }
        };
        self.no_literal = saved;
        result
    }

    /// Whether a `(` follows the `]` closing the brackets the parser is in.
    fn called_after_brackets(&self) -> bool {
        self.close_after(0).is_some_and(|k| self.at_n(k + 1, "("))
    }

    /// The offset of the `]` closing the brackets the parser is in.
    fn close_after(&self, from: usize) -> Option<usize> {
        let mut depth = 0usize;
        let mut k = from;
        loop {
            match self.peek_at(k) {
                L::Op("[" | "(" | "{") => depth += 1,
                L::Op("]") if depth == 0 => return Some(k),
                L::Op("]" | ")" | "}") => depth = depth.checked_sub(1)?,
                L::Eof => return None,
                _ => {}
            }
            k += 1;
        }
    }

    /// Whether the call after the brackets has exactly one argument (a
    /// trailing comma allowed).
    fn one_argument_after_brackets(&self) -> bool {
        let Some(close) = self.close_after(0) else {
            return false;
        };
        if !self.at_n(close + 1, "(") {
            return false;
        }
        let (mut depth, mut k, mut commas) = (0usize, close + 2, 0usize);
        let empty = self.at_n(k, ")");
        loop {
            match self.peek_at(k) {
                L::Op("(" | "[" | "{") => depth += 1,
                L::Op(")") if depth == 0 => break,
                L::Op(")" | "]" | "}") => depth = depth.saturating_sub(1),
                // `x...` spreads: no conversion.
                L::Op("...") if depth == 0 => return false,
                L::Op(",") if depth == 0 && !self.at_n(k + 1, ")") => commas += 1,
                L::Eof => return false,
                _ => {}
            }
            k += 1;
        }
        !empty && commas == 0
    }

    fn slice_rest(&mut self, x: Item, open: Item, start: Option<Item>) -> Item {
        let mut items = vec![x.with("operand"), open];
        if let Some(s) = start {
            items.push(s.with("start"));
        }
        items.push(self.expect(":"));
        if !self.at("]") && !self.at(":") {
            items.push(self.expr().with("end"));
        }
        if self.at(":") {
            items.push(self.take());
            items.push(self.expr().with("capacity"));
        }
        items.push(self.expect("]"));
        node("slice_expression", items)
    }

    fn arguments(&mut self) -> Item {
        let saved = self.no_literal;
        self.no_literal = false;
        let mut items = vec![self.expect("(")];
        while !self.eof() && !self.at(")") {
            let before = self.i;
            let e = self.expr();
            if self.at("...") {
                let dots = self.take();
                items.push(node("variadic_argument", vec![e, dots]));
            } else {
                items.push(e);
            }
            if self.at(",") {
                items.push(self.take());
            } else {
                break;
            }
            if self.i == before {
                break;
            }
        }
        items.push(self.expect(")"));
        self.no_literal = saved;
        node("argument_list", items)
    }

    /// `new(T)` / `make(T, n)`: a type first.
    fn special_arguments(&mut self) -> Item {
        let saved = self.no_literal;
        self.no_literal = false;
        let mut items = vec![self.expect("(")];
        if !self.at(")") {
            items.push(self.ty());
            while self.at(",") {
                items.push(self.take());
                if self.at(")") {
                    break;
                }
                items.push(self.expr());
            }
        }
        items.push(self.expect(")"));
        self.no_literal = saved;
        node("argument_list", items)
    }

    fn literal_value(&mut self) -> Item {
        let saved = self.no_literal;
        self.no_literal = false;
        let mut items = vec![self.expect("{")];
        while !self.eof() && !self.at("}") {
            let before = self.i;
            let first = self.literal_element();
            if self.at(":") {
                let colon = self.take();
                let value = self.literal_element().with("value");
                items.push(node("keyed_element", vec![first.with("key"), colon, value]));
            } else {
                items.push(first);
            }
            if self.at(",") {
                items.push(self.take());
            } else {
                break;
            }
            if self.i == before {
                break;
            }
        }
        // Newlines after the last element ended no statement.
        while self.peek() == L::Term {
            self.i += 1;
        }
        items.push(self.expect("}"));
        self.no_literal = saved;
        node("literal_value", items)
    }

    fn literal_element(&mut self) -> Item {
        let inner = if self.at("{") {
            self.literal_value()
        } else {
            self.expr()
        };
        node("literal_element", vec![inner])
    }
}

fn is_assignment(o: &str) -> bool {
    matches!(
        o,
        "=" | "+=" | "-=" | "*=" | "/=" | "%=" | "&=" | "|=" | "^=" | "<<=" | ">>=" | "&^="
    )
}

fn binary_precedence(o: &str) -> u8 {
    match o {
        "||" => 1,
        "&&" => 2,
        "==" | "!=" | "<" | "<=" | ">" | ">=" => 3,
        "+" | "-" | "|" | "^" => 4,
        "*" | "/" | "%" | "<<" | ">>" | "&" | "&^" => 5,
        _ => 0,
    }
}

/// Whether `item`, read as an expression, could be a type: a name, a
/// qualified name, a pointer to or instance of one, or a type node.
fn typeable(item: &Item) -> bool {
    match item.kind() {
        "identifier" => true,
        // `pkg.T`: a qualified name has two parts.
        "selector_expression" => {
            matches!(item.items().first(), Some(Item::Tok(t)) if t.kind == "identifier")
        }
        "unary_expression" => {
            matches!(item.items().first(), Some(Item::Tok(t)) if t.kind == "*")
                && item.items().get(1).is_some_and(typeable)
        }
        "index_expression" => {
            item.items().first().is_some_and(typeable) && item.items().get(2).is_some_and(typeable)
        }
        "type_instantiation_expression" => true,
        kind => TYPE_KINDS.contains(&kind),
    }
}

/// How strongly tree-sitter-go prefers `item` read as a type argument
/// over the same text as an index (its dynamic precedences, summed): a
/// bare name −1, a qualified one 0, a pointer its target's, an instance 3
/// and its arguments'. A score of −1 or less ties the index, which wins.
fn type_score(item: &Item) -> i32 {
    match item.kind() {
        "identifier" => -1,
        "unary_expression" => item.items().get(1).map_or(0, type_score),
        "index_expression" => 3 + item.items().get(2).map_or(0, type_score),
        _ => 0,
    }
}

/// Whether `item` names a type a conversion could be to (`T`, `pkg.T`).
fn type_name_like(item: &Item) -> bool {
    match item.kind() {
        "identifier" => true,
        "selector_expression" => {
            matches!(item.items().first(), Some(Item::Tok(t)) if t.kind == "identifier")
        }
        _ => false,
    }
}

/// Whether `item`, read as an expression, can only be a type: a type
/// node, or a pointer to one (`*[2]byte`).
fn only_a_type(item: &Item) -> bool {
    let kind = item.kind();
    if TYPE_KINDS.contains(&kind) && kind != "type_identifier" {
        return true;
    }
    // A pointer to a type, or to an instance (`*B[A]`, which reads more
    // strongly as a type than as an index).
    kind == "unary_expression"
        && matches!(item.items().first(), Some(Item::Tok(t)) if t.kind == "*")
        && item.items().get(1).is_some_and(|inner| {
            only_a_type(inner)
                || inner.kind() == "type_instantiation_expression"
                || (inner.kind() == "index_expression" && typeable(inner))
        })
}

/// An `expression_statement`'s expression (a header's condition).
fn unwrap_expression(item: Item) -> Item {
    match item {
        Item::Node(n) if n.kind == "expression_statement" && n.items.len() == 1 => {
            n.items.into_iter().next().unwrap_or_else(|| unreachable!())
        }
        other => other,
    }
}

/// Whether `x := y.(type)` holds a type switch guard.
fn guard_in(item: &Item) -> bool {
    item.items()
        .get(2)
        .and_then(|right| right.items().first())
        .is_some_and(|e| e.kind() == "__guard")
}

/// An expression read where a type was meant (`T{…}`, `pkg.T[int]`): as
/// that type.
fn typeify(item: Item) -> Item {
    let field = match &item {
        Item::Tok(t) => t.field,
        Item::Node(n) => n.field,
    };
    let with_field = |i: Item| match field {
        Some(f) => i.with(f),
        None => i,
    };
    match item {
        Item::Tok(t) if t.kind == "identifier" => with_field(Item::Tok(Tok {
            kind: "type_identifier",
            field: None,
            ..t
        })),
        Item::Node(n) if n.kind == "selector_expression" && n.items.len() == 3 => {
            let mut parts = n.items.into_iter();
            let pkg = parts.next().unwrap_or_else(|| unreachable!());
            let dot = parts.next().unwrap_or_else(|| unreachable!());
            let name = parts.next().unwrap_or_else(|| unreachable!());
            with_field(node(
                "qualified_type",
                vec![
                    pkg.renamed("package_identifier").with("package"),
                    dot,
                    name.renamed("type_identifier").with("name"),
                ],
            ))
        }
        Item::Node(n) if n.kind == "index_expression" && n.items.len() == 4 => {
            let mut parts = n.items.into_iter();
            let operand = typeify(clear_field(parts.next().unwrap_or_else(|| unreachable!())));
            let open = parts.next().unwrap_or_else(|| unreachable!());
            let index = typeify(clear_field(parts.next().unwrap_or_else(|| unreachable!())));
            let close = parts.next().unwrap_or_else(|| unreachable!());
            let args = node(
                "type_arguments",
                vec![open, node("type_elem", vec![index]), close],
            );
            with_field(node(
                "generic_type",
                vec![operand.with("type"), args.with("type_arguments")],
            ))
        }
        Item::Node(n) if n.kind == "type_instantiation_expression" => {
            let mut parts = n.items.into_iter();
            let ty = clear_field(parts.next().unwrap_or_else(|| unreachable!()));
            let mut args = Vec::new();
            for p in parts {
                if matches!(&p, Item::Tok(t) if matches!(t.kind, "[" | "]" | ",")) {
                    args.push(p);
                } else {
                    args.push(node("type_elem", vec![p]));
                }
            }
            with_field(node(
                "generic_type",
                vec![
                    ty.with("type"),
                    node("type_arguments", args).with("type_arguments"),
                ],
            ))
        }
        Item::Node(n) if n.kind == "unary_expression" && n.items.len() == 2 => {
            let mut parts = n.items.into_iter();
            let star = clear_field(parts.next().unwrap_or_else(|| unreachable!()));
            let inner = typeify(clear_field(parts.next().unwrap_or_else(|| unreachable!())));
            with_field(node("pointer_type", vec![star, inner]))
        }
        other => other,
    }
}

fn clear_field(mut item: Item) -> Item {
    match &mut item {
        Item::Tok(t) => t.field = None,
        Item::Node(n) => n.field = None,
    }
    item
}

/// The end of an escape sequence at `j` (a `\`): `\x` and two hex digits,
/// `\u` and four, `\U` and eight, two or three octal digits, else one
/// character.
fn escape(b: &[u8], j: usize) -> usize {
    let at = |k: usize| b.get(k).copied();
    match at(j + 1) {
        Some(b'x') => (j + 4).min(b.len()),
        Some(b'u') => (j + 6).min(b.len()),
        Some(b'U') => (j + 10).min(b.len()),
        Some(c) if c.is_ascii_digit() => {
            let mut k = j + 1;
            while k < j + 4 && at(k).is_some_and(|c| c.is_ascii_digit()) {
                k += 1;
            }
            if k - (j + 1) >= 2 { k } else { j + 2 }
        }
        Some(_) => {
            let c = std::str::from_utf8(&b[j + 1..])
                .ok()
                .and_then(|s| s.chars().next());
            j + 1 + c.map_or(1, char::len_utf8)
        }
        None => j + 1,
    }
}
