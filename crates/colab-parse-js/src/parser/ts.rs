//! TypeScript's additions to the grammar (tree-sitter-typescript's):
//! types, type annotations and parameters, the declarations JavaScript
//! lacks (interfaces, type aliases, enums, namespaces, `declare`,
//! signatures), class members with modifiers, and the lookahead that tells
//! type arguments from comparisons.

use super::*;

/// Types tree-sitter-typescript reads as `predefined_type`.
const PREDEFINED: &[&str] = &[
    "any", "number", "boolean", "string", "symbol", "void", "unknown", "never", "object",
];

impl<'s> Parser<'s> {
    // ---- lookahead ---------------------------------------------------------

    /// Type arguments (or parameters) at token `n`, a `<`: the token after
    /// the closing `>`, if what lies between can be types.
    pub(super) fn scan_type_arguments(&mut self, n: usize) -> Option<usize> {
        if !self.is(n, "<") {
            return None;
        }
        let mut depth = 0usize;
        let mut k = n;
        loop {
            let t = self.tk(k);
            match t.k {
                L::Op("<") => depth += 1,
                L::Op(">") => depth -= 1,
                // `>>` may close this list and an enclosing one: then the
                // list ends inside the token (which the parser splits).
                L::Op(">>" | ">>>") => {
                    let n = if t.k == L::Op(">>") { 2 } else { 3 };
                    if depth <= n {
                        return (depth < n).then_some(k).or(Some(k + 1));
                    }
                    depth -= n;
                }
                L::Op("(" | "[" | "{") => {
                    k = self.scan_balanced(k)?;
                    continue;
                }
                L::Op(
                    "." | "," | "|" | "&" | "?" | ":" | "=>" | "=" | "..." | "-" | "?." | "*",
                )
                | L::Name
                | L::Number
                | L::Quote
                | L::Frag
                | L::Escape
                | L::Backtick => {}
                _ => return None,
            }
            k += 1;
            if depth == 0 {
                return Some(k);
            }
            if k > n + 10_000 {
                return None;
            }
        }
    }

    /// After a parameter list's `)`, at token `n`: a return type and `=>`
    /// (an arrow function's `(x): T => …`).
    pub(super) fn scan_return_type_arrow(&mut self, n: usize) -> bool {
        if !self.is(n, ":") {
            return false;
        }
        let mut k = n + 1;
        let mut angle = 0usize;
        loop {
            match self.tk(k).k {
                L::Op("=>") if angle == 0 => return true,
                L::Op("(" | "[" | "{") => match self.scan_balanced(k) {
                    Some(e) => {
                        k = e;
                        continue;
                    }
                    None => return false,
                },
                L::Op("<") => angle += 1,
                L::Op(">") => angle = angle.saturating_sub(1),
                L::Op(">>") => angle = angle.saturating_sub(2),
                L::Op(">>>") => angle = angle.saturating_sub(3),
                L::Op(";" | ")" | "]" | "}" | "," | "=") | L::Eof if angle == 0 => return false,
                _ => {}
            }
            k += 1;
            if k > n + 10_000 {
                return false;
            }
        }
    }

    /// Whether a TypeScript arrow function starts at token `n`:
    /// `async`, type parameters, then a name or parameter list, a return
    /// type, `=>`.
    pub(super) fn ts_arrow_at(&mut self, n: usize) -> bool {
        let mut n = n;
        if self.is(n, "async") && !self.tk(n + 1).nl && !self.is(n + 1, "=>") {
            n += 1;
        }
        if self.is(n, "<") {
            let Some(k) = self.scan_type_arguments(n) else {
                return false;
            };
            n = k;
            if !self.is(n, "(") {
                return false;
            }
        }
        if self.is_ident(n) {
            return self.is(n + 1, "=>");
        }
        if self.is(n, "(") {
            let Some(k) = self.scan_balanced(n) else {
                return false;
            };
            return self.is(k, "=>") || self.scan_return_type_arrow(k);
        }
        false
    }

    // ---- types ---------------------------------------------------------------

    pub(super) fn type_annotation(&mut self) -> Item {
        let colon = self.expect(":");
        let t = self.ty();
        node("type_annotation", vec![colon, t])
    }

