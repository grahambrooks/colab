//! Expressions by precedence climbing, at tree-sitter-kotlin's
//! precedences; calls, navigation and indexing; lambdas, `if`, `when`,
//! `try`, strings and the other primaries.
//!
//! tree-sitter-kotlin's prefix operators bind tighter than its postfix
//! ones (`!a.b` is `(!a).b`), and an infix call is any name between two
//! expressions; both are copied.

use super::{Item, Parser, node};
use crate::lexer::{L, Lx};

const AS: u8 = 12;
const MULTIPLY: u8 = 11;
const ADD: u8 = 10;
const RANGE: u8 = 9;
const INFIX: u8 = 8;
const ELVIS: u8 = 7;
const IN: u8 = 6;
const RELATIONAL: u8 = 5;
const EQUAL: u8 = 4;
const AND: u8 = 3;
const OR: u8 = 2;

impl Parser<'_> {
    pub(crate) fn expression(&mut self) -> Item {
        let saved = std::mem::replace(&mut self.rhs, false);
        let e = self.expr_bp(0);
        self.rhs = saved;
        e
    }

    /// The operator here and its precedence, if one continues the
    /// expression.
    fn binary_op(&self) -> Option<u8> {
        if self.stops() {
            return None;
        }
        Some(match self.peek() {
            L::Op(o) => match o {
                "as?" => AS,
                "*" | "/" | "%" => MULTIPLY,
                "+" | "-" => ADD,
                ".." | "..<" => RANGE,
                "?:" => ELVIS,
                "!in" | "!is" => IN,
                "<" | ">" | "<=" | ">=" => RELATIONAL,
                "==" | "!=" | "===" | "!==" => EQUAL,
                "&&" => AND,
                "||" => OR,
                _ => return None,
            },
            L::Word => match self.word() {
                "as" => AS,
                "in" | "is" => IN,
                _ if self.infix_here() => INFIX,
                _ => return None,
            },
            _ => return None,
        })
    }

    /// Whether the name here is an infix call's (`a to b`).
    fn infix_here(&self) -> bool {
        if !self.is_ident(self.i)
            || (self.accessor_ok && matches!(self.word(), "get" | "set"))
            || self.nl_at(self.i)
        {
            return false;
        }
        self.expression_starts(self.i + 1)
    }

    /// Whether an expression can start at token `n`.
    pub(crate) fn expression_starts(&self, n: usize) -> bool {
        match self.kind_at(n) {
            L::Word => !matches!(
                self.word_at(n),
                "as" | "in"
                    | "is"
                    | "else"
                    | "by"
                    | "where"
                    | "catch"
                    | "finally"
                    | "class"
                    | "interface"
                    | "val"
                    | "var"
                    | "typealias"
                    | "package"
                    | "import"
            ),
            L::Op(o) => matches!(
                o,
                "(" | "[" | "{" | "!" | "-" | "+" | "++" | "--" | "::" | "@" | "*"
            ),
            L::Eof | L::StrClose(_) | L::Error | L::Dollar => false,
            _ => true,
        }
    }

    /// The right operand of an operator binding at `prec - 1`. An operator
    /// binding tighter than a comparison leaves a generic call after its
    /// operand to the operation (`a ?: b<T>()` calls `a ?: b`); a looser one
    /// keeps it (`a && b<T>()`).
    fn operand(&mut self, prec: u8) -> Item {
        let saved = std::mem::replace(&mut self.rhs, prec > RELATIONAL + 1);
        let r = self.expr_bp(prec);
        self.rhs = saved;
        r
    }

    pub(crate) fn expr_bp(&mut self, min: u8) -> Item {
        let mut left = self.prefix();
        loop {
            // A generic call left by the operand calls the operation.
            if self.at("<") && !self.stops() && self.type_arguments_call() {
                if self.rhs {
                    break;
                }
                left = self.postfix(left);
            }
            let Some(prec) = self.binary_op() else {
                break;
            };
            if prec < min {
                break;
            }
            left = match self.peek() {
                L::Op("as?") | L::Word if matches!(self.word(), "as" | "") && prec == AS => {
                    let op = self.punct();
                    let t = self.type_();
                    node(
                        "as_expression",
                        vec![left.with("left"), op, t.with("right")],
                    )
                }
                L::Word if self.word() == "is" => {
                    let op = self.punct();
                    let t = self.type_();
                    node(
                        "is_expression",
                        vec![left.with("left"), op, t.with("right")],
                    )
                }
                L::Op("!is") => {
                    // Where a statement may end, the scanner skips the `!`:
                    // the token is `is`.
                    let mut op = self.punct();
                    if self.nl_blind == 0
                        && let Item::Tok(t) = &mut op
                    {
                        t.start += 1;
                    }
                    let t = self.type_();
                    node(
                        "is_expression",
                        vec![left.with("left"), op, t.with("right")],
                    )
                }
                L::Word if self.word() == "in" => {
                    let op = self.punct();
                    let r = self.operand(prec + 1);
                    node(
                        "in_expression",
                        vec![left.with("left"), op, r.with("right")],
                    )
                }
                L::Op("!in") => {
                    let op = self.punct();
                    let r = self.operand(prec + 1);
                    node(
                        "in_expression",
                        vec![left.with("left"), op, r.with("right")],
                    )
                }
                L::Op(".." | "..<") => {
                    let op = self.punct();
                    let r = self.operand(prec + 1);
                    node("range_expression", vec![left, op, r])
                }
                L::Word => {
                    let name = self.leaf("identifier");
                    let r = self.operand(prec + 1);
                    node("infix_expression", vec![left, name, r])
                }
                _ => {
                    let op = self.punct().with("operator");
                    let r = self.operand(prec + 1);
                    node(
                        "binary_expression",
                        vec![left.with("left"), op, r.with("right")],
                    )
                }
            };
        }
        left
    }

    /// Prefix operators bind before postfix ones: `!a.b` is `(!a).b`.
    fn prefix(&mut self) -> Item {
        let e = self.prefix_operand();
        self.postfix(e)
    }

    fn prefix_operand(&mut self) -> Item {
        match self.peek() {
            L::Op("++" | "--" | "+" | "-" | "!") => {
                let op = self.punct().with("operator");
                let arg = self.prefix_operand().with("argument");
                node("unary_expression", vec![op, arg])
            }
            L::Op("*") => {
                let star = self.punct();
                let e = self.expr_bp(OR);
                node("spread_expression", vec![star, e])
            }
            L::Op("@") => self.annotated_expression(),
            L::Label if !matches!(self.text_at(self.i), "this@" | "super@" | "return@") => {
                let label = self.leaf("label");
                if self.at("{") {
                    let l = self.lambda_literal();
                    return node("labeled_expression", vec![label, l]);
                }
                let e = self.expression();
                node("labeled_expression", vec![label, e])
            }
            _ => self.primary(),
        }
    }

    /// `@Ann expr`, the expression on this line or the next.
    fn annotated_expression(&mut self) -> Item {
        let ann = self.annotation();
        let e = self.expression();
        node("annotated_expression", vec![ann, e])
    }

    fn postfix(&mut self, mut e: Item) -> Item {
        loop {
            match self.peek() {
                L::Op("." | "::") if !self.stops() => {
                    let op = self.punct();
                    let name = self.identifier();
                    e = node("navigation_expression", vec![e, op, name]);
                }
                // `?.`, spaces allowed between: one token.
                L::Op("?") if self.is(self.i + 1, ".") => {
                    let start = self.tk(self.i).start;
                    self.advance();
                    let dot = self.advance();
                    let op = Self::tok(Lx { start, ..dot }, "?.", false);
                    let name = self.identifier();
                    e = node("navigation_expression", vec![e, op, name]);
                }
                L::Op("[") if !self.stops() => {
                    let mut items = vec![e, self.punct()];
                    self.blind(|p| p.comma_list(&mut items, "]", Self::expression));
                    items.push(self.expect("]"));
                    e = node("index_expression", items);
                }
                L::Op("(") if !self.stops() => {
                    let args = self.value_arguments();
                    e = node("call_expression", vec![e, args]);
                    e = self.trailing_lambda(e);
                }
                // A generic call after an operator calls the whole operation
                // (`a + b<T>()` is `(a + b)<T>()`): the operator's loop takes
                // it.
                L::Op("<") if self.rhs => return e,
                L::Op("<") if !self.stops() && self.type_arguments_call() => {
                    let targs = self.type_arguments();
                    let mut items = vec![e, targs];
                    if self.at("(") {
                        items.push(self.value_arguments());
                        e = node("call_expression", items);
                        e = self.trailing_lambda(e);
                    } else {
                        items.push(self.annotated_lambda());
                        e = node("call_expression", items);
                    }
                }
                L::Op("{") if self.lambda_follows() => {
                    let l = self.annotated_lambda();
                    e = node("call_expression", vec![e, l]);
                }
                L::Label if self.is(self.i + 1, "{") && !self.stops() => {
                    let l = self.annotated_lambda();
                    e = node("call_expression", vec![e, l]);
                }
                L::Op("!!" | "++" | "--") if !self.stops() => {
                    let op = self.punct().with("operator");
                    e = node("unary_expression", vec![e.with("argument"), op]);
                }
                _ => return e,
            }
        }
    }

    /// Whether `{` here is a trailing lambda.
    fn lambda_follows(&self) -> bool {
        !self.no_lambda
    }

    /// A call's lambda after its arguments, as a second call.
    fn trailing_lambda(&mut self, e: Item) -> Item {
        if (self.at("{") && self.lambda_follows())
            || (self.peek() == L::Label && self.is(self.i + 1, "{") && !self.stops())
        {
            let l = self.annotated_lambda();
            return node("call_expression", vec![e, l]);
        }
        e
    }

    /// At `<` after an expression: whether type arguments and a call
    /// follow, as tree-sitter's parse reads them. A lone name in the
    /// brackets before one plain argument is a comparison to it.
    fn type_arguments_call(&self) -> bool {
        let Some(end) = self.scan_type_arguments(self.i) else {
            return false;
        };
        // One name (or `A.B`) in the brackets could be compared to.
        let simple = (self.i + 1..end - 1).all(|k| {
            if (k - self.i) % 2 == 1 {
                self.kind_at(k) == L::Word
            } else {
                self.is(k, ".")
            }
        });
        let close = match self.kind_at(end) {
            L::Op("(") => {
                if !simple || self.is(end + 1, ")") {
                    return true;
                }
                let close = self.scan_balanced(end).unwrap_or(end);
                if !self.one_expression(end, close) {
                    return true;
                }
                close
            }
            L::Op("{") => {
                if !simple {
                    return true;
                }
                self.scan_balanced(end).unwrap_or(end)
            }
            L::Op("::") => return true,
            _ => return false,
        };
        // Then the comparison reading wins unless what follows the call binds
        // tighter than a comparison (`f<T>(x).g`, `f<T>(x) + y`; not
        // `f<T>(x) != null`).
        if self.stops_at(close) {
            return false;
        }
        match self.kind_at(close) {
            L::Op(o) => matches!(
                o,
                "." | "?"
                    | "::"
                    | "("
                    | "["
                    | "{"
                    | "!!"
                    | "++"
                    | "--"
                    | "*"
                    | "/"
                    | "%"
                    | "+"
                    | "-"
                    | ".."
                    | "..<"
                    | "?:"
                    | "!in"
                    | "!is"
                    | "as?"
            ),
            L::Word => matches!(self.word_at(close), "as" | "in" | "is") || self.is_ident(close),
            L::Label => true,
            _ => false,
        }
    }

    pub(crate) fn value_arguments(&mut self) -> Item {
        let mut items = vec![self.expect("(")];
        self.blind(|p| {
            p.comma_list(&mut items, ")", |p| {
                let mut a = Vec::new();
                if p.kind_at(p.i) == L::Word && p.is(p.i + 1, "=") {
                    a.push(p.identifier());
                    a.push(p.punct());
                }
                a.push(p.expression());
                node("value_argument", a)
            });
        });
        items.push(self.expect(")"));
        node("value_arguments", items)
    }

    fn annotated_lambda(&mut self) -> Item {
        let mut items = Vec::new();
        while self.at("@") {
            items.push(self.annotation());
        }
        if self.peek() == L::Label {
            items.push(self.leaf("label"));
        }
        items.push(self.lambda_literal());
        node("annotated_lambda", items)
    }

    pub(crate) fn lambda_literal(&mut self) -> Item {
        self.sighted(false, |p| {
            let mut items = vec![p.punct()];
            if let Some(arrow) = p.lambda_arrow() {
                if arrow > p.i {
                    let mut params = Vec::new();
                    p.blind(|p| {
                        loop {
                            params.push(if p.at("(") {
                                p.multi_variable_declaration()
                            } else {
                                p.variable_declaration()
                            });
                            if p.at(",") {
                                params.push(p.punct());
                                if p.at("->") {
                                    break;
                                }
                                continue;
                            }
                            break;
                        }
                    });
                    items.push(node("lambda_parameters", params));
                }
                items.push(p.expect("->"));
            }
            p.statements(&mut items, super::Ctx::Statements, |_| false);
            items.push(p.expect("}"));
            node("lambda_literal", items)
        })
    }

    /// In a lambda, after `{`: the `->` ending its parameters, if it has
    /// any.
    fn lambda_arrow(&self) -> Option<usize> {
        let mut k = self.i;
        let mut depth = 0usize;
        loop {
            match self.kind_at(k) {
                L::Op("->") if depth == 0 => return Some(k),
                L::Op("(" | "<") => depth += 1,
                L::Op(")" | ">") => depth = depth.checked_sub(1)?,
                L::Op("->") => {}
                // Parameters are names: no keyword, no two names in a row
                // (`val x: () -> Unit` is a statement).
                L::Word
                    if depth == 0
                        && (super::KEYWORDS.contains(&self.word_at(k))
                            || (k > self.i && self.kind_at(k - 1) == L::Word)) =>
                {
                    return None;
                }
                L::Word | L::Op("," | ":" | "." | "?" | "*" | "@") => {}
                _ => return None,
            }
            k += 1;
        }
    }

    // ---- primaries --------------------------------------------------------

    fn primary(&mut self) -> Item {
        match self.peek() {
            L::Number => self.leaf("number_literal"),
            L::Float => self.leaf("float_literal"),
            L::Char => self.character_literal(),
            L::StrOpen(_) => self.string(),
            L::Op("(") => {
                let items = vec![self.punct(), self.blind(Self::expression), self.expect(")")];
                node("parenthesized_expression", items)
            }
            L::Op("[") => {
                let mut items = vec![self.punct()];
                self.blind(|p| p.comma_list(&mut items, "]", Self::expression));
                items.push(self.expect("]"));
                node("collection_literal", items)
            }
            L::Op("{") => self.lambda_literal(),
            L::Op("::") => {
                let c = self.punct();
                let n = self.identifier();
                node("callable_reference", vec![c, n])
            }
            L::Label => {
                let t = self.text_at(self.i);
                match t {
                    "this@" => {
                        let items = vec![self.punct(), self.identifier()];
                        node("this_expression", items)
                    }
                    "super@" => {
                        let items = vec![self.punct(), self.identifier()];
                        node("super_expression", items)
                    }
                    _ => {
                        let mut items = vec![self.punct(), self.identifier().with("label")];
                        if !self.stops() && self.expression_starts(self.i) {
                            items.push(self.expression());
                        }
                        node("return_expression", items)
                    }
                }
            }
            L::Word => self.word_primary(),
            _ => {
                let m = self.missing("identifier", true);
                if !self.eof() && !matches!(self.peek(), L::Op(")" | "]" | "}" | ",")) {
                    let e = self.error_token();
                    return node("ERROR", vec![m, e]);
                }
                m
            }
        }
    }

    fn word_primary(&mut self) -> Item {
        match self.word() {
            "this" => self.wrapped("this_expression"),
            "super" => {
                let mut items = vec![self.punct()];
                if self.at("<") && self.scan_type_arguments(self.i).is_some() {
                    items.push(self.punct());
                    items.push(self.type_());
                    items.push(self.expect(">"));
                }
                node("super_expression", items)
            }
            "if" => self.if_expression(),
            "when" => self.when_expression(),
            "try" => self.try_expression(),
            "return" => {
                let mut items = vec![self.punct()];
                if !self.stops() && self.expression_starts(self.i) && !self.at("}") {
                    items.push(self.expression());
                }
                node("return_expression", items)
            }
            "throw" => {
                let items = vec![self.punct(), self.expression()];
                node("throw_expression", items)
            }
            "object" => {
                let mut items = vec![self.punct()];
                if self.at(":") {
                    items.push(self.punct());
                    items.push(self.delegation_specifiers());
                }
                items.push(self.class_body());
                node("object_literal", items)
            }
            "fun" => {
                let mut items = vec![self.punct()];
                if !self.at("(") {
                    let t = self.receiver_type_before_params();
                    items.push(t);
                    items.push(self.expect("."));
                }
                items.push(self.function_value_parameters());
                if self.at(":") {
                    items.push(self.punct());
                    items.push(self.type_());
                }
                if self.at("where") {
                    items.push(self.type_constraints());
                }
                if self.at("{") || (self.at("=") && !self.stops()) {
                    items.push(self.function_body());
                }
                node("anonymous_function", items)
            }
            _ => self.leaf("identifier"),
        }
    }

    /// `fun T.(x) {}`: the type before `.(`.
    fn receiver_type_before_params(&mut self) -> Item {
        let mut k = self.i;
        while !self.eof_at(k) && !(self.is(k, ".") && self.is(k + 1, "(")) {
            k += 1;
        }
        self.receiver_type(k)
    }

    fn eof_at(&self, k: usize) -> bool {
        self.kind_at(k) == L::Eof
    }

    /// A branch of `if`: a block, an assignment or an expression.
    fn branch(&mut self) -> Item {
        if self.at("{") {
            return self.block();
        }
        self.statement_like()
    }

    /// An expression, or an assignment.
    fn statement_like(&mut self) -> Item {
        let e = self.expression();
        if matches!(self.peek(), L::Op("=" | "+=" | "-=" | "*=" | "/=" | "%=")) && !self.stops() {
            let items = vec![
                e.with("left"),
                self.punct().with("operator"),
                self.expression().with("right"),
            ];
            return node("assignment", items);
        }
        e
    }

    fn if_expression(&mut self) -> Item {
        let mut items = vec![self.punct(), self.expect("(")];
        items.push(self.blind(Self::expression).with("condition"));
        items.push(self.expect(")"));
        if self.at(";") {
            items.push(self.punct());
        } else if !self.at("else") {
            items.push(self.branch());
        }
        // `else` on the next line continues the `if`.
        let semi_else = self.at(";") && self.is(self.i + 1, "else");
        if semi_else {
            items.push(self.punct());
        }
        if self.at("else") && !self.is(self.i + 1, "->") {
            items.push(self.punct());
            if self.at(";") {
                items.push(self.punct());
            } else {
                items.push(self.branch());
            }
        }
        node("if_expression", items)
    }

    fn when_expression(&mut self) -> Item {
        let mut items = vec![self.punct()];
        if self.at("(") {
            let mut s = vec![self.punct()];
            self.blind(|p| {
                if p.at("val") || p.at("@") {
                    while p.at("@") {
                        s.push(p.annotation());
                    }
                    s.push(p.punct());
                    s.push(p.variable_declaration());
                    s.push(p.expect("="));
                }
                s.push(p.expression());
            });
            s.push(self.expect(")"));
            items.push(node("when_subject", s));
        }
        items.push(self.expect("{"));
        self.sighted(false, |p| {
            // `in` on a line of its own starts the next entry's range test.
            p.in_when = true;
            while !p.eof() && !p.at("}") {
                let before = p.i;
                items.push(p.when_entry());
                if p.i == before {
                    items.push(p.error_token());
                }
            }
        });
        items.push(self.expect("}"));
        node("when_expression", items)
    }

    fn when_entry(&mut self) -> Item {
        let mut items = Vec::new();
        if self.at("else") {
            items.push(self.punct());
        } else {
            self.blind(|p| {
                loop {
                    let c = if (p.at("in") || p.at("!in")) && p.expression_starts(p.i + 1) {
                        let op = p.punct();
                        let e = p.expression();
                        node("range_test", vec![op, e])
                    } else if p.at("is") || p.at("!is") {
                        let op = p.punct();
                        let t = p.type_();
                        node("type_test", vec![op, t])
                    } else {
                        p.expression()
                    };
                    items.push(c.with("condition"));
                    if p.at(",") {
                        items.push(p.punct());
                        if p.at("->") {
                            break;
                        }
                        continue;
                    }
                    break;
                }
            });
        }
        items.push(self.expect("->"));
        items.push(if self.at("{") {
            self.block()
        } else {
            self.statement()
        });
        self.semi(&mut items);
        node("when_entry", items)
    }

    fn try_expression(&mut self) -> Item {
        let mut items = vec![self.punct(), self.block()];
        while self.at("catch") {
            let mut c = vec![self.punct(), self.expect("(")];
            self.blind(|p| {
                while p.at("@") {
                    c.push(p.annotation());
                }
                c.push(p.identifier());
                c.push(p.expect(":"));
                c.push(p.type_());
            });
            c.push(self.expect(")"));
            c.push(self.block());
            items.push(node("catch_block", c));
        }
        if self.at("finally") {
            let f = vec![self.punct(), self.block()];
            items.push(node("finally_block", f));
        }
        node("try_expression", items)
    }

    /// An explicit delegation's target: a primary and its chain, no lambda.
    ///
    /// It takes a lambda only when `lambda`.
    pub(crate) fn delegate_target(&mut self, lambda: bool) -> Item {
        let saved = std::mem::replace(&mut self.no_lambda, !lambda);
        let e = self.prefix();
        self.no_lambda = saved;
        e
    }

    // ---- literals ---------------------------------------------------------

    fn character_literal(&mut self) -> Item {
        let t = self.advance();
        let mut items = vec![Self::tok(
            crate::lexer::Lx {
                end: t.start + 1,
                ..t
            },
            "'",
            false,
        )];
        let inner = crate::lexer::Lx {
            start: t.start + 1,
            end: t.end.saturating_sub(1).max(t.start + 1),
            ..t
        };
        if self.src[inner.start..inner.end].starts_with('\\') {
            items.push(Self::tok(inner, "escape_sequence", true));
        } else {
            items.push(Self::hidden_tok(inner));
        }
        if t.end > t.start + 1 && self.src[..t.end].ends_with('\'') {
            items.push(Self::tok(
                crate::lexer::Lx {
                    start: t.end - 1,
                    ..t
                },
                "'",
                false,
            ));
        } else {
            items.push(self.missing("'", false));
        }
        node("character_literal", items)
    }

    fn string(&mut self) -> Item {
        let open = self.peek();
        let multiline = open == L::StrOpen("\"\"\"");
        let mut items = vec![self.punct()];
        loop {
            match self.peek() {
                L::Content => items.push(self.leaf("string_content")),
                L::Escape => items.push(self.leaf("escape_sequence")),
                L::InterpOpen => {
                    let open = self.punct();
                    let e = self.blind(Self::expression);
                    let close = self.expect("}");
                    items.push(node("interpolation", vec![open, e, close]));
                }
                L::Dollar => {
                    let d = self.punct();
                    let n = self.identifier();
                    items.push(node("interpolation", vec![d, n]));
                }
                _ => break,
            }
        }
        if matches!(self.peek(), L::StrClose(_)) {
            items.push(self.punct());
        } else {
            items.push(self.missing(if multiline { "\"\"\"" } else { "\"" }, false));
        }
        node(
            if multiline {
                "multiline_string_literal"
            } else {
                "string_literal"
            },
            items,
        )
    }
}
