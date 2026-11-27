//! Declarations: imports, properties, functions, initializers, classes,
//! structs, actors, extensions, enums, protocols and the rest; modifiers,
//! attributes, parameters and accessors.

use super::{Item, Parser, hidden, node};
use crate::lexer::L;

/// Modifier words, by the node each is in.
const MODIFIERS: &[(&str, &str)] = &[
    ("public", "visibility_modifier"),
    ("private", "visibility_modifier"),
    ("internal", "visibility_modifier"),
    ("fileprivate", "visibility_modifier"),
    ("open", "visibility_modifier"),
    ("package", "visibility_modifier"),
    ("override", "member_modifier"),
    ("convenience", "member_modifier"),
    ("required", "member_modifier"),
    ("nonisolated", "member_modifier"),
    ("infix", "function_modifier"),
    ("postfix", "function_modifier"),
    ("prefix", "function_modifier"),
    ("mutating", "mutation_modifier"),
    ("nonmutating", "mutation_modifier"),
    ("static", "property_modifier"),
    ("dynamic", "property_modifier"),
    ("optional", "property_modifier"),
    ("class", "property_modifier"),
    ("distributed", "property_modifier"),
    ("final", "inheritance_modifier"),
    ("inout", "parameter_modifier"),
    ("borrowing", "parameter_modifier"),
    ("consuming", "parameter_modifier"),
    ("sending", "parameter_modifier"),
    ("isolated", "parameter_modifier"),
    ("weak", "ownership_modifier"),
    ("unowned", "ownership_modifier"),
    ("lazy", "property_behavior_modifier"),
];

/// Declaration keywords.
const DECLARATIONS: &[&str] = &[
    "import",
    "let",
    "var",
    "typealias",
    "func",
    "init",
    "deinit",
    "subscript",
    "class",
    "struct",
    "actor",
    "extension",
    "enum",
    "indirect",
    "protocol",
    "associatedtype",
    "precedencegroup",
    "macro",
    "operator",
];