    /// A call signature's return type after its `)`: a type, an
    /// `asserts` clause, or a type predicate.
    pub(super) fn return_type(&mut self, items: &mut Vec<Item>) {
        if !self.at(":") {
            return;
        }
        if self.is(1, "asserts") && (self.is_ident(2) || self.is(2, "this")) && !self.is(2, "is") {
            let colon = self.punct();
            let kw = self.punct();
            let inner = if self.is(1, "is") {
                self.type_predicate()
            } else if self.at("this") {
                self.leaf("this")
            } else {
                self.binding_identifier()
            };
            let a = node("asserts", vec![kw, inner]);
            items.push(node("asserts_annotation", vec![colon, a]).with("return_type"));
            return;
        }
        if (self.is_ident(1) || self.is(1, "this")) && self.is(2, "is") {
            let colon = self.punct();
            let p = self.type_predicate();
            items.push(node("type_predicate_annotation", vec![colon, p]).with("return_type"));
            return;
        }
        items.push(self.type_annotation().with("return_type"));
    }

    fn type_predicate(&mut self) -> Item {
        let name = if self.at("this") {
            self.leaf("this")
        } else if PREDEFINED.contains(&self.word()) {
            // A predefined type's word, as an identifier over it.
            node("identifier", vec![self.punct()])
        } else {
            self.leaf("identifier")
        };
        let is = self.punct();
        let t = self.ty().with("type");
        node("type_predicate", vec![name.with("name"), is, t])
    }

    pub(super) fn type_parameters(&mut self) -> Item {
        let mut items = vec![self.punct()];
        while !self.is_closing_angle() && !self.eof() {
            let before = self.i;
            let mut p = Vec::new();
            if self.at("const") {
                p.push(self.punct());
            }
            if self.is_ident(0) {
                p.push(self.leaf("type_identifier").with("name"));
            } else {
                p.push(self.missing("type_identifier", true).with("name"));
            }
            if self.at("extends") || self.at(":") {
                let c = vec![self.punct(), self.ty()];
                p.push(node("constraint", c).with("constraint"));
            }
            if self.at("=") {
                let d = vec![self.punct(), self.ty()];
                p.push(node("default_type", d).with("value"));
            }
            items.push(node("type_parameter", p));
            if self.i == before || !self.at(",") {
                break;
            }
            items.push(self.punct());
        }
        items.push(self.close_angle());
        node("type_parameters", items)
    }

    fn is_closing_angle(&mut self) -> bool {
        matches!(
            self.peek(),
            L::Op(">" | ">>" | ">>>" | ">=" | ">>=" | ">>>=")
        )
    }

    pub(super) fn type_arguments(&mut self) -> Item {
        let mut items = vec![self.punct()];
        while !self.is_closing_angle() && !self.eof() {
            let before = self.i;
            items.push(self.ty());
            if self.i == before || !self.at(",") {
                break;
            }
            items.push(self.punct());
        }
        items.push(self.close_angle());
        node("type_arguments", items)
    }

    /// A type: a function or constructor type, a conditional type, a
    /// union.
    pub(super) fn ty(&mut self) -> Item {
        if self.at("<") || (self.at("(") && self.function_type_at()) {
            return self.function_type();
        }
        if self.at("new") || (self.at("abstract") && self.is(1, "new")) {
            let mut items = Vec::new();
            if self.at("abstract") {
                items.push(self.punct());
            }
            items.push(self.punct());
            if self.at("<") {
                items.push(self.type_parameters().with("type_parameters"));
            }
            items.push(self.formal_parameters().with("parameters"));
            items.push(self.expect("=>"));
            items.push(self.ty().with("type"));
            return node("constructor_type", items);
        }
        let t = self.union_ty();
        if self.at("extends") && !self.tk(0).nl {
            let kw = self.punct();
            // A function type may be what is tested for; a conditional
            // may not (its `?` is this one's).
            let right = if self.at("<") || (self.at("(") && self.function_type_at()) {
                self.function_type()
            } else {
                self.union_ty()
            }
            .with("right");
            let q = self.expect("?");
            let then = self.ty().with("consequence");
            let colon = self.expect(":");
            let alt = self.ty().with("alternative");
            return node(
                "conditional_type",
                vec![t.with("left"), kw, right, q, then, colon, alt],
            );
        }
        t
    }

    /// At `(` in a type: whether it opens a function type's parameters (a
    /// parameter list, then `=>`), not a parenthesized type
    /// (`((a: T) => U)`), as TypeScript tells them.
    fn function_type_at(&mut self) -> bool {
        let params = match self.tk(1).k {
            L::Op(")" | "...") => true,
            L::Op("{" | "[") => true,
            L::Name => {
                let w = self.word_at(1);
                matches!(self.tk(2).k, L::Op(":" | "," | "?" | "=" | ")"))
                    || (matches!(w, "public" | "private" | "protected" | "readonly")
                        && self.tk(2).k == L::Name)
            }
            _ => false,
        };
        params && self.scan_balanced(0).is_some_and(|k| self.is(k, "=>"))
    }

