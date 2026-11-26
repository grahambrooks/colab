//! Types and names, and the scans that look ahead over them.

use super::{Item, PREDEFINED, Parser, node};
use crate::lexer::L;

/// Where a type is read: in a declaration `T?` is always nullable; after
/// `as`, `is` or in a cast's parentheses a `?` may be a conditional's.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum TypeCtx {
    Declaration,
    Expression,
    /// After `new`: array ranks may hold sizes.
    New,
}

impl Parser<'_> {
    // ---- scanning ahead --------------------------------------------------

    /// Past the bracket at `n` and its match.
    pub(super) fn scan_balanced(&self, n: usize) -> Option<usize> {
        let mut depth = 0usize;
        let mut k = n;
        loop {
            match self.kind_at(k) {
                L::Op("(" | "[" | "{") | L::InterpOpen => depth += 1,
                L::Op(")" | "]" | "}") | L::InterpClose => {
                    depth = depth.checked_sub(1)?;
                    if depth == 0 {
                        return Some(k + 1);
                    }
                }
                L::Eof => return None,
                _ => {}
            }
            k += 1;
        }
    }

    /// A name at `n` (`A`, `A.B<C>.D`, `global::A`): the token after it.
    pub(super) fn scan_name(&self, n: usize) -> Option<usize> {
        if !self.is_ident(n) {
            return None;
        }
        let mut n = n + 1;
        if self.is(n, "::") {
            if !self.is_ident(n + 1) {
                return None;
            }
            n += 2;
        }
        loop {
            if self.is(n, "<")
                && let Some(k) = self.scan_type_arguments(n)
            {
                n = k;
            }
            if self.is(n, ".") && self.is_ident(n + 1) {
                n += 2;
                continue;
            }
            return Some(n);
        }
    }

    /// Type arguments at `n` (a `<`): the token after their `>`.
    pub(super) fn scan_type_arguments(&self, n: usize) -> Option<usize> {
        let mut n = n + 1;
        while self.is(n, ",") {
            n += 1;
        }
        if self.is(n, ">") {
            return Some(n + 1);
        }
        loop {
            n = self.scan_type_in(n, TypeCtx::Declaration)?;
            if self.is(n, ",") {
                n += 1;
                continue;
            }
            return self.is(n, ">").then_some(n + 1);
        }
    }

    /// A type at `n`: the token after it.
    pub(super) fn scan_type(&self, n: usize) -> Option<usize> {
        self.scan_type_in(n, TypeCtx::Declaration)
    }

    pub(super) fn scan_type_in(&self, n: usize, ctx: TypeCtx) -> Option<usize> {
        let mut n = n;
        if self.is(n, "ref") {
            n += 1;
            if self.is(n, "readonly") {
                n += 1;
            }
            return self.scan_type_in(n, ctx);
        }
        if self.is(n, "scoped")
            && (self.is(n + 1, "ref") || self.scan_name(n + 1).is_some_and(|k| self.is_ident(k)))
        {
            n += 1;
        }
        let w = self.word_at(n);
        let mut k = if PREDEFINED.contains(&w) {
            n + 1
        } else if self.is(n, "(") {
            self.scan_tuple_type(n)?
        } else if self.is(n, "delegate") && self.is(n + 1, "*") {
            let mut k = n + 2;
            while self.kind_at(k) == L::Name {
                k += 1;
            }
            if self.is(k, "[") {
                k = self.scan_balanced(k)?;
            }
            if !self.is(k, "<") {
                return None;
            }
            self.scan_angles(k)?
        } else {
            self.scan_name(n)?
        };
        let mut nullable_ok = true;
        loop {
            if self.is(k, "?") && nullable_ok {
                if ctx == TypeCtx::Expression && !self.nullable_follows(k + 1) {
                    return Some(k);
                }
                k += 1;
                nullable_ok = false;
                continue;
            }
            if self.is(k, "*") {
                k += 1;
                nullable_ok = true;
                continue;
            }
            if self.is(k, "[") {
                let mut j = k + 1;
                while self.is(j, ",") {
                    j += 1;
                }
                if self.is(j, "]") {
                    k = j + 1;
                    nullable_ok = true;
                    continue;
                }
                if ctx == TypeCtx::New {
                    k = self.scan_balanced(k)?;
                    nullable_ok = true;
                    continue;
                }
            }
            return Some(k);
        }
    }

    /// `delegate*<…>`'s angle brackets: past the `>` matching `n`'s `<`.
    fn scan_angles(&self, n: usize) -> Option<usize> {
        let mut depth = 0usize;
        let mut k = n;
        loop {
            match self.kind_at(k) {
                L::Op("<") => depth += 1,
                L::Op(">") => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(k + 1);
                    }
                }
                L::Op(";" | "{" | "}") | L::Eof => return None,
                _ => {}
            }
            k += 1;
        }
    }

    /// `(T a, U b)` at `n`: the token after it (two elements at least).
    fn scan_tuple_type(&self, n: usize) -> Option<usize> {
        let mut k = n + 1;
        let mut count = 0;
        loop {
            k = self.scan_type(k)?;
            if self.is_ident(k) {
                k += 1;
            }
            count += 1;
            if self.is(k, ",") {
                k += 1;
                continue;
            }
            return (self.is(k, ")") && count >= 2).then_some(k + 1);
        }
    }

    /// In an expression, whether the `?` before `n` makes a nullable type
    /// rather than starting a conditional: what follows cannot start an
    /// expression.
    pub(super) fn nullable_follows(&self, n: usize) -> bool {
        match self.kind_at(n) {
            // `int?[]` and `(bool? [])x` are nullable arrays; `c ? [] : …`
            // is a conditional.
            L::Op("[") => {
                if self.adjacent(n - 1) {
                    return true;
                }
                let mut k = n + 1;
                while self.is(k, ",") {
                    k += 1;
                }
                self.is(k, "]") && !self.is(k + 1, ":")
            }
            L::Op(o) => matches!(
                o,
                ")" | ","
                    | ";"
                    | "]"
                    | "}"
                    | "?"
                    | "??"
                    | "=>"
                    | "="
                    | ">"
                    | "=="
                    | "!="
                    | "&&"
                    | "||"
                    | "{"
                    | ":"
            ),
            L::Eof => true,
            L::Name => matches!(self.word_at(n), "when" | "and" | "or" | "is" | "as"),
            _ => false,
        }
    }

    // ---- building --------------------------------------------------------

    /// A type in a declaration.
    pub(super) fn ty(&mut self) -> Item {
        self.ty_in(TypeCtx::Declaration)
    }

    pub(super) fn ty_in(&mut self, ctx: TypeCtx) -> Item {
        if self.at("ref") {
            let mut items = vec![self.punct()];
            if self.at("readonly") {
                items.push(self.punct());
            }
            items.push(self.ty_in(ctx).with("type"));
            return node("ref_type", items);
        }
        if self.at("scoped")
            && (self.is(self.i + 1, "ref")
                || self.scan_name(self.i + 1).is_some_and(|k| self.is_ident(k)))
        {
            let kw = self.punct();
            let t = if self.at("ref") {
                self.ty_in(ctx)
            } else {
                self.name()
            };
            return node("scoped_type", vec![kw, t.with("type")]);
        }
        if self.at("var") && !matches!(self.kind_at(self.i + 1), L::Op("." | "<" | "::")) {
            let t = self.punct();
            return node("implicit_type", vec![t]);
        }
        let w = self.word();
        let mut t = if PREDEFINED.contains(&w) {
            self.leaf("predefined_type")
        } else if self.at("(") && self.scan_tuple_type(self.i).is_some() {
            self.tuple_type()
        } else if self.at("delegate") && self.is(self.i + 1, "*") {
            self.function_pointer_type()
        } else if self.is_ident(self.i) {
            self.name()
        } else {
            return self.missing("identifier", true);
        };
        loop {
            if self.at("?")
                && matches!(
                    t.kind(),
                    "array_type"
                        | "predefined_type"
                        | "tuple_type"
                        | "identifier"
                        | "generic_name"
                        | "qualified_name"
                        | "alias_qualified_name"
                )
            {
                if ctx == TypeCtx::Expression && !self.nullable_follows(self.i + 1) {
                    return t;
                }
                let q = self.punct();
                t = node("nullable_type", vec![t.with("type"), q]);
                continue;
            }
            if self.at("*") && t.kind() != "array_type" {
                if ctx == TypeCtx::Expression
                    && !matches!(self.kind_at(self.i + 1), L::Op(")" | "*" | "," | ">" | "["))
                {
                    return t;
                }
                let s = self.punct();
                t = node("pointer_type", vec![t.with("type"), s]);
                continue;
            }
            if self.at("[") {
                let mut j = self.i + 1;
                while self.is(j, ",") {
                    j += 1;
                }
                let sized = !self.is(j, "]");
                if sized && ctx != TypeCtx::New {
                    return t;
                }
                let mut rank = vec![self.punct()];
                if sized {
                    loop {
                        if self.at(",") {
                            rank.push(self.punct());
                            continue;
                        }
                        if self.at("]") || self.eof() {
                            break;
                        }
                        let before = self.i;
                        rank.push(self.expression());
                        if self.i == before {
                            rank.push(self.error_token());
                        }
                    }
                } else {
                    while self.at(",") {
                        rank.push(self.punct());
                    }
                }
                rank.push(self.expect("]"));
                let rank = node("array_rank_specifier", rank).with("rank");
                t = node("array_type", vec![t.with("type"), rank]);
                continue;
            }
            return t;
        }
    }

    fn tuple_type(&mut self) -> Item {
        let mut items = vec![self.punct()];
        self.comma_list(&mut items, ")", false, |p| {
            let mut e = vec![p.ty().with("type")];
            if p.is_ident(p.i) {
                e.push(p.identifier().with("name"));
            }
            node("tuple_element", e)
        });
        items.push(self.expect(")"));
        node("tuple_type", items)
    }

    fn function_pointer_type(&mut self) -> Item {
        let mut items = vec![self.punct(), self.punct()];
        if self.at("managed") {
            let t = self.punct();
            items.push(node("calling_convention", vec![t]));
        } else if self.at("unmanaged") {
            let mut c = vec![self.punct()];
            if self.at("[") {
                c.push(self.punct());
                self.comma_list(&mut c, "]", false, |p| match p.word() {
                    "Cdecl" | "Stdcall" | "Thiscall" | "Fastcall" => p.punct(),
                    _ => p.identifier(),
                });
                c.push(self.expect("]"));
            }
            items.push(node("calling_convention", c));
        }
        items.push(self.expect("<"));
        // Parameters, then the return type before `>`.
        loop {
            let mut param = Vec::new();
            if self.at("ref") || self.at("out") || self.at("in") {
                param.push(self.punct());
            }
            let t = self.ty();
            if self.at(",") {
                param.push(t.with("type"));
                items.push(node("function_pointer_parameter", param));
                items.push(self.punct());
                continue;
            }
            if param.is_empty() {
                items.push(t.with("returns"));
            } else {
                // `ref T` as the return type.
                let mut r = param;
                r.push(t.with("type"));
                items.push(node("ref_type", r).with("returns"));
            }
            break;
        }
        items.push(self.expect(">"));
        node("function_pointer_type", items)
    }

    /// A name: `A`, `A<B>`, `A.B.C`, `global::A.B`.
    pub(super) fn name(&mut self) -> Item {
        self.name_until(usize::MAX)
    }

    /// A name ending before token `end`.
    pub(super) fn name_until(&mut self, end: usize) -> Item {
        let first = self.simple_name_type();
        let mut n = if self.at("::") && self.i < end {
            let colons = self.punct();
            let name = self.simple_name_type().with("name");
            node(
                "alias_qualified_name",
                vec![first.with("alias"), colons, name],
            )
        } else {
            first
        };
        while self.at(".") && self.is_ident(self.i + 1) && self.i < end {
            let dot = self.punct();
            let name = self.simple_name_type().with("name");
            n = node("qualified_name", vec![n.with("qualifier"), dot, name]);
        }
        n
    }

    /// An identifier, with type arguments if a list follows.
    fn simple_name_type(&mut self) -> Item {
        let id = self.identifier();
        if self.at("<") && self.scan_type_arguments(self.i).is_some() {
            let args = self.type_argument_list();
            return node("generic_name", vec![id, args]);
        }
        id
    }

    pub(super) fn type_argument_list(&mut self) -> Item {
        let mut items = vec![self.punct()];
        if self.at(",") || self.at(">") {
            while self.at(",") {
                items.push(self.punct());
            }
        } else {
            self.comma_list(&mut items, ">", false, Self::ty);
        }
        items.push(self.expect(">"));
        node("type_argument_list", items)
    }

    /// At `>`: how many touching tokens make the operator, and its kind
    /// (`>`, `>=`, `>>`, `>>=`, `>>>`, `>>>=`).
    pub(super) fn joined_greater(&self) -> (usize, &'static str) {
        let i = self.i;
        let gt = |k: usize| self.is(i + k, ">") && self.adjacent(i + k - 1);
        let eq = |k: usize| self.is(i + k, "=") && self.adjacent(i + k - 1);
        if gt(1) {
            if gt(2) {
                if eq(3) {
                    return (4, ">>>=");
                }
                return (3, ">>>");
            }
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
