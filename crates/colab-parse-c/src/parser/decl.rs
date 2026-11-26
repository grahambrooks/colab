//! Declarations: their specifiers and types, declarators, classes,
//! enums, templates, namespaces, and the scans that look ahead over them.

use super::{Ctx, Dialect, Item, PRIMITIVES, Parser, QUALIFIERS, SIZED, node};
use crate::lexer::L;

/// What a declarator declares: its name's kind, and whether it may lack
/// one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Dk {
    /// A variable or function: `identifier`.
    Normal,
    /// A class member: `field_identifier`.
    Field,
    /// A `typedef`'s: `type_identifier`.
    Type,
    /// No name (a type descriptor's).
    Abstract,
    /// A parameter's: a name or none.
    Param,
}

impl Dk {
    fn abstract_ok(self) -> bool {
        matches!(self, Dk::Abstract | Dk::Param)
    }
}

const MS_POINTER: &[(&str, &str)] = &[
    ("__restrict", "ms_restrict_modifier"),
    ("__uptr", "ms_unsigned_ptr_modifier"),
    ("__sptr", "ms_signed_ptr_modifier"),
    ("_unaligned", "ms_unaligned_ptr_modifier"),
    ("__unaligned", "ms_unaligned_ptr_modifier"),
];

const MS_CALL: &[&str] = &[
    "__cdecl",
    "__clrcall",
    "__stdcall",
    "__fastcall",
    "__thiscall",
    "__vectorcall",
];

/// Operators `operator` may name, longest first.
const OPERATOR_NAMES: &[&str] = &[
    "->*", "<=>", "<<=", "+=", "-=", "*=", "/=", "%=", "^=", "&=", "|=", "<<", "==", "!=", "<=",
    "&&", "||", "++", "--", "->", "+", "-", "*", "/", "%", "^", "&", "|", "~", "!", "=", "<", ",",
];