    fn function_type(&mut self) -> Item {
        let mut items = Vec::new();
        if self.at("<") {
            items.push(self.type_parameters().with("type_parameters"));
        }
        items.push(self.formal_parameters().with("parameters"));
        items.push(self.expect("=>"));
        let ret = if (self.is_ident(0) || self.at("this")) && self.is(1, "is") {
            self.type_predicate()
        } else if self.at("asserts") && (self.is_ident(1) || self.is(1, "this")) {
            let kw = self.punct();
            let inner = if self.is(1, "is") {
                self.type_predicate()
            } else if self.at("this") {
                self.leaf("this")
            } else {
                self.binding_identifier()
            };
            node("asserts", vec![kw, inner])
        } else {
            self.ty()
        };
        items.push(ret.with("return_type"));
        node("function_type", items)
    }

    fn union_ty(&mut self) -> Item {
        let mut left = if self.at("|") {
            let bar = self.punct();
            let t = self.intersection_ty();
            node("union_type", vec![bar, t])
        } else {
            self.intersection_ty()
        };
        while self.at("|") {
            let bar = self.punct();
            let right = self.intersection_ty();
            left = node("union_type", vec![left, bar, right]);
        }
        left
    }

    fn intersection_ty(&mut self) -> Item {
        let mut left = if self.at("&") {
            let amp = self.punct();
            let t = self.operator_ty();
            node("intersection_type", vec![amp, t])
        } else {
            self.operator_ty()
        };
        while self.at("&") {
            let amp = self.punct();
            let right = self.operator_ty();
            left = node("intersection_type", vec![left, amp, right]);
        }
        left
    }

    /// `keyof T`, `readonly T`, `infer T`, or a primary type.
    fn operator_ty(&mut self) -> Item {
        match self.word() {
            "keyof" if self.starts_type(1) => {
                let kw = self.punct();
                let t = self.postfix_ty();
                node("index_type_query", vec![kw, t])
            }
            // `readonly` takes the whole type after it (tree-sitter's
            // precedence), union and all.
            "readonly" if self.starts_type(1) => {
                let kw = self.punct();
                let t = self.ty();
                node("readonly_type", vec![kw, t])
            }
            "infer" if self.is_ident(1) => {
                let kw = self.punct();
                let mut items = vec![kw, self.leaf("type_identifier")];
                // `infer U extends X`: a constraint, never itself a
                // conditional (its `? :` are the enclosing type's).
                if self.at("extends") {
                    items.push(self.punct());
                    items.push(self.union_ty());
                }
                node("infer_type", items)
            }
            _ => self.postfix_ty(),
        }
    }

    /// Whether token `n` can start a type.
    fn starts_type(&mut self, n: usize) -> bool {
        match self.tk(n).k {
            L::Name | L::Number | L::Quote | L::Backtick => true,
            L::Op(o) => matches!(
                o,
                "(" | "[" | "{" | "<" | "-" | "|" | "&" | "*" | "?" | "{|"
            ),
            _ => false,
        }
    }

    /// A primary type and its `[]` / `[K]` suffixes.
    fn postfix_ty(&mut self) -> Item {
        let mut t = self.primary_ty();
        while self.at("[") && !self.tk(0).nl {
            let open = self.punct();
            if self.at("]") {
                let close = self.punct();
                t = node("array_type", vec![t, open, close]);
            } else {
                let k = self.ty();
                let close = self.expect("]");
                t = node("lookup_type", vec![t, open, k, close]);
            }
        }
        t
    }

