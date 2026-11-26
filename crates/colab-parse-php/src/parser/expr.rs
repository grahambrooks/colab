//! Expressions by precedence climbing, at tree-sitter-php's precedences;
//! variables and their member, call and subscript chains; strings,
//! heredocs and nowdocs with their interpolations.

use super::{Item, Parser, node};
use crate::lexer::L;

const ASSIGNMENT: u8 = 4;
const TERNARY: u8 = 5;
const NEG: u8 = 19;
const INSTANCEOF: u8 = 20;
const CAST: u8 = 21;
const EXPONENTIAL: u8 = 22;
const CLONE: u8 = 23;

const AUGMENTED: &[&str] = &[
    "**=", "*=", "/=", "%=", "+=", "-=", ".=", "<<=", ">>=", "&=", "^=", "|=", "??=",
];

const CAST_TYPES: &[&str] = &[
    "array", "binary", "bool", "boolean", "double", "float", "int", "integer", "object", "real",
    "string", "unset",
];

/// The kinds an assignment's left side may be.
const VARIABLES: &[&str] = &[
    "variable_name",
    "dynamic_variable_name",
    "subscript_expression",
    "member_access_expression",
    "nullsafe_member_access_expression",
    "scoped_property_access_expression",
    "function_call_expression",
    "member_call_expression",
    "nullsafe_member_call_expression",
    "scoped_call_expression",
    "list_literal",
];