impl Parser<'_> {
    fn modifier_kind(&self, n: usize) -> Option<&'static str> {
        let w = self.word_at(n);
        MODIFIERS.iter().find(|(m, _)| *m == w).map(|(_, k)| *k)
    }

    /// Past the attribute at `n` (`@`).
    pub(crate) fn scan_attribute(&self, n: usize) -> Option<usize> {
        let mut k = n + 1;
        if self.kind_at(k) != L::Word {
            return None;
        }
        k += 1;
        while self.is(k, ".") && self.kind_at(k + 1) == L::Word {
            k += 2;
        }
        if self.is(k, "<") {
            k = self.scan_type_arguments(k).unwrap_or(k);
        }
        if self.attribute_arguments_at(k) {
            k = self.scan_balanced(k)?;
        }
        Some(k)
    }

    /// Whether the `(` at `n` holds an attribute's arguments. tree-sitter
    /// takes them after a space too, where they read as arguments
    /// (`@Sendable ()`, `@escaping (String)`), not where they cannot
    /// (`@Sendable (_ r: R) -> V`, `(A<B, C>)`: a function type's).
    fn attribute_arguments_at(&self, n: usize) -> bool {
        if !self.is(n, "(") || self.nl_at(n) {
            return false;
        }
        if self.touches(n) {
            return true;
        }
        let Some(end) = self.scan_balanced(n) else {
            return false;
        };
        if self.is(end, "->") || matches!(self.word_at(end), "async" | "throws" | "rethrows") {
            return false;
        }
        (n + 1..end - 1).all(|k| {
            matches!(
                self.kind_at(k),
                L::Word | L::Int | L::Real | L::Op("." | "," | ":" | "*")
            ) && !(self.kind_at(k) == L::Word && self.kind_at(k + 1) == L::Word)
        })
    }

    /// Past the modifiers at `n`.
    fn scan_modifiers(&self, n: usize) -> usize {
        let mut k = n;
        loop {
            if self.is(k, "@") {
                match self.scan_attribute(k) {
                    Some(e) => k = e,
                    None => return k,
                }
                continue;
            }
            if self.modifier_kind(k).is_some() && self.declaration_follows(k + 1) {
                k += 1;
                // `private(set)`, `unowned(safe)`, `nonisolated(unsafe)`.
                if self.is(k, "(") && self.touches(k) && self.is(k + 2, ")") {
                    k += 3;
                }
                continue;
            }
            return k;
        }
    }

    /// Whether more modifiers and a declaration keyword come at `n`.
    fn declaration_follows(&self, n: usize) -> bool {
        let mut k = n;
        if self.is(k, "(") && self.is(k + 2, ")") {
            k += 3;
        }
        loop {
            if self.is(k, "@") {
                match self.scan_attribute(k) {
                    Some(e) => k = e,
                    None => return false,
                }
                continue;
            }
            if self.keyword_at(k) {
                return true;
            }
            if self.modifier_kind(k).is_some() {
                k += 1;
                if self.is(k, "(") && self.is(k + 2, ")") {
                    k += 3;
                }
                continue;
            }
            return false;
        }
    }

    /// Whether a declaration keyword is at `n`.
    fn keyword_at(&self, n: usize) -> bool {
        let w = self.word_at(n);
        if !DECLARATIONS.contains(&w) {
            return w == "async" && matches!(self.word_at(n + 1), "let" | "var");
        }
        match w {
            "actor" => self.kind_at(n + 1) == L::Word,
            "macro" => self.kind_at(n + 1) == L::Word && self.is(n + 2, "("),
            "operator" => false,
            "init" => {
                matches!(self.kind_at(n + 1), L::Op("(" | "?" | "!" | "<"))
                    || (self.kind_at(n + 1) == L::Custom
                        && matches!(self.text_at(n + 1), "?<" | "!<"))
            }
            "indirect" => self.is(n + 1, "enum") || self.is(n + 1, "case"),
            _ => true,
        }
    }

    /// A declaration starting here, if one does (`local`: in a function's
    /// statements).
    pub(crate) fn declaration_here(&mut self, local: bool) -> Option<Item> {
        // `prefix operator +++`.
        if matches!(self.word(), "prefix" | "infix" | "postfix") && self.is(self.i + 1, "operator")
        {
            return Some(self.operator_declaration());
        }
        let k = self.scan_modifiers(self.i);
        if !self.keyword_at(k) {
            return None;
        }
        if self.word_at(k) == "import" && local {
            return None;
        }
        Some(self.declaration(local))
    }

    pub(crate) fn modifiers(&mut self) -> Item {
        let mut items = Vec::new();
        loop {
            if self.at("@") {
                items.push(self.attribute());
                continue;
            }
            if let Some(kind) = self.modifier_kind(self.i)
                && self.declaration_follows(self.i + 1)
            {
                items.push(self.modifier(kind));
                continue;
            }
            // In a switch, attributes before `case`.
            break;
        }
        node("modifiers", items)
    }

    fn modifier(&mut self, kind: &'static str) -> Item {
        let mut items = vec![self.punct()];
        if self.at("(") && self.touches(self.i) && self.is(self.i + 2, ")") {
            if kind == "ownership_modifier" {
                // `unowned(safe)` is one token.
                let a = self.tk(self.i - 1);
                self.i += 3;
                let end = self.tk(self.i - 1).end;
                let text = &self.src[a.start..end];
                let k = super::names::anonymous(text).unwrap_or("unowned");
                items = vec![Self::tok(crate::lexer::Lx { end, ..a }, k, false)];
            } else {
                items.push(self.punct());
                items.push(self.punct());
                items.push(self.punct());
            }
        }
        node(kind, items)
    }

    pub(crate) fn attribute(&mut self) -> Item {
        let mut items = vec![self.punct()];
        items.push(self.user_type());
        if self.attribute_arguments_at(self.i) {
            items.push(self.punct());
            self.blind(|p| {
                while !p.eof() && !p.at(")") {
                    let before = p.i;
                    p.attribute_argument(&mut items);
                    if p.at(",") {
                        items.push(p.punct());
                    } else if p.i == before {
                        items.push(p.error_token());
                    }
                }
            });
            items.push(self.expect(")"));
        }
        node("attribute", items)
    }

    fn attribute_argument(&mut self, items: &mut Vec<Item>) {
        // `iOS 13.0`, `before: macOS 14`: names, then a version.
        let version = |p: &Self, k: usize| {
            p.kind_at(k) == L::Word
                && matches!(p.kind_at(k + 1), L::Int | L::Real | L::Word)
                && !p.is(k + 1, ":")
        };
        if self.kind_at(self.i) == L::Word && self.is(self.i + 1, ":") {
            items.push(self.simple_identifier());
            items.push(self.punct());
            if version(self, self.i) {
                self.version(items);
            } else if !self.at(")")
                && !self.at(",")
                && !(self.kind_at(self.i) == L::Word && self.is(self.i + 1, ":"))
            {
                items.push(self.expression());
            }
            return;
        }
        if version(self, self.i) {
            self.version(items);
            return;
        }
        items.push(self.expression());
    }

    fn version(&mut self, items: &mut Vec<Item>) {
        while self.kind_at(self.i) == L::Word {
            items.push(self.simple_identifier());
        }
        self.version_numbers(items);
    }

    fn declaration(&mut self, local: bool) -> Item {
        let mut items = Vec::new();
        let k = self.scan_modifiers(self.i);
        if k > self.i {
            let m = self.modifiers();
            if local {
                // In statements the modifiers are the declaration's own.
                if let Item::Node(n) = m {
                    items.extend(n.items);
                }
            } else {
                items.push(m);
            }
        }
        match self.word() {
            "import" => {
                items.push(self.punct());
                if matches!(
                    self.word(),
                    "typealias" | "struct" | "class" | "enum" | "protocol" | "let" | "var" | "func"
                ) {
                    items.push(self.punct());
                }
                items.push(self.identifier());
                node("import_declaration", items)
            }
            "let" | "var" | "async" => self.property_declaration(items),
            "typealias" => {
                items.push(self.punct());
                items.push(self.type_identifier().with("name"));
                if self.at("<") {
                    items.push(self.type_parameters());
                }
                items.push(self.expect("="));
                items.push(self.type_().with("value"));
                node("typealias_declaration", items)
            }
            "func" => self.function_declaration(items, true),
            "init" => self.init_declaration(items),
            "deinit" => {
                items.push(self.punct());
                items.push(self.function_body());
                node("deinit_declaration", items)
            }
            "subscript" => self.subscript_declaration(items),
            "protocol" => self.protocol_declaration(items),
            "associatedtype" => {
                items.push(self.punct());
                items.push(self.type_identifier().with("name"));
                if self.at(":") {
                    items.push(self.punct());
                    items.push(self.type_().with("must_inherit"));
                }
                if self.at("where") {
                    items.push(self.type_constraints());
                }
                if self.at("=") {
                    items.push(self.punct());
                    items.push(self.type_().with("default_value"));
                }
                node("associatedtype_declaration", items)
            }
            "precedencegroup" => {
                items.push(self.punct());
                items.push(self.simple_identifier());
                items.push(self.expect("{"));
                let mut attrs = Vec::new();
                while self.kind_at(self.i) == L::Word && self.is(self.i + 1, ":") {
                    let mut a = vec![self.simple_identifier(), self.punct()];
                    a.push(if matches!(self.word(), "true" | "false") {
                        self.wrapped("boolean_literal")
                    } else {
                        self.simple_identifier()
                    });
                    attrs.push(node("precedence_group_attribute", a));
                }
                if !attrs.is_empty() {
                    items.push(node("precedence_group_attributes", attrs));
                }
                items.push(self.expect("}"));
                node("precedence_group_declaration", items)
            }
            "macro" => {
                items.push(self.punct());
                items.push(self.simple_identifier());
                if self.at("<") {
                    items.push(self.type_parameters());
                }
                self.function_parameters(&mut items);
                if self.at("->") {
                    items.push(self.punct());
                    items.push(self.unannotated_type());
                }
                if self.at("=") {
                    let eq = self.punct();
                    let body = if self.at("#") && self.is(self.i + 1, "externalMacro") {
                        let e = vec![self.punct(), self.punct(), self.value_arguments()];
                        node("external_macro_definition", e)
                    } else {
                        self.expression()
                    };
                    items.push(
                        node("macro_definition", vec![eq, body.with("body")]).with("definition"),
                    );
                }
                if self.at("where") {
                    items.push(self.type_constraints());
                }
                node("macro_declaration", items)
            }
            _ => self.class_declaration(items),
        }
    }

    fn operator_declaration(&mut self) -> Item {
        let mut items = vec![self.punct(), self.punct()];
        items.push(self.operator_token());
        if self.at(":") {
            items.push(self.punct());
            items.push(self.simple_identifier());
        }
        if self.at("{") {
            let mut b = vec![self.punct()];
            while !self.eof() && !self.at("}") {
                b.push(self.primary());
            }
            b.push(self.expect("}"));
            items.push(node("deprecated_operator_declaration_body", b));
        }
        node("operator_declaration", items)
    }

    /// An operator as a name (`func +`, `operator <>`).
    pub(crate) fn operator_token(&mut self) -> Item {
        match self.peek() {
            L::Custom => self.leaf("custom_operator"),
            L::Op("!") => self.leaf("bang"),
            L::Op(o)
                if !matches!(
                    o,
                    "(" | ")" | "[" | "]" | "{" | "}" | "," | ":" | ";" | "@" | "\\"
                ) =>
            {
                self.punct()
            }
            _ => self.missing("custom_operator", true),
        }
    }

    pub(crate) fn identifier(&mut self) -> Item {
        let mut items = vec![self.simple_identifier()];
        while self.at(".") && self.kind_at(self.i + 1) == L::Word {
            items.push(self.punct());
            items.push(self.simple_identifier());
        }
        node("identifier", items)
    }

    pub(crate) fn type_identifier(&mut self) -> Item {
        if self.peek() == L::Word {
            self.name_leaf("type_identifier")
        } else {
            self.missing("type_identifier", true)
        }
    }

    pub(crate) fn value_binding_pattern(&mut self) -> Item {
        let t = self.punct().with("mutability");
        node("value_binding_pattern", vec![t])
    }

    fn property_declaration(&mut self, mut items: Vec<Item>) -> Item {
        if self.at("async") {
            items.push(self.punct());
        }
        items.push(self.value_binding_pattern());
        loop {
            items.push(self.bound_pattern().with("name"));
            if self.at(":") {
                items.push(self.type_annotation());
            }
            if self.at("where") {
                items.push(self.type_constraints());
            }
            if self.at("=") {
                items.push(self.punct());
                items.push(self.property_value().with("value"));
                if self.at("{") && self.observers_follow() {
                    items.push(self.willset_didset_block());
                }
            } else if self.at("{") {
                if self.observers_follow() {
                    items.push(self.willset_didset_block());
                } else {
                    items.push(self.computed_property().with("computed_value"));
                }
            }
            if self.at(",") {
                items.push(self.punct());
                continue;
            }
            break;
        }
        node("property_declaration", items)
    }

    /// A property's value: an expression, a trailing closure after it
    /// calling the call before it as anywhere else (`let a = x.f(1) { }`
    /// is one call); `{ willSet … }` / `{ didSet … }` are its observers.
    fn property_value(&mut self) -> Item {
        self.expression()
    }

    /// At `{`: whether `willSet` / `didSet` start it.
    pub(crate) fn observers_follow(&self) -> bool {
        let mut k = self.i + 1;
        while self.is(k, "@") {
            k = self.scan_attribute(k).unwrap_or(k + 1);
        }
        while self.modifier_kind(k).is_some() {
            k += 1;
        }
        matches!(self.word_at(k), "willSet" | "didSet")
    }

    fn willset_didset_block(&mut self) -> Item {
        let mut items = vec![self.punct()];
        self.sighted(|p| {
            while !p.eof() && !p.at("}") {
                let before = p.i;
                let mut c = Vec::new();
                if p.at("@") || p.modifier_kind(p.i).is_some() {
                    let mut m = Vec::new();
                    while p.at("@") || p.modifier_kind(p.i).is_some() {
                        m.push(if p.at("@") {
                            p.attribute()
                        } else {
                            let k = p.modifier_kind(p.i).unwrap_or("member_modifier");
                            p.modifier(k)
                        });
                    }
                    c.push(node("modifiers", m));
                }
                let kind = if p.at("willSet") {
                    "willset_clause"
                } else {
                    "didset_clause"
                };
                c.push(p.punct());
                if p.at("(") {
                    c.push(p.punct());
                    c.push(p.simple_identifier());
                    c.push(p.expect(")"));
                }
                c.extend(p.block_items());
                items.push(node(kind, c));
                if p.i == before {
                    items.push(p.error_token());
                }
            }
        });
        items.push(self.expect("}"));
        node("willset_didset_block", items)
    }

    /// `{ get … set … }` or `{ statements }`.
    pub(crate) fn computed_property(&mut self) -> Item {
        if !self.accessors_follow() {
            let items = self.block_items();
            return node("computed_property", items);
        }
        let mut items = vec![self.punct()];
        self.sighted(|p| {
            while !p.eof() && !p.at("}") {
                let before = p.i;
                items.push(p.accessor());
                while p.at(";") {
                    let t = p.advance();
                    items.push(Self::hidden_tok(t.start, t.end));
                }
                if p.i == before {
                    items.push(p.error_token());
                }
            }
        });
        items.push(self.expect("}"));
        node("computed_property", items)
    }

    /// At `{`: whether accessors (`get`, `set`, `_modify`) start it.
    fn accessors_follow(&self) -> bool {
        let mut k = self.i + 1;
        while self.is(k, "@") {
            k = self.scan_attribute(k).unwrap_or(k + 1);
        }
        while matches!(self.word_at(k), "mutating" | "nonmutating") {
            k += 1;
        }
        matches!(self.word_at(k), "get" | "set" | "_modify")
            && (matches!(self.kind_at(k + 1), L::Op("{" | "}" | "("))
                || self.nl_at(k + 1)
                || matches!(self.word_at(k + 1), "async" | "throws" | "get" | "set"))
    }

    fn accessor(&mut self) -> Item {
        let mut items = Vec::new();
        while self.at("@") {
            items.push(self.attribute());
        }
        let mut spec = Vec::new();
        if matches!(self.word(), "mutating" | "nonmutating") {
            spec.push(self.wrapped("mutation_modifier"));
        }
        let w = self.word();
        spec.push(self.punct());
        let (kind, spec_kind) = match w {
            "get" => {
                while matches!(self.word(), "async" | "throws" | "rethrows") {
                    spec.push(if self.at("async") {
                        self.punct()
                    } else {
                        self.throws()
                    });
                }
                ("computed_getter", "getter_specifier")
            }
            "set" => ("computed_setter", "setter_specifier"),
            _ => ("computed_modify", "modify_specifier"),
        };
        items.push(node(spec_kind, spec));
        if kind == "computed_setter" && self.at("(") {
            items.push(self.punct());
            items.push(self.simple_identifier());
            items.push(self.expect(")"));
        }
        if self.at("{") {
            items.extend(self.block_items());
        }
        node(kind, items)
    }

    pub(crate) fn throws(&mut self) -> Item {
        if self.is(self.i + 1, "(") && self.touches(self.i + 1) {
            let t = self.advance();
            let mut items = vec![Self::hidden_tok(t.start, t.end), self.punct()];
            items.push(self.unannotated_type().with("type"));
            items.push(self.expect(")"));
            return node("throws_clause", items);
        }
        self.leaf("throws")
    }

    /// Effects and a result: `async`, `throws`, `-> T`.
    fn signature_tail(&mut self, items: &mut Vec<Item>, result: bool) {
        if self.at("async") {
            items.push(self.punct());
        }
        if matches!(self.word(), "throws" | "rethrows") {
            items.push(self.throws());
        }
        if result && self.at("->") {
            items.push(self.punct());
            if self.parameter_modifier_here() {
                items.push(self.parameter_modifiers());
            }
            items.push(self.possibly_unwrapped_type().with("return_type"));
        }
        if self.at("where") {
            items.push(self.type_constraints());
        }
    }

    fn function_declaration(&mut self, mut items: Vec<Item>, body: bool) -> Item {
        if self.at("class") {
            items.push(self.punct());
        }
        items.push(self.punct());
        let name = if self.peek() == L::Word {
            self.simple_identifier()
        } else {
            self.operator_token()
        };
        items.push(name.with("name"));
        if self.at("<") {
            items.push(self.type_parameters());
        }
        self.function_parameters(&mut items);
        self.signature_tail(&mut items, true);
        if self.at("{") {
            items.push(self.function_body());
        } else if body {
            return node("protocol_function_declaration", items);
        }
        node("function_declaration", items)
    }

    pub(crate) fn function_body(&mut self) -> Item {
        let items = self.block_items();
        node("function_body", items).with("body")
    }

    /// `(…)` parameter lists, one or more.
    fn function_parameters(&mut self, items: &mut Vec<Item>) {
        while self.at("(") {
            items.push(self.punct());
            self.blind(|p| {
                while !p.eof() && !p.at(")") {
                    let before = p.i;
                    if p.at("@") && !p.is(p.i + 1, "escaping") && !p.is(p.i + 1, "autoclosure") {
                        items.push(p.attribute());
                    }
                    items.push(p.parameter());
                    if p.at("=") {
                        items.push(p.punct());
                        items.push(p.expression().with("default_value"));
                    }
                    if p.at(",") {
                        items.push(p.punct());
                    } else if p.i == before {
                        items.push(p.error_token());
                    } else {
                        break;
                    }
                }
            });
            items.push(self.expect(")"));
        }
    }

    fn parameter(&mut self) -> Item {
        let mut items = Vec::new();
        if self.kind_at(self.i + 1) == L::Word && !self.is(self.i + 1, ":") {
            items.push(self.simple_identifier().with("external_name"));
        }
        items.push(self.simple_identifier().with("name"));
        items.push(self.expect(":"));
        if self.parameter_modifier_here() {
            items.push(self.parameter_modifiers());
        }
        items.push(self.possibly_unwrapped_type().with("type"));
        if self.at("...") {
            items.push(self.punct());
        }
        node("parameter", items)
    }

    pub(crate) fn parameter_modifier_here(&self) -> bool {
        matches!(
            self.word(),
            "inout" | "borrowing" | "consuming" | "sending" | "isolated"
        ) && matches!(self.kind_at(self.i + 1), L::Word | L::Op("(" | "[" | "@"))
            || (self.at("@")
                && matches!(self.word_at(self.i + 1), "escaping" | "autoclosure")
                && self.touches(self.i + 1))
    }

    pub(crate) fn parameter_modifiers(&mut self) -> Item {
        let mut items = Vec::new();
        while self.parameter_modifier_here() {
            let m = if self.at("@") {
                let a = self.advance();
                let b = self.advance();
                let kind = if &self.src[b.start..b.end] == "escaping" {
                    "@escaping"
                } else {
                    "@autoclosure"
                };
                let t = Self::tok(crate::lexer::Lx { end: b.end, ..a }, kind, false);
                node("parameter_modifier", vec![t])
            } else {
                self.wrapped("parameter_modifier")
            };
            items.push(m);
        }
        node("parameter_modifiers", items)
    }

    fn init_declaration(&mut self, mut items: Vec<Item>) -> Item {
        if self.at("class") {
            items.push(self.punct());
        }
        items.push(self.punct().with("name"));
        // `init?<T>`: `?<` lexes as one operator.
        if self.peek() == L::Custom && matches!(self.text_at(self.i).as_bytes(), [b'?' | b'!', ..])
        {
            self.split_first();
        }
        if self.at("?") {
            items.push(self.punct());
        } else if self.at("!") {
            items.push(self.leaf("bang"));
        }
        if self.at("<") {
            items.push(self.type_parameters());
        }
        self.function_parameters(&mut items);
        self.signature_tail(&mut items, false);
        if self.at("{") {
            items.push(self.function_body());
        }
        node("init_declaration", items)
    }

    fn subscript_declaration(&mut self, mut items: Vec<Item>) -> Item {
        items.push(self.punct());
        if self.at("<") {
            items.push(self.type_parameters());
        }
        self.function_parameters(&mut items);
        if self.at("->") {
            items.push(self.punct());
            if self.parameter_modifier_here() {
                items.push(self.parameter_modifiers());
            }
            items.push(self.possibly_unwrapped_type().with("return_type"));
        }
        if self.at("where") {
            items.push(self.type_constraints());
        }
        if self.at("{") {
            items.push(self.computed_property());
        }
        node("subscript_declaration", items)
    }

    fn inheritance(&mut self, items: &mut Vec<Item>) {
        items.push(self.punct());
        loop {
            while self.at("@") {
                items.push(self.attribute());
            }
            let t = self.unannotated_type().with("inherits_from");
            items.push(node("inheritance_specifier", vec![t]));
            if self.at(",") || self.at("&") {
                items.push(self.punct());
                continue;
            }
            break;
        }
    }

    fn class_declaration(&mut self, mut items: Vec<Item>) -> Item {
        if self.at("indirect") {
            items.push(self.punct());
        }
        let kind = self.word();
        items.push(self.punct().with("declaration_kind"));
        if kind == "extension" {
            items.push(self.unannotated_type().with("name"));
        } else {
            items.push(self.type_identifier().with("name"));
        }
        if self.at("<") {
            items.push(self.type_parameters());
        }
        if self.at(":") {
            self.inheritance(&mut items);
        }
        if self.at("where") {
            items.push(self.type_constraints());
        }
        let body = if kind == "enum" {
            self.enum_class_body()
        } else {
            self.class_body("class_body", false)
        };
        items.push(body.with("body"));
        node("class_declaration", items)
    }

    fn protocol_declaration(&mut self, mut items: Vec<Item>) -> Item {
        items.push(self.punct().with("declaration_kind"));
        items.push(self.type_identifier().with("name"));
        if self.at("<") {
            items.push(self.type_parameters());
        }
        if self.at(":") {
            self.inheritance(&mut items);
        }
        if self.at("where") {
            items.push(self.type_constraints());
        }
        items.push(self.class_body("protocol_body", true).with("body"));
        node("protocol_declaration", items)
    }

    /// A class's or protocol's members between braces, separated as
    /// statements are.
    fn class_body(&mut self, kind: &'static str, protocol: bool) -> Item {
        let mut items = vec![self.expect("{")];
        self.sighted(|p| {
            loop {
                while p.at(";") {
                    let t = p.advance();
                    items.push(Self::hidden_tok(t.start, t.end));
                }
                if p.eof() || p.at("}") {
                    break;
                }
                let before = p.i;
                items.push(p.member(protocol));
                if p.i == before {
                    items.push(p.error_token());
                    continue;
                }
                if p.at("}") && !p.nl_at(p.i) {
                    break;
                }
                match p.semi_item() {
                    Some(s) => items.push(s),
                    None if p.eof() || p.at("}") => break,
                    None => {
                        p.errors = true;
                        items.push(p.error_token());
                    }
                }
            }
        });
        items.push(self.expect("}"));
        node(kind, items)
    }

    fn member(&mut self, protocol: bool) -> Item {
        if matches!(self.peek(), L::Directive(_)) {
            return self.directive();
        }
        if protocol {
            let k = self.scan_modifiers(self.i);
            if matches!(self.word_at(k), "var" | "let") {
                return self.protocol_property();
            }
        }
        match self.declaration_here(false) {
            Some(d) => d,
            None => {
                let e = self.expression();
                node("ERROR", vec![e])
            }
        }
    }

    fn protocol_property(&mut self) -> Item {
        let mut items = Vec::new();
        if self.scan_modifiers(self.i) > self.i {
            items.push(self.modifiers());
        }
        let binding = self.value_binding_pattern();
        let name = if self.peek() == L::Word && !self.at("_") {
            self.simple_identifier().with("bound_identifier")
        } else {
            self.bound_pattern()
        };
        let p = vec![binding, name];
        items.push(node("pattern", p).with("name"));
        if self.at(":") {
            items.push(self.type_annotation());
        }
        if self.at("where") {
            items.push(self.type_constraints());
        }
        let mut r = vec![self.expect("{")];
        while !self.eof() && !self.at("}") {
            let mut spec = Vec::new();
            if matches!(self.word(), "mutating" | "nonmutating") {
                spec.push(self.wrapped("mutation_modifier"));
            }
            match self.word() {
                "get" => {
                    spec.push(self.punct());
                    while matches!(self.word(), "async" | "throws" | "rethrows") {
                        spec.push(if self.at("async") {
                            self.punct()
                        } else {
                            self.throws()
                        });
                    }
                    r.push(node("getter_specifier", spec));
                }
                "set" => {
                    spec.push(self.punct());
                    r.push(node("setter_specifier", spec));
                }
                _ => r.push(self.error_token()),
            }
        }
        r.push(self.expect("}"));
        items.push(node("protocol_property_requirements", r));
        node("protocol_property_declaration", items)
    }

    fn enum_class_body(&mut self) -> Item {
        let mut items = vec![self.expect("{")];
        self.sighted(|p| {
            while !p.eof() && !p.at("}") {
                let before = p.i;
                while p.at(";") {
                    let t = p.advance();
                    items.push(Self::hidden_tok(t.start, t.end));
                }
                if p.at("}") {
                    break;
                }
                let k = p.scan_modifiers(p.i);
                let case = p.is(k, "case") || (p.is(k, "indirect") && p.is(k + 1, "case"));
                items.push(if case {
                    p.enum_entry()
                } else {
                    p.member(false)
                });
                if p.i == before {
                    items.push(p.error_token());
                }
            }
        });
        items.push(self.expect("}"));
        node("enum_class_body", items)
    }

    fn enum_entry(&mut self) -> Item {
        let mut items = Vec::new();
        if self.scan_modifiers(self.i) > self.i {
            items.push(self.modifiers());
        }
        if self.at("indirect") {
            items.push(self.punct());
        }
        items.push(self.punct());
        loop {
            items.push(self.simple_identifier().with("name"));
            if self.at("(") {
                items.push(self.enum_type_parameters().with("data_contents"));
            } else if self.at("=") {
                items.push(self.punct());
                items.push(self.expression().with("raw_value"));
            }
            if self.at(",") {
                items.push(self.punct());
                continue;
            }
            break;
        }
        if self.at(";") {
            items.push(self.punct());
        }
        node("enum_entry", items)
    }

    fn enum_type_parameters(&mut self) -> Item {
        let mut items = vec![self.punct()];
        self.blind(|p| {
            while !p.eof() && !p.at(")") {
                let before = p.i;
                if p.at("_") && p.kind_at(p.i + 1) == L::Word && p.is(p.i + 2, ":") {
                    items.push(p.leaf("wildcard_pattern"));
                }
                if p.kind_at(p.i) == L::Word && p.is(p.i + 1, ":") {
                    items.push(p.simple_identifier());
                    items.push(p.punct());
                }
                items.push(p.type_());
                if p.at("=") {
                    items.push(p.punct());
                    items.push(p.expression());
                }
                if p.at(",") {
                    items.push(p.punct());
                } else if p.i == before {
                    items.push(p.error_token());
                } else {
                    break;
                }
            }
        });
        items.push(self.expect(")"));
        node("enum_type_parameters", items)
    }

    pub(crate) fn type_parameters(&mut self) -> Item {
        let mut items = vec![self.punct()];
        self.blind(|p| {
            while !p.eof() && !p.at_close_angle() {
                let before = p.i;
                if p.at("where") {
                    items.push(p.type_constraints());
                    continue;
                }
                let mut t = Vec::new();
                if p.at("@") {
                    let mut m = Vec::new();
                    while p.at("@") {
                        m.push(p.attribute());
                    }
                    t.push(node("type_parameter_modifiers", m));
                }
                if p.at("let") {
                    t.push(p.punct());
                }
                if p.at("each") && p.kind_at(p.i + 1) == L::Word {
                    let e = vec![p.punct(), p.user_type()];
                    t.push(node("type_parameter_pack", e));
                } else {
                    t.push(p.type_identifier());
                }
                if p.at(":") {
                    t.push(p.punct());
                    t.push(p.type_());
                }
                items.push(node("type_parameter", t));
                if p.at(",") {
                    items.push(p.punct());
                } else if p.i == before {
                    items.push(p.error_token());
                } else if !p.at("where") {
                    break;
                }
            }
        });
        items.push(self.close_angle());
        node("type_parameters", items)
    }

    pub(crate) fn type_constraints(&mut self) -> Item {
        let mut items = vec![self.leaf("where_keyword")];
        loop {
            let mut c = Vec::new();
            while self.at("@") {
                c.push(self.attribute());
            }
            // The constrained type: `T`, `T.Element`, `Self.A.B`.
            let constrained = if self.kind_at(self.i) == L::Word && !self.is(self.i + 1, "<") {
                self.identifier()
            } else {
                self.unannotated_type()
            };
            c.push(constrained.with("constrained_type"));
            let kind = if self.at(":") {
                c.push(self.punct());
                c.push(self.possibly_unwrapped_type().with("inherits_from"));
                "inheritance_constraint"
            } else {
                c.push(if self.at("==") || self.at("=") {
                    self.punct()
                } else {
                    self.missing("==", false)
                });
                c.push(self.type_().with("must_equal"));
                "equality_constraint"
            };
            items.push(node("type_constraint", vec![node(kind, c)]));
            if self.at(",") && !self.at_close_angle() {
                items.push(self.punct());
                continue;
            }
            break;
        }
        node("type_constraints", items)
    }

    pub(crate) fn type_annotation(&mut self) -> Item {
        let items = vec![self.punct(), self.possibly_unwrapped_type().with("type")];
        node("type_annotation", items)
    }

    /// A binding's name pattern: a name, a tuple, `_`.
    pub(crate) fn bound_pattern(&mut self) -> Item {
        let mut items = Vec::new();
        if self.at("(") {
            items.push(self.tuple_pattern());
        } else if self.at("_") {
            items.push(self.leaf("wildcard_pattern"));
        } else {
            items.push(self.simple_identifier().with("bound_identifier"));
        }
        if self.at("?") && self.touches(self.i) {
            items.push(self.punct());
        }
        node("pattern", items)
    }

    /// Hidden: the version numbers of an attribute or availability
    /// argument (`13.0.1`), which lex as reals.
    pub(crate) fn version_numbers(&mut self, items: &mut Vec<Item>) {
        while matches!(self.peek(), L::Int | L::Real)
            || (self.at(".") && matches!(self.kind_at(self.i + 1), L::Int | L::Real))
        {
            if self.peek() == L::Real {
                let t = self.advance();
                let text = &self.src[t.start..t.end];
                let mut at = t.start;
                for (k, part) in text.split('.').enumerate() {
                    if k > 0 {
                        items.push(Self::tok(
                            crate::lexer::Lx {
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
                        crate::lexer::Lx {
                            start: at,
                            end: at + part.len(),
                            ..t
                        },
                        "integer_literal",
                        true,
                    ));
                    at += part.len();
                }
            } else if self.at(".") {
                items.push(self.punct());
            } else {
                items.push(self.leaf("integer_literal"));
            }
        }
        let _ = hidden;
    }
}
