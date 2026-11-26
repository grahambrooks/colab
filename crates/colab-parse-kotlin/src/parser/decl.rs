//! Declarations: classes, objects, functions, properties and type aliases,
//! their modifiers and annotations, parameters, accessors and bodies.

use super::{Ctx, Item, MODIFIERS, Parser, node};
use crate::lexer::L;

const USE_SITES: &[&str] = &[
    "field", "property", "get", "set", "receiver", "param", "setparam", "delegate",
];

impl Parser<'_> {
    /// Past the annotation at `n` (`@`): where it ends and where its
    /// arguments start, if it has any.
    pub(crate) fn scan_annotation(&self, n: usize) -> Option<(usize, Option<usize>)> {
        let mut k = n + 1;
        if USE_SITES.contains(&self.word_at(k)) && self.is(k + 1, ":") {
            k += 2;
        }
        if self.is(k, "[") {
            return Some((self.scan_balanced(k)?, None));
        }
        if self.kind_at(k) != L::Word {
            return None;
        }
        k += 1;
        while self.is(k, ".") && self.kind_at(k + 1) == L::Word {
            k += 2;
        }
        if self.is(k, "<") {
            k = self.scan_type_arguments(k)?;
        }
        if self.is(k, "(") && self.tk(k - 1).end == self.tk(k).start {
            let args = k;
            return Some((self.scan_balanced(k)?, Some(args)));
        }
        Some((k, None))
    }

    /// Past the modifiers at `n`: where they end, and where the last
    /// annotation's arguments start and end, if it had any.
    fn scan_modifiers(&self, n: usize) -> (usize, Option<(usize, usize)>) {
        let mut k = n;
        let mut last_args = None;
        loop {
            if self.is(k, "@") {
                match self.scan_annotation(k) {
                    Some((end, args)) => {
                        last_args = args.map(|a| (a, end));
                        k = end;
                    }
                    None => break,
                }
                continue;
            }
            if MODIFIERS.iter().any(|(m, _)| *m == self.word_at(k)) {
                last_args = None;
                k += 1;
                continue;
            }
            break;
        }
        (k, last_args)
    }

    /// The declaration keyword at `k` (after modifiers), in `ctx`.
    fn declaration_keyword(&self, k: usize, members: bool) -> Option<&'static str> {
        Some(match self.word_at(k) {
            "class" => "class",
            "interface" => "class",
            "fun" if self.is(k + 1, "interface") => "class",
            "fun" if !self.is(k + 1, "(") => "fun",
            "object" if self.kind_at(k + 1) == L::Word => "object",
            "val" | "var" => "property",
            "typealias" => "typealias",
            "companion" if self.is(k + 1, "object") => "companion",
            "init" if members && self.is(k + 1, "{") => "init",
            "constructor" if members && self.is(k + 1, "(") => "constructor",
            _ => return None,
        })
    }

    /// A declaration starting here, if one does.
    /// A declaration starting here, if one does.
    ///
    /// Annotations before a declaration are its modifiers. tree-sitter's
    /// parse sometimes reads them, and the declaration, as an annotated
    /// expression instead (`@Ann(x)` and a parenthesized `(x)`; or the
    /// declaration as an infix expression, a class body as a lambda):
    /// both readings are valid to its grammar, and which one its GLR parse
    /// keeps depends on how many others it is keeping at the time. The
    /// declaration is the reading it keeps most often, and Kotlin's.
    pub(crate) fn declaration_here(&mut self) -> Option<Item> {
        let (k, _) = self.scan_modifiers(self.i);
        let kw = self.declaration_keyword(k, self.members)?;
        Some(self.declaration(kw))
    }

    /// Whether the brackets `open..end` hold one plain expression (a
    /// parenthesized expression's).
    pub(crate) fn one_expression(&self, open: usize, end: usize) -> bool {
        if end <= open + 2 {
            return false;
        }
        let mut depth = 0usize;
        !(open + 1..end - 1).any(|k| {
            match self.kind_at(k) {
                L::Op("(" | "[" | "{") | L::InterpOpen => depth += 1,
                L::Op(")" | "]" | "}") => depth = depth.saturating_sub(1),
                L::Op("," | "=") if depth == 0 => return true,
                _ => {}
            }
            false
        })
    }

    /// A class member: a declaration, else what is there as an error.
    pub(crate) fn member(&mut self) -> Item {
        let (k, _) = self.scan_modifiers(self.i);
        if let Some(kw) = self.declaration_keyword(k, true) {
            return self.declaration(kw);
        }
        let e = self.expression();
        node("ERROR", vec![e])
    }

    fn declaration(&mut self, kw: &str) -> Item {
        let mut items = Vec::new();
        if self.at("@") || MODIFIERS.iter().any(|(m, _)| *m == self.word()) {
            items.push(self.modifiers());
        }
        match kw {
            "class" => self.class_declaration(items),
            "object" => {
                items.push(self.punct());
                items.push(self.identifier().with("name"));
                if self.at(":") {
                    items.push(self.punct());
                    items.push(self.delegation_specifiers());
                }
                if self.at("{") {
                    items.push(self.class_body());
                }
                node("object_declaration", items)
            }
            "companion" => {
                items.push(self.punct());
                items.push(self.punct());
                if self.kind_at(self.i) == L::Word && !self.stops() {
                    items.push(self.identifier().with("name"));
                }
                if self.at(":") {
                    items.push(self.punct());
                    items.push(self.delegation_specifiers());
                }
                if self.at("{") {
                    items.push(self.class_body());
                }
                node("companion_object", items)
            }
            "init" => {
                items.push(self.punct());
                items.push(self.block());
                node("anonymous_initializer", items)
            }
            "constructor" => {
                items.push(self.punct());
                items.push(self.function_value_parameters());
                if self.at(":") {
                    items.push(self.punct());
                    let mut c = vec![if self.at("this") || self.at("super") {
                        self.punct()
                    } else {
                        self.missing("this", false)
                    }];
                    c.push(self.value_arguments());
                    items.push(node("constructor_delegation_call", c));
                }
                if self.at("{") {
                    items.push(self.block());
                }
                node("secondary_constructor", items)
            }
            "fun" => self.function_declaration(items),
            "property" => self.property_declaration(items),
            _ => {
                items.push(self.punct());
                items.push(self.identifier().with("type"));
                if self.at("<") {
                    items.push(self.type_parameters());
                }
                items.push(self.expect("="));
                items.push(self.type_());
                node("type_alias", items)
            }
        }
    }

    pub(crate) fn modifiers(&mut self) -> Item {
        let mut items = Vec::new();
        loop {
            if self.at("@") {
                items.push(self.annotation());
                continue;
            }
            let w = self.word();
            if let Some((_, kind)) = MODIFIERS.iter().find(|(m, _)| *m == w) {
                items.push(self.wrapped(kind));
                continue;
            }
            break;
        }
        node("modifiers", items)
    }

    pub(crate) fn annotation(&mut self) -> Item {
        let mut items = vec![self.punct()];
        if USE_SITES.contains(&self.word()) && self.is(self.i + 1, ":") {
            let v = vec![self.punct(), self.punct()];
            items.push(node("use_site_target", v));
        }
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
        node("annotation", items)
    }

    /// An annotation's type and its arguments, when they touch it
    /// (`@Composable () -> Unit` is an annotated function type).
    pub(crate) fn unescaped_annotation(&mut self) -> Item {
        let t = self.user_type_only();
        if self.at("(") && self.prev_end() == self.tk(self.i).start {
            let args = self.value_arguments();
            return node("constructor_invocation", vec![t, args]);
        }
        t
    }

    fn class_declaration(&mut self, mut items: Vec<Item>) -> Item {
        let enum_ = items.iter().any(|m| match m {
            Item::Node(n) => n
                .items
                .iter()
                .any(|i| matches!(i, Item::Node(c) if c.kind == "class_modifier" && self.text_of(i) == "enum")),
            Item::Tok(_) => false,
        });
        if self.at("fun") {
            items.push(self.punct());
        }
        items.push(self.punct());
        items.push(self.identifier().with("name"));
        if self.at("<") {
            items.push(self.type_parameters());
        }
        // A primary constructor: parameters, or modifiers and `constructor`.
        let (k, _) = self.scan_modifiers(self.i);
        if self.at("(") || (self.is(k, "constructor") && self.is(k + 1, "(")) {
            let mut c = Vec::new();
            if !self.at("(") {
                if k > self.i {
                    c.push(self.modifiers());
                }
                c.push(self.punct());
            }
            c.push(self.class_parameters());
            items.push(node("primary_constructor", c));
        }
        if self.at(":") {
            items.push(self.punct());
            items.push(self.delegation_specifiers());
        }
        if self.at("where") {
            items.push(self.type_constraints());
        }
        if self.at("{") {
            items.push(if enum_ {
                self.enum_class_body()
            } else {
                self.class_body()
            });
        }
        node("class_declaration", items)
    }

    pub(crate) fn text_of(&self, item: &Item) -> &str {
        &self.src[item.start()..item.end()]
    }

    fn class_parameters(&mut self) -> Item {
        let mut items = vec![self.punct()];
        self.blind(|p| {
            p.comma_list(&mut items, ")", |p| {
                let mut c = Vec::new();
                if p.at("@")
                    || MODIFIERS.iter().any(|(m, _)| *m == p.word())
                        && p.kind_at(p.i + 1) == L::Word
                {
                    c.push(p.modifiers());
                }
                if p.at("val") || p.at("var") {
                    c.push(p.punct());
                }
                c.push(p.identifier());
                c.push(p.expect(":"));
                c.push(p.type_());
                if p.at("=") {
                    c.push(p.punct());
                    c.push(p.expression());
                }
                node("class_parameter", c)
            });
        });
        items.push(self.expect(")"));
        node("class_parameters", items)
    }

    /// `item (, item)* [,]` until `close` (not taken).
    pub(crate) fn comma_list(
        &mut self,
        items: &mut Vec<Item>,
        close: &str,
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
                if self.at(close) {
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

    pub(crate) fn delegation_specifiers(&mut self) -> Item {
        let mut items = Vec::new();
        loop {
            let mut d = Vec::new();
            while self.at("@") {
                d.push(self.annotation());
            }
            let t = self.type_();
            if self.at("(") && !self.nl_at(self.i) {
                let args = self.value_arguments();
                d.push(node("constructor_invocation", vec![t, args]));
            } else if self.at("by") {
                let by = self.punct();
                // At the file's top, `: I by d {` reads the class body as
                // `d`'s lambda.
                let lambda = items.is_empty()
                    && self.top
                    && self.is_ident(self.i)
                    && self.is(self.i + 1, "{");
                let e = self.delegate_target(lambda);
                d.push(node("explicit_delegation", vec![t, by, e]));
            } else {
                d.push(t);
            }
            items.push(node("delegation_specifier", d));
            if self.at(",") {
                items.push(self.punct());
                continue;
            }
            break;
        }
        node("delegation_specifiers", items)
    }

    pub(crate) fn class_body(&mut self) -> Item {
        self.sighted(true, |p| {
            let mut items = vec![p.expect("{")];
            p.statements(&mut items, Ctx::Members, |_| false);
            items.push(p.expect("}"));
            node("class_body", items)
        })
    }

    fn enum_class_body(&mut self) -> Item {
        self.sighted(true, |p| {
            let mut items = vec![p.expect("{")];
            while !p.eof() && !p.at("}") && !p.at(";") {
                let before = p.i;
                let mut e = Vec::new();
                if p.at("@")
                    || MODIFIERS.iter().any(|(m, _)| *m == p.word())
                        && p.kind_at(p.i + 1) == L::Word
                {
                    e.push(p.modifiers());
                }
                e.push(p.identifier());
                if p.at("(") {
                    e.push(p.value_arguments());
                }
                if p.at("{") {
                    e.push(p.class_body());
                }
                items.push(node("enum_entry", e));
                if p.at(",") {
                    items.push(p.punct());
                } else if p.i == before {
                    items.push(p.error_token());
                } else if !p.at(";") && !p.at("}") {
                    break;
                }
            }
            if p.at(";") {
                items.push(p.punct());
                p.statements(&mut items, Ctx::Members, |_| false);
            }
            items.push(p.expect("}"));
            node("enum_class_body", items)
        })
    }

    fn function_declaration(&mut self, mut items: Vec<Item>) -> Item {
        items.push(self.punct());
        if self.at("<") {
            items.push(self.type_parameters());
        }
        // A receiver: the type before the name's `.`.
        if let Some(dot) = self.receiver_dot() {
            items.push(self.receiver_type(dot));
            if self.at(".") {
                items.push(self.punct());
            }
        }
        items.push(self.identifier().with("name"));
        items.push(self.function_value_parameters());
        if self.at(":") {
            items.push(self.punct());
            items.push(self.type_());
        }
        if self.at("where") {
            items.push(self.type_constraints());
        }
        if self.at("{") || self.at("=") {
            items.push(self.function_body());
        }
        node("function_declaration", items)
    }

    /// In a function's or property's head: the `.` before the name, when a
    /// receiver type is there.
    fn receiver_dot(&self) -> Option<usize> {
        let mut k = self.i;
        // `fun @receiver:Ann T.f()`.
        while self.is(k, "@") {
            k = self.scan_annotation(k)?.0;
        }
        let start = k;
        let mut last_dot = None;
        let mut depth = 0usize;
        loop {
            match self.kind_at(k) {
                L::Op("(") if depth == 0 && k > start && self.kind_at(k - 1) == L::Word => break,
                L::Op("(" | "<") => depth += 1,
                L::Op(")" | ">") => depth = depth.checked_sub(1)?,
                L::Op(".") if depth == 0 => last_dot = Some(k),
                L::Op("?" | "->" | ",") => {}
                // Two names in a row end the head (`val name by x`).
                L::Word
                    if depth == 0
                        && k > start
                        && (self.kind_at(k - 1) == L::Word || self.nl_at(k)) =>
                {
                    break;
                }
                L::Word => {}
                L::Op(":" | "=" | "{") if depth == 0 => break,
                L::Op(_) if depth > 0 => {}
                _ => break,
            }
            k += 1;
        }
        last_dot.filter(|d| self.kind_at(d + 1) == L::Word)
    }

    pub(crate) fn function_value_parameters(&mut self) -> Item {
        let mut items = vec![self.expect("(")];
        self.blind(|p| {
            p.comma_list(&mut items, ")", |p| {
                let mut part = Vec::new();
                if p.at("@")
                    || (MODIFIERS
                        .iter()
                        .any(|(m, k)| *m == p.word() && *k == "parameter_modifier")
                        && p.kind_at(p.i + 1) == L::Word)
                {
                    part.push(p.parameter_modifiers());
                }
                part.push(p.parameter());
                if p.at("=") {
                    part.push(p.punct());
                    part.push(p.expression());
                }
                node("", part)
            });
        });
        items.push(self.expect(")"));
        flatten(node("function_value_parameters", items))
    }

    fn parameter_modifiers(&mut self) -> Item {
        let mut items = Vec::new();
        loop {
            if self.at("@") {
                items.push(self.annotation());
            } else if matches!(self.word(), "vararg" | "noinline" | "crossinline") {
                items.push(self.wrapped("parameter_modifier"));
            } else {
                break;
            }
        }
        node("parameter_modifiers", items)
    }

    pub(crate) fn parameter(&mut self) -> Item {
        let items = vec![self.identifier(), self.expect(":"), self.type_()];
        node("parameter", items)
    }

    pub(crate) fn function_body(&mut self) -> Item {
        if self.at("{") {
            let b = self.block();
            return node("function_body", vec![b]);
        }
        let eq = self.punct();
        let e = self.expression();
        node("function_body", vec![eq, e])
    }

    pub(crate) fn variable_declaration(&mut self) -> Item {
        let mut items = Vec::new();
        while self.at("@") {
            items.push(self.annotation());
        }
        items.push(self.identifier());
        if self.at(":") {
            items.push(self.punct());
            items.push(self.type_());
        }
        node("variable_declaration", items)
    }

    pub(crate) fn multi_variable_declaration(&mut self) -> Item {
        let mut items = vec![self.punct()];
        self.blind(|p| p.comma_list(&mut items, ")", Self::variable_declaration));
        items.push(self.expect(")"));
        node("multi_variable_declaration", items)
    }

    fn property_declaration(&mut self, mut items: Vec<Item>) -> Item {
        items.push(self.punct());
        if self.at("<") {
            items.push(self.type_parameters());
        }
        if !self.at("(")
            && let Some(dot) = self.receiver_dot()
        {
            items.push(self.receiver_type(dot));
            if self.at(".") {
                items.push(self.punct());
            }
        }
        items.push(if self.at("(") {
            self.multi_variable_declaration()
        } else {
            self.variable_declaration()
        });
        if self.at("where") {
            items.push(self.type_constraints());
        }
        if self.at("=") {
            items.push(self.punct());
            let saved = self.accessor_ok;
            self.accessor_ok = true;
            items.push(self.expression());
            self.accessor_ok = saved;
        } else if self.at("by") {
            let by = self.punct();
            let saved = self.accessor_ok;
            self.accessor_ok = true;
            let e = self.expression();
            self.accessor_ok = saved;
            items.push(node("property_delegate", vec![by, e]));
        }
        if self.at(";") && self.accessor_after(self.i + 1) {
            items.push(self.punct());
        }
        let mut got = (false, false);
        while let Some(which) = self.accessor_here() {
            if (which == "get" && got.0) || (which == "set" && got.1) {
                break;
            }
            if which == "get" {
                got.0 = true;
                items.push(self.getter());
            } else {
                got.1 = true;
                items.push(self.setter());
            }
        }
        node("property_declaration", items)
    }

    /// Whether a getter or setter starts at `n`.
    fn accessor_after(&self, n: usize) -> bool {
        let (k, _) = self.scan_modifiers(n);
        matches!(self.word_at(k), "get" | "set")
    }

    /// `get` or `set` (after modifiers) here, on this line or the next.
    fn accessor_here(&self) -> Option<&'static str> {
        let (k, _) = self.scan_modifiers(self.i);
        let w = match self.word_at(k) {
            "get" => "get",
            "set" => "set",
            _ => return None,
        };
        if self.is(k + 1, "[") || self.is(k + 1, ".") || self.is(k + 1, "=") {
            return None;
        }
        // On a line of its own, `set(…)` with no body on that line is no
        // setter to tree-sitter's scanner (it ends the statement there).
        if w == "set"
            && self.nl_at(self.i)
            && self.is(k + 1, "(")
            && self.tk(k).end == self.tk(k + 1).start
        {
            let mut c = k + 1;
            while !self.is(c, ")") && self.kind_at(c) != L::Eof {
                c += 1;
            }
            if self.nl_at(c + 1) {
                return None;
            }
        }
        Some(w)
    }

    fn getter(&mut self) -> Item {
        let mut items = Vec::new();
        if self.word() != "get" {
            items.push(self.modifiers());
        }
        items.push(self.punct());
        if self.at("(") && self.is(self.i + 1, ")") {
            items.push(self.punct());
            items.push(self.punct());
            if self.at(":") {
                items.push(self.punct());
                items.push(self.type_());
            }
            items.push(self.function_body());
        }
        node("getter", items)
    }

    fn setter(&mut self) -> Item {
        let mut items = Vec::new();
        if self.word() != "set" {
            items.push(self.modifiers());
        }
        items.push(self.punct());
        if self.at("(") {
            items.push(self.punct());
            self.blind(|p| {
                if p.at("@") || matches!(p.word(), "vararg" | "noinline" | "crossinline") {
                    items.push(p.parameter_modifiers());
                }
                items.push(p.identifier());
                if p.at(":") {
                    items.push(p.punct());
                    items.push(p.type_());
                }
                if p.at("=") {
                    items.push(p.punct());
                    items.push(p.expression());
                }
            });
            items.push(self.expect(")"));
            if self.at(":") {
                items.push(self.punct());
                items.push(self.type_());
            }
            items.push(self.function_body());
        }
        node("setter", items)
    }
}

/// `kind`'s hidden-rule children (kind `""`) in its place.
pub(crate) fn flatten(item: Item) -> Item {
    match item {
        Item::Node(mut n) => {
            let mut out = Vec::with_capacity(n.items.len());
            for i in n.items.drain(..) {
                match i {
                    Item::Node(c) if c.kind.is_empty() => out.extend(c.items),
                    other => out.push(other),
                }
            }
            n.items = out;
            Item::Node(n)
        }
        t => t,
    }
}
