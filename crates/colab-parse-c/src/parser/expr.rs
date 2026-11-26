//! Expressions by precedence climbing (tree-sitter-cpp's levels), names
//! and qualified names, lambdas, initializer lists and literals.

use super::decl::Qk;
use super::{Dialect, Item, PRIMITIVES, Parser, Tok, node};
use crate::lexer::L;

// tree-sitter-c's precedences, shifted to start at 1.
const ASSIGN: u8 = 1;
const CONDITIONAL: u8 = 2;
const CAST: u8 = 15;
const SIZEOF: u8 = 16;
const UNARY: u8 = 17;

const ASSIGNMENTS: &[&str] = &["=", "*=", "/=", "%=", "+=", "-=", "<<=", "&=", "^=", "|="];

impl Parser<'_> {
    /// Whether token `n` can start an expression.
    pub(crate) fn expression_start(&self, n: usize) -> bool {
        match self.kind_at(n) {
            L::Name if !self.cpp() && self.is_ident(n) => true,
            L::Name => !matches!(
                self.word_at(n),
                "else"
                    | "case"
                    | "default"
                    | "public"
                    | "private"
                    | "protected"
                    | "typedef"
                    | "namespace"
                    | "using"
                    | "template"
                    | "friend"
                    | "struct"
                    | "class"
                    | "union"
                    | "enum"
                    | "return"
                    | "break"
                    | "continue"
                    | "goto"
            ),
            L::Number | L::StrOpen(_) | L::CharOpen(_) | L::RawOpen(_) => true,
            L::Op(o) => matches!(
                o,
                "(" | "[" | "!" | "~" | "-" | "+" | "++" | "--" | "*" | "&" | "::" | "{"
            ),
            _ => false,
        }
    }

    /// An expression or a comma expression (`a, b`, right-nested).
    pub(crate) fn comma_expression(&mut self) -> Item {
        let left = self.expression();
        if !self.at(",") {
            return left;
        }
        let comma = self.punct();
        let right = self.comma_expression();
        node(
            "comma_expression",
            vec![left.with("left"), comma, right.with("right")],
        )
    }

    pub(crate) fn expression(&mut self) -> Item {
        self.expr_bp(ASSIGN)
    }

    /// An expression where a comma ends it (an argument's).
    pub(crate) fn expr_no_comma(&mut self) -> Item {
        self.expr_bp(ASSIGN)
    }

    /// A bracketed part: `>` compares again.
    fn nested<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> T {
        let saved = std::mem::replace(&mut self.no_gt, 0);
        let r = f(self);
        self.no_gt = saved;
        r
    }

    fn binary_prec(&self) -> Option<(u8, usize, &'static str)> {
        let op = match self.peek() {
            L::Op(">") => {
                if self.no_gt > 0 {
                    return None;
                }
                let (n, kind) = self.joined_greater();
                return Some(match kind {
                    ">>=" => (ASSIGN, n, kind),
                    ">>" => (12, n, kind),
                    _ => (10, n, kind),
                });
            }
            L::Op(o) => o,
            L::Name if !self.cpp() => return None,
            L::Name => match self.word() {
                "or" => "or",
                "and" => "and",
                "bitor" => "bitor",
                "xor" => "xor",
                "bitand" => "bitand",
                "not_eq" => "not_eq",
                "and_eq" => "and_eq",
                "or_eq" => "or_eq",
                "xor_eq" => "xor_eq",
                _ => return None,
            },
            _ => return None,
        };
        let prec = match op {
            "||" | "or" => 4,
            "&&" | "and" => 5,
            "|" | "bitor" => 6,
            "^" | "xor" => 7,
            "&" | "bitand" => 8,
            "==" | "!=" | "not_eq" => 9,
            "<" | "<=" => 10,
            "<=>" => 11,
            "<<" => 12,
            "+" | "-" => 13,
            "*" | "/" | "%" => 14,
            "?" => CONDITIONAL,
            o if ASSIGNMENTS.contains(&o) || matches!(o, "and_eq" | "or_eq" | "xor_eq") => ASSIGN,
            _ => return None,
        };
        Some((prec, 1, op))
    }

    pub(crate) fn expr_bp(&mut self, min: u8) -> Item {
        let mut left = self.unary();
        while let Some((prec, n, op)) = self.binary_prec() {
            if prec < min {
                break;
            }
            if op == "?" {
                let q = self.punct();
                let mut items = vec![left.with("condition"), q];
                if !self.at(":") {
                    let c = self.nested(Self::comma_expression);
                    items.push(c.with("consequence"));
                }
                items.push(self.expect(":"));
                items.push(self.expr_bp(CONDITIONAL).with("alternative"));
                left = node("conditional_expression", items);
                continue;
            }
            let o = if n > 1 {
                self.joined(n, op)
            } else {
                self.punct()
            }
            .with("operator");
            if prec == ASSIGN {
                let right = if self.at("{") {
                    self.initializer_list()
                } else {
                    self.expression()
                };
                left = node(
                    "assignment_expression",
                    vec![left.with("left"), o, right.with("right")],
                );
                continue;
            }
            let right = self.expr_bp(prec + 1).with("right");
            left = node("binary_expression", vec![left.with("left"), o, right]);
        }
        left
    }

    fn unary(&mut self) -> Item {
        match self.peek() {
            // `-1`: one literal where an expression starts.
            L::Op("-" | "+") if self.kind_at(self.i + 1) == L::Number && self.adjacent(self.i) => {
                let start = self.tk(self.i).start;
                self.advance();
                let t = self.advance();
                let n = Item::Tok(Tok {
                    kind: "number_literal",
                    named: true,
                    start,
                    end: t.end,
                    field: None,
                    hidden: false,
                    missing: false,
                    extra: false,
                });
                let lit = self.with_suffix(n);
                return self.postfix(lit);
            }
            L::Op("!" | "~" | "-" | "+") => {
                let op = self.punct().with("operator");
                let arg = self.expr_bp(UNARY).with("argument");
                // tree-sitter-c reads `!b++` as `(!b)++`.
                if !self.cpp()
                    && let Item::Node(n) = &arg
                    && n.kind == "update_expression"
                    && n.items.last().is_some_and(
                        |i| matches!(i, Item::Tok(t) if t.kind == "++" || t.kind == "--"),
                    )
                    && let Item::Node(mut n) = arg
                {
                    let post = n
                        .items
                        .pop()
                        .unwrap_or_else(|| unreachable!("checked above"));
                    let inner = n.items.pop().unwrap_or_else(|| unreachable!("an argument"));
                    let u = node("unary_expression", vec![op, inner]).with("argument");
                    return node("update_expression", vec![u, post]);
                }
                return node("unary_expression", vec![op, arg]);
            }
            L::Name if matches!(self.word(), "not" | "compl") && self.cpp() => {
                let op = self.punct().with("operator");
                let arg = self.expr_bp(UNARY).with("argument");
                return node("unary_expression", vec![op, arg]);
            }
            L::Op("++" | "--") => {
                let op = self.punct().with("operator");
                let arg = self.expr_bp(UNARY).with("argument");
                return node("update_expression", vec![op, arg]);
            }
            L::Op("*" | "&") => {
                let op = self.punct().with("operator");
                let arg = self.expr_bp(CAST).with("argument");
                return node("pointer_expression", vec![op, arg]);
            }
            L::Op("(") => {
                if let Some(c) = self.cast_or_literal() {
                    return c;
                }
            }
            L::Name => match self.word() {
                "sizeof" => return self.sizeof(),
                "alignof" | "_Alignof" | "__alignof__" | "__alignof" | "_alignof"
                    if self.is(self.i + 1, "(") =>
                {
                    let items = vec![
                        self.punct(),
                        self.punct(),
                        self.type_descriptor().with("type"),
                        self.expect(")"),
                    ];
                    return node("alignof_expression", items);
                }
                "offsetof" if self.is(self.i + 1, "(") => {
                    let items = vec![
                        self.punct(),
                        self.punct(),
                        self.type_descriptor().with("type"),
                        self.expect(","),
                        self.ident_as("field_identifier").with("member"),
                        self.expect(")"),
                    ];
                    return node("offsetof_expression", items);
                }
                "__extension__" => {
                    let kw = self.punct();
                    let e = self.expr_bp(UNARY);
                    return node("extension_expression", vec![kw, e]);
                }
                _ if !self.cpp() => {}
                "co_await" => {
                    let op = self.punct().with("operator");
                    let arg = self.expr_bp(UNARY).with("argument");
                    return node("co_await_expression", vec![op, arg]);
                }
                "new" => return self.new_expression(None),
                "delete" => return self.delete_expression(None),
                _ => {}
            },
            L::Op("::") if self.is(self.i + 1, "new") && self.cpp() => {
                let c = self.punct();
                return self.new_expression(Some(c));
            }
            L::Op("::") if self.is(self.i + 1, "delete") && self.cpp() => {
                let c = self.punct();
                return self.delete_expression(Some(c));
            }
            _ => {}
        }
        let p = self.primary();
        self.postfix(p)
    }

    /// `(T)x` or `(T){…}`, if the parentheses hold a type.
    fn cast_or_literal(&mut self) -> Option<Item> {
        let e = self.scan_type_descriptor(self.i + 1)?;
        if !self.is(e, ")") {
            return None;
        }
        let next = e + 1;
        let literal = self.is(next, "{");
        if !literal && !self.cast_value_start(next) {
            return None;
        }
        // A name alone in the parentheses is a type only before what
        // cannot continue an expression.
        if !self.clearly_type(self.i + 1, e)
            && !self.cast_after_name(next)
            && !(literal && !self.cpp())
        {
            return None;
        }
        let open = self.punct();
        let ty = self.nested(Self::type_descriptor).with("type");
        let close = self.expect(")");
        if literal {
            let v = self.initializer_list().with("value");
            return Some(node(
                "compound_literal_expression",
                vec![open, ty, close, v],
            ));
        }
        let v = self.expr_bp(CAST).with("value");
        Some(node("cast_expression", vec![open, ty, close, v]))
    }

    /// Whether the type descriptor over `from..to` cannot be an expression
    /// (a keyword type, a pointer or reference, a template).
    fn clearly_type(&self, from: usize, to: usize) -> bool {
        // Words inside template arguments say nothing about the whole.
        let mut angle = 0usize;
        (from..to).any(|k| {
            match self.kind_at(k) {
                L::Op("<") => angle += 1,
                L::Op(">") => angle = angle.saturating_sub(1),
                _ => {}
            }
            if angle > 0 {
                return false;
            }
            let w = self.word_at(k);
            PRIMITIVES.contains(&w)
                || super::SIZED.contains(&w)
                || super::QUALIFIERS.contains(&w)
                || matches!(w, "struct" | "union" | "enum" | "class" | "typename" | "auto")
                || (matches!(self.kind_at(k), L::Op("*" | "&" | "&&")) && k + 1 == to)
                || matches!(self.kind_at(k), L::Op("*") if matches!(self.kind_at(k + 1), L::Op(")" | "*")))
        })
    }

    /// Whether `from..to` in `sizeof(…)` / `alignas(…)` is a type to
    /// tree-sitter: a clear one, or a qualified name; a plain or templated
    /// name is an expression (`sizeof(T<U>)` a parenthesized one).
    pub(crate) fn type_in_parens(&self, from: usize, to: usize) -> bool {
        self.clearly_type(from, to) || (from..to).any(|k| self.is(k, "::"))
    }

    /// After `(name)`: whether a cast's value follows (an identifier, a
    /// literal, a parenthesis), not an operator.
    fn cast_after_name(&self, n: usize) -> bool {
        match self.kind_at(n) {
            L::Name => {
                self.is_ident(n)
                    || matches!(
                        self.word_at(n),
                        "sizeof" | "this" | "new" | "true" | "false" | "nullptr"
                    )
            }
            L::Number | L::StrOpen(_) | L::CharOpen(_) | L::RawOpen(_) => true,
            // `(f)(x)` is a cast of `(x)` to tree-sitter; `(f)()` a call.
            // C's `(a)(b)` is a call, `(a)(int)b` a cast of a cast.
            L::Op("(") if self.cpp() => !self.is(n + 1, ")"),
            L::Op("(") => self.scan_type_descriptor(n + 1).is_some_and(|e| {
                self.is(e, ")") && self.clearly_type(n + 1, e) && self.cast_value_start(e + 1)
            }),
            L::Op("!" | "~") => true,
            _ => false,
        }
    }

    fn cast_value_start(&self, n: usize) -> bool {
        match self.kind_at(n) {
            L::Op("{") => false,
            _ => self.expression_start(n),
        }
    }

    fn sizeof(&mut self) -> Item {
        let kw = self.punct();
        if self.at("...") {
            let items = vec![
                kw,
                self.punct(),
                self.expect("("),
                self.identifier().with("value"),
                self.expect(")"),
            ];
            return node("sizeof_expression", items);
        }
        // `sizeof(int)` takes a type; `sizeof(x)` a parenthesized value.
        if self.at("(")
            && self
                .scan_type_descriptor(self.i + 1)
                .is_some_and(|e| self.is(e, ")") && self.type_in_parens(self.i + 1, e))
            && !self.is(self.scan_balanced(self.i).unwrap_or(0), "{")
            // tree-sitter-c's `sizeof(T)-1` takes the cast `(T)-1`.
            && !(!self.cpp()
                && self.scan_balanced(self.i).is_some_and(|e| {
                    matches!(self.kind_at(e), L::Op("-" | "+"))
                        && self.adjacent(e)
                        && self.kind_at(e + 1) == L::Number
                }))
        {
            let items = vec![
                kw,
                self.punct(),
                self.nested(Self::type_descriptor).with("type"),
                self.expect(")"),
            ];
            return node("sizeof_expression", items);
        }
        let v = self.expr_bp(SIZEOF).with("value");
        node("sizeof_expression", vec![kw, v])
    }

    fn new_expression(&mut self, colons: Option<Item>) -> Item {
        let mut items: Vec<Item> = colons.into_iter().collect();
        items.push(self.punct());
        if self.at("(") {
            items.push(self.argument_list().with("placement"));
        }
        items.push(self.type_specifier().with("type"));
        if self.at("[") {
            items.push(self.new_declarator().with("declarator"));
        }
        if self.at("(") {
            items.push(self.argument_list().with("arguments"));
        } else if self.at("{") {
            items.push(self.initializer_list().with("arguments"));
        }
        node("new_expression", items)
    }

    fn new_declarator(&mut self) -> Item {
        let mut items = vec![
            self.punct(),
            self.nested(Self::expression).with("length"),
            self.expect("]"),
        ];
        if self.at("[") {
            items.push(self.new_declarator());
        }
        node("new_declarator", items)
    }

    fn delete_expression(&mut self, colons: Option<Item>) -> Item {
        let mut items: Vec<Item> = colons.into_iter().collect();
        items.push(self.punct());
        if self.at("[") && self.is(self.i + 1, "]") {
            items.push(self.punct());
            items.push(self.punct());
        }
        items.push(self.expr_bp(CAST));
        node("delete_expression", items)
    }

    fn postfix(&mut self, mut e: Item) -> Item {
        loop {
            match self.peek() {
                L::Op("(") => {
                    let args = self.argument_list().with("arguments");
                    e = node("call_expression", vec![e.with("function"), args]);
                }
                // C: `a[i]`, the index a field of the expression's own.
                L::Op("[") if !self.cpp() => {
                    let open = self.punct();
                    let index = self.nested(Self::comma_expression).with("index");
                    let close = self.expect("]");
                    e = node(
                        "subscript_expression",
                        vec![e.with("argument"), open, index, close],
                    );
                }
                L::Op("[") if !self.at_attribute() => {
                    let mut items = vec![self.punct()];
                    self.nested(|p| {
                        p.comma_list(&mut items, "]", false, |p| {
                            if p.at("{") {
                                p.initializer_list()
                            } else {
                                p.expression()
                            }
                        });
                    });
                    items.push(self.expect("]"));
                    let idx = node("subscript_argument_list", items).with("indices");
                    e = node("subscript_expression", vec![e.with("argument"), idx]);
                }
                L::Op(o @ ("." | "->" | ".*")) => {
                    let _ = o;
                    let op = self.punct().with("operator");
                    let field = self.field_name().with("field");
                    e = node("field_expression", vec![e.with("argument"), op, field]);
                }
                L::Op("++" | "--") => {
                    let op = self.punct().with("operator");
                    e = node("update_expression", vec![e.with("argument"), op]);
                }
                _ => return e,
            }
        }
    }

    /// A field expression's field: a name, `~T`, `f<T>`, `A::b`,
    /// `template f<T>`.
    fn field_name(&mut self) -> Item {
        if !self.cpp() {
            return self.ident_as("field_identifier");
        }
        if self.at("~") {
            let t = self.punct();
            let id = self.identifier();
            return node("destructor_name", vec![t, id]);
        }
        if self.at("template") {
            let kw = self.punct();
            let m = self.template_method();
            return node("dependent_name", vec![kw, m]);
        }
        if self.at("operator") {
            return self.operator_name();
        }
        if self.is(self.i + 1, "::") {
            // `x.A::b`
            let scope = self.ident_as("namespace_identifier").with("scope");
            let colons = self.punct();
            let name = self.field_name().with("name");
            return node("qualified_identifier", vec![scope, colons, name]);
        }
        if self.is(self.i + 1, "<")
            && self
                .scan_template_args(self.i + 1)
                .is_some_and(|k| self.generic_follows(k))
        {
            return self.template_method();
        }
        self.ident_as("field_identifier")
    }

    fn template_method(&mut self) -> Item {
        let name = self.ident_as("field_identifier").with("name");
        let a = self.template_argument_list().with("arguments");
        node("template_method", vec![name, a])
    }

    /// Whether token `k`, after a template-argument list's `>`, makes the
    /// list a template's rather than comparisons.
    pub(crate) fn generic_follows(&self, k: usize) -> bool {
        match self.kind_at(k) {
            L::Op(o) => matches!(
                o,
                "(" | "::"
                    | "{"
                    | ")"
                    | ";"
                    | ","
                    | "]"
                    | "}"
                    | ">"
                    | "."
                    | "->"
                    | "="
                    | "=="
                    | "!="
                    | "&&"
                    | "||"
                    | "?"
                    | ":"
                    | "..."
                    | "["
            ),
            L::Eof => true,
            _ => false,
        }
    }

    fn primary(&mut self) -> Item {
        match self.peek() {
            L::Number => {
                let n = self.leaf("number_literal");
                self.with_suffix(n)
            }
            L::StrOpen(_) | L::RawOpen(_) => self.string_or_concatenation(),
            L::CharOpen(_) => {
                let c = self.char_literal();
                self.with_suffix(c)
            }
            L::Op("(") => self.paren_primary(),
            L::Op("[") if !self.at_attribute() && self.cpp() => self.lambda(),
            L::Op("{") => self.initializer_list(),
            L::Op("::") => self.name_primary(),
            L::Name => self.word_primary(),
            _ => self.missing("identifier", true),
        }
    }

    fn word_primary(&mut self) -> Item {
        match self.word() {
            "true" | "TRUE" => return self.leaf("true"),
            "false" | "FALSE" => return self.leaf("false"),
            "NULL" | "nullptr" => {
                let t = self.punct();
                return node("null", vec![t]);
            }
            "this" if self.cpp() => return self.leaf("this"),
            "requires" if self.cpp() => return self.requires_expression(),
            "decltype" if self.cpp() => {
                let d = self.type_specifier();
                return d;
            }
            "_Generic" if self.is(self.i + 1, "(") => return self.generic_expression(),
            "asm" | "__asm__" | "__asm"
                if self.is(self.i + 1, "(") || self.is(self.i + 1, "volatile") =>
            {
                return self.gnu_asm();
            }
            // tree-sitter-c reads a primitive in an expression as a name
            // (`va_arg(ap, int)`).
            w if (PRIMITIVES.contains(&w) || super::SIZED.contains(&w)) && !self.cpp() => {
                return self.leaf("identifier");
            }
            w if PRIMITIVES.contains(&w) => {
                let t = self.leaf("primitive_type");
                if self.at("{") {
                    let v = self.initializer_list().with("value");
                    return node("compound_literal_expression", vec![t.with("type"), v]);
                }
                if self.at("(") {
                    let args = self.argument_list().with("arguments");
                    return node("call_expression", vec![t.with("function"), args]);
                }
                return t;
            }
            _ => {}
        }
        if self.is_ident(self.i) || self.at("operator") || self.at("template") {
            return self.name_primary();
        }
        self.error_token()
    }

    /// A name in an expression; `T{…}` a compound literal; `X "s"` a
    /// concatenated string.
    fn name_primary(&mut self) -> Item {
        if self.is_ident(self.i)
            && matches!(self.kind_at(self.i + 1), L::StrOpen(_) | L::RawOpen(_))
        {
            return self.string_or_concatenation();
        }
        if !self.cpp() {
            return self.identifier();
        }
        let start = self.i;
        let name = self.expr_name(true);
        if self.at("{") && self.dialect == Dialect::Cpp && self.i > start {
            // `T{…}`: the name as a type.
            let ty = self.retype(name);
            let v = self.initializer_list().with("value");
            return node("compound_literal_expression", vec![ty.with("type"), v]);
        }
        name
    }

    /// An expression's name re-read as a type's (`identifier` to
    /// `type_identifier`, `template_function` to `template_type`).
    fn retype(&mut self, name: Item) -> Item {
        match name {
            Item::Tok(t) if t.kind == "identifier" => Item::Tok(Tok {
                kind: "type_identifier",
                ..t
            }),
            Item::Node(mut n) if n.kind == "template_function" => {
                n.kind = "template_type";
                if let Some(Item::Tok(t)) = n.items.first_mut()
                    && t.kind == "identifier"
                {
                    t.kind = "type_identifier";
                }
                Item::Node(n)
            }
            Item::Node(mut n) if n.kind == "qualified_identifier" => {
                if let Some(last) = n.items.pop() {
                    let field = last.field();
                    let mut r = self.retype(last.without_field());
                    if let Some(f) = field {
                        r = r.with(f);
                    }
                    n.items.push(r);
                }
                Item::Node(n)
            }
            other => other,
        }
    }

    /// A (possibly qualified) name in an expression (`expr`) or a
    /// declarator: `x`, `f<T>`, `a::b`, `::x`, `A<T>::f`, `~A`,
    /// `operator+`.
    pub(crate) fn expr_name(&mut self, expr: bool) -> Item {
        let qk = if expr { Qk::Expr } else { Qk::Decl };
        if self.at("::") {
            let colons = self.punct();
            let name = self.qualified_name_tail(qk).with("name");
            return node("qualified_identifier", vec![colons, name]);
        }
        self.qualified_name_tail(qk)
    }

    /// The name after a scope's `::` (or a name alone).
    pub(crate) fn qualified_name_tail(&mut self, qk: Qk) -> Item {
        if self.at("~") {
            let t = self.punct();
            let id = self.identifier();
            return node("destructor_name", vec![t, id]);
        }
        if self.at("operator") {
            return self.operator_name();
        }
        let template_kw = self.at("template") && self.is_ident(self.i + 1);
        if template_kw && !self.is(self.i + 2, "<") {
            // `A::template f` without arguments.
            let kw = self.punct();
            let id = self.identifier();
            return node("", vec![kw, id]);
        }
        let k = if template_kw { self.i + 1 } else { self.i };
        let args_end = if self.is(k + 1, "<") {
            self.scan_template_args(k + 1)
        } else {
            None
        };
        // A scope: `a::`, `A<T>::`.
        let scope_end = args_end.unwrap_or(k + 1);
        if self.is(scope_end, "::") && self.is_ident(k) {
            let scope = if args_end.is_some() {
                let kw = if template_kw {
                    Some(self.punct())
                } else {
                    None
                };
                let name = self.ident_as("type_identifier").with("name");
                let a = self.template_argument_list().with("arguments");
                let t = node("template_type", vec![name, a]);
                match kw {
                    Some(kw) => node("dependent_name", vec![kw, t]),
                    None => t,
                }
            } else {
                self.ident_as("namespace_identifier")
            };
            return self.qualified_after(scope, qk);
        }
        if let Some(e) = args_end {
            let generic = match qk {
                Qk::Expr => self.generic_follows(e),
                _ => true,
            };
            if generic {
                let kw = if template_kw {
                    Some(self.punct())
                } else {
                    None
                };
                let name = self.identifier().with("name");
                let a = self.template_argument_list().with("arguments");
                let t = node("template_function", vec![name, a]);
                return match kw {
                    Some(kw) => node("dependent_name", vec![kw, t]),
                    None => t,
                };
            }
        }
        if template_kw {
            let kw = self.punct();
            let id = self.identifier();
            return node("", vec![kw, id]);
        }
        self.identifier()
    }

    fn paren_primary(&mut self) -> Item {
        let open = self.punct();
        // `({ … })`: a statement expression.
        if self.at("{") {
            let c = self.compound_statement();
            let close = self.expect(")");
            return node("parenthesized_expression", vec![open, c, close]);
        }
        // `(... op pack)`: a left fold.
        if self.at("...") && self.dialect == Dialect::Cpp {
            let dots = self.punct().with("left");
            let op = self.fold_operator();
            let right = self.nested(Self::expression).with("right");
            let close = self.expect(")");
            return node("fold_expression", vec![open, dots, op, right, close]);
        }
        let e = self.nested(Self::expression);
        let e = if self.at(",") && !self.is(self.i + 1, "...") {
            // `(a, b)`: a comma expression, `e` its left.
            let comma = self.punct();
            let right = self.nested(Self::comma_expression);
            node(
                "comma_expression",
                vec![e.with("left"), comma, right.with("right")],
            )
        } else {
            e
        };
        // `(pack op ...)` and `(init op ... op pack)`.
        if self.dialect == Dialect::Cpp
            && self.fold_operator_at()
            && self.is(self.i + self.fold_len(), "...")
        {
            let op = self.fold_operator();
            let dots = self.punct();
            if self.at(")") {
                let close = self.punct();
                return node(
                    "fold_expression",
                    vec![open, e.with("left"), op, dots.with("right"), close],
                );
            }
            let op2 = self.fold_operator();
            let right = self.nested(Self::expression).with("right");
            let close = self.expect(")");
            return node(
                "fold_expression",
                vec![open, e.with("left"), op, dots, op2, right, close],
            );
        }
        let close = self.expect(")");
        node("parenthesized_expression", vec![open, e, close])
    }

    fn fold_len(&self) -> usize {
        if self.at(">") {
            self.joined_greater().0
        } else {
            1
        }
    }

    fn fold_operator_at(&self) -> bool {
        match self.peek() {
            L::Op(o) => matches!(
                o,
                "+" | "-"
                    | "*"
                    | "/"
                    | "%"
                    | "^"
                    | "&"
                    | "|"
                    | "="
                    | "<"
                    | ">"
                    | "<<"
                    | "+="
                    | "-="
                    | "*="
                    | "/="
                    | "%="
                    | "^="
                    | "&="
                    | "|="
                    | "<<="
                    | "=="
                    | "!="
                    | "<="
                    | "&&"
                    | "||"
                    | ","
                    | ".*"
                    | "->*"
            ),
            L::Name => matches!(
                self.word(),
                "or" | "and" | "bitor" | "xor" | "bitand" | "not_eq"
            ),
            _ => false,
        }
    }

    fn fold_operator(&mut self) -> Item {
        if self.at(">") {
            let (n, kind) = self.joined_greater();
            return self.joined(n, kind).with("operator");
        }
        self.punct().with("operator")
    }

    // ---- lambdas -------------------------------------------------------------

    fn lambda(&mut self) -> Item {
        let mut items = vec![self.lambda_captures().with("captures")];
        if self.at("<") {
            items.push(self.template_parameter_list().with("template_parameters"));
            if self.at("requires") {
                items.push(self.requires_clause().with("constraint"));
            }
        }
        if self.at("(")
            || matches!(self.word(), "mutable" | "constexpr" | "noexcept")
            || self.at("->")
        {
            let mut d = Vec::new();
            if self.at("(") {
                d.push(self.parameter_list().with("parameters"));
            }
            self.lambda_specifiers(&mut d);
            items.push(node("abstract_function_declarator", d).with("declarator"));
        }
        items.push(self.compound_statement().with("body"));
        node("lambda_expression", items)
    }

    fn lambda_specifiers(&mut self, items: &mut Vec<Item>) {
        loop {
            match self.word() {
                "mutable" | "constexpr" | "consteval" | "const" => {
                    items.push(self.wrapped("type_qualifier"));
                }
                "noexcept" => {
                    let mut n = vec![self.punct()];
                    if self.at("(") {
                        n.push(self.punct());
                        if !self.at(")") {
                            n.push(self.expression());
                        }
                        n.push(self.expect(")"));
                    }
                    items.push(node("noexcept", n));
                }
                "requires" => items.push(self.requires_clause()),
                _ if self.at("->") => {
                    let arrow = self.punct();
                    let t = self.type_descriptor();
                    items.push(node("trailing_return_type", vec![arrow, t]));
                }
                _ if self.at_attribute() => items.push(self.attribute_declaration()),
                _ => return,
            }
        }
    }

    fn lambda_captures(&mut self) -> Item {
        let mut items = vec![self.punct()];
        self.nested(|p| {
            p.comma_list(&mut items, "]", false, |p| {
                if (p.at("=") || p.at("&")) && matches!(p.kind_at(p.i + 1), L::Op("," | "]")) {
                    return p.wrapped("lambda_default_capture");
                }
                if p.at("this") {
                    return p.leaf("this");
                }
                if p.at("*") && p.is(p.i + 1, "this") {
                    let s = p.punct();
                    let t = p.leaf("this");
                    return node("", vec![s, t]);
                }
                let amp = if p.at("&") { Some(p.punct()) } else { None };
                let dots = if p.at("...") { Some(p.punct()) } else { None };
                if p.is_ident(p.i) && p.is(p.i + 1, "=") {
                    let mut c: Vec<Item> = amp.into_iter().collect();
                    c.extend(dots);
                    c.push(p.identifier().with("left"));
                    c.push(p.punct());
                    c.push(p.expression().with("right"));
                    return node("lambda_capture_initializer", c);
                }
                let mut c: Vec<Item> = amp.into_iter().collect();
                let name = p.expr_name(true);
                if p.at("...") {
                    let d = p.punct();
                    c.push(node(
                        "parameter_pack_expansion",
                        vec![name.with("pattern"), d],
                    ));
                } else {
                    c.push(name);
                }
                if c.len() == 1 {
                    c.pop().unwrap_or_else(|| p.missing("identifier", true))
                } else {
                    node("", c)
                }
            });
        });
        items.push(self.expect("]"));
        node("lambda_capture_specifier", items)
    }

    // ---- lists ---------------------------------------------------------------------

    pub(crate) fn argument_list(&mut self) -> Item {
        let mut items = vec![self.expect("(")];
        self.nested(|p| {
            p.comma_list(&mut items, ")", false, |p| {
                // `F({ x = 1; }, …)`: a compound statement, its `;`s say.
                let e = if p.at("{") && p.braces_hold_statements() {
                    p.compound_statement()
                } else if p.at("{") {
                    p.initializer_list()
                } else {
                    p.expression()
                };
                if p.at("...") {
                    let d = p.punct();
                    return node("parameter_pack_expansion", vec![e.with("pattern"), d]);
                }
                e
            });
        });
        items.push(self.expect(")"));
        node("argument_list", items)
    }

    pub(crate) fn initializer_list(&mut self) -> Item {
        let mut items = vec![self.expect("{")];
        self.nested(|p| {
            p.comma_list(&mut items, "}", true, |p| {
                // Designators: `.x = v`, `[i] = v`, `x: v`.
                if p.at(".") && p.is_ident(p.i + 1)
                    || p.at("[") && !p.at_attribute() && p.designator_at()
                {
                    let mut d = Vec::new();
                    while p.at(".") || (p.at("[") && !p.at_attribute()) {
                        if p.at(".") {
                            let dot = p.punct();
                            let f = p.ident_as("field_identifier");
                            d.push(node("field_designator", vec![dot, f]).with("designator"));
                        } else {
                            let open = p.punct();
                            let a = p.expression();
                            if p.at("...") {
                                let dots = p.punct();
                                let b = p.expression().with("end");
                                let close = p.expect("]");
                                d.push(
                                    node(
                                        "subscript_range_designator",
                                        vec![open, a.with("start"), dots, b, close],
                                    )
                                    .with("designator"),
                                );
                            } else {
                                let close = p.expect("]");
                                d.push(
                                    node("subscript_designator", vec![open, a, close])
                                        .with("designator"),
                                );
                            }
                        }
                    }
                    d.push(p.expect("="));
                    let v = if p.at("{") {
                        p.initializer_list()
                    } else {
                        p.expression()
                    };
                    d.push(v.with("value"));
                    return node("initializer_pair", d);
                }
                if p.is_ident(p.i) && p.is(p.i + 1, ":") && !p.is(p.i + 2, ":") {
                    let f = p.leaf("field_identifier").with("designator");
                    let c = p.punct();
                    let v = if p.at("{") {
                        p.initializer_list()
                    } else {
                        p.expression()
                    };
                    return node("initializer_pair", vec![f, c, v.with("value")]);
                }
                let e = if p.at("{") {
                    p.initializer_list()
                } else {
                    p.expression()
                };
                if p.at("...") {
                    let d = p.punct();
                    return node("parameter_pack_expansion", vec![e.with("pattern"), d]);
                }
                e
            });
        });
        items.push(self.expect("}"));
        node("initializer_list", items)
    }

    /// At `{`: whether the braces hold statements (a `;` at their level).
    pub(crate) fn braces_hold_statements(&self) -> bool {
        let mut depth = 0usize;
        let mut k = self.i;
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

    /// At `[` in an initializer list: a designator's (`[i] = v`).
    fn designator_at(&self) -> bool {
        let mut k = self.i;
        while self.is(k, "[") || self.is(k, ".") {
            if self.is(k, ".") {
                k += 2;
                continue;
            }
            match self.scan_balanced(k) {
                Some(e) => k = e,
                None => return false,
            }
        }
        self.is(k, "=")
    }

    pub(crate) fn parenthesized_expression(&mut self) -> Item {
        let open = self.expect("(");
        let e = self.nested(Self::comma_expression);
        let close = self.expect(")");
        node("parenthesized_expression", vec![open, e, close])
    }

    pub(crate) fn requires_expression(&mut self) -> Item {
        let mut items = vec![self.punct()];
        if self.at("(") {
            items.push(self.parameter_list().with("parameters"));
        }
        let mut seq = vec![self.expect("{")];
        while !self.eof() && !self.at("}") {
            let before = self.i;
            if self.at("typename") {
                let r = vec![self.punct(), self.type_name(), self.expect(";")];
                seq.push(node("type_requirement", r));
            } else if self.at("{") {
                let mut r = vec![self.punct(), self.expression(), self.expect("}")];
                if self.at("noexcept") {
                    r.push(self.punct());
                }
                if self.at("->") {
                    let arrow = self.punct();
                    let t = self.type_descriptor();
                    r.push(node("trailing_return_type", vec![arrow, t]));
                }
                r.push(self.expect(";"));
                seq.push(node("compound_requirement", r));
            } else {
                let mut r = Vec::new();
                if !self.at(";") {
                    r.push(self.comma_expression());
                }
                r.push(self.expect(";"));
                seq.push(node("simple_requirement", r));
            }
            if self.i == before {
                seq.push(self.error_token());
            }
        }
        seq.push(self.expect("}"));
        items.push(node("requirement_seq", seq).with("requirements"));
        node("requires_expression", items)
    }

    fn generic_expression(&mut self) -> Item {
        let mut items = vec![
            self.punct(),
            self.punct(),
            self.expression(),
            self.expect(","),
        ];
        loop {
            items.push(self.type_descriptor());
            items.push(self.expect(":"));
            items.push(self.expression());
            if self.at(",") {
                items.push(self.punct());
                continue;
            }
            break;
        }
        items.push(self.expect(")"));
        node("generic_expression", items)
    }

    pub(crate) fn gnu_asm(&mut self) -> Item {
        let mut items = vec![self.punct()];
        while matches!(self.word(), "volatile" | "__volatile__" | "inline" | "goto") {
            items.push(self.wrapped("gnu_asm_qualifier"));
        }
        items.push(self.expect("("));
        items.push(self.string_or_concatenation().with("assembly_code"));
        let lists = [
            ("gnu_asm_output_operand_list", "output_operands"),
            ("gnu_asm_input_operand_list", "input_operands"),
            ("gnu_asm_clobber_list", "clobbers"),
            ("gnu_asm_goto_list", "goto_labels"),
        ];
        for (kind, field) in lists {
            if !self.at(":") {
                break;
            }
            let mut l = vec![self.punct()];
            loop {
                if self.at(":") || self.at(")") || self.eof() {
                    break;
                }
                let item = match kind {
                    "gnu_asm_clobber_list" => self.string_or_concatenation().with("register"),
                    "gnu_asm_goto_list" => self.identifier().with("label"),
                    _ => {
                        let mut o = Vec::new();
                        if self.at("[") {
                            o.push(self.punct());
                            o.push(self.identifier().with("symbol"));
                            o.push(self.expect("]"));
                        }
                        o.push(self.string_literal().with("constraint"));
                        o.push(self.expect("("));
                        o.push(self.expression().with("value"));
                        o.push(self.expect(")"));
                        let k = if kind == "gnu_asm_output_operand_list" {
                            "gnu_asm_output_operand"
                        } else {
                            "gnu_asm_input_operand"
                        };
                        node(k, o).with("operand")
                    }
                };
                l.push(item);
                if self.at(",") {
                    l.push(self.punct());
                    continue;
                }
                break;
            }
            items.push(node(kind, l).with(field));
        }
        items.push(self.expect(")"));
        node("gnu_asm_expression", items)
    }

    // ---- literals ------------------------------------------------------------------

    /// A literal with a touching suffix as a `user_defined_literal`.
    fn with_suffix(&mut self, lit: Item) -> Item {
        if self.peek() == L::Suffix && self.dialect == Dialect::Cpp {
            let s = self.leaf("literal_suffix");
            return node("user_defined_literal", vec![lit, s]);
        }
        lit
    }

    /// Strings (and words between them) side by side: one
    /// `concatenated_string`, or the string alone.
    pub(crate) fn string_or_concatenation(&mut self) -> Item {
        let mut parts = Vec::new();
        let first = self.string_part();
        let mut strings = usize::from(matches!(
            first.kind(),
            "string_literal" | "raw_string_literal"
        ));
        parts.push(first);
        loop {
            match self.peek() {
                L::StrOpen(_) | L::RawOpen(_) => {
                    parts.push(self.string_part());
                    strings += 1;
                }
                // C++'s second part must be a string; C's may be a word
                // (`"a" PRIu64 "b"`).
                L::Name
                    if self.is_ident(self.i)
                        && (parts.len() >= 2 || !self.cpp())
                        && strings >= 1
                        && !self.is(self.i + 1, "(") =>
                {
                    parts.push(self.leaf("identifier"));
                }
                _ => break,
            }
        }
        if parts.len() == 1 {
            let p = parts
                .pop()
                .unwrap_or_else(|| self.missing("string_literal", true));
            return self.with_suffix(p);
        }
        node("concatenated_string", parts)
    }

    fn string_part(&mut self) -> Item {
        match self.peek() {
            L::StrOpen(_) => self.string_literal(),
            L::RawOpen(_) => self.raw_string_literal(),
            _ => self.leaf("identifier"),
        }
    }

    pub(crate) fn string_literal(&mut self) -> Item {
        if !matches!(self.peek(), L::StrOpen(_)) {
            return self.missing("string_literal", true);
        }
        let mut items = vec![self.punct()];
        loop {
            match self.peek() {
                L::StrContent => items.push(self.leaf("string_content")),
                L::Escape => items.push(self.leaf("escape_sequence")),
                L::Close(_) => {
                    items.push(self.punct());
                    break;
                }
                _ => {
                    items.push(self.missing("\"", false));
                    break;
                }
            }
        }
        node("string_literal", items)
    }

    fn raw_string_literal(&mut self) -> Item {
        let mut items = vec![self.punct()];
        if self.peek() == L::RawDelimiter {
            items.push(self.leaf("raw_string_delimiter").with("delimiter"));
        }
        items.push(self.expect("("));
        if self.peek() == L::RawContent {
            items.push(self.leaf("raw_string_content"));
        }
        items.push(self.expect(")"));
        if self.peek() == L::RawDelimiter {
            items.push(self.leaf("raw_string_delimiter"));
        }
        if matches!(self.peek(), L::Close(_)) {
            items.push(self.punct());
        } else {
            items.push(self.missing("\"", false));
        }
        node("raw_string_literal", items)
    }

    pub(crate) fn char_literal(&mut self) -> Item {
        let mut items = vec![self.punct()];
        loop {
            match self.peek() {
                L::Character => items.push(self.leaf("character")),
                L::Escape => items.push(self.leaf("escape_sequence")),
                L::Close(_) => {
                    items.push(self.punct());
                    break;
                }
                _ => {
                    items.push(self.missing("'", false));
                    break;
                }
            }
        }
        node("char_literal", items)
    }

    /// At `>`: how many touching tokens make the operator, and its kind.
    pub(crate) fn joined_greater(&self) -> (usize, &'static str) {
        let i = self.i;
        let gt = |k: usize| self.is(i + k, ">") && self.adjacent(i + k - 1);
        let eq = |k: usize| self.is(i + k, "=") && self.adjacent(i + k - 1);
        if gt(1) {
            if eq(2) {
                return (3, ">>=");
            }
            return (2, ">>");
        }
        if eq(1) {
            return (2, ">=");
        }
        (1, ">")
    }
}
