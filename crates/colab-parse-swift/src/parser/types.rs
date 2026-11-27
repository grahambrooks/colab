//! Types (user, tuple, function, array, dictionary, optional, metatype,
//! `some` / `any`, compositions) and patterns.

use super::{Item, Node, Parser, hidden, node};
use crate::lexer::{L, Lx};

impl Parser<'_> {
    /// Whether the current token closes type arguments (`>`, or an
    /// operator starting with one, which is split).
    pub(crate) fn at_close_angle(&self) -> bool {
        matches!(self.peek(), L::Op(_) | L::Custom) && self.text_at(self.i).starts_with('>')
    }

    /// The `>` closing type arguments; an operator starting with `>`
    /// (`>>`, `>?`) is split, its rest lexed again as an operator.
    pub(crate) fn close_angle(&mut self) -> Item {
        if !self.at_close_angle() {
            return self.missing(">", false);
        }
        let t = self.tk(self.i);
        if t.end - t.start > 1 {
            let rest = &self.src[t.start + 1..t.end];
            let k = crate::lexer::fixed(rest);
            self.toks[self.i] = Lx {
                k: L::Op(">"),
                end: t.start + 1,
                ..t
            };
            self.toks.insert(
                self.i + 1,
                Lx {
                    k,
                    start: t.start + 1,
                    end: t.end,
                    nl: false,
                },
            );
        }
        self.punct()
    }

    /// The current token cut after its first character, both parts lexed
    /// again as operators.
    pub(crate) fn split_first(&mut self) {
        let t = self.tk(self.i);
        let first = crate::lexer::fixed(&self.src[t.start..t.start + 1]);
        let rest = crate::lexer::fixed(&self.src[t.start + 1..t.end]);
        self.toks[self.i] = Lx {
            k: first,
            end: t.start + 1,
            ..t
        };
        self.toks.insert(
            self.i + 1,
            Lx {
                k: rest,
                start: t.start + 1,
                end: t.end,
                nl: false,
            },
        );
    }

    /// Past the type arguments at `n` (`<`), if they are.
    pub(crate) fn scan_type_arguments(&self, n: usize) -> Option<usize> {
        let mut depth = 0usize;
        let mut k = n;
        loop {
            let t = self.text_at(k);
            match self.kind_at(k) {
                L::Op("<") => depth += 1,
                L::Op(_) | L::Custom if t.trim_start_matches(['?', '!']).starts_with('>') => {
                    // `>>`, `>?`, `?>`: as many closings as `>`s, the rest
                    // after.
                    // `>?>`: the `?` an optional's between closings.
                    let rest = t.trim_start_matches(['?', '!']);
                    let closes = if rest.bytes().all(|b| matches!(b, b'>' | b'?' | b'!')) {
                        rest.bytes().filter(|b| *b == b'>').count()
                    } else {
                        rest.bytes().take_while(|b| *b == b'>').count()
                    };
                    if closes >= depth {
                        return Some(k + 1);
                    }
                    depth -= closes;
                }
                L::Word | L::Int => {}
                L::Op(
                    "." | "," | "?" | "!" | "[" | "]" | ":" | "(" | ")" | "->" | "&" | "@" | "~"
                    | "...",
                ) => {}
                L::Op("??") => {}
                _ => return None,
            }
            k += 1;
        }
    }

    pub(crate) fn type_arguments(&mut self) -> Item {
        let mut items = vec![self.punct()];
        self.blind(|p| {
            while !p.eof() && !p.at_close_angle() {
                let before = p.i;
                items.push(if p.peek() == L::Int {
                    p.leaf("integer_literal")
                } else {
                    p.type_()
                });
                if p.at(",") {
                    items.push(p.punct());
                } else if p.i == before {
                    items.push(p.error_token());
                } else {
                    break;
                }
            }
        });
        items.push(self.close_angle());
        node("type_arguments", items)
    }

    /// `_type`: attributes, then the type, its field `name`.
    pub(crate) fn type_(&mut self) -> Item {
        let mut items = Vec::new();
        if self.at("@") {
            let mut m = Vec::new();
            while self.at("@") {
                m.push(self.attribute());
            }
            items.push(node("type_modifiers", m));
        }
        items.push(self.unannotated_type().with("name"));
        if items.len() == 1 {
            return items
                .pop()
                .unwrap_or_else(|| self.missing("type_identifier", true));
        }
        hidden(items)
    }

    /// A type and an implicitly unwrapped `!` touching it.
    pub(crate) fn possibly_unwrapped_type(&mut self) -> Item {
        let t = self.type_();
        if self.at("!") && self.touches(self.i) {
            let bang = self.punct();
            return hidden(vec![t, bang]);
        }
        t
    }

    pub(crate) fn user_type(&mut self) -> Item {
        let mut items = vec![self.type_identifier()];
        loop {
            if self.at("<") && self.touches(self.i) && self.scan_type_arguments(self.i).is_some() {
                items.push(self.type_arguments());
            }
            if self.at(".") && self.kind_at(self.i + 1) == L::Word {
                items.push(self.punct());
                items.push(self.type_identifier());
                continue;
            }
            break;
        }
        node("user_type", items)
    }

    pub(crate) fn unannotated_type(&mut self) -> Item {
        let mut t = match self.peek() {
            L::Op("(") => {
                let tuple = self.tuple_type();
                if self.at("async") || matches!(self.word(), "throws" | "rethrows") || self.at("->")
                {
                    return self.function_type(tuple);
                }
                tuple
            }
            L::Op("[") => self.bracket_type(),
            L::Op("~") => {
                let items = vec![self.punct(), self.type_identifier().with("suppressed")];
                node("suppressed_constraint", items)
            }
            L::Word
                if matches!(self.word(), "some" | "any" | "each" | "repeat")
                    && matches!(self.kind_at(self.i + 1), L::Word | L::Op("(" | "[" | "~")) =>
            {
                let kind = match self.word() {
                    "some" => "opaque_type",
                    "any" => "existential_type",
                    "each" => "type_parameter_pack",
                    _ => "type_pack_expansion",
                };
                let items = vec![self.punct(), self.unannotated_type()];
                node(kind, items)
            }
            _ => self.user_type(),
        };
        loop {
            // `T?>`, `T!>`: the `?` / `!` split off a type's closing angle.
            if self.peek() == L::Custom
                && self.touches(self.i)
                && matches!(self.text_at(self.i).as_bytes(), [b'?' | b'!', b'>', ..])
            {
                self.split_first();
            }
            // `T?`, `T??`.
            if (self.at("?") || self.at("??")) && self.touches(self.i) {
                let mut items = vec![t.field_over("wrapped")];
                while (self.at("?") || self.at("??")) && self.touches(self.i) {
                    items.push(if self.at("?") {
                        self.anon_node("?")
                    } else {
                        self.punct()
                    });
                }
                t = node("optional_type", items);
                continue;
            }
            // `[T].Index`.
            if self.at(".")
                && self.kind_at(self.i + 1) == L::Word
                && matches!(t.kind(), "array_type" | "dictionary_type")
            {
                let mut items = vec![t];
                while self.at(".") && self.kind_at(self.i + 1) == L::Word {
                    items.push(self.punct());
                    items.push(self.type_identifier());
                }
                t = node("bracket_qualified_type", items);
                continue;
            }
            // `T?.Type`, `(T).Protocol` (a name's `.Type` is a user type's).
            if self.at(".") && matches!(self.word_at(self.i + 1), "Type" | "Protocol") {
                let items = vec![t, self.punct(), self.punct()];
                t = node("metatype", items);
                continue;
            }
            if self.at("&") && matches!(self.kind_at(self.i + 1), L::Word | L::Op("(" | "[" | "~"))
            {
                let mut items = vec![t];
                while self.at("&") {
                    items.push(self.punct());
                    items.push(self.unannotated_type());
                }
                t = node("protocol_composition_type", items);
                continue;
            }
            if (self.at("async") || matches!(self.word(), "throws" | "rethrows") || self.at("->"))
                && !self.nl_at(self.i)
                && self.function_type_follows()
            {
                return self.function_type(t);
            }
            return t;
        }
    }

    /// `[T]` or `[K: V]`.
    pub(crate) fn bracket_type(&mut self) -> Item {
        let mut items = vec![self.punct()];
        let first = self.blind(Self::type_);
        if self.at(":") {
            items.push(first.with("key"));
            items.push(self.punct());
            items.push(self.blind(Self::type_).with("value"));
            items.push(self.expect("]"));
            node("dictionary_type", items)
        } else {
            items.push(first.with("element"));
            items.push(self.expect("]"));
            node("array_type", items)
        }
    }

    /// After a type: whether `async` / `throws` / `->` make it a function
    /// type's parameters.
    fn function_type_follows(&self) -> bool {
        let mut k = self.i;
        while matches!(self.word_at(k), "async" | "throws" | "rethrows") {
            k += 1;
        }
        self.is(k, "->")
    }

    fn function_type(&mut self, params: Item) -> Item {
        let mut items = vec![params.field_over("params")];
        if self.at("async") {
            items.push(self.punct());
        }
        if matches!(self.word(), "throws" | "rethrows") {
            items.push(self.throws());
        }
        items.push(self.expect("->"));
        items.push(self.type_().with("return_type"));
        node("function_type", items)
    }

    fn tuple_type(&mut self) -> Item {
        // `(any P)`, `(some P)`, `([K: V])`: a parenthesized type, the
        // tuple's one item.
        if matches!(self.word_at(self.i + 1), "any" | "some") || self.is(self.i + 1, "[") {
            let (start, errors, toks) = (self.i, self.errors, self.toks.len());
            let open = self.punct();
            let t = self.blind(Self::unannotated_type);
            if matches!(
                t.kind(),
                "existential_type" | "opaque_type" | "dictionary_type"
            ) && self.at(")")
            {
                let close = self.punct();
                let item = node("tuple_type_item", vec![open, t.with("element"), close]);
                return node("tuple_type", vec![item]);
            }
            if self.toks.len() == toks {
                (self.i, self.errors) = (start, errors);
            } else {
                let mut items = vec![
                    open,
                    node("tuple_type_item", vec![t.with("type")]).with("element"),
                ];
                items.push(self.expect(")"));
                return node("tuple_type", items);
            }
        }
        let mut items = vec![self.punct()];
        self.blind(|p| {
            while !p.eof() && !p.at(")") {
                let before = p.i;
                let mut e = Vec::new();
                if p.at("_") && p.kind_at(p.i + 1) == L::Word && p.is(p.i + 2, ":") {
                    e.push(p.leaf("wildcard_pattern"));
                    e.push(p.simple_identifier().with("name"));
                    e.push(p.punct());
                } else if p.kind_at(p.i) == L::Word && p.is(p.i + 1, ":") {
                    e.push(p.simple_identifier().with("name"));
                    e.push(p.punct());
                }
                if p.parameter_modifier_here() {
                    e.push(p.parameter_modifiers());
                }
                e.push(p.type_().with("type"));
                items.push(node("tuple_type_item", e).with("element"));
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
        node("tuple_type", items)
    }

    // ---- patterns -------------------------------------------------------------

    /// A pattern (`with_expr`: a `case`'s, where an expression is one).
    pub(crate) fn pattern(&mut self, with_expr: bool) -> Item {
        let mut items = Vec::new();
        self.pattern_items(&mut items, with_expr);
        let mut p = node("pattern", items);
        // `x as T`: a type-casting pattern around the pattern.
        if self.at("as") {
            let items = vec![p, self.punct(), self.type_()];
            p = node("pattern", items);
        }
        p
    }

    fn pattern_items(&mut self, items: &mut Vec<Item>, with_expr: bool) {
        if self.at("let") || self.at("var") {
            items.push(self.value_binding_pattern());
            self.bound_items(items);
        } else if self.at("case") {
            items.push(self.punct());
            if self.at("let") || self.at("var") {
                items.push(self.value_binding_pattern());
                self.bound_items(items);
            } else {
                self.case_pattern(items);
            }
        } else if self.at("is") {
            items.push(self.punct());
            items.push(self.type_());
        } else if self.at("_") {
            items.push(self.leaf("wildcard_pattern"));
        } else if self.at("(") {
            items.push(self.tuple_pattern());
        } else if (self.at(".") && self.kind_at(self.i + 1) == L::Word)
            || (with_expr
                && self
                    .qualified_case_end()
                    .is_some_and(|e| self.is(e, "(") && self.touches(e) && self.bindings_in(e)))
        {
            self.case_pattern(items);
        } else if with_expr {
            items.push(self.expression());
        } else if self.qualified_case_end().is_some() {
            // `catch E.failed`.
            self.case_pattern(items);
        } else {
            items.push(self.simple_identifier().with("bound_identifier"));
        }
        if self.at("?") && self.touches(self.i) {
            items.push(self.punct());
        }
    }

    /// A pattern bound by `let` / `var`: a name, a tuple, a case.
    fn bound_items(&mut self, items: &mut Vec<Item>) {
        if self.at("(") {
            items.push(self.tuple_pattern());
        } else if self.at("_") {
            items.push(self.leaf("wildcard_pattern"));
        } else if self.at(".") || self.qualified_case_end().is_some() {
            self.case_pattern(items);
        } else {
            items.push(self.simple_identifier().with("bound_identifier"));
        }
    }

    /// At `A.B.c`: the token after it, if names joined by dots are here.
    fn qualified_case_end(&self) -> Option<usize> {
        let mut k = self.i;
        if self.kind_at(k) != L::Word {
            return None;
        }
        k += 1;
        let start = k;
        while self.is(k, ".") && self.kind_at(k + 1) == L::Word {
            k += 2;
        }
        (k > start).then_some(k)
    }

    /// `.some(let x)`, `Foo.bar(x)`.
    fn case_pattern(&mut self, items: &mut Vec<Item>) {
        if self.peek() == L::Word {
            // The type: every name but the last.
            let mut t = vec![self.type_identifier()];
            while self.at(".") && self.kind_at(self.i + 1) == L::Word && self.is(self.i + 2, ".") {
                t.push(self.punct());
                t.push(self.type_identifier());
            }
            items.push(node("user_type", t));
        }
        items.push(self.expect("."));
        items.push(self.simple_identifier());
        if self.at("(") {
            items.push(self.tuple_pattern());
        }
    }

    /// Whether the parentheses at `n` hold a binding (`let`, `var`, `_`).
    fn bindings_in(&self, n: usize) -> bool {
        let end = self.scan_balanced(n).unwrap_or(n);
        (n..end).any(|k| matches!(self.word_at(k), "let" | "var" | "_"))
    }

    /// Hidden: `( item, … )`, each item a pattern.
    pub(crate) fn tuple_pattern(&mut self) -> Item {
        let mut items = vec![self.punct()];
        self.blind(|p| {
            while !p.eof() && !p.at(")") {
                let before = p.i;
                if p.kind_at(p.i) == L::Word && p.is(p.i + 1, ":") {
                    items.push(p.simple_identifier());
                    items.push(p.punct());
                }
                items.push(p.pattern(true));
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
        hidden(items)
    }
}

/// Whether `item` is a node of `kind`.
#[allow(dead_code)]
pub(crate) fn is_node(item: &Item, kind: &str) -> bool {
    matches!(item, Item::Node(Node { kind: k, .. }) if *k == kind)
}