    fn primary_ty(&mut self) -> Item {
        match self.peek() {
            L::Op("(") => {
                let items = vec![self.punct(), self.ty(), self.expect(")")];
                node("parenthesized_type", items)
            }
            L::Op("{") => self.object_type("object_type"),
            L::Op("[") => self.tuple_type(),
            L::Op("-") if self.tk(1).k == L::Number => {
                let op = self.punct().with("operator");
                let n = self.leaf("number").with("argument");
                let u = node("unary_expression", vec![op, n]);
                node("literal_type", vec![u])
            }
            L::Number => {
                let n = self.leaf("number");
                node("literal_type", vec![n])
            }
            L::Quote => {
                let s = self.string();
                node("literal_type", vec![s])
            }
            L::Backtick => self.template_literal_type(),
            L::Op("*") => self.leaf("existential_type"),
            L::Op("?") => {
                let q = self.punct();
                let t = self.postfix_ty();
                node("flow_maybe_type", vec![q, t])
            }
            L::Name => {
                let w = self.word();
                match w {
                    "true" | "false" | "null" | "undefined" => {
                        let l = self.leaf(w_kind(w));
                        node("literal_type", vec![l])
                    }
                    "this" => self.leaf("this_type"),
                    "typeof" => self.type_query(),
                    // Two tokens, both of the alias's kind.
                    "unique" if self.is(1, "symbol") => {
                        let u = self.punct_as("unique symbol");
                        let s = self.punct_as("unique symbol");
                        node("predefined_type", vec![u, s])
                    }
                    "const" => self.punct(),
                    "import" if self.is(1, "(") => self.type_query_operand(),
                    _ if PREDEFINED.contains(&w) && !self.is(1, ".") => {
                        node("predefined_type", vec![self.punct()])
                    }
                    _ => self.type_reference(),
                }
            }
            _ => self.missing("type_identifier", true),
        }
    }

    /// The current token, anonymous, as `kind`.
    fn punct_as(&mut self, kind: &'static str) -> Item {
        let t = self.advance();
        Self::tok(t, kind, false)
    }

    /// A named type: `T`, `a.b.T`, and either with type arguments.
    fn type_reference(&mut self) -> Item {
        let first = self.tk(0);
        if !(first.k == L::Name) {
            return self.missing("type_identifier", true);
        }
        let mut t = if self.is(1, ".") && self.tk(2).k == L::Name {
            // `a.b.T`: identifiers, then the type's name.
            // The module part's links are `member_expression`s, the outermost
            // a `nested_identifier`.
            let mut module = self.leaf("identifier");
            loop {
                let dot = self.punct();
                if self.is(1, ".") && self.tk(2).k == L::Name {
                    let p = self.leaf("property_identifier").with("property");
                    module = node("member_expression", vec![module.with("object"), dot, p]);
                    continue;
                }
                let module = module.renamed_if("member_expression", "nested_identifier");
                let name = self.leaf("type_identifier").with("name");
                break node(
                    "nested_type_identifier",
                    vec![module.with("module"), dot, name],
                );
            }
        } else {
            self.leaf("type_identifier")
        };
        if self.at("<") && !self.tk(0).nl {
            let args = self.type_arguments().with("type_arguments");
            t = node("generic_type", vec![t.with("name"), args]);
        }
        t
    }

    /// `typeof x.y[0]()`: a query of an expression's type.
    fn type_query(&mut self) -> Item {
        let kw = self.punct();
        let e = self.type_query_operand();
        node("type_query", vec![kw, e])
    }

    fn type_query_operand(&mut self) -> Item {
        let mut e = if self.at("import") {
            node("import", vec![self.punct()])
        } else if self.at("this") {
            self.leaf("this")
        } else {
            self.leaf("identifier")
        };
        loop {
            e = match self.peek() {
                L::Op("." | "?.") => {
                    let dot = self.punct();
                    let p = self.member_property();
                    node("member_expression", vec![e.with("object"), dot, p])
                }
                L::Op("[") if !self.tk(0).nl => {
                    let open = self.punct();
                    let index = match self.peek() {
                        L::Quote => self.string(),
                        L::Number => self.leaf("number"),
                        _ => node("predefined_type", vec![self.punct()]),
                    };
                    let close = self.expect("]");
                    node(
                        "subscript_expression",
                        vec![e.with("object"), open, index.with("index"), close],
                    )
                }
                L::Op("(") => {
                    let args = self.arguments().with("arguments");
                    node("call_expression", vec![e.with("function"), args])
                }
                L::Op("<") if self.scan_type_arguments(0).is_some() => {
                    let args = self.type_arguments().with("type_arguments");
                    node("instantiation_expression", vec![e.with("function"), args])
                }
                _ => return e,
            };
        }
    }

    fn template_literal_type(&mut self) -> Item {
        let mut items = vec![self.leaf_as("`")];
        loop {
            match self.peek() {
                L::Frag => items.push(self.leaf("string_fragment")),
                L::Escape => items.push(self.leaf("escape_sequence")),
                L::Op("${") => {
                    let s = vec![self.punct(), self.ty(), self.expect("}")];
                    items.push(node("template_type", s));
                }
                L::Backtick => {
                    let t = self.advance();
                    items.push(Self::tok(t, "`", false));
                    break;
                }
                _ => {
                    items.push(self.missing("`", false));
                    break;
                }
            }
        }
        node("template_literal_type", items)
    }

