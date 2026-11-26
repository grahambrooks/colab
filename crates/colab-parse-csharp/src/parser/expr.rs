//! Expressions (by precedence climbing, tree-sitter-c-sharp's levels),
//! patterns, queries, and string literals.

use super::types::TypeCtx;
use super::{ASSIGNMENTS, Item, PREDEFINED, Parser, node};
use crate::lexer::L;

// tree-sitter-c-sharp's precedences.
const COALESCING: u8 = 3;
const REL: u8 = 10;
const SHIFT: u8 = 11;
const WITH: u8 = 14;
const SWITCH: u8 = 15;
const RANGE: u8 = 16;
const UNARY: u8 = 17;

/// Where a pattern is: what its constant expressions may hold.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum PatternCtx {
    /// After `is`, and in a subpattern: only operators binding tighter
    /// than `is`.
    Is,
    /// After `case`, in a switch arm: any binary operator.
    Case,
    /// After an `is` in a chain tree-sitter does not take into it: a
    /// constant is only its primary expression.
    Narrow,
}

/// Words that cannot follow a cast's `)` as its value: operators read as
/// words.
const NOT_CAST_VALUE: &[&str] = &[
    "is", "as", "switch", "with", "when", "and", "or", "in", "out",
];

/// A query's clause keywords: in a query, never a cast's value or a
/// pattern's designation.
const QUERY_WORDS: &[&str] = &[
    "select",
    "where",
    "orderby",
    "group",
    "by",
    "into",
    "on",
    "equals",
    "ascending",
    "descending",
    "join",
    "let",
    "from",
];

