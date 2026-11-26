//! Declarations: namespaces and `use`, functions, classes, interfaces,
//! traits and enums with their members; parameters, types and attributes.

use super::{Item, Parser, node};
use crate::lexer::L;

/// `primitive_type`'s words that are tokens of their own (case as written
/// in the grammar); the others are leaves.
const PRIMITIVE_TOKENS: &[&str] = &["array", "bool", "float", "int", "null", "object", "string"];
const PRIMITIVE_WORDS: &[&str] = &["callable", "false", "iterable", "mixed", "true", "void"];

const MODIFIERS: &[&str] = &[
    "var",
    "public",
    "protected",
    "private",
    "static",
    "final",
    "abstract",
    "readonly",
];

impl Parser<'_> {
    // ---- names ------------------------------------------------------------

    /// `A\B\C` as a `namespace_name`.
    pub(crate) fn namespace_name(&mut self) -> Item {
        let mut items = vec![self.any_name()];
        while self.at("\\") && self.kind_at(self.i + 1) == L::Name {
            items.push(self.punct());
            items.push(self.any_name());
        }
        node("namespace_name", items)
    }

    /// Whether a qualified name (`\A`, `A\B`) or a relative one
    /// (`namespace\A`) starts here.
    pub(crate) fn qualified_here(&self) -> bool {
        (self.at("\\") && self.kind_at(self.i + 1) == L::Name)
            || (self.peek() == L::Name
                && self.is(self.i + 1, "\\")
                && self.kind_at(self.i + 2) == L::Name)
    }

    /// A `_name`: a name, a qualified or a relative name.
    pub(crate) fn class_name(&mut self) -> Item {
        if self.at_kw("namespace") && self.is(self.i + 1, "\\") {
            let mut prefix = vec![self.punct()];
            // `namespace\A\B`: the namespace part, then the last name.
            let mut k = self.i + 1;
            let mut parts = 0;
            while self.kind_at(k) == L::Name && self.is(k + 1, "\\") {
                parts += 1;
                k += 2;
            }
            if parts > 0 {
                prefix.push(self.punct());
                let mut ns = vec![self.any_name()];
                for _ in 1..parts {
                    ns.push(self.punct());
                    ns.push(self.any_name());
                }
                prefix.push(node("namespace_name", ns));
            }
            prefix.push(self.expect("\\"));
            let mut items: Vec<Item> = prefix.into_iter().map(|p| p.with("prefix")).collect();
            items.push(self.any_name());
            return node("relative_name", items);
        }
        if !self.qualified_here() {
            return if self.peek() == L::Name {
                self.leaf("name")
            } else {
                self.missing("name", true)
            };
        }
        let mut prefix = Vec::new();
        if self.at("\\") {
            prefix.push(self.punct());
        }
        // `A\B\C`: the names before the last are the namespace part.
        let mut names = 1;
        let mut k = self.i;
        while self.is(k + 1, "\\") && self.kind_at(k + 2) == L::Name {
            names += 1;
            k += 2;
        }
        if names > 1 {
            let mut ns = vec![self.any_name()];
            for _ in 1..names - 1 {
                ns.push(self.punct());
                ns.push(self.any_name());
            }
            prefix.push(node("namespace_name", ns));
            prefix.push(self.punct());
        }
        let mut items: Vec<Item> = prefix.into_iter().map(|p| p.with("prefix")).collect();
        items.push(self.any_name());
        node("qualified_name", items)
    }

    // ---- `use` ------------------------------------------------------------

    pub(crate) fn namespace_use_declaration(&mut self) -> Item {
        let mut items = vec![self.punct()];
        // `use function A\{b, c};`: a group, its type on the declaration.
        let typed =
            (self.at_kw("function") || self.at_kw("const")) && self.kind_at(self.i + 1) == L::Name;
        let mut k = self.i + usize::from(typed);
        if self.is(k, "\\") {
            k += 1;
        }
        while self.kind_at(k) == L::Name && self.is(k + 1, "\\") {
            k += 2;
        }
        if self.is(k, "{") && k > self.i {
            if typed {
                items.push(self.punct().with("type"));
            }
            if self.at("\\") {
                items.push(self.punct());
            }
            let mut ns = vec![self.any_name()];
            while self.at("\\") && self.kind_at(self.i + 1) == L::Name {
                ns.push(self.punct());
                ns.push(self.any_name());
            }
            items.push(node("namespace_name", ns));
            items.push(self.expect("\\"));
            let mut g = vec![self.punct()];
            self.comma_list(&mut g, "}", true, Self::namespace_use_clause);
            g.push(self.expect("}"));
            items.push(node("namespace_use_group", g).with("body"));
        } else {
            self.comma_list(&mut items, ";", false, Self::namespace_use_clause);
        }
        self.semicolon(&mut items);
        node("namespace_use_declaration", items)
    }

    fn namespace_use_clause(&mut self) -> Item {
        let mut items = Vec::new();
        if (self.at_kw("function") || self.at_kw("const"))
            && (self.kind_at(self.i + 1) == L::Name || self.is(self.i + 1, "\\"))
        {
            items.push(self.punct().with("type"));
        }
        items.push(self.class_name());
        if self.at_kw("as") {
            items.push(self.punct());
            items.push(self.any_name().with("alias"));
        }
        node("namespace_use_clause", items)
    }

    // ---- attributes -------------------------------------------------------

    pub(crate) fn attribute_list(&mut self) -> Item {
        let mut groups = Vec::new();
        while self.at("#[") {
            let mut g = vec![self.punct()];
            self.comma_list(&mut g, "]", true, |p| {
                let mut a = vec![p.class_name()];
                if p.at("(") {
                    a.push(p.arguments().with("parameters"));
                }
                node("attribute", a)
            });
            g.push(self.expect("]"));
            groups.push(node("attribute_group", g));
        }
        node("attribute_list", groups)
    }

    /// `#[…]` then a declaration; `None` (nothing read) for an attributed
    /// closure, which is an expression.
    pub(crate) fn attributed_declaration(&mut self) -> Option<Item> {
        let mut k = self.i;
        while self.is(k, "#[") {
            k = self.scan_balanced(k)?;
        }
        let w = self.word_at(k).to_ascii_lowercase();
        let decl = match w.as_str() {
            "function" => {
                self.kind_at(k + 1) == L::Name
                    || self.is(k + 1, "&") && self.kind_at(k + 2) == L::Name
            }
            "class" | "interface" | "trait" | "enum" | "abstract" | "final" | "readonly" => true,
            "const" => true,
            _ => false,
        };
        if !decl {
            return None;
        }
        let attrs = vec![self.attribute_list().with("attributes")];
        Some(match w.as_str() {
            "function" => self.function_definition(attrs),
            "const" => self.const_declaration(attrs, false),
            _ => self.type_declaration(attrs),
        })
    }

    // ---- functions --------------------------------------------------------

    pub(crate) fn function_definition(&mut self, mut items: Vec<Item>) -> Item {
        items.push(self.punct());
        if self.at("&") {
            items.push(self.wrapped("reference_modifier"));
        }
        items.push(self.any_name().with("name"));
        items.push(self.formal_parameters().with("parameters"));
        self.return_type(&mut items);
        items.push(self.compound().with("body"));
        node("function_definition", items)
    }

    pub(crate) fn return_type(&mut self, items: &mut Vec<Item>) {
        if self.at(":") {
            items.push(self.punct());
            let t = if self.at_kw("never") {
                self.leaf("bottom_type")
            } else {
                self.type_()
            };
            items.push(t.with("return_type"));
        }
    }

    pub(crate) fn formal_parameters(&mut self) -> Item {
        let mut items = vec![self.expect("(")];
        self.comma_list(&mut items, ")", true, Self::parameter);
        items.push(self.expect(")"));
        node("formal_parameters", items)
    }

    fn parameter(&mut self) -> Item {
        let mut items = Vec::new();
        if self.at("#[") {
            items.push(self.attribute_list().with("attributes"));
        }
        let promoted = matches!(
            self.word().to_ascii_lowercase().as_str(),
            "public" | "protected" | "private" | "readonly"
        );
        if promoted {
            while matches!(
                self.word().to_ascii_lowercase().as_str(),
                "public" | "protected" | "private"
            ) {
                items.push(self.visibility_modifier().with("visibility"));
            }
            if self.at_kw("readonly") {
                items.push(self.wrapped("readonly_modifier").with("readonly"));
            }
            // `readonly public`: the visibility after.
            while matches!(
                self.word().to_ascii_lowercase().as_str(),
                "public" | "protected" | "private"
            ) {
                items.push(self.visibility_modifier().with("visibility"));
            }
        }
        if !self.at("$") && !self.at("&") && !self.at("...") {
            items.push(self.type_().with("type"));
        }
        if promoted {
            let name = if self.at("&") {
                self.by_ref()
            } else {
                self.simple_variable()
            };
            items.push(name.with("name"));
            if self.at("=") {
                items.push(self.punct());
                items.push(self.expression().with("default_value"));
            }
            if self.at("{") {
                items.push(self.property_hook_list());
            }
            return node("property_promotion_parameter", items);
        }
        if self.at("&") {
            items.push(
                self.wrapped("reference_modifier")
                    .with("reference_modifier"),
            );
        }
        if self.at("...") {
            items.push(self.punct());
            items.push(self.simple_variable().with("name"));
            return node("variadic_parameter", items);
        }
        items.push(self.simple_variable().with("name"));
        if self.at("=") {
            items.push(self.punct());
            items.push(self.expression().with("default_value"));
        }
        node("simple_parameter", items)
    }

    // ---- types ------------------------------------------------------------

    /// A type: one, a union, an intersection, or a DNF type.
    pub(crate) fn type_(&mut self) -> Item {
        let mut pieces = Vec::new();
        let mut dnf = false;
        let mut union = false;
        loop {
            if self.at("(") {
                dnf = true;
                let open = self.punct();
                let inner = self.intersection_or_single();
                let close = self.expect(")");
                pieces.push(open);
                pieces.push(inner);
                pieces.push(close);
            } else {
                pieces.push(self.intersection_or_single());
            }
            if self.at("|") {
                union = true;
                pieces.push(self.punct());
                continue;
            }
            break;
        }
        if dnf {
            return node("disjunctive_normal_form_type", pieces);
        }
        if union {
            return node("union_type", pieces);
        }
        pieces.pop().unwrap_or_else(|| self.missing("name", true))
    }

    fn intersection_or_single(&mut self) -> Item {
        let first = self.single_type();
        if !(self.at("&") && self.type_starts(self.i + 1)) {
            return first;
        }
        let mut items = vec![first];
        while self.at("&") && self.type_starts(self.i + 1) {
            items.push(self.punct());
            items.push(self.single_type());
        }
        node("intersection_type", items)
    }

    /// Whether a type (not a parameter's variable) starts at `n`.
    fn type_starts(&self, n: usize) -> bool {
        matches!(self.kind_at(n), L::Name | L::Op("?" | "\\" | "("))
    }

    /// `_types`: an optional, a named or a primitive type.
    fn single_type(&mut self) -> Item {
        if self.at("?") {
            let q = self.punct();
            let t = self.named_or_primitive();
            return node("optional_type", vec![q, t]);
        }
        self.named_or_primitive()
    }

    fn named_or_primitive(&mut self) -> Item {
        let w = self.word();
        if PRIMITIVE_TOKENS.contains(&w) {
            return self.wrapped("primitive_type");
        }
        if PRIMITIVE_WORDS.iter().any(|p| p.eq_ignore_ascii_case(w)) && !self.is(self.i + 1, "\\") {
            return self.leaf("primitive_type");
        }
        self.named_type()
    }

    pub(crate) fn named_type(&mut self) -> Item {
        let n = self.class_name();
        node("named_type", vec![n])
    }

    // ---- constants --------------------------------------------------------

    /// `const` (with `items` read before it: attributes, modifiers) and its
    /// elements; in a class when `member`.
    pub(crate) fn const_declaration(&mut self, mut items: Vec<Item>, member: bool) -> Item {
        items.push(self.punct());
        // A type when the word after `const` is not the element's name.
        if !(self.kind_at(self.i) == L::Name && self.is(self.i + 1, "=")) && !self.at(";") {
            items.push(self.type_().with("type"));
        }
        self.comma_list(&mut items, ";", false, |p| {
            let name = if member || p.peek() == L::Name {
                p.any_name()
            } else {
                p.name()
            };
            let v = vec![name, p.expect("="), p.expression()];
            node("const_element", v)
        });
        self.semicolon(&mut items);
        node("const_declaration", items)
    }

    // ---- classes ----------------------------------------------------------

    fn visibility_modifier(&mut self) -> Item {
        let mut items = vec![self.punct()];
        // `public(set)`.
        if self.at("(")
            && self.adjacent(self.i - 1)
            && self.kind_at(self.i + 1) == L::Name
            && self.is(self.i + 2, ")")
        {
            items.push(self.punct());
            items.push(self.leaf("operation"));
            items.push(self.punct());
        }
        node("visibility_modifier", items)
    }

    /// One modifier keyword as its node.
    fn modifier(&mut self) -> Item {
        match self.word().to_ascii_lowercase().as_str() {
            "var" => self.leaf("var_modifier"),
            "static" => self.wrapped("static_modifier"),
            "final" => self.wrapped("final_modifier"),
            "abstract" => self.wrapped("abstract_modifier"),
            "readonly" => self.wrapped("readonly_modifier"),
            _ => self.visibility_modifier(),
        }
    }

    fn at_modifier(&self) -> bool {
        MODIFIERS.iter().any(|m| self.at_kw(m))
    }

    /// A class, interface, trait or enum declaration (`items`: its
    /// attributes).
    pub(crate) fn type_declaration(&mut self, mut items: Vec<Item>) -> Item {
        while self.at_modifier() && !self.at_kw("class") {
            items.push(self.modifier());
        }
        let w = self.word().to_ascii_lowercase();
        items.push(self.punct());
        items.push(self.name_or_missing().with("name"));
        let kind = match w.as_str() {
            "interface" => {
                if self.at_kw("extends") {
                    items.push(self.base_clause());
                }
                items.push(self.declaration_list("declaration_list").with("body"));
                "interface_declaration"
            }
            "trait" => {
                items.push(self.declaration_list("declaration_list").with("body"));
                "trait_declaration"
            }
            "enum" => {
                if self.at(":") {
                    items.push(self.punct());
                    items.push(self.leaf("primitive_type"));
                }
                if self.at_kw("implements") {
                    items.push(self.interface_clause());
                }
                items.push(self.declaration_list("enum_declaration_list").with("body"));
                "enum_declaration"
            }
            _ => {
                if self.at_kw("extends") {
                    items.push(self.base_clause());
                }
                if self.at_kw("implements") {
                    items.push(self.interface_clause());
                }
                items.push(self.declaration_list("declaration_list").with("body"));
                "class_declaration"
            }
        };
        node(kind, items)
    }

    fn name_or_missing(&mut self) -> Item {
        if self.peek() == L::Name {
            self.leaf("name")
        } else {
            self.missing("name", true)
        }
    }

    pub(crate) fn base_clause(&mut self) -> Item {
        let mut items = vec![self.punct()];
        self.comma_list(&mut items, "{", false, Self::class_name);
        node("base_clause", items)
    }

    pub(crate) fn interface_clause(&mut self) -> Item {
        let mut items = vec![self.punct()];
        self.comma_list(&mut items, "{", false, Self::class_name);
        node("class_interface_clause", items)
    }

    pub(crate) fn declaration_list(&mut self, kind: &'static str) -> Item {
        let mut items = vec![self.expect("{")];
        while !self.eof() && !self.at("}") {
            let before = self.i;
            items.push(self.member());
            if self.i == before {
                items.push(self.error_token());
            }
        }
        items.push(self.expect("}"));
        node(kind, items)
    }

    fn member(&mut self) -> Item {
        if self.at_kw("use") {
            return self.use_declaration();
        }
        let mut items = Vec::new();
        if self.at("#[") {
            items.push(self.attribute_list().with("attributes"));
        }
        if self.at_kw("case") {
            items.push(self.punct());
            items.push(self.any_name().with("name"));
            if self.at("=") {
                items.push(self.punct());
                items.push(self.expression().with("value"));
            }
            self.semicolon(&mut items);
            return node("enum_case", items);
        }
        let mut modifiers = 0;
        while self.at_modifier() {
            // `static function` / `static fn` in a property's default is
            // not here; a modifier before `function` / `const` is.
            items.push(self.modifier());
            modifiers += 1;
        }
        if self.at_kw("const") {
            return self.const_declaration(items, true);
        }
        if self.at_kw("function") {
            items.push(self.punct());
            if self.at("&") {
                items.push(self.wrapped("reference_modifier"));
            }
            items.push(self.any_name().with("name"));
            items.push(self.formal_parameters().with("parameters"));
            self.return_type(&mut items);
            if self.at("{") {
                items.push(self.compound().with("body"));
            } else {
                self.semicolon(&mut items);
            }
            return node("method_declaration", items);
        }
        if modifiers == 0 {
            let e = self.error_statement();
            items.push(e);
            return node("ERROR", items);
        }
        if !self.at("$") {
            items.push(self.type_().with("type"));
        }
        self.comma_list(&mut items, ";", false, |p| {
            let mut v = vec![p.simple_variable().with("name")];
            if p.at("=") {
                v.push(p.punct());
                v.push(p.expression().with("default_value"));
            }
            node("property_element", v)
        });
        if self.at("{") {
            items.push(self.property_hook_list());
        } else {
            self.semicolon(&mut items);
        }
        node("property_declaration", items)
    }

    fn property_hook_list(&mut self) -> Item {
        let mut items = vec![self.punct()];
        while !self.eof() && !self.at("}") {
            let before = self.i;
            let mut h = Vec::new();
            if self.at("#[") {
                h.push(self.attribute_list().with("attributes"));
            }
            if self.at_kw("final") {
                h.push(self.wrapped("final_modifier").with("final"));
            }
            if self.at("&") {
                h.push(
                    self.wrapped("reference_modifier")
                        .with("reference_modifier"),
                );
            }
            h.push(self.any_name());
            if self.at("(") {
                h.push(self.formal_parameters().with("parameters"));
            }
            if self.at("=>") {
                h.push(self.punct());
                h.push(self.expression().with("body"));
                self.semicolon(&mut h);
            } else if self.at("{") {
                h.push(self.compound().with("body"));
            } else {
                self.semicolon(&mut h);
            }
            items.push(node("property_hook", h));
            if self.i == before {
                items.push(self.error_token());
            }
        }
        items.push(self.expect("}"));
        node("property_hook_list", items)
    }

    /// A trait `use` in a class.
    fn use_declaration(&mut self) -> Item {
        let mut items = vec![self.punct()];
        self.comma_list(&mut items, ";", false, Self::class_name);
        if !self.at("{") {
            self.semicolon(&mut items);
            return node("use_declaration", items);
        }
        let mut list = vec![self.punct()];
        while !self.eof() && !self.at("}") {
            let before = self.i;
            let target = if self.is(self.i + 1, "::")
                || self.qualified_here() && {
                    let mut k = self.i;
                    while self.kind_at(k) == L::Name || self.is(k, "\\") {
                        k += 1;
                    }
                    self.is(k, "::")
                } {
                let scope = self.class_name();
                let c = self.punct();
                let name = self.any_name();
                node("class_constant_access_expression", vec![scope, c, name])
            } else {
                self.any_name()
            };
            let clause = if self.at_kw("insteadof") {
                let mut v = vec![target, self.punct()];
                self.comma_list(&mut v, ";", false, Self::class_name);
                node("use_instead_of_clause", v)
            } else {
                let mut v = vec![target, self.expect_kw("as")];
                if matches!(
                    self.word().to_ascii_lowercase().as_str(),
                    "public" | "protected" | "private"
                ) {
                    v.push(self.visibility_modifier());
                }
                if self.peek() == L::Name {
                    v.push(self.any_name());
                }
                node("use_as_clause", v)
            };
            list.push(clause);
            self.semicolon(&mut list);
            if self.i == before {
                list.push(self.error_token());
            }
        }
        list.push(self.expect("}"));
        items.push(node("use_list", list));
        node("use_declaration", items)
    }

    /// `&$x` (a `_variable` after `&`).
    pub(crate) fn by_ref(&mut self) -> Item {
        let amp = self.punct();
        let v = self.unary_operand_variable();
        node("by_ref", vec![amp, v])
    }

    /// A class body after `class` in `new class(…) extends … {}`.
    pub(crate) fn anonymous_class(&mut self, mut items: Vec<Item>) -> Item {
        while self.at_modifier() && !self.at_kw("class") {
            items.push(self.modifier());
        }
        items.push(self.punct());
        if self.at("(") {
            items.push(self.arguments());
        }
        if self.at_kw("extends") {
            items.push(self.base_clause());
        }
        if self.at_kw("implements") {
            items.push(self.interface_clause());
        }
        items.push(self.declaration_list("declaration_list").with("body"));
        node("anonymous_class", items)
    }
}