    fn tuple_type(&mut self) -> Item {
        let mut items = vec![self.punct()];
        while !self.at("]") && !self.eof() {
            let before = self.i;
            items.push(self.tuple_member());
            if self.i == before || !self.at(",") {
                break;
            }
            items.push(self.punct());
        }
        items.push(self.expect("]"));
        node("tuple_type", items)
    }

    fn tuple_member(&mut self) -> Item {
        // `name: T`, `name?: T`, `...rest: T[]`.
        if self.is_ident(0) && self.is(1, ":") {
            let name = self.leaf("identifier").with("name");
            let t = self.type_annotation().with("type");
            return node("required_parameter", vec![name, t]);
        }
        if self.is_ident(0) && self.is(1, "?") && self.is(2, ":") {
            let name = self.leaf("identifier").with("name");
            let q = self.punct();
            let t = self.type_annotation().with("type");
            return node("optional_parameter", vec![name, q, t]);
        }
        if self.at("...") {
            if self.is_ident(1) && self.is(2, ":") {
                let dots = self.punct();
                let id = self.leaf("identifier");
                let rest = node("rest_pattern", vec![dots, id]).with("name");
                let t = self.type_annotation().with("type");
                return node("required_parameter", vec![rest, t]);
            }
            let dots = self.punct();
            let t = self.ty();
            return node("rest_type", vec![dots, t]);
        }
        let t = self.ty();
        if self.at("?") {
            let q = self.punct();
            return node("optional_type", vec![t, q]);
        }
        t
    }

    /// `{ … }`: an object type, or an interface's body (`kind`).
    pub(super) fn object_type(&mut self, kind: &'static str) -> Item {
        let mut items = vec![self.punct()];
        while !self.at("}") && !self.eof() {
            if self.at(",") || self.at(";") {
                items.push(self.punct());
                continue;
            }
            let before = self.i;
            items.push(self.type_member());
            if self.i == before {
                items.push(self.error_statement());
                continue;
            }
            // The last member needs no separator.
            if self.at(",") || self.at(";") {
                items.push(self.punct());
            } else if !self.at("}") && !self.asi_after(false) {
                items.push(self.missing(";", false));
            }
        }
        items.push(self.expect("}"));
        node(kind, items)
    }

    /// One member of an object type or interface.
    fn type_member(&mut self) -> Item {
        if self.at("(") || self.at("<") {
            let mut items = Vec::new();
            self.call_signature(&mut items);
            return node("call_signature", items);
        }
        if self.at("new") && (self.is(1, "(") || self.is(1, "<")) {
            let mut items = vec![self.punct()];
            if self.at("<") {
                items.push(self.type_parameters().with("type_parameters"));
            }
            items.push(self.formal_parameters().with("parameters"));
            if self.at(":") {
                items.push(self.type_annotation().with("type"));
            }
            return node("construct_signature", items);
        }
        if self.index_signature_at() {
            return self.index_signature(Vec::new());
        }
        let mut items = Vec::new();
        self.member_modifiers(&mut items, false);
        if self.index_signature_at() {
            return self.index_signature(items);
        }
        let name = self.property_name().with("name");
        items.push(name);
        if self.at("?") {
            items.push(self.punct());
        }
        if self.at("(") || self.at("<") {
            self.call_signature(&mut items);
            return node("method_signature", items);
        }
        if self.at(":") {
            items.push(self.type_annotation().with("type"));
        }
        node("property_signature", items)
    }

    /// At `[` (after `-`/`+`/`readonly`): an index signature, not a
    /// computed property name.
    fn index_signature_at(&mut self) -> bool {
        let mut n = 0;
        if self.is(n, "-") || self.is(n, "+") {
            n += 1;
        }
        if self.is(n, "readonly") {
            n += 1;
        }
        self.is(n, "[")
            && self.tk(n + 1).k == L::Name
            && (self.is(n + 2, ":") || self.is(n + 2, "in"))
    }