impl Parser<'_> {
    pub(crate) fn expression(&mut self) -> Item {
        self.expr_bp(0)
    }

    /// The binary operator here: its precedence and whether it groups to
    /// the right.
    fn binary_prec(&self) -> Option<(u8, bool)> {
        Some(match self.peek() {
            L::Op(o) => match o {
                "??" => (6, true),
                "||" => (7, false),
                "&&" => (8, false),
                "|" => (9, false),
                "^" => (10, false),
                "&" => (11, false),
                "==" | "!=" | "<>" | "===" | "!==" | "<=>" => (12, false),
                "<" | ">" | "<=" | ">=" => (13, false),
                "|>" => (14, false),
                "." => (15, false),
                "<<" | ">>" => (16, false),
                "+" | "-" => (17, false),
                "*" | "/" | "%" => (18, false),
                "**" => (EXPONENTIAL, true),
                _ => return None,
            },
            L::Name => match self.word().to_ascii_lowercase().as_str() {
                "or" => (1, false),
                "xor" => (2, false),
                "and" => (3, false),
                "instanceof" => (INSTANCEOF, false),
                _ => return None,
            },
            _ => return None,
        })
    }

    pub(crate) fn expr_bp(&mut self, min: u8) -> Item {
        let mut left = self.unary();
        loop {
            if self.at("?") && TERNARY >= min {
                let mut items = vec![left.with("condition"), self.punct()];
                if !self.at(":") {
                    items.push(self.expression().with("body"));
                }
                items.push(self.expect(":"));
                items.push(self.expr_bp(TERNARY + 1).with("alternative"));
                left = node("conditional_expression", items);
                continue;
            }
            let Some((prec, right)) = self.binary_prec() else {
                break;
            };
            if prec < min {
                break;
            }
            let instanceof = self.peek() == L::Name;
            let op = self.punct().with("operator");
            let rhs = if instanceof && prec == INSTANCEOF {
                self.class_name_reference()
            } else {
                self.expr_bp(if right { prec } else { prec + 1 })
            };
            left = node(
                "binary_expression",
                vec![left.with("left"), op, rhs.with("right")],
            );
        }
        left
    }

    fn unary(&mut self) -> Item {
        match self.peek() {
            L::Op("!") => {
                let op = self.punct().with("operator");
                let arg = self.expr_bp(NEG + 1).with("argument");
                return node("unary_op_expression", vec![op, arg]);
            }
            L::Op("+" | "-" | "~") => {
                let op = self.punct().with("operator");
                let arg = self.expr_bp(CAST + 1).with("argument");
                return node("unary_op_expression", vec![op, arg]);
            }
            L::Op("@") => {
                let op = self.punct();
                let arg = self.expr_bp(CAST);
                return node("error_suppression_expression", vec![op, arg]);
            }
            L::Op("++" | "--") => {
                let op = self.punct().with("operator");
                let arg = self.variable_chain().with("argument");
                return node("update_expression", vec![op, arg]);
            }
            L::Op("(") if self.cast_here() => {
                let items = vec![
                    self.punct(),
                    self.leaf("cast_type").with("type"),
                    self.punct(),
                    self.expr_bp(CAST).with("value"),
                ];
                return node("cast_expression", items);
            }
            L::Name => {
                let w = self.word().to_ascii_lowercase();
                match w.as_str() {
                    "clone" => {
                        let kw = self.punct();
                        // `clone($a, [...])` takes arguments; `clone($a)` a
                        // parenthesized value.
                        let placeholder = matches!(self.kind_at(self.i + 1), L::Op("?" | "..."))
                            && self.is(self.i + 2, ")");
                        if self.at("(") && (self.top_level_comma(self.i) || placeholder) {
                            let args = self.arguments().with("arguments");
                            return node("clone_expression", vec![kw, args]);
                        }
                        let arg = self.expr_bp(CLONE);
                        return node("clone_expression", vec![kw, arg]);
                    }
                    "print" => {
                        let kw = self.punct();
                        let arg = self.expression();
                        return node("print_intrinsic", vec![kw, arg]);
                    }
                    "throw" => {
                        let kw = self.punct();
                        let arg = self.expression();
                        return node("throw_expression", vec![kw, arg]);
                    }
                    "include" | "include_once" | "require" | "require_once" => {
                        let kind = match w.as_str() {
                            "include" => "include_expression",
                            "include_once" => "include_once_expression",
                            "require" => "require_expression",
                            _ => "require_once_expression",
                        };
                        let kw = self.punct();
                        let arg = self.expression();
                        return node(kind, vec![kw, arg]);
                    }
                    "yield" => return self.yield_expression(),
                    "match" if self.is(self.i + 1, "(") => {
                        let kw = self.punct();
                        let cond = self.parenthesized().with("condition");
                        let body = self.match_block().with("body");
                        return node("match_expression", vec![kw, cond, body]);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        let p = self.primary();
        let p = self.postfix(p);
        self.assignment_after(p)
    }

    /// Whether the brackets at `n` hold a comma of their own.
    fn top_level_comma(&self, n: usize) -> bool {
        let Some(end) = self.scan_balanced(n) else {
            return false;
        };
        let mut depth = 0usize;
        (n + 1..end - 1).any(|k| {
            match self.kind_at(k) {
                L::Op("(" | "[" | "{" | "#[") => depth += 1,
                L::Op(")" | "]" | "}") => depth = depth.saturating_sub(1),
                L::Op(",") if depth == 0 => return true,
                _ => {}
            }
            false
        })
    }

    /// `(int)` and the other casts.
    fn cast_here(&self) -> bool {
        self.kind_at(self.i + 1) == L::Name
            && self.is(self.i + 2, ")")
            && CAST_TYPES
                .iter()
                .any(|c| c.eq_ignore_ascii_case(self.word_at(self.i + 1)))
    }

    fn assignment_after(&mut self, left: Item) -> Item {
        if !VARIABLES.contains(&left.kind()) {
            return left;
        }
        match self.peek() {
            L::Op("=") if self.is(self.i + 1, "&") => {
                let items = vec![
                    left.with("left"),
                    self.punct(),
                    self.punct(),
                    self.expr_bp(ASSIGNMENT).with("right"),
                ];
                node("reference_assignment_expression", items)
            }
            L::Op("=") => {
                let items = vec![
                    left.with("left"),
                    self.punct(),
                    self.expr_bp(ASSIGNMENT).with("right"),
                ];
                node("assignment_expression", items)
            }
            L::Op(o) if AUGMENTED.contains(&o) => {
                let items = vec![
                    left.with("left"),
                    self.punct().with("operator"),
                    self.expr_bp(ASSIGNMENT).with("right"),
                ];
                node("augmented_assignment_expression", items)
            }
            _ => left,
        }
    }

    fn yield_expression(&mut self) -> Item {
        let t = self.tk(self.i);
        if self.kw_at(self.i + 1, "from") && self.src[t.end..self.tk(self.i + 1).start] == *" " {
            let end = self.tk(self.i + 1).end;
            self.i += 2;
            let kw = Self::tok(
                crate::lexer::Lx {
                    k: L::Name,
                    start: t.start,
                    end,
                },
                "yield from",
                false,
            );
            let e = self.expression();
            return node("yield_expression", vec![kw, e]);
        }
        let mut items = vec![self.punct()];
        if !matches!(
            self.peek(),
            L::Op(";" | ")" | "," | "]") | L::Eof | L::EndTag
        ) {
            items.push(self.array_element());
        }
        node("yield_expression", items)
    }

    // ---- primaries --------------------------------------------------------

    fn primary(&mut self) -> Item {
        match self.peek() {
            L::Op("$") => self.simple_variable(),
            L::Int => self.leaf("integer"),
            L::Float => self.leaf("float"),
            L::StrOpen(_) => self.string(),
            L::Op("<<<") => self.heredoc(),
            L::Op("[") => {
                let end = self.scan_balanced(self.i).unwrap_or(0);
                if self.is(end, "=") {
                    self.list_literal()
                } else {
                    self.array()
                }
            }
            L::Op("(") => self.parenthesized(),
            L::Op("#[") => {
                let attrs = self.attribute_list().with("attributes");
                self.closure(vec![attrs])
            }
            L::Op("\\") => self.class_name(),
            L::Op("`") => self.string(),
            L::Name => self.word_primary(),
            _ => self.missing("name", true),
        }
    }

    fn word_primary(&mut self) -> Item {
        let w = self.word().to_ascii_lowercase();
        let next = self.kind_at(self.i + 1);
        match w.as_str() {
            "true" | "false"
                if next != L::Op("\\") && next != L::Op("(") && next != L::Op("::") =>
            {
                self.leaf("boolean")
            }
            "null" if next != L::Op("\\") && next != L::Op("::") => self.leaf("null"),
            "array" if next == L::Op("(") => self.array(),
            "list" if next == L::Op("(") => self.list_literal(),
            "new" => self.object_creation(),
            "fn" | "function" => self.closure(Vec::new()),
            "static"
                if matches!(
                    self.word_at(self.i + 1).to_ascii_lowercase().as_str(),
                    "fn" | "function"
                ) =>
            {
                self.closure(Vec::new())
            }
            "self" | "parent" | "static" if next == L::Op("::") => self.wrapped("relative_scope"),
            _ => self.class_name(),
        }
    }

    /// `[…]` or `array(…)`.
    fn array(&mut self) -> Item {
        let mut items = Vec::new();
        let close = if self.at("[") {
            items.push(self.punct());
            "]"
        } else {
            items.push(self.punct());
            items.push(self.expect("("));
            ")"
        };
        self.comma_list(&mut items, close, true, Self::array_element);
        items.push(self.expect(close));
        node("array_creation_expression", items)
    }

    fn array_element(&mut self) -> Item {
        if self.at("...") {
            let dots = self.punct();
            let e = self.expression();
            return node(
                "array_element_initializer",
                vec![node("variadic_unpacking", vec![dots, e])],
            );
        }
        let first = if self.at("&") {
            self.by_ref()
        } else {
            self.expression()
        };
        let mut items = vec![first];
        if self.at("=>") {
            items.push(self.punct());
            items.push(if self.at("&") {
                self.by_ref()
            } else {
                self.expression()
            });
        }
        node("array_element_initializer", items)
    }

    /// `list(…)` or `[…]` being assigned to.
    pub(crate) fn list_literal(&mut self) -> Item {
        let mut items = Vec::new();
        let close = if self.at("[") {
            items.push(self.punct());
            "]"
        } else {
            items.push(self.punct());
            items.push(self.expect("("));
            ")"
        };
        loop {
            if self.at(close) || self.eof() {
                break;
            }
            if !self.at(",") {
                let before = self.i;
                let e = self.list_element();
                items.push(e);
                if self.at("=>") {
                    items.push(self.punct());
                    items.push(self.list_element());
                }
                if self.i == before {
                    items.push(self.error_token());
                }
            }
            if self.at(",") {
                items.push(self.punct());
            } else {
                break;
            }
        }
        items.push(self.expect(close));
        node("list_literal", items)
    }

    fn list_element(&mut self) -> Item {
        // `[] => $d`: a key is an expression.
        let key = self.scan_balanced(self.i).is_some_and(|e| self.is(e, "=>"));
        if (self.at("[") || (self.at_kw("list") && self.is(self.i + 1, "("))) && !key {
            return self.list_literal();
        }
        if self.at("&") {
            return self.by_ref();
        }
        self.expression()
    }

    fn closure(&mut self, mut items: Vec<Item>) -> Item {
        if self.at_kw("static") {
            items.push(self.wrapped("static_modifier").with("static_modifier"));
        }
        let arrow = self.at_kw("fn");
        items.push(self.punct());
        if self.at("&") {
            items.push(
                self.wrapped("reference_modifier")
                    .with("reference_modifier"),
            );
        }
        items.push(self.formal_parameters().with("parameters"));
        if arrow {
            self.return_type(&mut items);
            items.push(self.expect("=>"));
            items.push(self.expression().with("body"));
            return node("arrow_function", items);
        }
        if self.at_kw("use") {
            let mut u = vec![self.punct(), self.expect("(")];
            self.comma_list(&mut u, ")", true, |p| {
                if p.at("&") {
                    p.by_ref()
                } else {
                    p.simple_variable()
                }
            });
            u.push(self.expect(")"));
            items.push(node("anonymous_function_use_clause", u));
        }
        self.return_type(&mut items);
        items.push(self.compound().with("body"));
        node("anonymous_function", items)
    }

    fn object_creation(&mut self) -> Item {
        let kw = self.punct();
        let mut k = self.i;
        while self.is(k, "#[") {
            k = self.scan_balanced(k).unwrap_or(k + 1);
        }
        while matches!(
            self.word_at(k).to_ascii_lowercase().as_str(),
            "readonly" | "final" | "abstract"
        ) {
            k += 1;
        }
        if self.kw_at(k, "class") {
            let mut attrs = Vec::new();
            if self.at("#[") {
                attrs.push(self.attribute_list().with("attributes"));
            }
            let class = self.anonymous_class(attrs);
            return node("object_creation_expression", vec![kw, class]);
        }
        let class = self.class_name_reference();
        let mut items = vec![kw, class];
        if self.at("(") {
            items.push(self.arguments());
        }
        node("object_creation_expression", items)
    }

    /// What `new` and `instanceof` name: a class name, a variable chain or
    /// a parenthesized expression.
    fn class_name_reference(&mut self) -> Item {
        if self.at("(") {
            return self.parenthesized();
        }
        // A variable chain: `$a->b`, `$a['k']`, `static::$c[0]`.
        let base = if self.at("$") {
            Some(self.simple_variable())
        } else if self.is(self.i + 1, "::") && self.is(self.i + 2, "$") && self.peek() == L::Name {
            Some(if self.at_kw("static") {
                self.leaf("name")
            } else {
                self.class_name()
            })
        } else {
            None
        };
        if let Some(mut e) = base {
            loop {
                match self.peek() {
                    L::Op("[") => e = self.subscript(e),
                    L::Op("->" | "?->") => {
                        let kind = if self.at("->") {
                            "member_access_expression"
                        } else {
                            "nullsafe_member_access_expression"
                        };
                        let op = self.punct();
                        let mut items = vec![e.with("object"), op];
                        self.member_name(&mut items);
                        e = node(kind, items);
                    }
                    L::Op("::") if self.is(self.i + 1, "$") => {
                        let op = self.punct();
                        let name = self.simple_variable().with("name");
                        e = node(
                            "scoped_property_access_expression",
                            vec![e.with("scope"), op, name],
                        );
                    }
                    _ => return e,
                }
            }
        }
        if self.at_kw("static") {
            return self.leaf("name");
        }
        let n = self.class_name();
        if self.at("::") && self.is(self.i + 1, "$") {
            let op = self.punct();
            let name = self.simple_variable().with("name");
            return node(
                "scoped_property_access_expression",
                vec![n.with("scope"), op, name],
            );
        }
        n
    }

    /// `$name`, `$$name`, `${expr}`.
    pub(crate) fn simple_variable(&mut self) -> Item {
        let dollar = self.expect("$");
        match self.peek() {
            L::Name => {
                let n = self.leaf("name");
                node("variable_name", vec![dollar, n])
            }
            L::Op("$") => {
                let inner = self.simple_variable();
                node("dynamic_variable_name", vec![dollar, inner])
            }
            L::Op("{") => {
                let items = vec![dollar, self.punct(), self.expression(), self.expect("}")];
                node("dynamic_variable_name", items)
            }
            _ => {
                let n = self.missing("name", true);
                node("variable_name", vec![dollar, n])
            }
        }
    }

    /// A variable and its chain, for `&` and `++`.
    pub(crate) fn variable_chain(&mut self) -> Item {
        let p = self.primary();
        self.postfix(p)
    }

    pub(crate) fn unary_operand_variable(&mut self) -> Item {
        self.variable_chain()
    }

    fn subscript(&mut self, e: Item) -> Item {
        let mut items = vec![e, self.punct()];
        if !self.at("]") {
            items.push(self.expression());
        }
        items.push(self.expect("]"));
        node("subscript_expression", items)
    }

    /// After `->` or `::`: a name, a variable, or `{expr}`.
    fn member_name(&mut self, items: &mut Vec<Item>) {
        match self.peek() {
            L::Name => items.push(self.leaf("name").with("name")),
            L::Op("$") => items.push(self.simple_variable().with("name")),
            L::Op("{") => {
                items.push(self.punct());
                items.push(self.expression().with("name"));
                items.push(self.expect("}"));
            }
            _ => items.push(self.missing("name", true).with("name")),
        }
    }

    fn postfix(&mut self, mut e: Item) -> Item {
        loop {
            match self.peek() {
                L::Op("[") => e = self.subscript(e),
                L::Op("->" | "?->") => {
                    let nullsafe = self.at("?->");
                    let op = self.punct();
                    let mut items = vec![e.with("object"), op];
                    self.member_name(&mut items);
                    let kind = if self.at("(") {
                        items.push(self.arguments().with("arguments"));
                        if nullsafe {
                            "nullsafe_member_call_expression"
                        } else {
                            "member_call_expression"
                        }
                    } else if nullsafe {
                        "nullsafe_member_access_expression"
                    } else {
                        "member_access_expression"
                    };
                    e = node(kind, items);
                }
                L::Op("::") => {
                    let op = self.punct();
                    if self.at("$") {
                        let name = self.simple_variable().with("name");
                        if self.at("(") {
                            let args = self.arguments().with("arguments");
                            e = node(
                                "scoped_call_expression",
                                vec![e.with("scope"), op, name, args],
                            );
                        } else {
                            e = node(
                                "scoped_property_access_expression",
                                vec![e.with("scope"), op, name],
                            );
                        }
                    } else if self.at("{") {
                        let call = self.scan_balanced(self.i).is_some_and(|k| self.is(k, "("));
                        if call {
                            let mut items = vec![e.with("scope"), op];
                            self.member_name(&mut items);
                            items.push(self.arguments().with("arguments"));
                            e = node("scoped_call_expression", items);
                        } else {
                            // `Foo::{$name}`: a constant named by an
                            // expression, in a `name` node.
                            let open = self.punct();
                            let name = node("name", vec![self.expression()]);
                            let items = vec![e, op, open, name, self.expect("}")];
                            e = node("class_constant_access_expression", items);
                        }
                    } else if self.is(self.i + 1, "(") && self.peek() == L::Name {
                        let name = self.leaf("name").with("name");
                        let args = self.arguments().with("arguments");
                        e = node(
                            "scoped_call_expression",
                            vec![e.with("scope"), op, name, args],
                        );
                    } else {
                        let name = self.any_name();
                        e = node("class_constant_access_expression", vec![e, op, name]);
                    }
                }
                L::Op("(") if Self::callable(&e) => {
                    let args = self.arguments().with("arguments");
                    e = node("function_call_expression", vec![e.with("function"), args]);
                }
                L::Op("++" | "--") if VARIABLES.contains(&e.kind()) => {
                    let op = self.punct().with("operator");
                    e = node("update_expression", vec![e.with("argument"), op]);
                }
                _ => return e,
            }
        }
    }

    /// Whether `(` after `e` calls it.
    fn callable(e: &Item) -> bool {
        !matches!(
            e.kind(),
            "integer" | "float" | "boolean" | "null" | "heredoc" | "nowdoc"
        )
    }

    pub(crate) fn arguments(&mut self) -> Item {
        let mut items = vec![self.expect("(")];
        self.comma_list(&mut items, ")", true, |p| {
            if p.at("...") && matches!(p.kind_at(p.i + 1), L::Op(")" | ",")) {
                return p.wrapped("variadic_placeholder");
            }
            let mut a = Vec::new();
            if p.kind_at(p.i) == L::Name && p.is(p.i + 1, ":") {
                a.push(p.leaf("name").with("name"));
                a.push(p.punct());
            }
            if p.at("&") {
                a.push(p.wrapped("reference_modifier").with("reference_modifier"));
            }
            if p.at("...") {
                let dots = p.punct();
                let e = p.expression();
                a.push(node("variadic_unpacking", vec![dots, e]));
            } else if p.at("?") && matches!(p.kind_at(p.i + 1), L::Op(")" | ",")) {
                a.push(p.wrapped("argument_placeholder"));
            } else if matches!(
                p.word().to_ascii_lowercase().as_str(),
                "self" | "parent" | "static"
            ) && matches!(p.kind_at(p.i + 1), L::Op(")" | ","))
            {
                a.push(p.leaf("name"));
            } else {
                a.push(p.expression());
            }
            node("argument", a)
        });
        items.push(self.expect(")"));
        node("arguments", items)
    }

    fn match_block(&mut self) -> Item {
        let mut items = vec![self.expect("{")];
        self.comma_list(&mut items, "}", true, |p| {
            if p.at_kw("default") && p.is(p.i + 1, "=>") {
                let v = vec![
                    p.punct(),
                    p.punct(),
                    p.expression().with("return_expression"),
                ];
                return node("match_default_expression", v);
            }
            let mut conds = Vec::new();
            p.comma_list(&mut conds, "=>", true, Self::expression);
            let mut v = vec![node("match_condition_list", conds).with("conditional_expressions")];
            v.push(p.expect("=>"));
            v.push(p.expression().with("return_expression"));
            node("match_conditional_expression", v)
        });
        items.push(self.expect("}"));
        node("match_block", items)
    }

    // ---- strings ----------------------------------------------------------

    /// A quoted string: `'…'`, `"…"` or `` `…` ``.
    fn string(&mut self) -> Item {
        let open = self.advance();
        let L::StrOpen(q) = open.k else {
            return self.missing("string", true);
        };
        // `b'` / `b"`: the opening is no token of its own.
        let first = if open.end - open.start == 1 {
            Self::tok(open, q, false)
        } else {
            Self::hidden_tok(open)
        };
        let mut items = vec![first];
        if q == "'" {
            loop {
                match self.peek() {
                    L::Content => items.push(self.leaf("string_content")),
                    L::Escape => items.push(self.leaf("escape_sequence")),
                    _ => break,
                }
            }
        } else {
            self.string_parts(&mut items);
        }
        if matches!(self.peek(), L::StrClose(_)) {
            items.push(self.punct());
        } else {
            items.push(self.missing(q, false));
        }
        let kind = match q {
            "'" => "string",
            "\"" => "encapsed_string",
            _ => "shell_command_expression",
        };
        node(kind, items)
    }

    /// An interpolated string's parts, to its closing quote or a heredoc's
    /// end.
    fn string_parts(&mut self, items: &mut Vec<Item>) {
        loop {
            match self.peek() {
                L::Content => items.push(self.leaf("string_content")),
                L::Escape => items.push(self.leaf("escape_sequence")),
                L::Op("$") if self.is(self.i + 1, "{") => {
                    let v = vec![
                        self.punct(),
                        self.punct(),
                        self.expression(),
                        self.expect("}"),
                    ];
                    items.push(node("dynamic_variable_name", v));
                }
                L::Op("$") => {
                    let var = self.simple_variable();
                    if self.at("->") {
                        let v = vec![
                            var.with("object"),
                            self.punct(),
                            self.any_name().with("name"),
                        ];
                        items.push(node("member_access_expression", v));
                    } else if self.at("[") {
                        let mut v = vec![var, self.punct()];
                        match self.peek() {
                            L::Int => v.push(self.leaf("integer")),
                            L::Op("-") => {
                                let m = self.punct();
                                let n = self.leaf("integer");
                                v.push(node("unary_op_expression", vec![m, n]));
                            }
                            L::Op("$") => v.push(self.simple_variable()),
                            L::Name => v.push(self.leaf("name")),
                            _ => v.push(self.missing("name", true)),
                        }
                        v.push(self.expect("]"));
                        items.push(node("subscript_expression", v));
                    } else {
                        items.push(var);
                    }
                }
                L::Op("{") => {
                    items.push(self.punct());
                    items.push(self.expression());
                    items.push(self.expect("}"));
                }
                _ => return,
            }
        }
    }

    fn heredoc(&mut self) -> Item {
        let mut items = vec![self.punct()];
        let nowdoc = self.at("'");
        if self.at("\"") || self.at("'") {
            items.push(self.punct());
        }
        if self.peek() == L::HeredocStart {
            items.push(self.leaf("heredoc_start").with("identifier"));
        } else {
            items.push(self.missing("heredoc_start", true));
        }
        if self.at("\"") || self.at("'") {
            items.push(self.punct());
        }
        let nl = (self.peek() == L::BodyNl).then(|| self.advance());
        let mut body = Vec::new();
        if nowdoc {
            while self.peek() == L::NowdocString {
                body.push(self.leaf("nowdoc_string"));
            }
        } else {
            self.string_parts(&mut body);
        }
        if body.is_empty() {
            if let Some(t) = nl {
                items.push(Self::hidden_tok(t));
            }
        } else {
            let mut b = Vec::new();
            if let Some(t) = nl {
                b.push(Self::hidden_tok(t));
            }
            b.extend(body);
            let kind = if nowdoc {
                "nowdoc_body"
            } else {
                "heredoc_body"
            };
            items.push(node(kind, b).with("value"));
        }
        if self.peek() == L::HeredocEnd {
            items.push(self.leaf("heredoc_end").with("end_tag"));
        } else {
            items.push(self.missing("heredoc_end", true));
        }
        node(if nowdoc { "nowdoc" } else { "heredoc" }, items)
    }
}
