//! Expressions at tree-sitter-swift's precedences; calls, navigation and
//! subscripts, closures, strings and the other primaries.
//!
//! Multiplication and addition climb as usual. Every other binary
//! operator's right operand is the rest of the expression: the grammar's
//! right side is a rule of lower precedence than any operator, so
//! `a == b && c` is `a == (b && c)`. A prefix operator binds before a call
//! or subscript (`!a.b()` is `(!a.b)()`), not before a member.

use super::{Item, Parser, hidden, node};
use crate::lexer::{L, Lx};

const ASSIGNMENTS: &[&str] = &["=", "+=", "-=", "*=", "/=", "%="];

impl Parser<'_> {
    /// An expression, an assignment among them.
    pub(crate) fn expression(&mut self) -> Item {
        let e = self.ternary();
        if !self.stops()
            && matches!(self.peek(), L::Op(o) if ASSIGNMENTS.contains(&o))
            && !(self.at("=") && self.touches(self.i) && false)
        {
            let target = node("directly_assignable_expression", vec![e]).with("target");
            let op = self.punct().with("operator");
            let result = self.expression().with("result");
            return node("assignment", vec![target, op, result]);
        }
        e
    }

    /// Whether an expression can start at token `n`.
    pub(crate) fn expression_starts(&self, n: usize) -> bool {
        match self.kind_at(n) {
            L::Word => !matches!(
                self.word_at(n),
                "case"
                    | "default"
                    | "else"
                    | "catch"
                    | "where"
                    | "in"
                    | "let"
                    | "var"
                    | "func"
                    | "class"
                    | "struct"
                    | "enum"
                    | "protocol"
                    | "extension"
                    | "import"
            ),
            L::Op(o) => matches!(
                o,
                "(" | "[" | "{" | "!" | "-" | "+" | "." | "&" | "~" | "\\" | "#" | "..<" | "..."
            ),
            L::Eof | L::StrClose(_) | L::Error | L::Directive(_) => false,
            _ => true,
        }
    }

    /// A ternary chain (`?` with a space before it), left to right.
    fn ternary(&mut self) -> Item {
        let mut c = self.binary();
        while self.at("?") && !self.touches(self.i) && !self.stops() {
            let q = self.punct();
            let t = self.blind(Self::expression);
            let colon = self.expect(":");
            let f = self.binary();
            c = node(
                "ternary_expression",
                vec![
                    c.with("condition"),
                    q,
                    t.with("if_true"),
                    colon,
                    f.with("if_false"),
                ],
            );
        }
        c
    }

    /// The operator here, if one binds loosely (its right side the rest of
    /// the expression): its node, its operands' fields.
    fn loose_op(&self) -> Option<(&'static str, &'static str, &'static str, bool)> {
        if self.stops() {
            return None;
        }
        Some(match self.peek() {
            // `!=` and `!==` lex as custom operators to tree-sitter-swift.
            L::Custom | L::Op("!=" | "!==" | "<=" | ">=") => {
                ("infix_expression", "lhs", "rhs", true)
            }
            L::Op(o) => match o {
                "??" => ("nil_coalescing_expression", "value", "if_nil", false),
                "<" | ">" => ("comparison_expression", "lhs", "rhs", true),
                "==" | "!=" | "===" | "!==" => ("equality_expression", "lhs", "rhs", true),
                "&&" => ("conjunction_expression", "lhs", "rhs", true),
                "||" => ("disjunction_expression", "lhs", "rhs", true),
                "&" | "|" | "^" | "<<" | ">>" => ("bitwise_operation", "lhs", "rhs", true),
                "..." | "..<"
                    if self.expression_starts(self.i + 1) && !self.stops_at(self.i + 1) =>
                {
                    ("range_expression", "start", "end", true)
                }
                _ => return None,
            },
            _ => return None,
        })
    }

    fn binary(&mut self) -> Item {
        self.loose(None)
    }

    /// Arithmetic, casts and checks, a call suffix calling all of them
    /// before it: a call binds loosest in tree-sitter-swift's grammar, so
    /// `a + b.f()` is `(a + b.f)()` and `a.f() + b` is `a.f() + b`. (A
    /// looser operator's right side is a rule of its own, so `a == b.f()`
    /// calls `b.f` alone.)
    fn called(&mut self, first: Option<Item>) -> Item {
        let mut e = self.tight(first);
        while self.call_here() {
            let called = self.postfix(e);
            e = self.tight(Some(called));
        }
        e
    }

    /// Whether a call suffix (arguments, a trailing closure) is here.
    fn call_here(&self) -> bool {
        match self.peek() {
            L::Op("(" | "[") => !self.stops() && !self.nl_at(self.i),
            L::Op("{") => !self.no_closure && !self.observers_follow() && !self.stops(),
            _ => false,
        }
    }

    /// `first`: the left operand, already read.
    pub(crate) fn loose(&mut self, first: Option<Item>) -> Item {
        let left = self.called(first);
        let Some((kind, l, r, op_field)) = self.loose_op() else {
            return left;
        };
        let op = if kind == "infix_expression" {
            self.leaf("custom_operator").with("op")
        } else if op_field {
            self.punct().with("op")
        } else {
            self.punct()
        };
        let rhs = self.loose(None);
        node(kind, vec![left.with(l), op, rhs.with(r)])
    }

    /// Multiplication and addition, then `is` and `as`.
    fn tight(&mut self, first: Option<Item>) -> Item {
        let mut e = self.arith(0, first);
        loop {
            if self.stops() {
                return e;
            }
            if self.at("is") {
                let items = vec![e.with("target"), self.punct().with("op"), self.type_()];
                e = node("check_expression", items);
                continue;
            }
            if self.at("as") {
                let op = self.as_operator();
                let t = self.type_();
                e = node("as_expression", vec![e.with("expr"), op, t]);
                continue;
            }
            return e;
        }
    }

    fn as_operator(&mut self) -> Item {
        let a = self.advance();
        let kind = if matches!(self.peek(), L::Op("?" | "!")) && self.touches(self.i) {
            let q = self.advance();
            let kind = if &self.src[q.start..q.end] == "?" {
                "as?"
            } else {
                "as!"
            };
            return node(
                "as_operator",
                vec![Self::tok(Lx { end: q.end, ..a }, kind, false)],
            );
        } else {
            "as"
        };
        node("as_operator", vec![Self::tok(a, kind, false)])
    }

    fn arith(&mut self, min: u8, first: Option<Item>) -> Item {
        let mut left = match first {
            Some(e) => e,
            None => self.unary(),
        };
        loop {
            if self.stops() {
                return left;
            }
            let (prec, kind) = match self.peek() {
                L::Op("*" | "/" | "%") => (11, "multiplicative_expression"),
                L::Op("+" | "-") => (10, "additive_expression"),
                _ => return left,
            };
            if prec < min {
                return left;
            }
            let op = self.punct().with("op");
            let rhs = self.arith(prec + 1, None);
            left = node(kind, vec![left.with("lhs"), op, rhs.with("rhs")]);
        }
    }

    fn unary(&mut self) -> Item {
        // `-(a)`, `!(a)`: the operator is the callee, as tree-sitter reads
        // it.
        if self.prefix_op_here().is_some() && self.is(self.i + 1, "(") {
            return if self.at("!") {
                self.leaf("bang")
            } else {
                self.punct()
            };
        }
        if let Some(op) = self.prefix_op() {
            let target = self.unary_operand().with("target");
            let e = node("prefix_expression", vec![op.with("operation"), target]);
            return self.postfix_chain(e, false, false).0;
        }
        let p = self.primary();
        self.postfix_chain(p, false, false).0
    }

    /// A prefix operator here (touching its operand).
    fn prefix_op(&mut self) -> Option<Item> {
        self.prefix_op_here()?;
        match self.peek() {
            L::Op("!") => Some(self.leaf("bang")),
            L::Custom => Some(self.leaf("custom_operator")),
            _ => Some(self.punct()),
        }
    }

    /// A prefix operator's operand: another prefix, or a primary and its
    /// members.
    fn unary_operand(&mut self) -> Item {
        if let Some(op) = self.prefix_op() {
            let target = self.unary_operand().with("target");
            return node("prefix_expression", vec![op.with("operation"), target]);
        }
        let mut e = self.primary();
        loop {
            if self.at(".") && matches!(self.kind_at(self.i + 1), L::Word | L::Int) && !self.stops()
            {
                e = self.navigation(e);
            } else if self.at("?") && self.touches(self.i) && self.is(self.i + 1, ".") {
                let q = self.anon_node("?");
                e = hidden(vec![e, q]);
            } else {
                return e;
            }
        }
    }

    fn navigation(&mut self, e: Item) -> Item {
        let dot = self.punct();
        let name = if self.peek() == L::Int {
            self.leaf("integer_literal")
        } else {
            self.simple_identifier()
        };
        let suffix = node("navigation_suffix", vec![dot, name.with("suffix")]).with("suffix");
        node("navigation_expression", vec![e.with("target"), suffix])
    }

    pub(crate) fn postfix(&mut self, e: Item) -> Item {
        self.postfix_chain(e, false, true).0
    }

    /// The postfixes after `e`; with `once`, only up to its first call (and
    /// whether there was one); without `calls`, stopping at a call suffix
    /// (an operand's, which [`Self::binary`] reads).
    fn postfix_chain(&mut self, mut e: Item, once: bool, calls: bool) -> (Item, bool) {
        loop {
            if once && e.kind() == "call_expression" {
                return (e, true);
            }
            match self.peek() {
                L::Op(".")
                    if matches!(self.kind_at(self.i + 1), L::Word | L::Int) && !self.stops() =>
                {
                    e = self.navigation(e);
                }
                L::Op("?")
                    if self.touches(self.i)
                        && matches!(self.kind_at(self.i + 1), L::Op("." | "(" | "[")) =>
                {
                    let q = self.anon_node("?");
                    e = hidden(vec![e, q]);
                }
                L::Op("!") if self.touches(self.i) => {
                    let b = self.leaf("bang").with("operation");
                    e = node("postfix_expression", vec![e.with("target"), b]);
                }
                L::Op("++" | "--") if self.touches(self.i) => {
                    let op = self.punct().with("operation");
                    e = node("postfix_expression", vec![e.with("target"), op]);
                }
                L::Op("(" | "[") if calls && !self.stops() && !self.nl_at(self.i) => {
                    let mut s = vec![self.value_arguments()];
                    // `f(x) { … }`: one suffix.
                    if self.at("{") && !self.no_closure && !self.stops() && !self.observers_follow()
                    {
                        s.extend(self.closure_items());
                    }
                    e = node("call_expression", vec![e, node("call_suffix", s)]);
                }
                L::Op("{")
                    if calls && !self.no_closure && !self.observers_follow() && !self.stops() =>
                {
                    let s = self.closure_suffix();
                    e = node("call_expression", vec![e, s]);
                }
                L::Op("<")
                    if self.touches(self.i)
                        && e.kind() == "simple_identifier"
                        && self.generic_follows() =>
                {
                    e = self.generic_type_expression(e);
                }
                L::Op("...")
                    if matches!(self.kind_at(self.i + 1), L::Op(")" | "]" | "," | ":"))
                        || self.stops_at(self.i + 1) =>
                {
                    let op = self.punct();
                    e = node("open_end_range_expression", vec![e.with("start"), op]);
                }
                _ => return (e, false),
            }
        }
    }

    /// At `<` after a name: whether type arguments and a call, member or
    /// closure follow (`Foo<Int>(…)`, `Foo<Int>.bar`).
    fn generic_follows(&self) -> bool {
        self.scan_type_arguments(self.i).is_some_and(|e| {
            (self.kind_at(e) == L::Op("(") && self.touches(e))
                // `R<S, any E> { … }`, `B<K, T>.x`: arguments no
                // comparison could hold.
                || ((self.is(e, "{") || self.is(e, "."))
                    && (self.only_types(self.i, e) || (self.i..e).any(|k| self.is(k, ","))))
        })
    }

    /// Whether tokens `from..to` hold a type no expression could be read
    /// as (`any P`, `some P`).
    fn only_types(&self, from: usize, to: usize) -> bool {
        (from..to).any(|k| {
            (matches!(self.word_at(k), "any" | "some") && self.kind_at(k + 1) == L::Word)
                // `T?]`, `T?,`: an optional type.
                || (self.is(k, "?")
                    && self.touches(k)
                    && self.kind_at(k - 1) == L::Word
                    && matches!(self.kind_at(k + 1), L::Op("]" | "," | ":")))
        })
    }

    fn generic_type_expression(&mut self, name: Item) -> Item {
        let mut t = match name {
            Item::Tok(mut t) => {
                t.kind = "type_identifier";
                vec![Item::Tok(t)]
            }
            other => vec![other],
        };
        t.push(self.type_arguments());
        while self.at(".") && self.kind_at(self.i + 1) == L::Word && self.is(self.i + 2, "<") {
            t.push(self.punct());
            t.push(self.type_identifier());
            t.push(self.type_arguments());
        }
        let ty = node("user_type", t);
        if self.at("{") {
            // `R<S, any E> { … }`, even where a closure would not follow.
            let saved = std::mem::replace(&mut self.no_closure, false);
            let suffix = self.constructor_suffix();
            self.no_closure = saved;
            return node(
                "constructor_expression",
                vec![ty.with("constructed_type"), suffix],
            );
        }
        if self.at("(") {
            let suffix = self.constructor_suffix();
            return node(
                "constructor_expression",
                vec![ty.with("constructed_type"), suffix],
            );
        }
        ty
    }

    fn constructor_suffix(&mut self) -> Item {
        let mut items = Vec::new();
        if self.at("(") {
            items.push(self.value_arguments());
        }
        if self.at("{") && !self.no_closure {
            items.extend(self.closure_items());
        }
        node("constructor_suffix", items)
    }

    /// Trailing closures: `{ … }`, then `name: { … }`, one suffix.
    pub(crate) fn closure_suffix(&mut self) -> Item {
        let items = self.closure_items();
        node("call_suffix", items)
    }

    /// A [`Self::closure_suffix`]'s items.
    fn closure_items(&mut self) -> Vec<Item> {
        let mut items = vec![self.lambda_literal()];
        while self.kind_at(self.i) == L::Word
            && self.is(self.i + 1, ":")
            && self.is(self.i + 2, "{")
        {
            items.push(self.simple_identifier().with("name"));
            items.push(self.punct());
            items.push(self.lambda_literal());
        }
        items
    }

    pub(crate) fn value_arguments(&mut self) -> Item {
        let close = if self.at("(") { ")" } else { "]" };
        let mut items = vec![self.punct()];
        self.blind(|p| {
            while !p.eof() && !p.at(close) {
                let before = p.i;
                items.push(p.value_argument());
                if p.at(",") {
                    items.push(p.punct());
                } else if p.i == before {
                    items.push(p.error_token());
                } else {
                    break;
                }
            }
        });
        items.push(self.expect(if close == ")" { ")" } else { "]" }));
        node("value_arguments", items)
    }

    fn label_here(&self, n: usize) -> bool {
        self.kind_at(n) == L::Word && self.is(n + 1, ":")
    }

    pub(crate) fn value_argument(&mut self) -> Item {
        let mut items = Vec::new();
        if self.at("@") {
            let mut m = Vec::new();
            while self.at("@") {
                m.push(self.attribute());
            }
            items.push(node("type_modifiers", m));
        }
        if self.label_here(self.i) {
            // `foo(_:bar:)`: labels only.
            if matches!(self.kind_at(self.i + 2), L::Op(")" | ",")) || self.label_here(self.i + 2) {
                while self.label_here(self.i) {
                    let l = node("value_argument_label", vec![self.simple_identifier()]);
                    items.push(l.with("reference_specifier"));
                    items.push(self.punct());
                }
                return node("value_argument", items);
            }
            let l = node("value_argument_label", vec![self.simple_identifier()]);
            items.push(l.with("name"));
            items.push(self.punct());
        }
        items.push(self.expression().with("value"));
        node("value_argument", items)
    }

    pub(crate) fn try_operator(&mut self) -> Item {
        let mut items = vec![self.punct()];
        if matches!(self.peek(), L::Op("?" | "!")) && self.touches(self.i) {
            items.push(self.punct());
        }
        node("try_operator", items)
    }

    // ---- primaries --------------------------------------------------------

    pub(crate) fn primary(&mut self) -> Item {
        match self.peek() {
            L::Int => self.leaf("integer_literal"),
            L::Hex => self.leaf("hex_literal"),
            L::Oct => self.leaf("oct_literal"),
            L::Bin => self.leaf("bin_literal"),
            L::Real => self.leaf("real_literal"),
            L::StrOpen(_) => self.string(),
            L::RawPart | L::RawEnd => self.raw_string(),
            L::Directive(_) => self.directive(),
            // `(any P).self`: a parenthesized type, a member's target.
            L::Op("(")
                if matches!(self.word_at(self.i + 1), "any" | "some")
                    && self.kind_at(self.i + 2) == L::Word
                    && self
                        .scan_balanced(self.i)
                        .is_some_and(|e| self.is(e, ".") && self.touches(e)) =>
            {
                let open = self.punct();
                let t = self.blind(Self::unannotated_type).with("element");
                let close = self.expect(")");
                hidden(vec![open, t, close])
            }
            L::Op("(") => self.tuple_expression(),
            L::Op("[") => self.bracket_primary(),
            L::Op("{") => self.lambda_literal(),
            L::Op("\\") => self.key_path(),
            L::Op("#") => self.hash_primary(),
            L::Op("...") if !self.expression_starts(self.i + 1) || !self.touches(self.i + 1) => {
                self.wrapped("fully_open_range")
            }
            L::Op("..<" | "...") => {
                let op = self.punct();
                let end = self.expression().with("end");
                node("open_start_range_expression", vec![op, end])
            }
            L::Op(o) if matches!(self.kind_at(self.i + 1), L::Op(")" | ",")) && o != ")" => {
                self.punct()
            }
            L::Custom if matches!(self.kind_at(self.i + 1), L::Op(")" | ",")) => {
                self.leaf("custom_operator")
            }
            L::Word => self.word_primary(),
            _ => {
                let m = self.missing("simple_identifier", true);
                if !self.eof() && !matches!(self.peek(), L::Op(")" | "]" | "}" | ",")) {
                    let e = self.error_token();
                    return node("ERROR", vec![m, e]);
                }
                m
            }
        }
    }

    fn word_primary(&mut self) -> Item {
        let next_starts = !self.nl_at(self.i + 1)
            && self.expression_starts(self.i + 1)
            && !matches!(self.kind_at(self.i + 1), L::Op("(" | "[" | "{"))
            && !(self.is(self.i + 1, ".") && self.touches(self.i + 1));
        match self.word() {
            "self" => self.wrapped("self_expression"),
            "super" => self.wrapped("super_expression"),
            "nil" => self.leaf("nil_literal"),
            "true" | "false" => self.wrapped("boolean_literal"),
            "if" => self.if_statement(),
            "switch" => self.switch_statement(),
            "try" => {
                let op = self.try_operator();
                let e = self.effect_operand(false).with("expr");
                node("try_expression", vec![op, e])
            }
            "await" => {
                let op = self.punct();
                let e = self.effect_operand(false).with("expr");
                node("await_expression", vec![op, e])
            }
            "consume" | "copy" if next_starts => {
                let op = self.punct();
                let e = self.effect_operand(false).with("expr");
                node("consume_expression", vec![op, e])
            }
            "unsafe" if next_starts => {
                let op = self.punct();
                let e = self.effect_operand(true).with("expr");
                node("unsafe_expression", vec![op, e])
            }
            "discard" if self.is(self.i + 1, "self") => {
                let items = vec![self.punct(), self.wrapped("self_expression")];
                node("discard_statement", items)
            }
            "each" | "repeat" if next_starts => {
                let kind = if self.word() == "each" {
                    "value_parameter_pack"
                } else {
                    "value_pack_expansion"
                };
                let items = vec![self.punct(), self.expression()];
                node(kind, items)
            }
            _ => self.name_leaf("simple_identifier"),
        }
    }

    /// `try` / `await` / `unsafe`'s operand: a postfix chain up to its first
    /// call, the effect then taking the rest's postfixes and operators
    /// (`try f().g` is `(try f()).g`, `try f() + 1` is `(try f()) + 1`), as
    /// tree-sitter's preference for direct calls reads it; else the whole
    /// expression (`try a + b`).
    /// (`assign`: `unsafe`'s, which may be an assignment.)
    fn effect_operand(&mut self, assign: bool) -> Item {
        if matches!(self.word(), "try" | "await" | "unsafe") {
            return self.primary();
        }
        let (start, errors, toks) = (self.i, self.errors, self.toks.len());
        if self.prefix_op_here().is_none() {
            let p = self.primary();
            let (e, called) = self.postfix_chain(p, true, true);
            if called {
                return e;
            }
            if self.toks.len() == toks {
                self.i = start;
                self.errors = errors;
            } else {
                // A `>` was split off a token: keep what was read.
                return e;
            }
        }
        if assign {
            self.expression()
        } else {
            self.ternary()
        }
    }

    /// Whether a prefix operator is here (without reading it).
    fn prefix_op_here(&self) -> Option<()> {
        // `reduce(0, +)`: an operator alone is no prefix.
        let touching = self.tk(self.i).end == self.tk(self.i + 1).start
            && !matches!(
                self.kind_at(self.i + 1),
                L::Op(")" | "," | "]" | "}" | ":" | ";") | L::Eof
            );
        let yes = match self.peek() {
            L::Op("!" | "-" | "+" | "~" | "&" | "++" | "--") => touching,
            L::Op(".") => touching && self.kind_at(self.i + 1) == L::Word,
            L::Custom => touching && self.expression_starts(self.i + 1),
            _ => false,
        };
        yes.then_some(())
    }

    fn tuple_expression(&mut self) -> Item {
        let mut items = vec![self.punct()];
        self.blind(|p| {
            while !p.eof() && !p.at(")") {
                let before = p.i;
                if p.label_here(p.i) {
                    items.push(p.simple_identifier().with("name"));
                    items.push(p.punct());
                }
                items.push(p.expression().with("value"));
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
        node("tuple_expression", items)
    }

    /// `[…]`: an array or dictionary literal (`[T]()` is a call of one).
    fn bracket_primary(&mut self) -> Item {
        // `[K: any V]()`: a type no literal could be, constructed.
        if let Some(end) = self.scan_balanced(self.i)
            && self.is(end, "(")
            && self.touches(end)
            && (self.only_types(self.i, end)
                || (self.i..end)
                    .any(|k| self.is(k, "<") && self.touches(k) && self.kind_at(k - 1) == L::Word))
        {
            let t = self.bracket_type();
            let suffix = self.constructor_suffix();
            return node(
                "constructor_expression",
                vec![t.with("constructed_type"), suffix],
            );
        }
        // `[T.U].x`, `[K: V].x`: names alone, a member after, read as a
        // type (a navigation's target).
        if let Some(end) = self.scan_balanced(self.i)
            && self.is(end, ".")
            && self.touches(end)
            && self.kind_at(self.i + 1) == L::Word
            && (self.i + 1..end - 1).all(|k| {
                matches!(self.kind_at(k), L::Word | L::Op("." | ":"))
                    && (self.kind_at(k) != L::Word || self.kind_at(k + 1) != L::Word)
            })
        {
            return self.bracket_type();
        }
        let mut items = vec![self.punct()];
        let mut dict = false;
        self.blind(|p| {
            if p.at(":") {
                dict = true;
                items.push(p.punct());
            }
            while !p.eof() && !p.at("]") {
                let before = p.i;
                let k = p.expression();
                if p.at(":") {
                    dict = true;
                    items.push(k.with("key"));
                    items.push(p.punct());
                    items.push(p.expression().with("value"));
                } else {
                    items.push(k.with("element"));
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
        items.push(self.expect("]"));
        node(
            if dict {
                "dictionary_literal"
            } else {
                "array_literal"
            },
            items,
        )
    }

    /// Whether tokens `from..to` read as a type (`Int`, `String: [Int]`).
    fn key_path(&mut self) -> Item {
        let mut items = vec![self.punct()];
        if self.peek() == L::Word && self.touches(self.i) {
            items.push(self.type_identifier());
            if self.at("<") && self.touches(self.i) {
                items.push(self.type_arguments());
            }
            return node("key_path_expression", items);
        }
        if self.at("[") {
            if let Item::Node(n) = self.unannotated_type() {
                items.push(Item::Node(n));
            }
            return node("key_path_expression", items);
        }
        // `\.a.b`: the components are navigation around the `\`, as
        // tree-sitter's parse reads them.
        node("key_path_expression", items)
    }

    fn hash_primary(&mut self) -> Item {
        let hash = self.punct();
        if self.peek() == L::DiagTail {
            let t = self.advance();
            return node("diagnostic", vec![hash, Self::hidden_tok(t.start, t.end)]);
        }
        match self.word() {
            "selector" => {
                let mut items = vec![hash, self.punct(), self.expect("(")];
                self.blind(|p| {
                    if matches!(p.word(), "getter" | "setter") && p.is(p.i + 1, ":") {
                        let a = p.advance();
                        let b = p.advance();
                        let kind = if &p.src[a.start..a.end] == "getter" {
                            "getter:"
                        } else {
                            "setter:"
                        };
                        items.push(Self::tok(Lx { end: b.end, ..a }, kind, false));
                    }
                    items.push(p.expression());
                });
                items.push(self.expect(")"));
                node("selector_expression", items)
            }
            "keyPath" => {
                let mut items = vec![hash, self.punct(), self.expect("(")];
                items.push(self.blind(Self::expression));
                items.push(self.expect(")"));
                node("key_path_string_expression", items)
            }
            "file" | "fileID" | "filePath" | "line" | "column" | "function" | "dsohandle" => {
                let items = vec![hash, self.punct()];
                node("special_literal", items)
            }
            "colorLiteral" | "fileLiteral" | "imageLiteral" => {
                let mut items = vec![hash, self.punct(), self.expect("(")];
                self.blind(|p| {
                    while !p.eof() && !p.at(")") {
                        items.push(p.simple_identifier());
                        items.push(p.expect(":"));
                        items.push(p.expression());
                        if p.at(",") {
                            items.push(p.punct());
                        } else {
                            break;
                        }
                    }
                });
                items.push(self.expect(")"));
                node("playground_literal", items)
            }
            _ => {
                let mut items = vec![hash, self.simple_identifier()];
                if self.at("<") && self.touches(self.i) {
                    items.push(self.type_parameters());
                }
                if (self.at("(") && self.touches(self.i)) || (self.at("{") && !self.no_closure) {
                    let mut s = Vec::new();
                    if self.at("(") {
                        s.push(self.value_arguments());
                    }
                    if self.at("{") && !self.no_closure {
                        s.extend(self.closure_items());
                    }
                    items.push(node("call_suffix", s));
                }
                node("macro_invocation", items)
            }
        }
    }

    // ---- closures ---------------------------------------------------------

    pub(crate) fn lambda_literal(&mut self) -> Item {
        let mut items = vec![self.punct()];
        if self.lambda_in().is_some() {
            while self.at("@") {
                items.push(self.attribute());
            }
            if self.at("[") {
                items.push(self.capture_list().with("captures"));
            }
            if !self.at("in") {
                items.push(self.lambda_function_type().with("type"));
            }
            items.push(self.expect("in"));
        }
        let mut body = Vec::new();
        self.sighted(|p| p.statement_list(&mut body, false));
        if !body.is_empty() {
            items.push(node("statements", body));
        }
        items.push(self.expect("}"));
        node("lambda_literal", items)
    }

    /// After a closure's `{`: the `in` ending its header, if it has one.
    fn lambda_in(&self) -> Option<usize> {
        let mut k = self.i;
        while self.is(k, "@") {
            k = self.scan_attribute(k)?;
        }
        loop {
            match self.kind_at(k) {
                L::Word if self.word_at(k) == "in" => return Some(k),
                L::Op("(" | "[") => k = self.scan_balanced(k)? - 1,
                L::Op("<") => k = self.scan_type_arguments(k)? - 1,
                L::Word
                    if !matches!(
                        self.word_at(k),
                        "let"
                            | "var"
                            | "if"
                            | "return"
                            | "for"
                            | "while"
                            | "guard"
                            | "switch"
                            | "func"
                            | "case"
                    ) => {}
                L::Op("," | ":" | "->" | "." | "?" | "!" | "@" | "&" | "...") => {}
                _ => return None,
            }
            k += 1;
        }
    }

    fn capture_list(&mut self) -> Item {
        let mut items = vec![self.punct()];
        self.blind(|p| {
            while !p.eof() && !p.at("]") {
                let before = p.i;
                let mut c = Vec::new();
                if p.at("self") && matches!(p.kind_at(p.i + 1), L::Op("," | "]")) {
                    c.push(p.wrapped("self_expression").with("name"));
                } else {
                    if matches!(p.word(), "weak" | "unowned")
                        && p.kind_at(p.i + 1) != L::Op(",")
                        && !p.is(p.i + 1, "]")
                    {
                        c.push(p.ownership());
                    }
                    c.push(p.simple_identifier().with("name"));
                    if p.at("=") {
                        c.push(p.punct());
                        c.push(p.expression().with("value"));
                    }
                }
                items.push(node("capture_list_item", c));
                if p.at(",") {
                    items.push(p.punct());
                } else if p.i == before {
                    items.push(p.error_token());
                } else {
                    break;
                }
            }
        });
        items.push(self.expect("]"));
        node("capture_list", items)
    }

    /// `weak`, `unowned`, `unowned(safe)`.
    fn ownership(&mut self) -> Item {
        let a = self.advance();
        if self.at("(") && self.touches(self.i) && self.is(self.i + 2, ")") {
            self.i += 3;
            let end = self.tk(self.i - 1).end;
            let kind = super::names::anonymous(&self.src[a.start..end]).unwrap_or("unowned");
            return node(
                "ownership_modifier",
                vec![Self::tok(Lx { end, ..a }, kind, false)],
            );
        }
        let kind = if &self.src[a.start..a.end] == "weak" {
            "weak"
        } else {
            "unowned"
        };
        node("ownership_modifier", vec![Self::tok(a, kind, false)])
    }

    fn lambda_function_type(&mut self) -> Item {
        let mut items = Vec::new();
        if self.at("(") {
            items.push(self.punct());
            self.blind(|p| {
                if !p.at(")") {
                    items.push(p.lambda_parameters());
                }
            });
            items.push(self.expect(")"));
        } else {
            items.push(self.lambda_parameters());
        }
        if self.at("async") {
            items.push(self.punct());
        }
        if matches!(self.word(), "throws" | "rethrows") {
            items.push(self.throws());
        }
        if self.at("->") {
            items.push(self.punct());
            if self.parameter_modifier_here() {
                items.push(self.parameter_modifiers());
            }
            items.push(self.possibly_unwrapped_type().with("return_type"));
        }
        node("lambda_function_type", items)
    }

    fn lambda_parameters(&mut self) -> Item {
        let mut items = Vec::new();
        loop {
            let mut p = Vec::new();
            if self.at("self") {
                p.push(self.wrapped("self_expression"));
            } else {
                if self.kind_at(self.i + 1) == L::Word && self.is(self.i + 2, ":") {
                    p.push(self.simple_identifier().with("external_name"));
                }
                p.push(self.simple_identifier().with("name"));
                if self.at(":") {
                    p.push(self.punct());
                    if self.parameter_modifier_here() {
                        p.push(self.parameter_modifiers());
                    }
                    p.push(self.possibly_unwrapped_type().with("type"));
                }
            }
            items.push(node("lambda_parameter", p));
            if self.at(",") {
                items.push(self.punct());
                if self.at("in") || self.at(")") {
                    break;
                }
                continue;
            }
            break;
        }
        node("lambda_function_type_parameters", items)
    }

    // ---- strings ----------------------------------------------------------

    fn string(&mut self) -> Item {
        let multi = self.peek() == L::StrOpen("\"\"\"");
        let text_kind = if multi {
            "multi_line_str_text"
        } else {
            "line_str_text"
        };
        let mut items = vec![self.punct()];
        loop {
            match self.peek() {
                L::Text => items.push(self.leaf(text_kind).with("text")),
                L::Escape => items.push(self.leaf("str_escaped_char").with("text")),
                L::Unicode => {
                    let t = self.advance();
                    let parts = vec![
                        Self::tok(
                            Lx {
                                end: t.start + 1,
                                ..t
                            },
                            "\\",
                            false,
                        ),
                        Self::tok(
                            Lx {
                                start: t.start + 1,
                                end: t.start + 2,
                                ..t
                            },
                            "u",
                            false,
                        ),
                        Self::hidden_tok(t.start + 2, t.end),
                    ];
                    items.push(node("str_escaped_char", parts).with("text"));
                }
                L::Op("\"") => items.push(self.punct().with("text")),
                L::InterpOpen => {
                    items.push(self.punct());
                    self.interpolation_contents(&mut items);
                    items.push(self.expect(")"));
                }
                _ => break,
            }
        }
        if matches!(self.peek(), L::StrClose(_)) {
            items.push(self.punct());
        } else {
            items.push(self.missing(if multi { "\"\"\"" } else { "\"" }, false));
        }
        node(
            if multi {
                "multi_line_string_literal"
            } else {
                "line_string_literal"
            },
            items,
        )
    }

    fn interpolation_contents(&mut self, items: &mut Vec<Item>) {
        self.blind(|p| {
            while !p.eof() && !p.at(")") {
                let before = p.i;
                let v = p.value_argument();
                let v = match v {
                    Item::Node(mut n) => {
                        n.kind = "interpolated_expression";
                        Item::Node(n)
                    }
                    t => t,
                };
                items.push(v.with("interpolation"));
                if p.at(",") {
                    items.push(p.punct());
                } else if p.i == before {
                    items.push(p.error_token());
                } else {
                    break;
                }
            }
        });
    }

    fn raw_string(&mut self) -> Item {
        let mut items = Vec::new();
        while self.peek() == L::RawPart {
            items.push(self.leaf("raw_str_part").with("text"));
            if self.peek() == L::RawInterp {
                let mut r = vec![self.leaf("raw_str_interpolation_start")];
                self.interpolation_contents(&mut r);
                r.push(self.expect(")"));
                items.push(node("raw_str_interpolation", r).with("interpolation"));
            }
        }
        if self.peek() == L::RawEnd {
            items.push(self.leaf("raw_str_end_part").with("text"));
        }
        node("raw_string_literal", items)
    }
}