    fn index_signature(&mut self, mut items: Vec<Item>) -> Item {
        if self.at("-") || self.at("+") {
            items.push(self.punct().with("sign"));
        }
        if self.at("readonly") {
            items.push(self.punct());
        }
        items.push(self.punct());
        if self.is(1, "in") {
            let mut m = vec![self.leaf("type_identifier").with("name"), self.punct()];
            m.push(self.ty().with("type"));
            if self.at("as") {
                m.push(self.punct());
                m.push(self.ty().with("alias"));
            }
            items.push(node("mapped_type_clause", m));
        } else {
            items.push(self.leaf("identifier").with("name"));
            items.push(self.expect(":"));
            items.push(self.ty().with("index_type"));
        }
        items.push(self.expect("]"));
        let t = if (self.at("-") || self.at("+")) && self.is(1, "?") && self.is(2, ":") {
            let kind = if self.at("-") { "-?:" } else { "+?:" };
            let op = self.joined(3, kind);
            let t = self.ty();
            let k = if kind == "-?:" {
                "omitting_type_annotation"
            } else {
                "adding_type_annotation"
            };
            node(k, vec![op, t])
        } else if self.at("?") && self.is(1, ":") && self.adjacent(0) {
            let op = self.joined(2, "?:");
            let t = self.ty();
            node("opting_type_annotation", vec![op, t])
        } else if self.at(":") {
            self.type_annotation()
        } else {
            self.missing("type_annotation", true)
        };
        items.push(t.with("type"));
        node("index_signature", items)
    }

    /// Type parameters, parameters and return type.
    pub(super) fn call_signature(&mut self, items: &mut Vec<Item>) {
        if self.at("<") {
            items.push(self.type_parameters().with("type_parameters"));
        }
        items.push(self.formal_parameters().with("parameters"));
        self.return_type(items);
    }

    /// Modifiers before a member's name: accessibility, `static`,
    /// `override`, `readonly`, `declare`, `abstract`, `accessor`,
    /// `async`, `get` / `set` / `*`. A word is one only when the name
    /// follows it.
    pub(super) fn member_modifiers(&mut self, items: &mut Vec<Item>, class: bool) {
        loop {
            let w = self.word();
            let is_mod = match w {
                "public" | "private" | "protected" | "static" | "override" | "readonly" => true,
                "declare" | "abstract" | "accessor" => class,
                "async" => !self.tk(1).nl,
                "get" | "set" => !self.is(1, "*"),
                _ => false,
            };
            if is_mod && self.name_follows(1) {
                let t = self.punct();
                items.push(match w {
                    "public" | "private" | "protected" => node("accessibility_modifier", vec![t]),
                    "override" => node("override_modifier", vec![t]),
                    _ => t,
                });
                continue;
            }
            if self.at("*") {
                items.push(self.punct());
            }
            return;
        }
    }

    /// Whether a member's name (or another modifier) starts at token `n`.
    fn name_follows(&mut self, n: usize) -> bool {
        matches!(
            self.tk(n).k,
            L::Name | L::Private | L::Number | L::Quote | L::Op("[" | "*" | "#")
        )
    }

    // ---- parameters ----------------------------------------------------------

    /// A TypeScript parameter: `required_parameter` or
    /// `optional_parameter` around its pattern.
    pub(super) fn ts_parameter(&mut self) -> Item {
        let mut items = Vec::new();
        while self.at("@") {
            items.push(self.decorator().with("decorator"));
        }
        loop {
            let w = self.word();
            let ok = matches!(
                w,
                "public" | "private" | "protected" | "override" | "readonly"
            ) && matches!(self.tk(1).k, L::Name | L::Op("{" | "[" | "..."));
            if !ok {
                break;
            }
            let t = self.punct();
            items.push(match w {
                "public" | "private" | "protected" => node("accessibility_modifier", vec![t]),
                "override" => node("override_modifier", vec![t]),
                _ => t,
            });
        }
        let pattern = if self.at("...") {
            let dots = self.punct();
            let p = self.binding();
            node("rest_pattern", vec![dots, p])
        } else if self.at("this") {
            self.leaf("this")
        } else {
            self.binding()
        };
        items.push(pattern.with("pattern"));
        let mut kind = "required_parameter";
        if self.at("?") {
            items.push(self.punct());
            kind = "optional_parameter";
        }
        if self.at(":") {
            items.push(self.type_annotation().with("type"));
        }
        if self.at("=") {
            items.push(self.punct());
            items.push(self.expression().with("value"));
        }
        node(kind, items)
    }

    // ---- declarations --------------------------------------------------------

    /// A TypeScript declaration at the start of a statement, if one starts
    /// here.
    pub(super) fn ts_declaration(&mut self) -> Option<Item> {
        match self.word() {
            "interface" if self.is_ident(1) => Some(self.interface_declaration()),
            "type" if self.is_ident(1) && (self.is(2, "=") || self.is(2, "<")) => {
                Some(self.type_alias_declaration())
            }
            "enum" if self.is_ident(1) => Some(self.enum_declaration()),
            "const" if self.is(1, "enum") => Some(self.enum_declaration()),
            "abstract" if self.is(1, "class") && !self.tk(1).nl => {
                let kw = self.punct();
                Some(self.class_ts(true, vec![kw], "abstract_class_declaration"))
            }
            "declare" if !self.tk(1).nl && self.declare_follows() => {
                Some(self.ambient_declaration())
            }
            "module"
                if !self.tk(1).nl
                    && (self.peek_at_kind(1, L::Quote) || self.is_ident(1))
                    && self.module_body_follows() =>
            {
                Some(self.module_declaration("module"))
            }
            _ => None,
        }
    }