impl Parser<'_> {
    // ---- scanning ahead ----------------------------------------------------

    /// Template arguments at `n` (a `<`): the token after their `>`.
    pub(crate) fn scan_template_args(&self, n: usize) -> Option<usize> {
        let mut angle = 1usize;
        let mut k = n + 1;
        loop {
            match self.kind_at(k) {
                L::Op("<") => angle += 1,
                // `>=` touching is a comparison, not a closer.
                L::Op(">") if self.is(k + 1, "=") && self.adjacent(k) => return None,
                L::Op(">") => {
                    angle -= 1;
                    if angle == 0 {
                        return Some(k + 1);
                    }
                }
                L::Op("(" | "[" | "{") => {
                    k = self.scan_balanced(k)?;
                    continue;
                }
                L::Op(")" | "]" | "}" | ";") | L::Eof => return None,
                L::Directive(_) | L::OtherDirective => return None,
                _ => {}
            }
            k += 1;
        }
    }

    /// A type's name at `n` (`A`, `A<B>::C`, `::A`, `typename A::B`): the
    /// token after it. Stops before a `::` that a destructor's `~` or an
    /// `operator` follows.
    pub(crate) fn scan_type_name(&self, n: usize) -> Option<usize> {
        if !self.cpp() {
            // C: a name, or a macro's type `NAME(T)`.
            if !self.is_ident(n) {
                return None;
            }
            if self.macro_type_at(n) {
                return self.scan_balanced(n + 1);
            }
            // `MAYBE_UNUSED unsigned x`: sized words after the name.
            let mut k = n + 1;
            while SIZED.contains(&self.word_at(k)) {
                k += 1;
            }
            return Some(k);
        }
        let mut k = n;
        if self.is(k, "::") {
            k += 1;
        }
        loop {
            if self.is(k, "template") {
                k += 1;
            }
            if !self.is_ident(k) {
                return None;
            }
            k += 1;
            if self.is(k, "<")
                && let Some(e) = self.scan_template_args(k)
            {
                k = e;
            }
            if self.is(k, "::") {
                if self.is(k + 1, "~") || self.is(k + 1, "operator") {
                    return Some(k);
                }
                k += 1;
                continue;
            }
            return Some(k);
        }
    }

    /// C's `NAME(T)` at `n` before a declarator (`STACK_OF(X509) *x;`): a
    /// `macro_type_specifier`.
    pub(crate) fn macro_type_at(&self, n: usize) -> bool {
        if self.cpp() || !self.is_ident(n) || !self.is(n + 1, "(") {
            return false;
        }
        let Some(e) = self.scan_balanced(n + 1) else {
            return false;
        };
        let inner_ok = {
            let mut k = n + 2;
            while QUALIFIERS.contains(&self.word_at(k)) {
                k += 1;
            }
            self.scan_type_specifier(k).is_some_and(|t| {
                let mut t = t;
                while matches!(self.kind_at(t), L::Op("*")) || QUALIFIERS.contains(&self.word_at(t))
                {
                    t += 1;
                }
                t + 1 == e
            })
        };
        // `;` after it: a field's type alone (`BIT(persist);`); a block's
        // `F(x);` is not a declaration for `c_declaration_at`.
        inner_ok
            && (self.is_ident(e)
                || self.is(e, ";")
                || (self.is(e, "*")
                    && (self.is_ident(e + 1) || self.is(e + 1, "*") || self.is(e + 1, "("))))
    }

    /// A type specifier at `n`: the token after it.
    pub(crate) fn scan_type_specifier(&self, n: usize) -> Option<usize> {
        let w = self.word_at(n);
        if PRIMITIVES.contains(&w) || (w == "auto" && self.cpp()) {
            return Some(n + 1);
        }
        if SIZED.contains(&w) {
            let mut k = n;
            while SIZED.contains(&self.word_at(k)) || PRIMITIVES.contains(&self.word_at(k)) {
                k += 1;
            }
            // C's `unsigned T x`.
            if !self.cpp() && self.is_ident(k) && self.is_ident(k + 1) {
                k += 1;
            }
            return Some(k);
        }
        // C++'s keywords are C's names.
        let w = if !self.cpp() && matches!(w, "class" | "typename" | "decltype") {
            ""
        } else {
            w
        };
        match w {
            "struct" | "class" | "union" | "enum" => {
                let mut k = n + 1;
                if self.is(k, "class") || self.is(k, "struct") {
                    k += 1;
                }
                if self.is(k, "{") {
                    return self.scan_balanced(k);
                }
                let e = self.scan_type_name(k)?;
                if self.is(e, "{") {
                    return self.scan_balanced(e);
                }
                Some(e)
            }
            "typename" => self.scan_type_name(n + 1),
            "decltype" if self.is(n + 1, "(") => {
                let e = self.scan_balanced(n + 1)?;
                if self.is(e, "::") {
                    return self.scan_type_name(e + 1);
                }
                Some(e)
            }
            _ => self.scan_type_name(n),
        }
    }

    /// A type descriptor at `n` (qualifiers, a type, an abstract
    /// declarator): the token after it.
    pub(crate) fn scan_type_descriptor(&self, n: usize) -> Option<usize> {
        let mut k = n;
        while QUALIFIERS.contains(&self.word_at(k)) {
            k += 1;
        }
        k = self.scan_type_specifier(k)?;
        loop {
            let w = self.word_at(k);
            if QUALIFIERS.contains(&w) {
                k += 1;
                continue;
            }
            match self.kind_at(k) {
                L::Op("*" | "&" | "&&") => k += 1,
                L::Op("[") if !(self.is(k + 1, "[") && self.adjacent(k)) => {
                    k = self.scan_balanced(k)?;
                }
                // An abstract function or parenthesized declarator.
                L::Op("(") => k = self.scan_balanced(k)?,
                L::Op("...") => return Some(k),
                _ => return Some(k),
            }
        }
    }

    /// Whether token `n` is a word that only starts a type.
    fn type_word(&self, n: usize) -> bool {
        let w = self.word_at(n);
        PRIMITIVES.contains(&w) || SIZED.contains(&w)
    }

    /// Whether a declaration (or a function definition) starts here, as
    /// tree-sitter's parse reads one: specifiers, then a type and a
    /// declarator (`a * b;` and `a < b > c;` are declarations), or in a
    /// file or class a constructor's declarator.
    pub(crate) fn declaration_at(&self, ctx: Ctx) -> bool {
        let mut n = self.i;
        loop {
            if self.is(n, "[") && self.is(n + 1, "[") && self.adjacent(n) {
                match self.scan_balanced(n) {
                    Some(e) => n = e,
                    None => return false,
                }
                continue;
            }
            let w = self.word_at(n);
            if matches!(
                w,
                "__attribute__" | "__attribute" | "__declspec" | "alignas" | "_Alignas"
            ) && self.is(n + 1, "(")
            {
                return true;
            }
            if self.storage(w)
                || QUALIFIERS.contains(&w)
                || (self.cpp() && matches!(w, "virtual" | "explicit"))
                || MS_CALL.contains(&w)
            {
                return true;
            }
            break;
        }
        let w = self.word_at(n);
        if !self.cpp() {
            return self.c_declaration_at(n, w, ctx);
        }
        if PRIMITIVES.contains(&w)
            || SIZED.contains(&w)
            || matches!(
                w,
                "struct" | "class" | "union" | "enum" | "typename" | "auto"
            )
        {
            // `int(x)` is a functional cast; `int (*f)()` a declaration.
            if PRIMITIVES.contains(&w) && self.is(n + 1, "(") && !self.is(n + 2, "*") {
                return self.dialect == Dialect::C;
            }
            return true;
        }
        if w == "decltype" {
            return self
                .scan_type_specifier(n)
                .is_some_and(|t| self.declarator_follows(t));
        }
        if w == "operator" || (self.is(n, "~") && self.is_ident(n + 1) && self.is(n + 2, "(")) {
            return ctx != Ctx::Block;
        }
        if !(self.is(n, "::") || self.is_ident(n)) {
            return false;
        }
        let Some(t) = self.scan_type_name(n) else {
            return false;
        };
        if self.is(t, "::") {
            // `A::~A()`, `A::operator T()`.
            return ctx != Ctx::Block;
        }
        if self.is(t, "(") {
            if ctx == Ctx::Block {
                // A constructor's definition is a block item too.
                return self.constructor_follows(t);
            }
            // `T (*f)(…)`: a type and a parenthesized declarator.
            if self.is(t + 1, "*") || self.is(t + 1, "&") || self.is(t + 1, "^") {
                return true;
            }
            // A constructor's (or a macro's) declarator; `F(x);` in a file
            // is a call to tree-sitter, in a class a declaration.
            return self.scan_balanced(t).is_some_and(|e| {
                (matches!(self.kind_at(e), L::Op("{" | ":" | "="))
                    || (self.is(e, ";") && ctx == Ctx::Field))
                    || matches!(
                        self.word_at(e),
                        "const" | "noexcept" | "override" | "final" | "try" | "throw"
                    )
                    || self.is_ident(e)
            });
        }
        self.declarator_follows(t)
    }

    /// C: after specifiers at `n`, whether a declaration starts (a type
    /// word, or a name or macro type before a declarator).
    fn c_declaration_at(&self, n: usize, w: &str, ctx: Ctx) -> bool {
        if PRIMITIVES.contains(&w) || SIZED.contains(&w) || matches!(w, "struct" | "union" | "enum")
        {
            return true;
        }
        if !self.is_ident(n) {
            return false;
        }
        let Some(t) = self.scan_type_name(n) else {
            return false;
        };
        let macro_type = t > n + 1;
        match self.kind_at(t) {
            L::Name => self.is_ident(t) || QUALIFIERS.contains(&self.word_at(t)),
            // A block's `M(T) * x;` multiplies (a macro type loses there).
            L::Op("*") if macro_type && ctx == Ctx::Block => false,
            L::Op("*") => {
                let mut k = t;
                while self.is(k, "*") || QUALIFIERS.contains(&self.word_at(k)) {
                    k += 1;
                }
                if self.is(k, "(") {
                    return self.is(k + 1, "*");
                }
                self.is_ident(k)
                    && matches!(
                        self.kind_at(k + 1),
                        L::Op(";" | "=" | "," | "[" | "(" | ")")
                    )
            }
            // `T (*f)(…);` is a call to tree-sitter-c, in a file too.
            _ => false,
        }
    }

    /// At `(` after a name: whether a constructor's definition follows
    /// its parameters (a body, an initializer list, `= default`, …).
    fn constructor_follows(&self, t: usize) -> bool {
        let Some(mut e) = self.scan_balanced(t) else {
            return false;
        };
        while matches!(self.word_at(e), "const" | "noexcept" | "override" | "final") {
            e += 1;
            if self.is(e, "(") {
                match self.scan_balanced(e) {
                    Some(k) => e = k,
                    None => return false,
                }
            }
        }
        match self.kind_at(e) {
            L::Op("{") => true,
            L::Op(":") => !self.is(e + 1, ":"),
            L::Op("=") => matches!(self.word_at(e + 1), "default" | "delete"),
            L::Name => self.is(e, "try"),
            _ => false,
        }
    }

    /// After a type at `t`: whether a declarator follows it.
    fn declarator_follows(&self, t: usize) -> bool {
        match self.kind_at(t) {
            L::Name => {
                let w = self.word_at(t);
                if self.is_ident(t) {
                    return !matches!(w, "final" | "override")
                        || matches!(self.kind_at(t + 1), L::Op(";" | "=" | "," | "(" | "["));
                }
                QUALIFIERS.contains(&w) || w == "operator"
            }
            L::Op("*" | "&" | "&&") => {
                let mut k = t;
                while matches!(self.kind_at(k), L::Op("*" | "&" | "&&"))
                    || QUALIFIERS.contains(&self.word_at(k))
                    || MS_POINTER.iter().any(|(w, _)| *w == self.word_at(k))
                {
                    k += 1;
                }
                if self.is(k, "(") {
                    // `T* (*f)(…)`; `a && (*b)` compares.
                    return !self.is(t, "&&") && (self.is(k + 1, "*") || self.is(k + 1, "&"));
                }
                if self.is(k, "operator") {
                    return true;
                }
                let Some(e) = self.scan_type_name(k) else {
                    return false;
                };
                if !self.is_ident(k) && !self.is(k, "::") {
                    return false;
                }
                // `T& A::operator+=(…)`, `T* A::~A`.
                if self.is(e, "::") {
                    return true;
                }
                matches!(
                    self.kind_at(e),
                    L::Op(";" | "=" | "," | "[" | "(" | ")" | "{" | ":")
                )
            }
            L::Op("(") => self.is(t + 1, "*") || self.is(t + 1, "&") || self.is(t + 1, "^"),
            L::Op("[") => self.is(t + 1, "[") && self.adjacent(t),
            _ => false,
        }
    }

    // ---- attributes and modifiers -------------------------------------------

    pub(crate) fn attribute_declaration(&mut self) -> Item {
        let mut items = vec![self.joined(2, "[[")];
        self.comma_list(&mut items, "]", false, |p| {
            let mut a = Vec::new();
            if p.peek() == L::Name && p.is(p.i + 1, "::") {
                a.push(p.leaf("identifier").with("prefix"));
                a.push(p.punct());
            }
            if p.peek() == L::Name {
                a.push(p.leaf("identifier").with("name"));
            } else {
                a.push(p.missing("identifier", true));
            }
            if p.at("(") {
                a.push(p.argument_list());
            }
            if p.at("...") {
                a.push(p.punct());
            }
            node("attribute", a)
        });
        if self.at("]") && self.is(self.i + 1, "]") && self.adjacent(self.i) {
            items.push(self.joined(2, "]]"));
        } else {
            items.push(self.missing("]]", false));
        }
        node("attribute_declaration", items)
    }

    fn attribute_specifier(&mut self) -> Item {
        let items = vec![
            self.punct(),
            self.expect("("),
            self.argument_list(),
            self.expect(")"),
        ];
        node("attribute_specifier", items)
    }

    /// Modifiers and the type: `_declaration_specifiers`. Whether a type
    /// was read.
    pub(crate) fn decl_specifiers(&mut self, items: &mut Vec<Item>, ctx: Ctx) -> bool {
        let mut have = false;
        loop {
            let w = self.word();
            if self.at_attribute() {
                items.push(self.attribute_declaration());
                continue;
            }
            if self.peek() == L::Name {
                if self.storage(w) {
                    items.push(self.wrapped("storage_class_specifier"));
                    continue;
                }
                if QUALIFIERS.contains(&w) {
                    items.push(self.wrapped("type_qualifier"));
                    continue;
                }
                match w {
                    "alignas" | "_Alignas" if self.is(self.i + 1, "(") => {
                        let a = self.alignas();
                        items.push(node("type_qualifier", vec![a]));
                        continue;
                    }
                    "virtual" if self.cpp() => {
                        items.push(self.punct());
                        continue;
                    }
                    "explicit" if self.dialect == Dialect::Cpp => {
                        let mut e = vec![self.punct()];
                        if self.at("(") {
                            e.push(self.punct());
                            e.push(self.expression());
                            e.push(self.expect(")"));
                        }
                        items.push(node("explicit_function_specifier", e));
                        continue;
                    }
                    "__attribute__" | "__attribute" if self.is(self.i + 1, "(") => {
                        items.push(self.attribute_specifier());
                        continue;
                    }
                    "__declspec" if self.is(self.i + 1, "(") => {
                        let d = vec![
                            self.punct(),
                            self.punct(),
                            self.identifier(),
                            self.expect(")"),
                        ];
                        items.push(node("ms_declspec_modifier", d));
                        continue;
                    }
                    _ => {}
                }
            }
            if !have && self.type_specifier_here(ctx) {
                items.push(self.type_specifier().with("type"));
                have = true;
                continue;
            }
            return have;
        }
    }

    /// Whether the type of a declaration starts here (not a constructor's
    /// name, nor a declarator).
    fn type_specifier_here(&self, ctx: Ctx) -> bool {
        let w = self.word();
        if !self.cpp() {
            if PRIMITIVES.contains(&w)
                || SIZED.contains(&w)
                || matches!(w, "struct" | "union" | "enum")
            {
                return true;
            }
            // A name, or `NAME(T)`, or a name before a parenthesized
            // declarator (`T (*f)(…)`, `T (MACRO *f)(…)`); `f(` alone is a
            // declarator's.
            return self.is_ident(self.i)
                && (!self.is(self.i + 1, "(")
                    || self.macro_type_at(self.i)
                    || self.is(self.i + 2, "*")
                    || (self.is_ident(self.i + 2) && self.is(self.i + 3, "*")));
        }
        if PRIMITIVES.contains(&w)
            || SIZED.contains(&w)
            || matches!(
                w,
                "struct" | "class" | "union" | "enum" | "typename" | "auto" | "decltype"
            )
        {
            return true;
        }
        if !(self.at("::") || self.is_ident(self.i)) {
            return false;
        }
        let Some(t) = self.scan_type_name(self.i) else {
            return false;
        };
        // `A(…)`, `A::A(…)`, `A::~A()`: a constructor's declarator; `A
        // (*f)(…)` a type before a parenthesized declarator.
        let constructor = (self.is(t, "(")
            && !matches!(self.kind_at(t + 1), L::Op("*" | "&" | "^")))
            || self.is(t, "::");
        !(constructor && (ctx != Ctx::Block || self.is(t, "::") || self.constructor_follows(t)))
    }

    fn alignas(&mut self) -> Item {
        let mut items = vec![self.punct(), self.punct()];
        let ty = self
            .scan_type_descriptor(self.i)
            .is_some_and(|e| self.is(e, ")") && self.type_in_parens(self.i, e));
        if ty {
            items.push(self.type_descriptor());
        } else {
            items.push(self.expression());
        }
        items.push(self.expect(")"));
        node("alignas_qualifier", items)
    }

    // ---- types ---------------------------------------------------------------

    pub(crate) fn type_specifier(&mut self) -> Item {
        let w = self.word();
        if !self.cpp() {
            return self.c_type_specifier();
        }
        match w {
            "struct" | "class" | "union" => return self.class_specifier(),
            "enum" => return self.enum_specifier(),
            "typename" => {
                let kw = self.punct();
                let t = self.type_specifier();
                return node("dependent_type", vec![kw, t]);
            }
            "auto" => {
                let a = self.leaf("auto");
                return node("placeholder_type_specifier", vec![a]);
            }
            "decltype" => {
                let d = self.decltype();
                if d.kind() == "placeholder_type_specifier" {
                    return d;
                }
                if self.at("::") {
                    return self.qualified_after(d, Qk::Type);
                }
                return d;
            }
            _ => {}
        }
        if SIZED.contains(&w) {
            return self.sized_type_specifier();
        }
        if PRIMITIVES.contains(&w) {
            return self.leaf("primitive_type");
        }
        let name = self.type_name();
        // `Concept auto`: a constrained placeholder.
        if self.at("auto") || (self.at("decltype") && self.is(self.i + 2, "auto")) {
            let a = if self.at("auto") {
                self.leaf("auto")
            } else {
                self.decltype_auto()
            };
            return node(
                "placeholder_type_specifier",
                vec![name.with("constraint"), a],
            );
        }
        name
    }

    /// C's type specifiers: `struct` / `union` / `enum`, sized and
    /// primitive types, `NAME(T)` and a name.
    fn c_type_specifier(&mut self) -> Item {
        let w = self.word();
        match w {
            "struct" | "union" => return self.class_specifier(),
            "enum" => return self.enum_specifier(),
            _ => {}
        }
        if SIZED.contains(&w) {
            return self.sized_type_specifier();
        }
        if PRIMITIVES.contains(&w) {
            return self.leaf("primitive_type");
        }
        if self.macro_type_at(self.i) {
            let name = self.leaf("identifier").with("name");
            let open = self.punct();
            let ty = self.type_descriptor().with("type");
            let close = self.expect(")");
            return node("macro_type_specifier", vec![name, open, ty, close]);
        }
        if SIZED.contains(&self.word_at(self.i + 1)) {
            let mut items = vec![self.leaf("type_identifier").with("type")];
            while SIZED.contains(&self.word()) {
                items.push(self.punct());
            }
            return node("sized_type_specifier", items);
        }
        self.ident_as("type_identifier")
    }

    fn decltype(&mut self) -> Item {
        if self.is(self.i + 2, "auto") && self.is(self.i + 3, ")") {
            let d = self.decltype_auto();
            return node("placeholder_type_specifier", vec![d]);
        }
        let items = vec![
            self.punct(),
            self.expect("("),
            self.expression(),
            self.expect(")"),
        ];
        node("decltype", items)
    }

    fn decltype_auto(&mut self) -> Item {
        let items = vec![
            self.punct(),
            self.expect("("),
            self.leaf("auto"),
            self.expect(")"),
        ];
        node("decltype", items)
    }

    fn sized_type_specifier(&mut self) -> Item {
        let mut items = Vec::new();
        while SIZED.contains(&self.word()) {
            items.push(self.punct());
        }
        // C's `typedef short int16_t;`: a primitive name (not a base word)
        // just before the end is the declarator.
        let declarator_next = !self.cpp()
            && !matches!(self.word(), "int" | "char" | "double" | "float")
            && matches!(self.kind_at(self.i + 1), L::Op(";" | "[" | "="));
        if PRIMITIVES.contains(&self.word()) && !declarator_next {
            items.push(self.leaf("primitive_type").with("type"));
            while SIZED.contains(&self.word()) {
                items.push(self.punct());
            }
        } else if !self.cpp() && self.is_ident(self.i) && self.is_ident(self.i + 1) {
            // C's `unsigned T x`: a type name after the sized words.
            items.push(self.leaf("type_identifier").with("type"));
        }
        node("sized_type_specifier", items)
    }

    /// A type's name: `T`, `A<B>`, `ns::T`, `::T`.
    pub(crate) fn type_name(&mut self) -> Item {
        if !self.cpp() {
            return self.ident_as("type_identifier");
        }
        if self.at("::") {
            let colons = self.punct();
            let name = self.type_name_segment(true).with("name");
            return node("qualified_identifier", vec![colons, name]);
        }
        self.type_name_segment(false)
    }

    /// One segment of a type's name and the rest after it: a scope
    /// (`namespace_identifier` or `template_type`) and `::`, or the type.
    fn type_name_segment(&mut self, _after_scope: bool) -> Item {
        let dependent = self.at("template");
        let template_kw = if dependent { Some(self.punct()) } else { None };
        let id_at = self.i;
        if !self.is_ident(id_at) {
            return self.missing("type_identifier", true);
        }
        let args = self.is(id_at + 1, "<") && self.scan_template_args(id_at + 1).is_some();
        let scoped = {
            let after = if args {
                self.scan_template_args(id_at + 1).unwrap_or(id_at + 1)
            } else {
                id_at + 1
            };
            self.is(after, "::") && !self.is(after + 1, "~") && !self.is(after + 1, "operator")
        };
        let seg = if args {
            let name = self.ident_as("type_identifier").with("name");
            let a = self.template_argument_list().with("arguments");
            node("template_type", vec![name, a])
        } else if scoped {
            self.ident_as("namespace_identifier")
        } else {
            self.ident_as("type_identifier")
        };
        let seg = match template_kw {
            Some(kw) => node("dependent_name", vec![kw, seg]),
            None => seg,
        };
        if scoped {
            return self.qualified_after(seg, Qk::Type);
        }
        seg
    }

    /// `scope :: name…` after the scope `scope`.
    pub(crate) fn qualified_after(&mut self, scope: Item, qk: Qk) -> Item {
        let colons = self.expect("::");
        let name = match qk {
            Qk::Type => self.type_name_segment(true),
            Qk::Expr | Qk::Decl => self.qualified_name_tail(qk),
        }
        .with("name");
        node(
            "qualified_identifier",
            vec![scope.with("scope"), colons, name],
        )
    }

    // ---- classes and enums -----------------------------------------------------

    fn class_specifier(&mut self) -> Item {
        let kind = match self.word() {
            "struct" => "struct_specifier",
            "union" => "union_specifier",
            _ => "class_specifier",
        };
        let mut items = vec![self.punct()];
        loop {
            match self.word() {
                "__attribute__" | "__attribute" if self.is(self.i + 1, "(") => {
                    items.push(self.attribute_specifier());
                }
                "alignas" | "_Alignas" if self.is(self.i + 1, "(") => items.push(self.alignas()),
                "__declspec" if self.is(self.i + 1, "(") => {
                    let d = vec![
                        self.punct(),
                        self.punct(),
                        self.identifier(),
                        self.expect(")"),
                    ];
                    items.push(node("ms_declspec_modifier", d));
                }
                _ if self.at_attribute() => items.push(self.attribute_declaration()),
                _ => break,
            }
        }
        if self.at("::")
            || (self.is_ident(self.i) && !matches!(self.word(), "final"))
            || (self.at("final") && !matches!(self.kind_at(self.i + 1), L::Op("{" | ":")))
        {
            items.push(self.type_name().with("name"));
        }
        if self.cpp() && (self.at("final") || self.at("override")) {
            items.push(self.wrapped("virtual_specifier"));
        }
        let body_follows = |p: &Self| {
            if !p.at(":") || p.is(p.i + 1, ":") {
                return false;
            }
            // `: B {`: a base clause before a body.
            let mut k = p.i + 1;
            let mut depth = 0usize;
            loop {
                match p.kind_at(k) {
                    L::Op("{") if depth == 0 => return true,
                    L::Op(";" | "}" | ")") if depth == 0 => return false,
                    L::Op("(" | "[") => depth += 1,
                    L::Op(")" | "]") => depth = depth.saturating_sub(1),
                    L::Eof => return false,
                    _ => {}
                }
                k += 1;
            }
        };
        if self.dialect == Dialect::Cpp && body_follows(self) {
            items.push(self.base_class_clause());
        }
        if self.at("{") {
            items.push(self.field_declaration_list().with("body"));
        }
        if matches!(self.word(), "__attribute__" | "__attribute") && self.is(self.i + 1, "(") {
            items.push(self.attribute_specifier());
        }
        node(kind, items)
    }

    fn base_class_clause(&mut self) -> Item {
        let mut items = vec![self.punct()];
        self.comma_list(&mut items, "{", false, |p| {
            let mut b = Vec::new();
            while p.at_attribute() {
                b.push(p.attribute_declaration());
            }
            loop {
                match p.word() {
                    "public" | "private" | "protected" => b.push(p.wrapped("access_specifier")),
                    "virtual" => b.push(p.punct()),
                    _ => break,
                }
            }
            b.push(p.type_name());
            if p.at("...") {
                b.push(p.punct());
            }
            node("", b)
        });
        node("base_class_clause", items)
    }

    pub(crate) fn field_declaration_list(&mut self) -> Item {
        let mut items = vec![self.expect("{")];
        while !self.eof() && !self.at("}") && !self.at_directive_end() {
            let before = self.i;
            items.push(self.item(Ctx::Field));
            if self.i == before {
                items.push(self.error_token());
            }
        }
        items.push(self.expect("}"));
        node("field_declaration_list", items)
    }

    /// One member of a class.
    pub(crate) fn field_item(&mut self) -> Item {
        let w = if self.cpp() { self.word() } else { "" };
        match w {
            "public" | "private" | "protected" if self.is(self.i + 1, ":") => {
                let a = self.wrapped("access_specifier");
                let c = self.punct();
                return node("", vec![a, c]);
            }
            "template" => return self.template(),
            "friend" => return self.friend(),
            "constexpr" if self.is(self.i + 1, "friend") => return self.friend(),
            "using" => return self.using(),
            "typedef" => return self.type_definition(),
            "static_assert" => return self.static_assert(),
            _ => {}
        }
        if self.at(";") {
            return self.punct();
        }
        if self.at("#if") || self.at("#ifdef") || self.at("#ifndef") {
            return self.preproc_conditional(&mut |p| p.item(Ctx::Field));
        }
        self.declaration(Ctx::Field)
    }

    fn friend(&mut self) -> Item {
        let mut items = Vec::new();
        if self.at("constexpr") {
            items.push(self.punct());
        }
        items.push(self.punct());
        // `friend class X;`: the class's name alone.
        let mut k = self.i;
        if matches!(self.word(), "class" | "struct" | "union") {
            k += 1;
        }
        if self.scan_type_name(k).is_some_and(|e| self.is(e, ";")) {
            if k > self.i {
                items.push(self.punct());
            }
            items.push(self.type_name());
            items.push(self.expect(";"));
            return node("friend_declaration", items);
        }
        items.push(self.declaration(Ctx::Top));
        node("friend_declaration", items)
    }

    fn enum_specifier(&mut self) -> Item {
        let mut items = vec![self.punct()];
        if self.cpp() && (self.at("class") || self.at("struct")) {
            items.push(self.punct());
        }
        while self.at_attribute() {
            items.push(self.attribute_declaration());
        }
        if self.at("::") || self.is_ident(self.i) {
            items.push(self.type_name().with("name"));
            if self.at(":") && !self.is(self.i + 1, ":") && !self.cpp() {
                // C: `: primitive` as `underlying_type`.
                items.push(self.punct());
                let base = if SIZED.contains(&self.word()) {
                    self.sized_type_specifier()
                } else {
                    self.leaf("primitive_type")
                };
                items.push(base.with("underlying_type"));
            } else if self.at(":") && !self.is(self.i + 1, ":") {
                items.push(self.punct());
                let base = if SIZED.contains(&self.word()) {
                    self.sized_type_specifier()
                } else if PRIMITIVES.contains(&self.word()) {
                    self.leaf("primitive_type")
                } else {
                    self.type_name()
                };
                items.push(base.with("base"));
            }
        }
        if self.at("{") {
            items.push(self.enumerator_list().with("body"));
        }
        if matches!(self.word(), "__attribute__" | "__attribute") && self.is(self.i + 1, "(") {
            items.push(self.attribute_specifier());
        }
        node("enum_specifier", items)
    }

    fn enumerator_list(&mut self) -> Item {
        let mut items = vec![self.punct()];
        self.enumerators(&mut items);
        items.push(self.expect("}"));
        node("enumerator_list", items)
    }

    fn enumerators(&mut self, items: &mut Vec<Item>) {
        loop {
            if self.at("}") || self.eof() || self.at_directive_end() {
                return;
            }
            let before = self.i;
            match self.peek() {
                L::Directive("#if" | "#ifdef" | "#ifndef") => {
                    items.push(self.preproc_conditional(&mut |p| {
                        let mut b = Vec::new();
                        p.enumerator_and_comma(&mut b);
                        node("", b)
                    }));
                    continue;
                }
                L::OtherDirective => {
                    items.push(self.preproc_call());
                    if self.at(",") {
                        items.push(self.punct());
                    }
                    continue;
                }
                _ => {}
            }
            self.enumerator_and_comma(items);
            if self.i == before {
                items.push(self.error_token());
            }
        }
    }

    fn enumerator_and_comma(&mut self, items: &mut Vec<Item>) {
        if !self.is_ident(self.i) && !self.at(",") {
            if !self.at_directive_end() && !self.at("}") {
                items.push(self.error_token());
            }
            return;
        }
        let mut e = vec![self.identifier().with("name")];
        while self.at_attribute() {
            e.push(self.attribute_declaration());
        }
        if self.at("=") {
            e.push(self.punct());
            e.push(self.expression().with("value"));
        }
        items.push(node("enumerator", e));
        if self.at(",") {
            items.push(self.punct());
        }
    }

    // ---- declarations ----------------------------------------------------------

    /// A declaration, a function definition, a field declaration or an
    /// empty declaration (`struct S {};`), by `ctx`.
    pub(crate) fn declaration(&mut self, ctx: Ctx) -> Item {
        let mut items = Vec::new();
        let have_type = self.decl_specifiers(&mut items, ctx);
        // `operator bool()`: a conversion's declarator.
        if !have_type && self.operator_cast_at() {
            return self.operator_cast_declaration(items);
        }
        if self.at(";") && have_type {
            if ctx == Ctx::Field {
                items.push(self.punct());
                return node("field_declaration", items);
            }
            if items.len() == 1 {
                let t = items.pop().map(Item::without_field);
                let mut e: Vec<Item> = t.into_iter().collect();
                e.push(self.punct());
                return node("", e);
            }
            items.push(self.punct());
            return node("declaration", items);
        }
        let dk = if ctx == Ctx::Field && have_type {
            Dk::Field
        } else {
            Dk::Normal
        };
        let first = self.declarator_init(dk);
        // Any declarator before braces holding statements is a function
        // definition to tree-sitter (`struct MACRO Name { … }`).
        let function = has_function(&first) || (self.at("{") && self.braces_hold_statements());
        if function && self.function_body_follows(have_type, ctx) {
            items.push(first.with("declarator"));
            self.function_body(&mut items);
            return node("function_definition", items);
        }
        let field = ctx == Ctx::Field && have_type;
        let mut d = first;
        loop {
            d = self.initialized(d, field);
            if field {
                items.extend(self.field_declarator_parts(d));
            } else {
                items.push(d.with("declarator"));
            }
            if self.at(",") {
                items.push(self.punct());
                d = self.declarator_init(dk);
                continue;
            }
            break;
        }
        if matches!(self.word(), "__attribute__" | "__attribute") && self.is(self.i + 1, "(") {
            items.push(self.attribute_specifier());
        }
        items.push(self.expect(";"));
        node(
            if field {
                "field_declaration"
            } else {
                "declaration"
            },
            items,
        )
    }

    /// A field's declarator with its bitfield or default value, as the
    /// field declaration's own children.
    fn field_declarator_parts(&mut self, d: Item) -> Vec<Item> {
        let mut out = Vec::new();
        // `initialized` wraps nothing for fields: `= v` and `{…}` and `:`
        // come here.
        out.push(d.with("declarator"));
        if self.at(":") && !self.is(self.i + 1, ":") {
            let b = vec![self.punct(), self.expression()];
            out.push(node("bitfield_clause", b));
        } else if self.at("=") {
            out.push(self.punct());
            let v = if self.at("{") {
                self.initializer_list()
            } else {
                self.expression()
            };
            out.push(v.with("default_value"));
        } else if self.at("{") {
            out.push(self.initializer_list().with("default_value"));
        }
        out
    }

    /// `d = v`, `d{…}`, `d(…)` as an `init_declarator` (not for fields).
    fn initialized(&mut self, d: Item, field: bool) -> Item {
        if field {
            return d;
        }
        if self.at("=") {
            let eq = self.punct();
            let v = if self.at("{") {
                self.initializer_list()
            } else {
                self.expression()
            };
            return node(
                "init_declarator",
                vec![d.with("declarator"), eq, v.with("value")],
            );
        }
        if self.at("{") && self.dialect == Dialect::Cpp {
            let v = self.initializer_list().with("value");
            return node("init_declarator", vec![d.with("declarator"), v]);
        }
        if self.at("(") && self.dialect == Dialect::Cpp {
            let v = self.argument_list().with("value");
            return node("init_declarator", vec![d.with("declarator"), v]);
        }
        d
    }

    fn function_body_follows(&self, have_type: bool, ctx: Ctx) -> bool {
        match self.peek() {
            L::Op("{") => true,
            L::Name if self.at("try") => true,
            L::Op(":") if !have_type && !self.is(self.i + 1, ":") => true,
            // `= 0;` after a member with a type is a field's default value
            // to tree-sitter; after a constructor, a pure virtual clause.
            L::Op("=") => {
                (ctx == Ctx::Field || !have_type)
                    && (self.is(self.i + 1, "default")
                        || self.is(self.i + 1, "delete")
                        || (!have_type
                            && self.kind_at(self.i + 1) == L::Number
                            && &self.src[self.tk(self.i + 1).start..self.tk(self.i + 1).end]
                                == "0"))
                    && self.is(self.i + 2, ";")
            }
            _ => false,
        }
    }

    /// A function's body: `{…}`, `try {…} catch…`, `: init {…}`,
    /// `= default;`, `= delete;`, `= 0;`.
    fn function_body(&mut self, items: &mut Vec<Item>) {
        if self.at(":") {
            items.push(self.field_initializer_list());
        }
        if self.at("try") {
            let mut t = vec![self.punct()];
            if self.at(":") {
                t.push(self.field_initializer_list());
            }
            t.push(self.compound_statement().with("body"));
            while self.at("catch") {
                let c = vec![
                    self.punct(),
                    self.parameter_list().with("parameters"),
                    self.compound_statement().with("body"),
                ];
                t.push(node("catch_clause", c));
            }
            items.push(node("try_statement", t).with("body"));
            return;
        }
        if self.at("=") {
            let kind = match self.word_at(self.i + 1) {
                "default" => "default_method_clause",
                "delete" => "delete_method_clause",
                _ => "pure_virtual_clause",
            };
            let eq = self.punct();
            let mut c = vec![eq];
            if kind == "pure_virtual_clause" {
                let t = self.advance();
                c.push(Self::hidden(t.start, t.end));
            } else {
                c.push(self.punct());
            }
            c.push(self.expect(";"));
            items.push(node(kind, c));
            return;
        }
        items.push(self.compound_statement().with("body"));
    }

    fn field_initializer_list(&mut self) -> Item {
        let mut items = vec![self.punct()];
        self.comma_list(&mut items, "{", false, |p| {
            let mut f = Vec::new();
            if p.at("::")
                || p.is(p.i + 1, "::")
                || (p.is(p.i + 1, "<")
                    && p.scan_template_args(p.i + 1).is_some_and(|e| p.is(e, "::")))
            {
                // A qualified field identifier: its last name a field's.
                f.push(field_last(p.expr_name(true)));
            } else if p.is(p.i + 1, "<") && p.scan_template_args(p.i + 1).is_some() {
                let name = p.ident_as("field_identifier").with("name");
                let a = p.template_argument_list().with("arguments");
                f.push(node("template_method", vec![name, a]));
            } else {
                f.push(p.ident_as("field_identifier"));
            }
            if p.at("{") {
                f.push(p.initializer_list());
            } else {
                f.push(p.argument_list());
            }
            if p.at("...") {
                f.push(p.punct());
            }
            node("field_initializer", f)
        });
        node("field_initializer_list", items)
    }

    fn operator_cast_at(&self) -> bool {
        let mut k = self.i;
        if self.is_ident(k) || self.is(k, "::") {
            match self.scan_type_name(k) {
                Some(e) if self.is(e, "::") => k = e + 1,
                _ => {}
            }
        }
        self.is(k, "operator")
            && (self.scan_type_specifier(k + 1).is_some()
                || QUALIFIERS.contains(&self.word_at(k + 1)))
            && !self.is(k + 1, "new")
            && !self.is(k + 1, "delete")
    }

    /// `operator T() …` (or `A::operator T`): a definition or declaration.
    fn operator_cast_declaration(&mut self, mut items: Vec<Item>) -> Item {
        let declarator = if self.at("operator") {
            self.operator_cast()
        } else {
            // `A::operator T`.
            let scope = self.scope_segment();
            let colons = self.expect("::");
            let cast = self.operator_cast().with("name");
            node(
                "qualified_identifier",
                vec![scope.with("scope"), colons, cast],
            )
        };
        items.push(declarator.with("declarator"));
        if self.at("{") || self.at("try") {
            self.function_body(&mut items);
            return node("function_definition", items);
        }
        if self.at("=") {
            items.push(self.punct());
            items.push(self.expression().with("default_value"));
        }
        items.push(self.expect(";"));
        node("declaration", items)
    }

    fn operator_cast(&mut self) -> Item {
        let mut items = vec![self.punct()];
        // The type, which a `(` after does not make a constructor's name.
        while QUALIFIERS.contains(&self.word()) {
            items.push(self.wrapped("type_qualifier"));
        }
        items.push(self.type_specifier().with("type"));
        while QUALIFIERS.contains(&self.word()) {
            items.push(self.wrapped("type_qualifier"));
        }
        items.push(self.declarator(Dk::Abstract).with("declarator"));
        node("operator_cast", items)
    }

    /// A scope's segment: `namespace_identifier` or `template_type`.
    fn scope_segment(&mut self) -> Item {
        if self.is(self.i + 1, "<") && self.scan_template_args(self.i + 1).is_some() {
            let name = self.ident_as("type_identifier").with("name");
            let a = self.template_argument_list().with("arguments");
            return node("template_type", vec![name, a]);
        }
        self.ident_as("namespace_identifier")
    }

    // ---- declarators -------------------------------------------------------------

    /// A declarator of a declaration, whose `(…)` may be an initializer's
    /// arguments rather than parameters.
    fn declarator_init(&mut self, dk: Dk) -> Item {
        self.declarator_with(dk, true)
    }

    pub(crate) fn declarator(&mut self, dk: Dk) -> Item {
        self.declarator_with(dk, false)
    }

    fn declarator_with(&mut self, dk: Dk, init: bool) -> Item {
        let abstract_ = dk == Dk::Abstract;
        match self.peek() {
            L::Op("*") => {
                let mut items = vec![self.punct()];
                self.pointer_modifiers(&mut items);
                if self.declarator_start(dk) {
                    items.push(self.declarator_with(dk, init).with("declarator"));
                }
                // Abstract when nothing named is inside it.
                let kind = if (abstract_ || dk == Dk::Param) && !self.named_inside(&items) {
                    "abstract_pointer_declarator"
                } else {
                    "pointer_declarator"
                };
                return node(kind, items);
            }
            L::Op("&" | "&&") if self.dialect == Dialect::Cpp => {
                let mut items = vec![self.punct()];
                if self.declarator_start(dk) {
                    items.push(self.declarator_with(dk, init));
                }
                let kind = if !self.named_inside(&items) {
                    "abstract_reference_declarator"
                } else {
                    "reference_declarator"
                };
                return node(kind, items);
            }
            _ => {}
        }
        let base = self.direct_declarator(dk);
        self.declarator_suffixes(base, dk, init)
    }

    /// Whether a declarator (of `dk`'s kind) continues at the current
    /// token after a `*` or `&`.
    fn declarator_start(&self, dk: Dk) -> bool {
        match self.peek() {
            L::Op("*" | "&" | "&&" | "(" | "::" | "~") => true,
            L::Op("[") => !dk.abstract_ok() || self.at_attribute(),
            L::Op("...") => dk == Dk::Param,
            L::Name => {
                self.is_ident(self.i)
                    || self.at("operator")
                    || (dk == Dk::Type && PRIMITIVES.contains(&self.word()))
            }
            _ => false,
        }
    }

    /// Whether `items` (a declarator's being built) hold a name.
    fn named_inside(&self, items: &[Item]) -> bool {
        items.iter().any(|i| match i {
            Item::Node(n) => {
                !n.kind.starts_with("abstract_")
                    && n.kind != "type_qualifier"
                    && n.kind != "ms_pointer_modifier"
            }
            Item::Tok(t) => t.named,
        })
    }

    fn pointer_modifiers(&mut self, items: &mut Vec<Item>) {
        loop {
            let w = self.word();
            if let Some(&(_, kind)) = MS_POINTER.iter().find(|(m, _)| *m == w) {
                // `__restrict` is a token of its own; the others wrap theirs.
                let m = if kind == "ms_restrict_modifier" {
                    self.leaf(kind)
                } else {
                    self.wrapped(kind)
                };
                items.push(node("ms_pointer_modifier", vec![m]));
                continue;
            }
            if QUALIFIERS.contains(&w) {
                items.push(self.wrapped("type_qualifier"));
                continue;
            }
            return;
        }
    }

    /// The name, or parenthesized declarator, a declarator's suffixes
    /// apply to; `None` (as an empty node) for an abstract one.
    fn direct_declarator(&mut self, dk: Dk) -> Option<Item> {
        match self.peek() {
            L::Op("(") => {
                if dk.abstract_ok() && !self.paren_declarator_at() {
                    return None;
                }
                let mut items = vec![self.punct()];
                if MS_CALL.contains(&self.word()) {
                    items.push(self.wrapped("ms_call_modifier"));
                }
                let inner = self.declarator(dk);
                let named = self.named_inside(std::slice::from_ref(&inner));
                items.push(inner);
                items.push(self.expect(")"));
                let kind = if dk.abstract_ok() && !named {
                    "abstract_parenthesized_declarator"
                } else {
                    "parenthesized_declarator"
                };
                Some(node(kind, items))
            }
            L::Op("[") if dk == Dk::Normal && !self.at_attribute() && self.cpp() => {
                // `auto [a, b] = …`
                let mut items = vec![self.punct()];
                self.comma_list(&mut items, "]", false, Self::identifier);
                items.push(self.expect("]"));
                Some(node("structured_binding_declarator", items))
            }
            L::Op("~") if dk != Dk::Abstract && self.cpp() => {
                let t = self.punct();
                let id = self.identifier();
                Some(node("destructor_name", vec![t, id]))
            }
            L::Op("::") if self.cpp() => Some(self.expr_name(false)),
            L::Name if self.at("operator") && self.cpp() => Some(self.operator_name()),
            L::Name if dk == Dk::Type && PRIMITIVES.contains(&self.word()) => {
                Some(self.leaf("primitive_type"))
            }
            L::Name if dk == Dk::Type && SIZED.contains(&self.word()) => {
                Some(self.leaf("primitive_type"))
            }
            L::Name if self.is_ident(self.i) && !self.cpp() => {
                let kind = match dk {
                    Dk::Field => "field_identifier",
                    Dk::Type => "type_identifier",
                    _ => "identifier",
                };
                Some(self.leaf(kind))
            }
            L::Name if self.is_ident(self.i) => {
                let scoped = {
                    let mut k = self.i + 1;
                    if self.is(k, "<")
                        && let Some(e) = self.scan_template_args(k)
                    {
                        k = e;
                    }
                    self.is(k, "::")
                };
                if scoped && dk != Dk::Type {
                    return Some(self.expr_name(false));
                }
                if self.is(self.i + 1, "<")
                    && dk != Dk::Type
                    && self
                        .scan_template_args(self.i + 1)
                        .is_some_and(|e| self.is(e, "("))
                {
                    let (kind, name_kind) = if dk == Dk::Field {
                        ("template_method", "field_identifier")
                    } else {
                        ("template_function", "identifier")
                    };
                    let name = self.ident_as(name_kind).with("name");
                    let a = self.template_argument_list().with("arguments");
                    return Some(node(kind, vec![name, a]));
                }
                let kind = match dk {
                    Dk::Field => "field_identifier",
                    Dk::Type => "type_identifier",
                    _ => "identifier",
                };
                Some(self.leaf(kind))
            }
            _ if dk.abstract_ok() => None,
            _ => Some(self.missing("identifier", true)),
        }
    }

    /// At `(` in an abstract-capable position: whether it opens a
    /// parenthesized declarator (`(*f)`, `(&r)`) rather than parameters.
    fn paren_declarator_at(&self) -> bool {
        let n = self.i + 1;
        match self.kind_at(n) {
            L::Op("*" | "&" | "&&" | "^") => true,
            L::Op("(") => true,
            _ => MS_CALL.contains(&self.word_at(n)),
        }
    }

    fn declarator_suffixes(&mut self, base: Option<Item>, dk: Dk, init: bool) -> Item {
        let mut d = base;
        loop {
            match self.peek() {
                L::Op("(") => {
                    // A declaration's `x(a + 1)` is an initializer.
                    // C has no constructor syntax: always parameters.
                    if init && dk == Dk::Normal && self.cpp() && !self.params_like(self.i) {
                        break;
                    }
                    let mut items = Vec::new();
                    let abstract_ = d.is_none()
                        || d.as_ref()
                            .is_some_and(|x| x.kind().starts_with("abstract_"));
                    if let Some(x) = d.take() {
                        items.push(x.with("declarator"));
                    }
                    items.push(self.parameter_list().with("parameters"));
                    self.function_declarator_seq(&mut items);
                    let kind = if abstract_ && dk.abstract_ok() {
                        "abstract_function_declarator"
                    } else {
                        "function_declarator"
                    };
                    d = Some(node(kind, items));
                }
                L::Op("[") if !self.at_attribute() => {
                    let abstract_ = d.is_none()
                        || d.as_ref()
                            .is_some_and(|x| x.kind().starts_with("abstract_"));
                    let mut items = Vec::new();
                    if let Some(x) = d.take() {
                        items.push(x.with("declarator"));
                    }
                    items.push(self.punct());
                    while QUALIFIERS.contains(&self.word()) || self.at("static") {
                        if self.at("static") {
                            items.push(self.punct());
                        } else {
                            items.push(self.wrapped("type_qualifier"));
                        }
                    }
                    if self.at("*") && self.is(self.i + 1, "]") {
                        items.push(self.punct().with("size"));
                    } else if !self.at("]") {
                        items.push(self.expression().with("size"));
                    }
                    items.push(self.expect("]"));
                    let kind = if abstract_ && dk.abstract_ok() {
                        "abstract_array_declarator"
                    } else {
                        "array_declarator"
                    };
                    d = Some(node(kind, items));
                }
                L::Op("[") if self.at_attribute() && d.is_some() => {
                    let mut items =
                        vec![d.take().unwrap_or_else(|| self.missing("identifier", true))];
                    while self.at_attribute() {
                        items.push(self.attribute_declaration());
                    }
                    d = Some(node("attributed_declarator", items));
                }
                _ => break,
            }
        }
        d.unwrap_or_else(|| node("", Vec::new()))
    }

    /// At `(`: whether the parentheses hold parameters (or nothing), not
    /// an initializer's arguments, as tree-sitter's parse prefers a
    /// function declarator where both read.
    fn params_like(&self, n: usize) -> bool {
        if self.is(n + 1, ")") {
            return true;
        }
        let Some(end) = self.scan_balanced(n) else {
            return false;
        };
        let mut k = n + 1;
        loop {
            if self.is(k, "...") {
                k += 1;
            } else {
                // Specifiers, a type, then a declarator's tokens.
                let mut any = false;
                while QUALIFIERS.contains(&self.word_at(k)) || self.storage(self.word_at(k)) {
                    k += 1;
                    any = true;
                }
                if self.at_attribute_at(k) {
                    return true;
                }
                let Some(t) = self.scan_type_specifier(k) else {
                    return false;
                };
                let _ = any;
                k = t;
                // The declarator: only a declarator's tokens (names,
                // `*`, `&`, brackets, nested parameter lists) to the
                // element's end; `x.y`, `a + b`, a literal make arguments.
                let mut depth = 0usize;
                loop {
                    match self.kind_at(k) {
                        L::Op("(" | "[") => depth += 1,
                        L::Op(")" | "]") if depth > 0 => depth -= 1,
                        L::Op(",") | L::Op(")") if depth == 0 => break,
                        L::Op("=") if depth == 0 => {
                            // A default value: to the element's end.
                            while k < end - 1 && !(self.is(k, ",") && depth == 0) {
                                k += 1;
                            }
                            break;
                        }
                        L::Op("*" | "&" | "&&" | "..." | "::" | "," | "<" | ">") => {}
                        L::Name
                            if self.is_ident(k)
                                || QUALIFIERS.contains(&self.word_at(k))
                                || self.type_word(k) => {}
                        L::Number if depth > 0 && self.is(k - 1, "[") => {}
                        _ => return false,
                    }
                    k += 1;
                    if k >= end {
                        return false;
                    }
                }
            }
            if self.is(k, ",") {
                k += 1;
                continue;
            }
            return k == end - 1;
        }
    }

    fn at_attribute_at(&self, k: usize) -> bool {
        self.is(k, "[") && self.is(k + 1, "[") && self.adjacent(k)
    }

    /// After a function declarator's parameters: qualifiers, a reference
    /// qualifier, `noexcept` / `throw(…)`, attributes, a trailing return
    /// type, `override` / `final` or a `requires` clause.
    fn function_declarator_seq(&mut self, items: &mut Vec<Item>) {
        if !self.cpp() {
            return self.c_function_declarator_tail(items);
        }
        loop {
            let w = self.word();
            match self.peek() {
                L::Name if QUALIFIERS.contains(&w) => items.push(self.wrapped("type_qualifier")),
                L::Name
                    if matches!(w, "__attribute__" | "__attribute") && self.is(self.i + 1, "(") =>
                {
                    items.push(self.attribute_specifier());
                }
                L::Op("&" | "&&") if self.dialect == Dialect::Cpp => {
                    items.push(self.wrapped("ref_qualifier"));
                }
                L::Name if w == "noexcept" => {
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
                L::Name if w == "throw" && self.is(self.i + 1, "(") => {
                    let mut t = vec![self.punct(), self.punct()];
                    self.comma_list(&mut t, ")", false, Self::type_descriptor);
                    t.push(self.expect(")"));
                    items.push(node("throw_specifier", t));
                }
                L::Op("[") if self.at_attribute() => items.push(self.attribute_declaration()),
                L::Op("->") if self.dialect == Dialect::Cpp => {
                    let arrow = self.punct();
                    let t = self.type_descriptor();
                    items.push(node("trailing_return_type", vec![arrow, t]));
                }
                L::Name if matches!(w, "override" | "final") => {
                    items.push(self.wrapped("virtual_specifier"));
                }
                L::Name if w == "requires" => items.push(self.requires_clause()),
                L::Name if matches!(w, "asm" | "__asm__" | "__asm") && self.is(self.i + 1, "(") => {
                    items.push(self.gnu_asm());
                }
                _ => return,
            }
        }
    }

    /// C: `asm(…)`, attribute specifiers, and (a definition's, before its
    /// body) macro words and calls.
    fn c_function_declarator_tail(&mut self, items: &mut Vec<Item>) {
        if matches!(self.word(), "asm" | "__asm__" | "__asm") && self.is(self.i + 1, "(") {
            items.push(self.gnu_asm());
        }
        let body_after = |p: &Self| {
            let mut k = p.i;
            loop {
                if matches!(p.word_at(k), "__attribute__" | "__attribute") && p.is(k + 1, "(") {
                    match p.scan_balanced(k + 1) {
                        Some(e) => k = e,
                        None => return false,
                    }
                } else if p.is_ident(k) {
                    k += 1;
                    if p.is(k, "(") {
                        match p.scan_balanced(k) {
                            Some(e) => k = e,
                            None => return false,
                        }
                    }
                } else {
                    return p.is(k, "{");
                }
            }
        };
        let words = body_after(self);
        loop {
            let w = self.word();
            if matches!(w, "__attribute__" | "__attribute") && self.is(self.i + 1, "(") {
                items.push(self.attribute_specifier());
            } else if words && self.is_ident(self.i) {
                if self.is(self.i + 1, "(") {
                    let f = self.leaf("identifier").with("function");
                    let args = self.argument_list().with("arguments");
                    items.push(node("call_expression", vec![f, args]));
                } else {
                    items.push(self.leaf("identifier"));
                }
            } else {
                return;
            }
        }
    }

    pub(crate) fn parameter_list(&mut self) -> Item {
        let mut items = vec![self.expect("(")];
        self.comma_list(&mut items, ")", false, Self::parameter);
        items.push(self.expect(")"));
        node("parameter_list", items)
    }

    fn parameter(&mut self) -> Item {
        if self.at("...") {
            if self.dialect == Dialect::C {
                let dots = self.punct();
                return node("variadic_parameter", vec![dots]);
            }
            return self.punct();
        }
        let mut items = Vec::new();
        let typed = self.decl_specifiers(&mut items, Ctx::Block);
        // A C parameter has a type: `f(T(int))`'s `T` is one.
        if !typed && !self.cpp() && self.is_ident(self.i) {
            items.push(self.ident_as("type_identifier").with("type"));
        }
        // `T... xs`, `T&&... xs`.
        if self.at("...")
            || (matches!(self.peek(), L::Op("&" | "&&")) && self.is(self.i + 1, "..."))
        {
            let reference = if self.at("...") {
                None
            } else {
                Some(self.punct())
            };
            let mut v = vec![self.punct()];
            if self.is_ident(self.i) {
                v.push(self.identifier());
            }
            let v = node("variadic_declarator", v);
            let d = match reference {
                Some(r) => node("reference_declarator", vec![r, v]),
                None => v,
            };
            items.push(d.with("declarator"));
            return node("variadic_parameter_declaration", items);
        }
        if !matches!(self.peek(), L::Op("," | ")" | "=" | ">")) {
            let d = self.declarator(Dk::Param);
            if !(d.kind().is_empty() && matches!(&d, Item::Node(n) if n.items.is_empty())) {
                items.push(d.with("declarator"));
            }
        }
        while matches!(self.word(), "__attribute__" | "__attribute") && self.is(self.i + 1, "(") {
            items.push(self.attribute_specifier());
        }
        if self.at("=") && self.dialect == Dialect::Cpp {
            items.push(self.punct());
            let v = if self.at("{") {
                self.initializer_list()
            } else {
                self.expression()
            };
            items.push(v.with("default_value"));
            return node("optional_parameter_declaration", items);
        }
        node("parameter_declaration", items)
    }

    /// `operator+`, `operator()`, `operator new[]`, `operator""_x`.
    pub(crate) fn operator_name(&mut self) -> Item {
        let mut items = vec![self.punct()];
        match self.peek() {
            L::Op("(") if self.is(self.i + 1, ")") && self.adjacent(self.i) => {
                items.push(self.joined(2, "()"));
            }
            L::Op("[") if self.is(self.i + 1, "]") && self.adjacent(self.i) => {
                items.push(self.joined(2, "[]"));
            }
            L::Op(">") => {
                let (n, kind) = self.joined_greater();
                items.push(self.joined(n, kind));
            }
            L::Name if matches!(self.word(), "new" | "delete") => {
                items.push(self.punct());
                if self.at("[") && self.is(self.i + 1, "]") && self.adjacent(self.i) {
                    items.push(self.joined(2, "[]"));
                }
            }
            L::Name
                if matches!(
                    self.word(),
                    "co_await"
                        | "xor"
                        | "bitand"
                        | "bitor"
                        | "compl"
                        | "not"
                        | "xor_eq"
                        | "and_eq"
                        | "or_eq"
                        | "not_eq"
                        | "and"
                        | "or"
                ) =>
            {
                items.push(self.punct());
            }
            L::StrOpen("\"") if matches!(self.kind_at(self.i + 1), L::Close(_)) => {
                items.push(self.joined(2, "\"\""));
                if self.peek() == L::Suffix {
                    items.push(self.leaf("identifier"));
                } else {
                    items.push(self.identifier());
                }
            }
            L::Op(o) if OPERATOR_NAMES.contains(&o) => items.push(self.punct()),
            _ => items.push(self.missing("+", false)),
        }
        node("operator_name", items)
    }

    // ---- type descriptors and template arguments -----------------------------------

    pub(crate) fn type_descriptor(&mut self) -> Item {
        let mut items = Vec::new();
        while QUALIFIERS.contains(&self.word()) {
            items.push(self.wrapped("type_qualifier"));
        }
        items.push(self.type_specifier().with("type"));
        while QUALIFIERS.contains(&self.word()) {
            items.push(self.wrapped("type_qualifier"));
        }
        if matches!(self.peek(), L::Op("*" | "&" | "&&" | "(" | "["))
            && !(self.at("[") && self.at_attribute())
        {
            let d = self.declarator(Dk::Abstract);
            if !matches!(&d, Item::Node(n) if n.kind.is_empty() && n.items.is_empty()) {
                items.push(d.with("declarator"));
            }
        }
        node("type_descriptor", items)
    }

    pub(crate) fn template_argument_list(&mut self) -> Item {
        let mut items = vec![self.punct()];
        self.no_gt += 1;
        self.comma_list(&mut items, ">", false, |p| {
            let ty = p
                .scan_type_descriptor(p.i)
                .is_some_and(|e| matches!(p.kind_at(e), L::Op("," | ">" | "...")))
                && !matches!(p.peek(), L::Number);
            let a = if ty {
                p.type_descriptor()
            } else {
                p.expr_no_comma()
            };
            if p.at("...") {
                let dots = p.punct();
                return node("parameter_pack_expansion", vec![a.with("pattern"), dots]);
            }
            a
        });
        self.no_gt -= 1;
        // `>>` closing two lists: the lexer gives single `>`s.
        items.push(self.expect(">"));
        node("template_argument_list", items)
    }

    // ---- templates, namespaces and other declarations -------------------------------

    pub(crate) fn template(&mut self) -> Item {
        let kw = self.punct();
        if !self.at("<") {
            // An explicit instantiation.
            let mut items = vec![kw];
            self.decl_specifiers(&mut items, Ctx::Block);
            if !self.at(";") {
                items.push(self.declarator(Dk::Normal).with("declarator"));
            }
            items.push(self.expect(";"));
            return node("template_instantiation", items);
        }
        let mut items = vec![kw, self.template_parameter_list().with("parameters")];
        if self.at("requires") {
            items.push(self.requires_clause());
        }
        let inner = match self.word() {
            "template" => self.template(),
            "using" => self.using(),
            "concept" => self.concept(),
            "friend" => self.friend(),
            _ => self.declaration(Ctx::Top),
        };
        items.push(inner);
        node("template_declaration", items)
    }

    pub(crate) fn template_parameter_list(&mut self) -> Item {
        let mut items = vec![self.punct()];
        self.no_gt += 1;
        self.comma_list(&mut items, ">", false, |p| match p.word() {
            "typename" | "class" if !p.is(p.i + 2, "::") => {
                let kw = p.punct();
                if p.at("...") {
                    let dots = p.punct();
                    let mut v = vec![kw, dots];
                    if p.is_ident(p.i) {
                        v.push(p.leaf("type_identifier"));
                    }
                    return node("variadic_type_parameter_declaration", v);
                }
                let name = if p.is_ident(p.i) {
                    Some(p.leaf("type_identifier"))
                } else {
                    None
                };
                if p.at("=") {
                    let mut o = vec![kw];
                    if let Some(n) = name {
                        o.push(n.with("name"));
                    }
                    o.push(p.punct());
                    o.push(p.type_specifier().with("default_type"));
                    return node("optional_type_parameter_declaration", o);
                }
                let mut t = vec![kw];
                t.extend(name);
                node("type_parameter_declaration", t)
            }
            "template" => {
                let kw = p.punct();
                let params = p.template_parameter_list().with("parameters");
                let inner = p.template_parameter_inner();
                node(
                    "template_template_parameter_declaration",
                    vec![kw, params, inner],
                )
            }
            _ => p.parameter(),
        });
        self.no_gt -= 1;
        items.push(self.expect(">"));
        node("template_parameter_list", items)
    }

    /// A template template parameter's own: `class T`, `typename... Ts`,
    /// `class T = U`.
    fn template_parameter_inner(&mut self) -> Item {
        let kw = if self.at("class") || self.at("typename") {
            self.punct()
        } else {
            self.missing("class", false)
        };
        if self.at("...") {
            let dots = self.punct();
            let mut v = vec![kw, dots];
            if self.is_ident(self.i) {
                v.push(self.leaf("type_identifier"));
            }
            return node("variadic_type_parameter_declaration", v);
        }
        let name = if self.is_ident(self.i) {
            Some(self.leaf("type_identifier"))
        } else {
            None
        };
        if self.at("=") {
            let mut o = vec![kw];
            if let Some(n) = name {
                o.push(n.with("name"));
            }
            o.push(self.punct());
            o.push(self.type_specifier().with("default_type"));
            return node("optional_type_parameter_declaration", o);
        }
        let mut t = vec![kw];
        t.extend(name);
        node("type_parameter_declaration", t)
    }

    /// `requires C` / `requires (a && b)`: a constraint of `&&` and `||`.
    pub(crate) fn requires_clause(&mut self) -> Item {
        let kw = self.punct();
        let c = self.constraint(0).with("constraint");
        node("requires_clause", vec![kw, c])
    }

    fn constraint(&mut self, min: u8) -> Item {
        let mut left = self.constraint_primary();
        loop {
            let (prec, kind) = match (self.peek(), self.word()) {
                (L::Op("||"), _) | (L::Name, "or") => (1, "constraint_disjunction"),
                (L::Op("&&"), _) | (L::Name, "and") => (2, "constraint_conjunction"),
                _ => break,
            };
            if prec < min {
                break;
            }
            let op = self.punct().with("operator");
            let right = self.constraint(prec + 1).with("right");
            left = node(kind, vec![left.with("left"), op, right]);
        }
        left
    }

    fn constraint_primary(&mut self) -> Item {
        match self.peek() {
            L::Op("(") => {
                let open = self.punct();
                let e = self.expression();
                let close = self.expect(")");
                node("", vec![open, e, close])
            }
            L::Name if matches!(self.word(), "true" | "TRUE") => self.leaf("true"),
            L::Name if matches!(self.word(), "false" | "FALSE") => self.leaf("false"),
            L::Name if self.at("requires") => self.requires_expression(),
            _ => self.type_name(),
        }
    }

    pub(crate) fn namespace(&mut self) -> Item {
        let mut items = Vec::new();
        if self.at("inline") {
            items.push(self.punct());
        }
        items.push(self.punct());
        if self.at_attribute() {
            items.push(self.attribute_declaration());
        }
        if self.is_ident(self.i) && self.is(self.i + 1, "=") {
            items.push(self.leaf("namespace_identifier").with("name"));
            items.push(self.punct());
            items.push(self.namespace_name());
            items.push(self.expect(";"));
            return node("namespace_alias_definition", items);
        }
        if self.is_ident(self.i) || self.at("::") {
            items.push(self.namespace_name().with("name"));
        }
        let mut body = vec![self.expect("{")];
        while !self.eof() && !self.at("}") && !self.at_directive_end() {
            let before = self.i;
            body.push(self.item(Ctx::Top));
            if self.i == before {
                body.push(self.error_token());
            }
        }
        body.push(self.expect("}"));
        items.push(node("declaration_list", body).with("body"));
        node("namespace_definition", items)
    }

    /// `a`, or `a::b::c` as nested `nested_namespace_specifier`s.
    fn namespace_name(&mut self) -> Item {
        let mut segs = Vec::new();
        let mut colons = Vec::new();
        if self.at("::") {
            colons.push(self.punct());
        }
        loop {
            let mut s = Vec::new();
            if self.at("inline") {
                s.push(self.punct());
            }
            s.push(self.ident_as("namespace_identifier"));
            segs.push(s);
            if self.at("::") {
                colons.push(self.punct());
                continue;
            }
            break;
        }
        // Right-nested: a :: (b :: c).
        if colons.is_empty() {
            let mut s = segs.pop().unwrap_or_default();
            return if s.len() == 1 {
                s.pop().unwrap_or_else(|| node("", Vec::new()))
            } else {
                node("", s)
            };
        }
        let mut acc: Option<Item> = None;
        let leading = colons.len() == segs.len();
        while let Some(s) = segs.pop() {
            match acc {
                None => acc = Some(node("", s)),
                Some(prev) => {
                    let c = colons.pop().unwrap_or_else(|| self.missing("::", false));
                    let mut items = s;
                    items.push(c);
                    items.push(prev);
                    acc = Some(node("nested_namespace_specifier", items));
                }
            }
        }
        let mut out = acc.unwrap_or_else(|| node("", Vec::new()));
        if leading && let Some(c) = colons.pop() {
            out = node("nested_namespace_specifier", vec![c, out]);
        }
        out
    }

    pub(crate) fn using(&mut self) -> Item {
        let mut items = vec![self.punct()];
        if self.at("namespace") || self.at("enum") {
            items.push(self.punct());
        } else if self.is_ident(self.i) && (self.is(self.i + 1, "=") || self.is(self.i + 1, "[")) {
            items.push(self.leaf("type_identifier").with("name"));
            while self.at_attribute() {
                items.push(self.attribute_declaration());
            }
            items.push(self.expect("="));
            items.push(self.type_descriptor().with("type"));
            items.push(self.expect(";"));
            return node("alias_declaration", items);
        }
        if self.at("typename") {
            items.push(self.punct());
        }
        items.push(self.expr_name(false));
        items.push(self.expect(";"));
        node("using_declaration", items)
    }

    pub(crate) fn static_assert(&mut self) -> Item {
        let mut items = vec![self.punct(), self.expect("(")];
        items.push(self.expr_no_comma().with("condition"));
        if self.at(",") {
            items.push(self.punct());
            items.push(self.string_or_concatenation().with("message"));
        }
        items.push(self.expect(")"));
        items.push(self.expect(";"));
        node("static_assert_declaration", items)
    }

    pub(crate) fn type_definition(&mut self) -> Item {
        let mut items = Vec::new();
        if self.at("__extension__") {
            items.push(self.punct());
        }
        items.push(self.punct());
        while QUALIFIERS.contains(&self.word()) {
            items.push(self.wrapped("type_qualifier"));
        }
        items.push(self.type_specifier().with("type"));
        while QUALIFIERS.contains(&self.word()) {
            items.push(self.wrapped("type_qualifier"));
        }
        loop {
            items.push(self.declarator(Dk::Type).with("declarator"));
            if self.at(",") {
                items.push(self.punct());
                continue;
            }
            break;
        }
        while matches!(self.word(), "__attribute__" | "__attribute") && self.is(self.i + 1, "(") {
            items.push(self.attribute_specifier());
        }
        items.push(self.expect(";"));
        node("type_definition", items)
    }

    pub(crate) fn linkage(&mut self) -> Item {
        let mut items = vec![self.punct(), self.string_literal().with("value")];
        if self.at("{") {
            let mut body = vec![self.punct()];
            while !self.eof() && !self.at("}") && !self.at_directive_end() {
                let before = self.i;
                body.push(self.item(Ctx::Top));
                if self.i == before {
                    body.push(self.error_token());
                }
            }
            body.push(self.expect("}"));
            items.push(node("declaration_list", body).with("body"));
        } else {
            items.push(self.declaration(Ctx::Top).with("body"));
        }
        node("linkage_specification", items)
    }

    pub(crate) fn concept(&mut self) -> Item {
        let items = vec![
            self.punct(),
            self.identifier().with("name"),
            self.expect("="),
            self.expression(),
            self.expect(";"),
        ];
        node("concept_definition", items)
    }
}

/// A qualified name whose last `identifier` is a `field_identifier`.
fn field_last(item: Item) -> Item {
    match item {
        Item::Tok(mut t) if t.kind == "identifier" => {
            t.kind = "field_identifier";
            Item::Tok(t)
        }
        Item::Node(mut n) if n.kind == "qualified_identifier" => {
            if let Some(last) = n.items.pop() {
                n.items.push(field_last(last));
            }
            Item::Node(n)
        }
        other => other,
    }
}

/// Whether a declarator declares a function: a function declarator, or
/// one under pointers, references, attributes or parentheses.
fn has_function(d: &Item) -> bool {
    match d {
        Item::Node(n) => match n.kind {
            "function_declarator" => true,
            "pointer_declarator"
            | "reference_declarator"
            | "attributed_declarator"
            | "parenthesized_declarator" => n
                .items
                .iter()
                .any(|c| matches!(c, Item::Node(_)) && has_function(c)),
            _ => false,
        },
        Item::Tok(_) => false,
    }
}

/// Where a qualified name is: a type's, an expression's or a
/// declarator's (which decides its last segment's kind).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Qk {
    Type,
    Expr,
    Decl,
}