impl Parser<'_> {
    /// Whether token `n` can start an expression.
    pub(super) fn expression_start(&self, n: usize) -> bool {
        match self.kind_at(n) {
            L::Name => !matches!(
                self.word_at(n),
                "is" | "as"
                    | "else"
                    | "case"
                    | "catch"
                    | "finally"
                    | "in"
                    | "out"
                    | "operator"
                    | "class"
                    | "struct"
                    | "interface"
                    | "enum"
                    | "namespace"
                    | "params"
            ),
            L::Int | L::Real | L::Quote | L::Verbatim | L::RawStart | L::InterpStart => true,
            L::Op(o) => matches!(
                o,
                "(" | "[" | "{" | "!" | "~" | "+" | "-" | "++" | "--" | "^" | "&" | "*" | ".."
            ),
            _ => false,
        }
    }

    /// Whether token `n` can start a cast's value (as tree-sitter's parse
    /// reads `(a) - b` and `(a)[0]` as casts).
    fn cast_value_start(&self, n: usize) -> bool {
        match self.kind_at(n) {
            L::Name => {
                let w = self.word_at(n);
                !NOT_CAST_VALUE.contains(&w)
                    && !(self.queries > 0 && QUERY_WORDS.contains(&w))
                    && self.expression_start(n)
            }
            L::Op("{" | "..") => false,
            _ => self.expression_start(n),
        }
    }

    pub(crate) fn expression(&mut self) -> Item {
        self.expr_bp(1)
    }

    /// At `?.name` (`Some(true)`) or a `?[` that touches (`Some(false)`):
    /// a conditional access, not a conditional.
    fn conditional_access_at(&self) -> Option<bool> {
        if !self.at("?") {
            return None;
        }
        if self.is(self.i + 1, ".") && self.is_ident(self.i + 2) {
            return Some(true);
        }
        (self.is(self.i + 1, "[") && self.adjacent(self.i)).then_some(false)
    }

    /// `f` with no `is` chain in force: a bracketed expression starts
    /// afresh.
    pub(super) fn nested<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> T {
        let saved = std::mem::replace(&mut self.chain_end, 0);
        let lambda = std::mem::replace(&mut self.no_lambda, false);
        let r = f(self);
        self.chain_end = saved;
        self.no_lambda = lambda;
        r
    }

    fn expr_bp(&mut self, min: u8) -> Item {
        let mut left = self.unary();
        loop {
            // `?.` / `?[` bind as tree-sitter's conditional access does: at
            // the conditional's precedence on the left (`x + b?.c` is
            // `(x + b)?.c`), then the access's postfixes.
            if let Some(binding) = self.conditional_access_at() {
                if min > 2 {
                    break;
                }
                let q = self.punct();
                let b = if binding {
                    let dot = self.punct();
                    let name = self.simple_name_expr().with("name");
                    node("member_binding_expression", vec![dot, name])
                } else {
                    self.nested(Self::bracketed_argument_list)
                        .renamed("element_binding_expression")
                };
                left = node(
                    "conditional_access_expression",
                    vec![left.with("condition"), q, b],
                );
                left = self.postfix(left);
                continue;
            }
            let (prec, op): (u8, &'static str) = match self.peek() {
                L::Op(">") => {
                    let (n, kind) = self.joined_greater();
                    if n > 1 && kind.ends_with('=') && kind != ">=" {
                        // `>>=`, `>>>=`: an assignment.
                        if min > 1 {
                            break;
                        }
                        let op = self.joined(n, kind).with("operator");
                        let right = self.expression().with("right");
                        left = node("assignment_expression", vec![left.with("left"), op, right]);
                        continue;
                    }
                    let prec = match kind {
                        ">>" | ">>>" => SHIFT,
                        _ => REL,
                    };
                    if prec < min {
                        break;
                    }
                    let op = self.joined(n, kind).with("operator");
                    let right = self.expr_bp(prec + 1).with("right");
                    left = node("binary_expression", vec![left.with("left"), op, right]);
                    continue;
                }
                L::Op(o) if ASSIGNMENTS.contains(&o) => (1, o),
                L::Op("?") => (2, "?"),
                L::Op("??") => (COALESCING, "??"),
                L::Op("||") => (4, "||"),
                L::Op("&&") => (5, "&&"),
                L::Op("|") => (6, "|"),
                L::Op("^") => (7, "^"),
                L::Op("&") => (8, "&"),
                L::Op(o @ ("==" | "!=")) => (9, o),
                L::Op(o @ ("<" | "<=")) => (REL, o),
                L::Op("<<") => (SHIFT, "<<"),
                L::Op(o @ ("+" | "-")) => (12, o),
                L::Op(o @ ("*" | "/" | "%")) => (13, o),
                L::Op("..") => (RANGE, ".."),
                L::Name => match self.word() {
                    "is" => (REL, "is"),
                    "as" => (REL, "as"),
                    "with" if self.is(self.i + 1, "{") => (WITH, "with"),
                    "switch" if self.is(self.i + 1, "{") => (SWITCH, "switch"),
                    _ => break,
                },
                _ => break,
            };
            if prec < min {
                break;
            }
            match op {
                "?" => {
                    let q = self.punct();
                    let consequence = self.expression().with("consequence");
                    let colon = self.expect(":");
                    let alternative = if self.at("#if") {
                        // A statement's tail in each branch (`return`'s).
                        self.tail_taken = true;
                        self.preproc_if_once(&mut |p, b| {
                            b.push(p.expression());
                            b.push(p.expect(";"));
                        })
                    } else {
                        self.expression()
                    }
                    .with("alternative");
                    left = node(
                        "conditional_expression",
                        vec![left.with("condition"), q, consequence, colon, alternative],
                    );
                }
                "??" => {
                    let o = self.punct().with("operator");
                    let right = self.expr_bp(COALESCING).with("right");
                    left = node("binary_expression", vec![left.with("left"), o, right]);
                }
                "is" => left = self.is_expression(left, min <= 2),
                "as" => {
                    let o = self.punct().with("operator");
                    let right = self.ty_in(TypeCtx::Expression).with("right");
                    left = node("as_expression", vec![left.with("left"), o, right]);
                }
                "with" => {
                    let mut items = vec![left, self.punct(), self.punct()];
                    self.comma_list(&mut items, "}", true, |p| {
                        let w = vec![p.identifier(), p.expect("="), p.expression()];
                        node("with_initializer", w)
                    });
                    items.push(self.expect("}"));
                    left = node("with_expression", items);
                }
                "switch" => {
                    let mut items = vec![left, self.punct(), self.punct()];
                    self.comma_list(&mut items, "}", true, |p| {
                        // `(a, b) => …` is a pattern and the arm's arrow.
                        let saved = std::mem::replace(&mut p.no_lambda, true);
                        let pattern = p.pattern_in(PatternCtx::Case);
                        p.no_lambda = saved;
                        let mut a = vec![pattern];
                        if p.at("when") {
                            a.push(p.when_clause());
                        }
                        a.push(p.expect("=>"));
                        a.push(p.expression());
                        node("switch_expression_arm", a)
                    });
                    items.push(self.expect("}"));
                    left = node("switch_expression", items);
                }
                ".." => {
                    let dots = self.punct();
                    let mut items = vec![left, dots];
                    if self.expression_start(self.i) && !self.at("..") {
                        items.push(self.expr_bp(UNARY));
                    }
                    left = node("range_expression", items);
                }
                _ if prec == 1 => {
                    let o = self.punct().with("operator");
                    let right = self.expression().with("right");
                    left = node("assignment_expression", vec![left.with("left"), o, right]);
                }
                _ => {
                    let o = self.punct().with("operator");
                    let right = self.expr_bp(prec + 1).with("right");
                    left = node("binary_expression", vec![left.with("left"), o, right]);
                }
            }
        }
        left
    }

    /// `x is …`; `first` when `x` starts the expression (no operand of a
    /// binary operator before it).
    fn is_expression(&mut self, left: Item, first: bool) -> Item {
        // `x is int` / `x is int[]` is a type; `x is Foo` a constant
        // pattern, `x is int i` a declaration, and so on.
        let start = self.i + 1;
        let type_only = !self.is(start, "(")
            && !self.is(start, "var")
            && self
                .scan_type_in(start, TypeCtx::Expression)
                .is_some_and(|t| {
                    self.scan_name(start) != Some(t)
                        && !self.is_ident(t)
                        && !matches!(self.kind_at(t), L::Op("(" | "{" | "["))
                        && !self.is(t, "and")
                        && !self.is(t, "or")
                });
        let kw = self.punct();
        if type_only {
            let right = self.ty_in(TypeCtx::Expression).with("right");
            return node(
                "is_expression",
                vec![left.with("left"), kw.with("operator"), right],
            );
        }
        // tree-sitter's parse may take the rest of a chain of `&&` / `||`
        // into the first `is`'s constant (`x is null || y` is
        // `x is (null || y)`); the chain's first `is` decides for all.
        let relational = {
            let k = if self.at("not") { self.i + 1 } else { self.i };
            matches!(self.kind_at(k), L::Op("<" | "<=" | ">"))
        };
        let swallow = if self.i < self.chain_end {
            false
        } else if relational {
            true
        } else {
            let (end, tests, ends_in_test, conditional) = self.is_chain(self.i);
            self.chain_end = end;
            !conditional && (tests == 1 || (ends_in_test && first))
        };
        if swallow {
            let pattern = self.pattern_in(PatternCtx::Is).with("pattern");
            return node(
                "is_pattern_expression",
                vec![left.with("expression"), kw, pattern],
            );
        }
        // Not taken: a bare name before `&&` / `||` is a type, other
        // constants are what binds tightest.
        if self.is_ident(self.i)
            && !matches!(self.word(), "not" | "var" | "_")
            && matches!(self.kind_at(self.i + 1), L::Op("&&" | "||"))
        {
            let right = self.identifier().with("right");
            return node(
                "is_expression",
                vec![left.with("left"), kw.with("operator"), right],
            );
        }
        let pattern = self.pattern_in(PatternCtx::Narrow).with("pattern");
        let e = node(
            "is_pattern_expression",
            vec![left.with("expression"), kw, pattern],
        );
        self.postfix(e)
    }

    /// Whether the pattern at `n`, after an `is`, may be a constant (so
    /// tree-sitter's parse forks there): not a declaration (`T x`), a
    /// `var`, a recursive or list pattern, or a type.
    fn is_forks(&self, n: usize) -> bool {
        let n = if self.is(n, "not") { n + 1 } else { n };
        match self.kind_at(n) {
            L::Op("(" | "{" | "[") => false,
            L::Name if self.is(n, "var") || PREDEFINED.contains(&self.word_at(n)) => false,
            L::Name if self.is_ident(n) => match self.scan_type_in(n, TypeCtx::Expression) {
                Some(t) => {
                    !(self.is_ident(t) && !matches!(self.word_at(t), "and" | "or" | "when"))
                        && !self.is(t, "(")
                        && !self.is(t, "{")
                }
                None => true,
            },
            _ => true,
        }
    }

    /// The chain an `is` at `n` heads: the token it ends at, how many of
    /// its `is` tests fork tree-sitter's parse, whether it ends in one, and
    /// whether a `? :` follows. tree-sitter takes the chain into the first
    /// `is` when it holds one such test, or (heading the expression) ends
    /// in one, and no `? :` follows.
    fn is_chain(&self, n: usize) -> (usize, usize, bool, bool) {
        let mut depth = 0usize;
        let mut k = n;
        let mut tests = 1;
        let mut after_last = false;
        let mut conditional = false;
        loop {
            match self.kind_at(k) {
                L::Op("(" | "[" | "{") | L::InterpOpen => depth += 1,
                L::Op(")" | "]" | "}") | L::InterpClose => {
                    if depth == 0 {
                        break;
                    }
                    depth -= 1;
                }
                L::Eof | L::Directive(_) => break,
                _ if depth > 0 => {}
                L::Op(";" | "," | "=>" | ":") => break,
                L::Op("?") => {
                    let access = (self.is(k + 1, ".") && self.is_ident(k + 2))
                        || (self.is(k + 1, "[") && self.adjacent(k));
                    if !access {
                        conditional = true;
                        break;
                    }
                }
                L::Op(o) if ASSIGNMENTS.contains(&o) => break,
                L::Op(
                    "&&" | "||" | "|" | "&" | "^" | "==" | "!=" | "??" | "<" | ">" | "<=" | "+"
                    | "-" | "*" | "/" | "%" | "<<",
                ) => {
                    let binary = !matches!(self.kind_at(k - 1), L::Op(_))
                        && !self.is(k - 1, "is")
                        && !self.is(k - 1, "not");
                    if binary {
                        after_last = true;
                    }
                }
                L::Name if self.is(k, "is") && self.is_forks(k + 1) => {
                    tests += 1;
                    after_last = false;
                }
                _ => {}
            }
            k += 1;
        }
        (k, tests, !after_last, conditional)
    }

    fn unary(&mut self) -> Item {
        match self.peek() {
            L::Op(o @ ("+" | "-" | "!" | "~" | "++" | "--" | "^" | "&" | "*")) => {
                let _ = o;
                let op = self.punct();
                let e = self.unary();
                return node("prefix_unary_expression", vec![op, e]);
            }
            L::Op("..") => {
                let dots = self.punct();
                let mut items = vec![dots];
                if self.expression_start(self.i) && !self.at("..") {
                    items.push(self.expr_bp(UNARY));
                }
                return node("range_expression", items);
            }
            L::Op("(") => {
                if let Some(cast) = self.cast() {
                    return cast;
                }
            }
            L::Name if self.at("await") && self.await_operand_at(self.i + 1) => {
                let kw = self.punct();
                let e = self.unary();
                return node("await_expression", vec![kw, e]);
            }
            _ => {}
        }
        let p = self.primary();
        self.postfix(p)
    }

    pub(super) fn await_operand_at(&self, n: usize) -> bool {
        self.expression_start(n)
            && !matches!(
                self.kind_at(n),
                L::Op("[" | "{" | "+" | "-" | "*" | "&" | "^" | "!")
            )
            || self.is(n, "(") && self.is_ident(n - 1)
    }

    /// `(T)value`, if this is a cast.
    fn cast(&mut self) -> Option<Item> {
        let t = self.scan_type_in(self.i + 1, TypeCtx::Expression)?;
        if !self.is(t, ")") || !self.cast_value_start(t + 1) {
            return None;
        }
        // `(x) => …` is a lambda, not a cast.
        if self.lambda_at(self.i) {
            return None;
        }
        let misread = self.cast_misread_at(t + 1);
        let open = self.punct();
        let ty = self.ty_in(TypeCtx::Expression).with("type");
        let close = self.expect(")");
        if misread {
            // `(T)a.F<A>(x)`: tree-sitter's parse prefers
            // `((T)a.F < A) > (x)`, the smaller tree of two with a cast.
            let value = if self.at("new") {
                let new = self.punct();
                let ty = self.identifier().with("type");
                node("object_creation_expression", vec![new, ty])
            } else {
                let mut e = self.identifier();
                while self.at(".") {
                    let dot = self.punct();
                    let name = self.identifier().with("name");
                    e = node(
                        "member_access_expression",
                        vec![e.with("expression"), dot, name],
                    );
                }
                e
            };
            let cast = node(
                "cast_expression",
                vec![open, ty, close, value.with("value")],
            );
            let lt = self.punct().with("operator");
            let arg = if self.at("(") {
                self.paren_expression()
            } else {
                self.name_expression()
            };
            let left = node(
                "binary_expression",
                vec![cast.with("left"), lt, arg.with("right")],
            );
            let gt = self.punct().with("operator");
            let right = self.expr_bp(REL + 1).with("right");
            return Some(node(
                "binary_expression",
                vec![left.with("left"), gt, right],
            ));
        }
        let value = self.unary().with("value");
        Some(node("cast_expression", vec![open, ty, close, value]))
    }

    /// At a cast's value: `a.b.F<A>(x)` or `new X<A>(x)`, with one type
    /// argument that reads as an expression and one parenthesized
    /// expression after it.
    fn cast_misread_at(&self, v: usize) -> bool {
        let mut k = v;
        if self.is(k, "new") {
            k += 1;
            if !self.is_ident(k) {
                return false;
            }
            k += 1;
        } else {
            if !self.is_ident(k) {
                return false;
            }
            k += 1;
            while self.is(k, ".") && self.is_ident(k + 1) {
                k += 2;
            }
        }
        if !self.is(k, "<") {
            return false;
        }
        let arg = k + 1;
        let close = if self.is(arg, "(") {
            self.scan_balanced(arg)
        } else {
            self.scan_name(arg)
        };
        let Some(close) = close else { return false };
        if !self.is(close, ">") || !self.is(close + 1, "(") || self.is(close + 2, ")") {
            return false;
        }
        !self.paren_has_comma(close + 1)
    }

    fn postfix(&mut self, mut e: Item) -> Item {
        loop {
            match self.peek() {
                L::Op("." | "->") if self.is_ident(self.i + 1) => {
                    let dot = self.punct();
                    let name = self.simple_name_expr().with("name");
                    e = node(
                        "member_access_expression",
                        vec![e.with("expression"), dot, name],
                    );
                }
                L::Op("::") if e.kind() == "identifier" && self.is_ident(self.i + 1) => {
                    let colons = self.punct();
                    let name = self.simple_name_expr().with("name");
                    e = node("alias_qualified_name", vec![e.with("alias"), colons, name]);
                }
                L::Op("(") => {
                    let args = self.argument_list().with("arguments");
                    e = node("invocation_expression", vec![e.with("function"), args]);
                }
                L::Op("[") => {
                    let args = self.bracketed_argument_list().with("subscript");
                    e = node(
                        "element_access_expression",
                        vec![e.with("expression"), args],
                    );
                }
                L::Op(o @ ("++" | "--" | "!")) => {
                    let _ = o;
                    let op = self.punct();
                    e = node("postfix_unary_expression", vec![e, op]);
                }
                _ => return e,
            }
        }
    }

    /// An identifier, or a generic name where a type-argument list and
    /// what follows it say so.
    pub(super) fn simple_name_expr(&mut self) -> Item {
        let generic = self.is(self.i + 1, "<")
            && self
                .scan_type_arguments(self.i + 1)
                .is_some_and(|k| self.generic_follows(k));
        let id = self.identifier();
        if generic {
            let args = self.type_argument_list();
            return node("generic_name", vec![id, args]);
        }
        id
    }

    /// Whether token `k`, after a `>`, makes what precedes it type
    /// arguments (C#'s disambiguation, and what tree-sitter's parse does).
    pub(super) fn generic_follows(&self, k: usize) -> bool {
        match self.kind_at(k) {
            L::Op(o) => {
                matches!(
                    o,
                    "(" | ")"
                        | "]"
                        | "}"
                        | ":"
                        | ";"
                        | ","
                        | "."
                        | "?"
                        | "=="
                        | "!="
                        | "|"
                        | "^"
                        | "&&"
                        | "||"
                        | "&"
                        | "["
                        | "=>"
                        | "?."
                ) || (o == ">" && self.generic_follows_after_close(k))
            }
            L::Eof | L::InterpClose => true,
            L::Name => matches!(self.word_at(k), "is" | "as" | "and" | "or" | "when"),
            _ => false,
        }
    }

    fn generic_follows_after_close(&self, _k: usize) -> bool {
        true
    }

    fn primary(&mut self) -> Item {
        match self.peek() {
            L::Int => self.leaf("integer_literal"),
            L::Real => self.leaf("real_literal"),
            L::Quote => {
                if self.src[self.tk(self.i).start..].starts_with('\'') {
                    self.char_literal()
                } else {
                    self.string_literal()
                }
            }
            L::Verbatim => self.leaf("verbatim_string_literal"),
            L::RawStart => self.raw_string(),
            L::InterpStart => self.interpolated_string(),
            L::Op("(") => self.paren_expression(),
            L::Op("[") if self.lambda_at(self.i) => self.lambda(),
            L::Op("[") => self.bracket_expression(),
            L::Op("{") => self.initializer_expression(),
            L::Name => self.word_expression(),
            _ => self.missing("identifier", true),
        }
    }

    fn word_expression(&mut self) -> Item {
        if self.lambda_at(self.i) {
            return self.lambda();
        }
        let w = self.word();
        match w {
            "true" | "false" => {
                let t = self.punct();
                return node("boolean_literal", vec![t]);
            }
            "null" => return self.leaf("null_literal"),
            "this" | "base" => return self.punct(),
            "new" => return self.new_expression(),
            "typeof" | "sizeof" if self.is(self.i + 1, "(") => {
                let kind = if w == "typeof" {
                    "typeof_expression"
                } else {
                    "sizeof_expression"
                };
                let items = vec![
                    self.punct(),
                    self.punct(),
                    self.ty().with("type"),
                    self.expect(")"),
                ];
                return node(kind, items);
            }
            "default" => {
                let mut items = vec![self.punct()];
                if self.at("(") && self.scan_type(self.i + 1).is_some_and(|k| self.is(k, ")")) {
                    items.push(self.punct());
                    items.push(self.ty().with("type"));
                    items.push(self.expect(")"));
                }
                return node("default_expression", items);
            }
            "checked" | "unchecked" if self.is(self.i + 1, "(") => {
                let items = vec![
                    self.punct(),
                    self.punct(),
                    self.expression(),
                    self.expect(")"),
                ];
                return node("checked_expression", items);
            }
            "__makeref" | "__reftype" if self.is(self.i + 1, "(") => {
                let kind = if w == "__makeref" {
                    "makeref_expression"
                } else {
                    "reftype_expression"
                };
                let items = vec![
                    self.punct(),
                    self.punct(),
                    self.expression(),
                    self.expect(")"),
                ];
                return node(kind, items);
            }
            "__refvalue" if self.is(self.i + 1, "(") => {
                let items = vec![
                    self.punct(),
                    self.punct(),
                    self.expression().with("value"),
                    self.expect(","),
                    self.ty().with("type"),
                    self.expect(")"),
                ];
                return node("refvalue_expression", items);
            }
            "stackalloc" => return self.stackalloc(),
            "throw" => {
                let items = vec![self.punct(), self.expression()];
                return node("throw_expression", items);
            }
            "ref" => {
                let items = vec![self.punct(), self.expression()];
                return node("ref_expression", items);
            }
            "delegate" if !self.is(self.i + 1, "*") => return self.anonymous_method(Vec::new()),
            "async" | "static" if self.anonymous_method_at(self.i) => {
                let mut mods = Vec::new();
                while self.at("async") || self.at("static") {
                    mods.push(self.leaf("modifier"));
                }
                return self.anonymous_method(mods);
            }
            "from" if self.query_at() => return self.query_expression(),
            _ => {}
        }
        if PREDEFINED.contains(&w) {
            return self.leaf("predefined_type");
        }
        if self.is_ident(self.i) {
            return self.simple_name_expr();
        }
        self.error_token()
    }

    fn anonymous_method_at(&self, mut n: usize) -> bool {
        while self.is(n, "async") || self.is(n, "static") {
            n += 1;
        }
        self.is(n, "delegate") && !self.is(n + 1, "*")
    }

    fn anonymous_method(&mut self, mut items: Vec<Item>) -> Item {
        items.push(self.punct());
        if self.at("(") {
            items.push(self.parameter_list().with("parameters"));
        }
        items.push(self.block());
        node("anonymous_method_expression", items)
    }

    // ---- lambdas ---------------------------------------------------------

    /// Whether a lambda starts at `n`: modifiers, an optional return type,
    /// then `x =>` or `(…) =>`.
    pub(super) fn lambda_at(&self, n: usize) -> bool {
        if self.no_lambda {
            return false;
        }
        let mut n = self.scan_attributes(n);
        while (self.is(n, "async") || self.is(n, "static"))
            && (self.kind_at(n + 1) == L::Name || self.is(n + 1, "(") || self.is(n + 1, "["))
        {
            n += 1;
        }
        if self.is_ident(n) && self.is(n + 1, "=>") {
            return true;
        }
        if self.is(n, "(") {
            return self.scan_balanced(n).is_some_and(|k| self.is(k, "=>"));
        }
        // An explicit return type: `int (x) => x`; `F(x) =>` (touching)
        // is a call, as in a switch arm's `nameof(X) => …`.
        // After `T?` only a typed parameter makes it a lambda's return type
        // (`long? (E e) => …`); `c ? () => …` is a conditional.
        let starts = n == 0 || matches!(self.kind_at(n - 1), L::Op("(" | "," | "=" | "=>" | "["));
        self.scan_type(n).is_some_and(|t| {
            self.is(t, "(")
                && (!self.is(t - 1, "?")
                    || starts
                    || self.scan_type(t + 1).is_some_and(|u| self.is_ident(u)))
                && (!self.adjacent(t - 1) || self.scan_name(n) != Some(t))
                && self.scan_balanced(t).is_some_and(|k| self.is(k, "=>"))
        })
    }

    fn lambda(&mut self) -> Item {
        let mut items = Vec::new();
        self.attribute_lists(&mut items);
        while (self.at("async") || self.at("static"))
            && (self.kind_at(self.i + 1) == L::Name
                || self.is(self.i + 1, "(")
                || self.is(self.i + 1, "["))
        {
            items.push(self.leaf("modifier"));
        }
        if self.is_ident(self.i) && self.is(self.i + 1, "=>") {
            let p = self.identifier().renamed("implicit_parameter");
            items.push(p.with("parameters"));
        } else {
            if !self.at("(") {
                items.push(self.ty().with("type"));
            }
            items.push(self.parameter_list().with("parameters"));
        }
        items.push(self.expect("=>"));
        let body = if self.at("{") {
            self.block()
        } else {
            self.expression()
        };
        items.push(body.with("body"));
        node("lambda_expression", items)
    }

    // ---- parentheses and brackets -----------------------------------------

    fn paren_expression(&mut self) -> Item {
        if self.lambda_at(self.i) {
            return self.lambda();
        }
        if self.paren_has_comma(self.i) {
            let mut items = vec![self.punct()];
            self.comma_list(&mut items, ")", false, Self::argument);
            items.push(self.expect(")"));
            return node("tuple_expression", items);
        }
        let open = self.punct();
        let e = self.nested(Self::expression);
        if self.at(",") {
            // A tuple after all (a comma the scan took for type arguments'):
            // the first element is an argument.
            let mut items = vec![open, node("argument", vec![e])];
            items.push(self.punct());
            self.comma_list(&mut items, ")", false, Self::argument);
            items.push(self.expect(")"));
            return node("tuple_expression", items);
        }
        let close = self.expect(")");
        node("parenthesized_expression", vec![open, e, close])
    }

    /// `[…]`: a collection, or (before `=`) an element binding.
    fn bracket_expression(&mut self) -> Item {
        if self
            .scan_balanced(self.i)
            .is_some_and(|k| self.is(k, "=") && !self.is(k + 1, "="))
        {
            return self
                .bracketed_argument_list()
                .renamed("element_binding_expression");
        }
        let mut items = vec![self.punct()];
        self.nested(|p| p.collection_items(&mut items));
        items.push(self.expect("]"));
        node("collection_expression", items)
    }

    /// Elements and commas, and conditionals holding more of them.
    fn collection_items(&mut self, items: &mut Vec<Item>) {
        loop {
            if self.at("]") || self.eof() || self.at_directive_end() {
                break;
            }
            if self.at("#if") {
                items.push(self.preproc_if_once(&mut |p, b| p.collection_items(b)));
                continue;
            }
            let before = self.i;
            let e = if self.at("..") {
                let dots = self.punct();
                let e = self.expression();
                node("spread_element", vec![dots, e])
            } else {
                node("expression_element", vec![self.expression()])
            };
            items.push(node("collection_element", vec![e]));
            if self.at(",") {
                items.push(self.punct());
                continue;
            }
            if self.i == before {
                items.push(self.error_token());
                continue;
            }
            break;
        }
    }

    pub(super) fn initializer_expression(&mut self) -> Item {
        self.nested(Self::initializer_inner)
    }

    fn initializer_inner(&mut self) -> Item {
        let mut items = vec![self.expect("{")];
        self.initializer_items(&mut items);
        items.push(self.expect("}"));
        node("initializer_expression", items)
    }

    /// Expressions and commas, and conditionals holding more of them.
    fn initializer_items(&mut self, items: &mut Vec<Item>) {
        loop {
            if self.at("}") || self.eof() || self.at_directive_end() {
                break;
            }
            if self.at("#if") {
                items.push(self.preproc_if_once(&mut |p, b| p.initializer_items(b)));
                continue;
            }
            let before = self.i;
            items.push(self.expression());
            if self.at(",") {
                items.push(self.punct());
                continue;
            }
            if self.i == before {
                items.push(self.error_token());
                continue;
            }
            break;
        }
    }

    pub(super) fn argument_list(&mut self) -> Item {
        let mut items = vec![self.expect("(")];
        self.comma_list(&mut items, ")", false, Self::argument);
        items.push(self.expect(")"));
        node("argument_list", items)
    }

    pub(super) fn bracketed_argument_list(&mut self) -> Item {
        let mut items = vec![self.expect("[")];
        self.comma_list(&mut items, "]", true, Self::argument);
        items.push(self.expect("]"));
        node("bracketed_argument_list", items)
    }

    fn argument(&mut self) -> Item {
        self.nested(Self::argument_inner)
    }

    fn argument_inner(&mut self) -> Item {
        let mut items = Vec::new();
        if self.is_ident(self.i) && self.is(self.i + 1, ":") {
            items.push(self.identifier().with("name"));
            items.push(self.punct());
        }
        if self.at("ref") || self.at("out") || self.at("in") {
            items.push(self.punct());
        }
        // `T x`: a declaration, whose name may even be a keyword to
        // tree-sitter (`not null` in a tuple pattern).
        let decl = self.scan_type(self.i).is_some_and(|t| {
            self.kind_at(t) == L::Name
                && (self.is_ident(t) || matches!(self.word_at(t), "null" | "true" | "false"))
                && matches!(self.kind_at(t + 1), L::Op("," | ")" | "]"))
        });
        if decl {
            let ty = self.ty().with("type");
            let name = if self.is_ident(self.i) {
                self.identifier()
            } else {
                self.leaf("identifier")
            };
            items.push(node("declaration_expression", vec![ty, name.with("name")]));
        } else {
            items.push(self.expression());
        }
        node("argument", items)
    }

    // ---- creation --------------------------------------------------------

    fn new_expression(&mut self) -> Item {
        if let Some(e) = self.new_misread() {
            return e;
        }
        let new = self.punct();
        let tuple_array = self.at("(")
            && self
                .scan_type_in(self.i, TypeCtx::New)
                .is_some_and(|t| self.is(t - 1, "]"));
        if self.at("(") && !tuple_array {
            let mut items = vec![new, self.argument_list()];
            if self.at("{") {
                items.push(self.initializer_expression());
            }
            return node("implicit_object_creation_expression", items);
        }
        if self.at("[") {
            let mut items = vec![new, self.punct()];
            while self.at(",") {
                items.push(self.punct());
            }
            items.push(self.expect("]"));
            items.push(self.initializer_expression());
            return node("implicit_array_creation_expression", items);
        }
        if self.at("{") {
            let mut items = vec![new, self.punct()];
            loop {
                if self.at("}") || self.eof() {
                    break;
                }
                let before = self.i;
                if self.is_ident(self.i) && self.is(self.i + 1, "=") {
                    items.push(self.identifier());
                    items.push(self.punct());
                }
                items.push(self.expression());
                if self.at(",") {
                    items.push(self.punct());
                    continue;
                }
                if self.i == before {
                    items.push(self.error_token());
                    continue;
                }
                break;
            }
            items.push(self.expect("}"));
            return node("anonymous_object_creation_expression", items);
        }
        let ty = self.ty_in(TypeCtx::New);
        if ty.kind() == "array_type" {
            let mut items = vec![new, ty.with("type")];
            if self.at("{") {
                items.push(self.initializer_expression());
            }
            return node("array_creation_expression", items);
        }
        let mut items = vec![new, ty.with("type")];
        if self.at("(") {
            items.push(self.argument_list().with("arguments"));
        }
        if self.at("{") {
            items.push(self.initializer_expression().with("initializer"));
        }
        node("object_creation_expression", items)
    }

    /// `new X<Y>(Z) { … }` with one type argument and one name in the
    /// parentheses: tree-sitter's parse prefers `(new X < Y) > (Z){ … }`,
    /// a cast's dynamic precedence winning, and so does colab's.
    fn new_misread(&mut self) -> Option<Item> {
        let n = self.i + 1;
        if !self.is_ident(n) || !self.is(n + 1, "<") {
            return None;
        }
        // One type argument that reads as an expression: a name, or a
        // tuple of declarations.
        let arg = n + 2;
        let close = if self.is(arg, "(") {
            self.scan_balanced(arg)?
        } else {
            self.scan_name(arg)?
        };
        if !self.is(close, ">") || !self.is(close + 1, "(") {
            return None;
        }
        let paren = close + 1;
        let inner_end = self.scan_name(paren + 1)?;
        if !self.is(inner_end, ")") || !self.is(inner_end + 1, "{") {
            return None;
        }
        // `new X<…>` is `new X`, then `<` and `>` as operators.
        let new = self.punct();
        let ty = self.identifier().with("type");
        let creation = node("object_creation_expression", vec![new, ty]);
        let lt = self.punct().with("operator");
        let arg = if self.at("(") {
            self.paren_expression()
        } else {
            self.name_expression()
        }
        .with("right");
        let left = node("binary_expression", vec![creation.with("left"), lt, arg]);
        let gt = self.punct().with("operator");
        let open = self.punct();
        let cast_ty = self.name().with("type");
        let close = self.expect(")");
        let value = self.initializer_expression().with("value");
        let cast = node("cast_expression", vec![open, cast_ty, close, value]).with("right");
        Some(node("binary_expression", vec![left.with("left"), gt, cast]))
    }

    /// `A.B.C` as an expression: identifiers and member accesses.
    fn name_expression(&mut self) -> Item {
        let mut e = self.simple_name_expr();
        while self.at(".") && self.is_ident(self.i + 1) {
            let dot = self.punct();
            let name = self.simple_name_expr().with("name");
            e = node(
                "member_access_expression",
                vec![e.with("expression"), dot, name],
            );
        }
        e
    }

    fn stackalloc(&mut self) -> Item {
        let kw = self.punct();
        if self.at("[") {
            let items = vec![
                kw,
                self.punct(),
                self.expect("]"),
                self.initializer_expression(),
            ];
            return node("implicit_stackalloc_expression", items);
        }
        let mut items = vec![kw, self.ty_in(TypeCtx::New).with("type")];
        if self.at("{") {
            items.push(self.initializer_expression());
        }
        node("stackalloc_expression", items)
    }

    // ---- queries ---------------------------------------------------------

    fn query_at(&self) -> bool {
        let n = self.i + 1;
        (self.is_ident(n) && self.is(n + 1, "in"))
            || self
                .scan_type(n)
                .is_some_and(|t| self.is_ident(t) && self.is(t + 1, "in"))
    }

    fn query_expression(&mut self) -> Item {
        self.queries += 1;
        let q = self.query_inner();
        self.queries -= 1;
        q
    }

    fn query_inner(&mut self) -> Item {
        let mut items = vec![self.query_from()];
        loop {
            loop {
                match self.word() {
                    "from" if self.query_at() => items.push(self.query_from()),
                    "join" => items.push(self.join_clause()),
                    "let" if self.is_ident(self.i + 1) && self.is(self.i + 2, "=") => {
                        let l = vec![
                            self.punct(),
                            self.identifier(),
                            self.punct(),
                            self.expression(),
                        ];
                        items.push(node("let_clause", l));
                    }
                    "orderby" => {
                        let mut o = vec![self.punct()];
                        loop {
                            o.push(self.expression());
                            if self.at("ascending") || self.at("descending") {
                                o.push(self.punct());
                            }
                            if self.at(",") {
                                o.push(self.punct());
                                continue;
                            }
                            break;
                        }
                        items.push(node("order_by_clause", o));
                    }
                    "where" => {
                        let w = vec![self.punct(), self.expression()];
                        items.push(node("where_clause", w));
                    }
                    _ => break,
                }
            }
            match self.word() {
                "select" => {
                    let s = vec![self.punct(), self.expression()];
                    items.push(node("select_clause", s));
                }
                "group" => {
                    let g = vec![
                        self.punct(),
                        self.expression(),
                        self.expect("by"),
                        self.expression(),
                    ];
                    items.push(node("group_clause", g));
                }
                _ => {
                    items.push(self.missing("select_clause", true));
                    break;
                }
            }
            if self.at("into") && self.is_ident(self.i + 1) {
                items.push(self.punct());
                items.push(self.identifier());
                continue;
            }
            break;
        }
        node("query_expression", items)
    }

    fn query_from(&mut self) -> Item {
        let mut items = vec![self.punct()];
        if !(self.is_ident(self.i) && self.is(self.i + 1, "in")) {
            items.push(self.ty().with("type"));
        }
        items.push(self.identifier().with("name"));
        items.push(self.expect("in"));
        items.push(self.expression());
        node("from_clause", items)
    }

    fn join_clause(&mut self) -> Item {
        let mut items = vec![self.punct()];
        if !(self.is_ident(self.i) && self.is(self.i + 1, "in")) {
            items.push(self.ty().with("type"));
        }
        items.push(self.identifier());
        items.push(self.expect("in"));
        items.push(self.expression());
        items.push(self.expect("on"));
        items.push(self.expression());
        items.push(self.expect("equals"));
        items.push(self.expression());
        if self.at("into") && self.is_ident(self.i + 1) {
            let j = vec![self.punct(), self.identifier()];
            items.push(node("join_into_clause", j));
        }
        node("join_clause", items)
    }

    // ---- patterns --------------------------------------------------------

    pub(crate) fn pattern_in(&mut self, ctx: PatternCtx) -> Item {
        let mut left = self.pattern_and(ctx);
        while self.at("or") && self.pattern_start(self.i + 1) {
            let op = self.punct().with("operator");
            let right = self.pattern_and(ctx).with("right");
            left = node("or_pattern", vec![left.with("left"), op, right]);
        }
        left
    }

    fn pattern_and(&mut self, ctx: PatternCtx) -> Item {
        let mut left = self.pattern_not(ctx);
        while self.at("and") && self.pattern_start(self.i + 1) {
            let op = self.punct().with("operator");
            let right = self.pattern_not(ctx).with("right");
            left = node("and_pattern", vec![left.with("left"), op, right]);
        }
        left
    }

    fn pattern_start(&self, n: usize) -> bool {
        self.expression_start(n) || matches!(self.kind_at(n), L::Op("<" | "<=" | ">"))
    }

    fn pattern_not(&mut self, ctx: PatternCtx) -> Item {
        // `not` takes the whole pattern after it: `not A and B` is
        // `not (A and B)` to tree-sitter.
        if self.at("not") && self.pattern_start(self.i + 1) {
            let kw = self.punct();
            let p = self.pattern_in(ctx);
            return node("negated_pattern", vec![kw, p]);
        }
        self.primary_pattern(ctx)
    }

    /// A relational pattern's expression: tree-sitter's takes a whole
    /// conditional (`x is < 0 ? a : b` is `x is < (0 ? a : b)`).
    fn relational_operand(&mut self, ctx: PatternCtx) -> Item {
        if ctx == PatternCtx::Narrow {
            return self.unary();
        }
        self.expr_bp(2)
    }

    fn primary_pattern(&mut self, ctx: PatternCtx) -> Item {
        match self.peek() {
            L::Op("<" | "<=") => {
                let op = self.punct();
                let e = self.relational_operand(ctx);
                return node("relational_pattern", vec![op, e]);
            }
            L::Op(">") => {
                let (n, kind) = self.joined_greater();
                let n = if kind == ">=" { n } else { 1 };
                let op = self.joined(n, if n == 2 { ">=" } else { ">" });
                let e = self.relational_operand(ctx);
                return node("relational_pattern", vec![op, e]);
            }
            L::Op("{") => {
                let mut items = vec![self.property_pattern_clause()];
                self.designation(&mut items);
                return node("recursive_pattern", items);
            }
            L::Op("[") => {
                let mut items = vec![self.punct()];
                self.comma_list(&mut items, "]", true, |p| {
                    if p.at("..") {
                        let dots = p.punct();
                        let mut s = vec![dots];
                        if p.pattern_start(p.i) && !p.at(",") && !p.at("]") {
                            s.push(p.pattern_in(PatternCtx::Is));
                        }
                        node("slice_pattern", s)
                    } else {
                        p.pattern_in(PatternCtx::Is)
                    }
                });
                items.push(self.expect("]"));
                self.designation(&mut items);
                return node("list_pattern", items);
            }
            L::Op("(") => return self.paren_pattern(ctx),
            _ => {}
        }
        if self.at("var") {
            if self.is(self.i + 1, "(") {
                let kw = self.punct();
                let d = self.parenthesized_designation();
                return node("var_pattern", vec![kw, d]);
            }
            if self.is_ident(self.i + 1) {
                let t = self.punct();
                let ty = node("implicit_type", vec![t]).with("type");
                let mut items = vec![ty];
                self.designation(&mut items);
                return node("declaration_pattern", items);
            }
        }
        if self.at("_")
            && !self.designator_at(self.i + 1)
            && !self.is(self.i + 1, "(")
            && !self.is(self.i + 1, ".")
        {
            return self.leaf("discard");
        }
        if let Some(t) = self.scan_type_in(self.i, TypeCtx::Expression) {
            let designated = self.designator_at(t);
            if designated {
                let ty = self.ty_in(TypeCtx::Expression).with("type");
                let mut items = vec![ty];
                self.designation(&mut items);
                return node("declaration_pattern", items);
            }
            // `nameof(X)`: a type and a parenthesized designation to
            // tree-sitter, when only names are in the parentheses.
            if self.is(t, "(")
                && self.scan_name(self.i) == Some(t)
                && self.scan_balanced(t).is_some_and(|e| {
                    !self.is(e, "{")
                        && !self.designator_at(e)
                        && (t + 1..e - 1).all(|j| self.is_ident(j) || self.is(j, ","))
                        && e > t + 2
                })
            {
                let ty = self.ty_in(TypeCtx::Expression).with("type");
                let d = self.parenthesized_designation();
                return node("declaration_pattern", vec![ty, d]);
            }
            if self.is(t, "(") || self.is(t, "{") {
                let name_only = self.scan_name(self.i) == Some(t);
                // `Foo(…)` / `Foo { … }`: a recursive pattern.
                if !name_only || self.is(t, "{") || self.positional_after(t) {
                    let ty = self.ty_in(TypeCtx::Expression).with("type");
                    let mut items = vec![ty];
                    if self.at("(") {
                        items.push(self.positional_clause());
                    }
                    if self.at("{") {
                        items.push(self.property_pattern_clause());
                    }
                    self.designation(&mut items);
                    return node("recursive_pattern", items);
                }
            }
            // A type that is not a name (`int`, `int[]`, `string?`) is a
            // type pattern; a name is a constant's expression.
            let name = self.scan_name(self.i) == Some(t);
            if !name && !self.is(t, ".") && !self.is(t, "::") {
                let ty = self.ty_in(TypeCtx::Expression).with("type");
                return node("type_pattern", vec![ty]);
            }
        }
        let e = self.pattern_constant(ctx);
        node("constant_pattern", vec![e])
    }

    /// A constant's (or a relational pattern's) expression. tree-sitter's
    /// takes any binary operator, even after `is` (`x is null || y` is
    /// `x is (null || y)`), unless the `is` chain is not taken; a name
    /// before `=>` is an arm's, not a lambda's.
    fn pattern_constant(&mut self, ctx: PatternCtx) -> Item {
        if ctx == PatternCtx::Narrow {
            // `x is A.B && …`: tree-sitter's constant is `A` (and `.B`
            // applies to the `is`); before anything else the whole name.
            if self.is_ident(self.i) {
                let mut k = self.i + 1;
                while self.is(k, ".") && self.is_ident(k + 1) {
                    k += 2;
                }
                if matches!(self.kind_at(k), L::Op("&&" | "||")) {
                    return self.simple_name_expr();
                }
            }
            return self.unary();
        }
        if self.is_ident(self.i) && self.is(self.i + 1, "=>") {
            return self.simple_name_expr();
        }
        self.expr_bp(COALESCING)
    }

    /// Whether the parentheses at `t` (after a name, or alone) hold a
    /// pattern (a positional pattern) rather than arguments: tree-sitter
    /// reads them as a call or a tuple whenever each element is an
    /// argument (`var s` and even `not null` read as declarations).
    fn positional_after(&self, t: usize) -> bool {
        let Some(end) = self.scan_balanced(t) else {
            return false;
        };
        // A designation or a property clause after them settles it.
        if (self.is_ident(end) && !matches!(self.word_at(end), "and" | "or" | "when"))
            || self.is(end, "{")
        {
            return true;
        }
        let mut k = t + 1;
        let mut depth = 0usize;
        let mut first = true;
        while k < end - 1 {
            match self.kind_at(k) {
                L::Op("(" | "[" | "{") if depth > 0 => depth += 1,
                L::Op(")" | "]" | "}") if depth > 0 => depth -= 1,
                _ if depth > 0 => {}
                L::Op(",") => {
                    first = true;
                    k += 1;
                    continue;
                }
                L::Op("<" | "<=" | ">" | "..") if first => return true,
                L::Op("{") => {
                    // A property pattern (`{ A: p }`, `T { }`), not an
                    // initializer.
                    if self.is_ident(k + 1) && self.is(k + 2, ":") || self.is(k + 1, "}") {
                        return true;
                    }
                    depth += 1;
                }
                L::Op("(" | "[") => depth += 1,
                L::Name if self.is(k, "and") || self.is(k, "or") => {
                    if !first && !matches!(self.kind_at(k + 1), L::Op("," | ")")) {
                        return true;
                    }
                }
                L::Name if first && self.is(k, "not") => {
                    // `not null` after a comma is a declaration to
                    // tree-sitter; first, `not` is the keyword (a pattern
                    // is what the parenthesis can start), and `not (…)` or
                    // `not 1` can be nothing else.
                    if k == t + 1
                        || !(self.kind_at(k + 1) == L::Name
                            && matches!(self.kind_at(k + 2), L::Op("," | ")")))
                    {
                        return true;
                    }
                }
                L::Name if first && self.is(k, "var") && self.is(k + 1, "(") => return true,
                // A first element that declares (`var s`, `T x`): the
                // pattern reading wins there, as `not` does.
                L::Name
                    if k == t + 1
                        && self.scan_type(k).is_some_and(|u| {
                            self.is_ident(u) && matches!(self.kind_at(u + 1), L::Op("," | ")"))
                        }) =>
                {
                    return true;
                }
                _ => {}
            }
            first = false;
            k += 1;
        }
        false
    }

    fn paren_pattern(&mut self, ctx: PatternCtx) -> Item {
        if self.paren_has_comma(self.i) || self.is(self.i + 1, ")") {
            // `(1, 2)` of constants is a tuple; anything else positional.
            if !self.is(self.i + 1, ")") && !self.positional_after(self.i) {
                let e = self.pattern_constant(ctx);
                return node("constant_pattern", vec![e]);
            }
            let mut items = vec![self.positional_clause()];
            if self.at("{") {
                items.push(self.property_pattern_clause());
            }
            self.designation(&mut items);
            return node("recursive_pattern", items);
        }
        // `(int)x` as a constant is a cast.
        if self
            .scan_type_in(self.i + 1, TypeCtx::Expression)
            .is_some_and(|t| self.is(t, ")") && self.cast_value_start(t + 1))
        {
            let e = self.expr_bp(COALESCING);
            return node("constant_pattern", vec![e]);
        }
        let open = self.punct();
        let p = self.pattern_in(ctx);
        let close = self.expect(")");
        node("parenthesized_pattern", vec![open, p, close])
    }

    fn positional_clause(&mut self) -> Item {
        let mut items = vec![self.punct()];
        self.comma_list(&mut items, ")", false, Self::subpattern);
        items.push(self.expect(")"));
        node("positional_pattern_clause", items)
    }

    fn property_pattern_clause(&mut self) -> Item {
        let mut items = vec![self.punct()];
        self.comma_list(&mut items, "}", true, Self::subpattern);
        items.push(self.expect("}"));
        node("property_pattern_clause", items)
    }

    fn subpattern(&mut self) -> Item {
        let mut items = Vec::new();
        if self.is_ident(self.i) && self.is(self.i + 1, ":") {
            items.push(self.identifier());
            items.push(self.punct());
        } else if self.is_ident(self.i) && self.is(self.i + 1, ".") {
            // `A.B: p`, an extended property pattern.
            let mut k = self.i + 1;
            while self.is(k, ".") && self.is_ident(k + 1) {
                k += 2;
            }
            if self.is(k, ":") {
                let mut e = self.identifier();
                while self.at(".") {
                    let dot = self.punct();
                    let name = self.identifier().with("name");
                    e = node(
                        "member_access_expression",
                        vec![e.with("expression"), dot, name],
                    );
                }
                items.push(e);
                items.push(self.punct());
            }
        }
        items.push(self.pattern_in(PatternCtx::Is));
        node("subpattern", items)
    }

    /// Whether token `n` names a pattern's designation: `when`, `and` and
    /// `or` only where nothing follows them (`is { } when)`).
    fn designator_at(&self, n: usize) -> bool {
        let w = self.word_at(n);
        self.is_ident(n)
            && (!matches!(w, "when" | "and" | "or")
                || matches!(self.kind_at(n + 1), L::Op(")" | "," | ";" | "]" | "}")))
            && !(self.queries > 0 && QUERY_WORDS.contains(&w))
    }

    /// A pattern's designation: a name, `_`, or `(a, b)`.
    fn designation(&mut self, items: &mut Vec<Item>) {
        if self.at("_") {
            items.push(self.leaf("discard"));
        } else if self.designator_at(self.i) {
            items.push(self.identifier().with("name"));
        } else if self.at("(") && self.designation_list_at(self.i) {
            items.push(self.parenthesized_designation());
        }
    }

    fn designation_list_at(&self, n: usize) -> bool {
        let Some(end) = self.scan_balanced(n) else {
            return false;
        };
        (n + 1..end - 1)
            .all(|k| self.is(k, ",") || self.is(k, "(") || self.is(k, ")") || self.is_ident(k))
    }

    fn parenthesized_designation(&mut self) -> Item {
        let mut items = vec![self.punct()];
        self.comma_list(&mut items, ")", false, |p| {
            if p.at("(") {
                p.parenthesized_designation()
            } else if p.at("_") {
                p.leaf("discard")
            } else {
                p.identifier().with("name")
            }
        });
        items.push(self.expect(")"));
        node("parenthesized_variable_designation", items)
    }

    // ---- strings ---------------------------------------------------------

    fn string_literal(&mut self) -> Item {
        let mut items = vec![self.quote()];
        loop {
            match self.peek() {
                L::StrContent => items.push(self.leaf("string_literal_content")),
                L::Escape => items.push(self.leaf("escape_sequence")),
                L::Quote => {
                    items.push(self.quote());
                    break;
                }
                _ => {
                    items.push(self.missing("\"", false));
                    break;
                }
            }
        }
        if self.peek() == L::Encoding {
            items.push(self.leaf("string_literal_encoding"));
        }
        node("string_literal", items)
    }

    fn quote(&mut self) -> Item {
        let t = self.advance();
        let kind = if self.src[t.start..].starts_with('\'') {
            "'"
        } else {
            "\""
        };
        Self::tok(t, kind, false)
    }

    fn char_literal(&mut self) -> Item {
        let mut items = vec![self.quote()];
        match self.peek() {
            L::CharContent => items.push(self.leaf("character_literal_content")),
            L::Escape => items.push(self.leaf("escape_sequence")),
            _ => {}
        }
        if self.peek() == L::Quote {
            items.push(self.quote());
        } else {
            items.push(self.missing("'", false));
        }
        node("character_literal", items)
    }

    fn raw_string(&mut self) -> Item {
        let mut items = vec![self.leaf("raw_string_start")];
        if self.peek() == L::RawContent {
            items.push(self.leaf("raw_string_content"));
        }
        if self.peek() == L::RawEnd {
            items.push(self.leaf("raw_string_end"));
        } else {
            items.push(self.missing("raw_string_end", true));
        }
        if self.peek() == L::RawSuffix {
            let t = self.advance();
            items.push(Self::hidden(t.start, t.end));
        }
        node("raw_string_literal", items)
    }

    fn interpolated_string(&mut self) -> Item {
        let mut items = vec![self.leaf("interpolation_start")];
        let interp_quote = |p: &mut Self| {
            let t = p.advance();
            match t.k {
                L::InterpQuote { raw: true } => Self::tok(t, "interpolation_quote", true),
                _ => Self::tok(t, "\"", false),
            }
        };
        if matches!(self.peek(), L::InterpQuote { .. }) {
            items.push(interp_quote(self));
        } else {
            items.push(self.missing("\"", false));
            return node("interpolated_string_expression", items);
        }
        loop {
            match self.peek() {
                L::InterpContent => items.push(self.leaf("string_content")),
                L::Escape => items.push(self.leaf("escape_sequence")),
                L::InterpOpen => items.push(self.interpolation()),
                L::InterpQuote { .. } => {
                    items.push(interp_quote(self));
                    break;
                }
                L::Bad => items.push(self.error_token()),
                _ => {
                    items.push(self.missing("\"", false));
                    break;
                }
            }
        }
        node("interpolated_string_expression", items)
    }

    fn interpolation(&mut self) -> Item {
        let mut items = vec![self.leaf("interpolation_brace")];
        items.push(self.expression());
        if self.at(",") {
            let a = vec![self.punct(), self.expression()];
            items.push(node("interpolation_alignment_clause", a));
        }
        if self.at(":") {
            let mut f = vec![self.punct()];
            if self.peek() == L::Format {
                let t = self.advance();
                f.push(Self::hidden(t.start, t.end));
            }
            items.push(node("interpolation_format_clause", f));
        }
        while !matches!(self.peek(), L::InterpClose | L::Eof | L::InterpQuote { .. }) {
            items.push(self.error_token());
        }
        if self.peek() == L::InterpClose {
            items.push(self.leaf("interpolation_brace"));
        } else {
            items.push(self.missing("interpolation_brace", true));
        }
        node("interpolation", items)
    }
}