    fn peek_at_kind(&mut self, n: usize, k: L) -> bool {
        self.tk(n).k == k
    }

    /// After `module` / `namespace` and its name: a body or the end.
    fn module_body_follows(&mut self) -> bool {
        let mut n = 1;
        if self.tk(n).k == L::Quote {
            while !matches!(self.tk(n + 1).k, L::Quote | L::Eof) {
                n += 1;
            }
            n += 2;
        } else {
            n += 1;
            while self.is(n, ".") && self.tk(n + 1).k == L::Name {
                n += 2;
            }
        }
        self.is(n, "{") || self.is(n, ";") || self.tk(n).nl || self.tk(n).k == L::Eof
    }

    fn declare_follows(&mut self) -> bool {
        matches!(
            self.word_at(1),
            "var"
                | "let"
                | "const"
                | "function"
                | "class"
                | "interface"
                | "type"
                | "enum"
                | "namespace"
                | "module"
                | "global"
                | "abstract"
                | "async"
        )
    }

    fn word_at(&mut self, n: usize) -> &'s str {
        let t = self.tk(n);
        if t.k == L::Name {
            &self.src[t.start..t.end]
        } else {
            ""
        }
    }

    fn interface_declaration(&mut self) -> Item {
        let mut items = vec![self.punct(), self.leaf("type_identifier").with("name")];
        if self.at("<") {
            items.push(self.type_parameters().with("type_parameters"));
        }
        if self.at("extends") {
            let mut e = vec![self.punct()];
            loop {
                e.push(self.type_reference().with("type"));
                if !self.at(",") {
                    break;
                }
                e.push(self.punct());
            }
            items.push(node("extends_type_clause", e));
        }
        items.push(self.object_type("interface_body").with("body"));
        node("interface_declaration", items)
    }

    fn type_alias_declaration(&mut self) -> Item {
        let mut items = vec![self.punct(), self.leaf("type_identifier").with("name")];
        if self.at("<") {
            items.push(self.type_parameters().with("type_parameters"));
        }
        items.push(self.expect("="));
        items.push(self.ty().with("value"));
        self.semicolon(&mut items);
        node("type_alias_declaration", items)
    }

    fn enum_declaration(&mut self) -> Item {
        let mut items = Vec::new();
        if self.at("const") {
            items.push(self.punct());
        }
        items.push(self.punct());
        items.push(self.binding_identifier().with("name"));
        let mut body = vec![self.expect("{")];
        while !self.at("}") && !self.eof() {
            let before = self.i;
            let name = self.property_name().with("name");
            if self.at("=") {
                let eq = self.punct();
                let v = self.expression().with("value");
                body.push(node("enum_assignment", vec![name, eq, v]));
            } else {
                body.push(name);
            }
            if self.i == before || !self.at(",") {
                break;
            }
            body.push(self.punct());
        }
        body.push(self.expect("}"));
        items.push(node("enum_body", body).with("body"));
        node("enum_declaration", items)
    }

    /// `module "m" { … }` / `namespace a.b { … }` (`kind` is the keyword's
    /// node: `module` or `internal_module`).
    pub(super) fn module_declaration(&mut self, kind: &'static str) -> Item {
        let mut items = vec![self.punct()];
        let name = if self.peek() == L::Quote {
            self.string()
        } else {
            self.nested_identifier()
        };
        items.push(name.with("name"));
        if self.at("{") {
            items.push(self.statement_block().with("body"));
        }
        node(kind, items)
    }

    fn ambient_declaration(&mut self) -> Item {
        let mut items = vec![self.punct()];
        if self.at("global") {
            items.push(self.punct());
            items.push(self.statement_block());
        } else if self.at("namespace") {
            items.push(self.module_declaration("internal_module"));
        } else {
            items.push(self.statement());
        }
        node("ambient_declaration", items)
    }

    /// A class with TypeScript's name, type parameters, heritage and
    /// members (`kind`: a declaration, an abstract one, or an expression).
    pub(super) fn class_ts(
        &mut self,
        named: bool,
        mut items: Vec<Item>,
        kind: &'static str,
    ) -> Item {
        items.push(self.expect("class"));
        if self.is_ident(0) && !self.at("extends") && !self.at("implements") {
            items.push(self.leaf("type_identifier").with("name"));
        } else if named {
            items.push(self.missing("type_identifier", true).with("name"));
        }
        if self.at("<") {
            items.push(self.type_parameters().with("type_parameters"));
        }
        let mut heritage = Vec::new();
        if self.at("extends") {
            let mut e = vec![self.punct()];
            loop {
                e.push(self.heritage_value().with("value"));
                if self.at("<") {
                    e.push(self.type_arguments().with("type_arguments"));
                }
                if !self.at(",") {
                    break;
                }
                e.push(self.punct());
            }
            heritage.push(node("extends_clause", e));
        }
        if self.at("implements") {
            let mut e = vec![self.punct()];
            loop {
                e.push(self.ty());
                if !self.at(",") {
                    break;
                }
                e.push(self.punct());
            }
            heritage.push(node("implements_clause", e));
        }
        if !heritage.is_empty() {
            items.push(node("class_heritage", heritage));
        }
        items.push(self.class_body_ts().with("body"));
        node(kind, items)
    }

    /// The expression a class extends: a primary and its member accesses
    /// and calls, stopping at type arguments (the clause's own).
    fn heritage_value(&mut self) -> Item {
        let mut e = self.primary();
        loop {
            e = match self.peek() {
                L::Op(".") => {
                    let dot = self.punct();
                    let p = self.member_property();
                    node("member_expression", vec![e.with("object"), dot, p])
                }
                L::Op("(") => {
                    let args = self.arguments().with("arguments");
                    node("call_expression", vec![e.with("function"), args])
                }
                _ => return e,
            };
        }
    }

    fn class_body_ts(&mut self) -> Item {
        let mut items = vec![self.expect("{")];
        while !self.at("}") && !self.eof() {
            let before = self.i;
            if self.at(";") {
                items.push(self.punct());
                continue;
            }
            if self.at("static") && self.is(1, "{") {
                let s = vec![self.punct(), self.statement_block().with("body")];
                items.push(node("class_static_block", s));
                continue;
            }
            let mut decorators = Vec::new();
            while self.at("@") {
                decorators.push(self.decorator().with("decorator"));
            }
            if self.index_signature_at() {
                items.extend(decorators);
                items.push(self.index_signature(Vec::new()));
                self.member_end(&mut items, false);
                continue;
            }
            let mut mods = Vec::new();
            self.member_modifiers(&mut mods, true);
            if self.index_signature_at() {
                items.extend(decorators);
                items.push(self.index_signature(mods));
                self.member_end(&mut items, false);
                continue;
            }
            let abstract_ = mods
                .iter()
                .any(|m| matches!(m, Item::Tok(t) if t.kind == "abstract"));
            let name = self.property_name().with("name");
            let mut member = mods;
            member.push(name);
            if self.at("?") || (self.at("!") && !self.is(1, "(")) {
                member.push(self.punct());
            }
            if self.at("(") || self.at("<") {
                self.call_signature(&mut member);
                if self.at("{") {
                    member.push(self.statement_block().with("body"));
                    // A method's decorators are the class body's.
                    items.extend(decorators);
                    items.push(node("method_definition", member));
                    if self.at(";") {
                        items.push(self.punct());
                    }
                } else {
                    items.extend(decorators);
                    let kind = if abstract_ {
                        "abstract_method_signature"
                    } else {
                        "method_signature"
                    };
                    items.push(node(kind, member));
                    self.member_end(&mut items, false);
                }
            } else {
                let mut field = decorators;
                field.extend(member);
                if self.at(":") {
                    field.push(self.type_annotation().with("type"));
                }
                let valued = self.at("=");
                if valued {
                    field.push(self.punct());
                    field.push(self.expression().with("value"));
                }
                items.push(node("public_field_definition", field));
                self.member_end(&mut items, valued);
            }
            if self.i == before {
                items.push(self.error_statement());
            }
        }
        items.push(self.expect("}"));
        node("class_body", items)
    }

    /// A member's end: `;`, `,`, or an automatic semicolon.
    fn member_end(&mut self, items: &mut Vec<Item>, after_expression: bool) {
        if self.at(",") {
            items.push(self.punct());
        } else {
            self.semicolon_after(items, after_expression);
        }
    }
}

fn w_kind(w: &str) -> &'static str {
    match w {
        "true" => "true",
        "false" => "false",
        "null" => "null",
        _ => "undefined",
    }
}
