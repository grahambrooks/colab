//! JavaScript, read by recursive descent into a tree of [`Item`]s:
//! tree-sitter-javascript's grammar (statements, the expression
//! precedences, patterns, classes, modules, JSX), with kinds and fields
//! named as in `names.rs`.
//!
//! Tokens are pulled from the lexer on demand ([`Parser::fill`]) in the
//! mode the parser is in: a `/` where an operand belongs is lexed again as
//! a regex, and JSX text and tags in their own modes (`relex`). A
//! statement ends at a `;` or where tree-sitter's scanner puts an
//! automatic semicolon ([`Parser::asi`]).

use crate::lexer::{Extra, L, Lexer, Lx, Mode, State};
use crate::names;

mod ts;

/// A node or a token of the tree being built.
#[derive(Debug, Clone)]
pub(crate) enum Item {
    Tok(Tok),
    Node(Node),
}

#[derive(Debug, Clone)]
pub(crate) struct Tok {
    pub kind: &'static str,
    pub named: bool,
    pub start: usize,
    pub end: usize,
    pub field: Option<&'static str>,
    pub missing: bool,
    pub extra: bool,
    /// No token: a zero-width mark where a node ends (a statement over
    /// its trailing comment).
    pub hidden: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct Node {
    /// `"ERROR"` for text the parser could not read.
    pub kind: &'static str,
    pub field: Option<&'static str>,
    pub items: Vec<Item>,
}

impl Item {
    fn kind(&self) -> &'static str {
        match self {
            Item::Tok(t) => t.kind,
            Item::Node(n) => n.kind,
        }
    }

    fn with(mut self, field: &'static str) -> Self {
        match &mut self {
            Item::Tok(t) => t.field = Some(field),
            Item::Node(n) => n.field = Some(field),
        }
        self
    }

    fn field(&self) -> Option<&'static str> {
        match self {
            Item::Tok(t) => t.field,
            Item::Node(n) => n.field,
        }
    }

    fn renamed(mut self, kind: &'static str) -> Self {
        match &mut self {
            Item::Tok(t) => t.kind = kind,
            Item::Node(n) => n.kind = kind,
        }
        self
    }

    fn is_named(&self) -> bool {
        match self {
            Item::Tok(t) => t.named,
            Item::Node(_) => true,
        }
    }
}

fn node(kind: &'static str, items: Vec<Item>) -> Item {
    Item::Node(Node {
        kind,
        field: None,
        items,
    })
}

/// Words that are never identifiers.
const KEYWORDS: &[&str] = &[
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "function",
    "if",
    "import",
    "in",
    "instanceof",
    "new",
    "null",
    "return",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "var",
    "void",
    "while",
    "with",
];

/// Keywords tree-sitter-javascript reads as identifiers where one fits.
const RESERVED_IDENTIFIERS: &[&str] = &["get", "set", "async", "await", "static", "export", "let"];

const AUGMENTED: &[&str] = &[
    "+=", "-=", "*=", "/=", "%=", "^=", "&=", "|=", ">>=", ">>>=", "<<=", "**=", "&&=", "||=",
    "??=",
];

/// Which language the parser reads: JavaScript (with JSX), TypeScript, or
/// TSX (TypeScript with JSX).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dialect {
    Js,
    Ts,
    Tsx,
}

pub(crate) struct Parser<'s> {
    src: &'s str,
    lx: Lexer<'s>,
    /// Tokens lexed so far, each with the lexer state before it.
    toks: Vec<(Lx, State)>,
    i: usize,
    state: State,
    pub extras: Vec<Extra>,
    pub errors: bool,
    /// Inside a `for (…` head, where `in` ends the left side.
    no_in: bool,
    dialect: Dialect,
}

impl<'s> Parser<'s> {
    pub fn new(src: &'s str, dialect: Dialect) -> Self {
        Self {
            src,
            lx: Lexer::new(src),
            toks: Vec::new(),
            i: 0,
            state: State::default(),
            extras: Vec::new(),
            errors: false,
            no_in: false,
            dialect,
        }
    }

    /// TypeScript or TSX: types are read.
    fn ts(&self) -> bool {
        self.dialect != Dialect::Js
    }

    /// JavaScript or TSX: `<` in an expression starts JSX.
    fn jsx(&self) -> bool {
        self.dialect != Dialect::Ts
    }

    /// Tokens `self.i .. self.i + n`, which touch, as one anonymous token
    /// of `kind` (`?:`, `-?:`).
    fn joined(&mut self, n: usize, kind: &'static str) -> Item {
        let start = self.tk(0).start;
        let mut end = start;
        for _ in 0..n {
            end = self.advance().end;
        }
        Item::Tok(Tok {
            kind,
            named: false,
            start,
            end,
            field: None,
            missing: false,
            extra: false,
            hidden: false,
        })
    }

    /// Whether tokens `n` and `n + 1` touch.
    fn adjacent(&mut self, n: usize) -> bool {
        let a = self.tk(n);
        let b = self.tk(n + 1);
        a.end == b.start
    }

    /// A `>` closing type arguments or parameters, splitting `>>`, `>=`
    /// and the like, which the lexer took whole.
    fn close_angle(&mut self) -> Item {
        let t = self.tk(0);
        let rest = match t.k {
            L::Op(">") => return self.punct(),
            L::Op(">>") => ">",
            L::Op(">>>") => ">>",
            L::Op(">=") => "=",
            L::Op(">>=") => ">=",
            L::Op(">>>=") => ">>=",
            _ => return self.missing(">", false),
        };
        let k = self.i;
        let first = Item::Tok(Tok {
            kind: ">",
            named: false,
            start: t.start,
            end: t.start + 1,
            field: None,
            missing: false,
            extra: false,
            hidden: false,
        });
        self.toks[k].0.start += 1;
        self.toks[k].0.k = L::Op(rest);
        self.toks[k].0.nl = false;
        self.toks[k].1.pos += 1;
        first
    }

    // ---- tokens ----------------------------------------------------------

    /// Whether a `/` after token `prev` starts a regex (the usual
    /// heuristic, for tokens the parser has not reached; it lexes again
    /// where it knows).
    fn regex_after(&self, prev: Option<&Lx>) -> bool {
        let Some(p) = prev else {
            return true;
        };
        match p.k {
            L::Name => matches!(
                &self.src[p.start..p.end],
                "return"
                    | "typeof"
                    | "instanceof"
                    | "in"
                    | "of"
                    | "new"
                    | "delete"
                    | "void"
                    | "throw"
                    | "case"
                    | "do"
                    | "else"
                    | "yield"
                    | "await"
            ),
            L::Number | L::Regex | L::Private | L::Backtick | L::Quote => false,
            L::Op(o) => !matches!(o, ")" | "]" | "}" | "++" | "--"),
            _ => true,
        }
    }

    /// Lex until token `i + n` is there.
    fn fill(&mut self, n: usize) {
        while self.toks.len() <= self.i + n {
            if let Some((t, _)) = self.toks.last()
                && t.k == L::Eof
            {
                return;
            }
            let regex = self.regex_after(self.toks.last().map(|(t, _)| t));
            let before = self.state.clone();
            let t = self
                .lx
                .next(&mut self.state, Mode::Code { regex }, &mut self.extras);
            self.toks.push((t, before));
        }
    }

    fn tk(&mut self, n: usize) -> Lx {
        self.fill(n);
        let k = (self.i + n).min(self.toks.len() - 1);
        self.toks[k].0
    }

    fn peek(&mut self) -> L {
        self.tk(0).k
    }

    fn text(&self, t: Lx) -> &'s str {
        &self.src[t.start..t.end]
    }

    /// Whether token `n` ahead is the operator or word `s`.
    fn is(&mut self, n: usize, s: &str) -> bool {
        let t = self.tk(n);
        match t.k {
            L::Op(o) => o == s,
            L::Name => self.text(t) == s,
            _ => false,
        }
    }

    fn at(&mut self, s: &str) -> bool {
        self.is(0, s)
    }

    fn word(&mut self) -> &'s str {
        let t = self.tk(0);
        if t.k == L::Name { self.text(t) } else { "" }
    }

    fn eof(&mut self) -> bool {
        self.peek() == L::Eof
    }

    /// Lex the current token again in `mode`, dropping what was lexed
    /// after it.
    fn relex(&mut self, mode: Mode) {
        self.fill(0);
        if self.i >= self.toks.len() {
            return;
        }
        let state = self.toks[self.i].1.clone();
        self.toks.truncate(self.i);
        let pos = state.pos;
        self.extras.retain(|e| e.start < pos);
        self.state = state;
        let before = self.state.clone();
        let t = self.lx.next(&mut self.state, mode, &mut self.extras);
        self.toks.push((t, before));
    }

    fn advance(&mut self) -> Lx {
        let t = self.tk(0);
        if t.k != L::Eof {
            self.i += 1;
        }
        t
    }

    fn tok(t: Lx, kind: &'static str, named: bool) -> Item {
        Item::Tok(Tok {
            kind,
            named,
            start: t.start,
            end: t.end,
            field: None,
            missing: false,
            extra: false,
            hidden: false,
        })
    }

    fn leaf(&mut self, kind: &'static str) -> Item {
        let t = self.advance();
        Self::tok(t, kind, true)
    }

    fn punct(&mut self) -> Item {
        let t = self.advance();
        let kind = match t.k {
            L::Op(o) => o,
            _ => names::anonymous(&self.src[t.start..t.end]).unwrap_or("ERROR"),
        };
        Self::tok(t, kind, false)
    }

    fn prev_end(&self) -> usize {
        if self.i == 0 {
            0
        } else {
            self.toks[self.i - 1].0.end
        }
    }

    fn missing(&mut self, kind: &'static str, named: bool) -> Item {
        self.errors = true;
        let at = self.prev_end();
        Item::Tok(Tok {
            kind,
            named,
            start: at,
            end: at,
            field: None,
            missing: true,
            extra: false,
            hidden: false,
        })
    }

    fn expect(&mut self, s: &'static str) -> Item {
        if self.at(s) {
            self.punct()
        } else {
            self.missing(s, false)
        }
    }

    fn is_ident(&mut self, n: usize) -> bool {
        let t = self.tk(n);
        t.k == L::Name && !KEYWORDS.contains(&self.text(t))
    }

    fn identifier(&mut self) -> Item {
        if self.is_ident(0) {
            let w = self.word();
            if w == "undefined" {
                return self.leaf("undefined");
            }
            self.leaf("identifier")
        } else {
            self.missing("identifier", true)
        }
    }

    /// A binding's name: an identifier (`undefined` too is one here).
    fn binding_identifier(&mut self) -> Item {
        if self.is_ident(0) {
            self.leaf("identifier")
        } else {
            self.missing("identifier", true)
        }
    }

    /// Whether an automatic semicolon goes before the current token, as
    /// tree-sitter's scanners put one: before `}` and at the end, and after
    /// a line end unless the next line continues the statement.
    /// `after_expression` says whether an expression ends here (`||` could
    /// follow), which tree-sitter-typescript's scanner asks: after one a
    /// `(` or `[` on the next line continues the statement, after a type it
    /// starts the next member.
    fn asi_after(&mut self, after_expression: bool) -> bool {
        let t = self.tk(0);
        if t.k == L::Eof {
            return true;
        }
        if t.k == L::Op("}") {
            // TypeScript: `}` then `:` (`({a}: T) => …`) only after an
            // expression.
            if self.ts() && self.is(1, ":") {
                return after_expression;
            }
            return true;
        }
        if !t.nl {
            return false;
        }
        let s = &self.src[t.start..];
        let b = s.as_bytes();
        if self.ts() {
            return match b.first().copied().unwrap_or(0) {
                b'`' | b',' | b'.' | b';' | b'*' | b'%' | b'>' | b'<' | b'=' | b'?' | b'^'
                | b'|' | b'&' | b'/' | b':' => false,
                b'(' | b'[' => !after_expression,
                b'+' => b.get(1) == Some(&b'+'),
                b'-' => b.get(1) == Some(&b'-'),
                b'!' => b.get(1) != Some(&b'='),
                b'i' => {
                    let w = if t.k == L::Name {
                        &self.src[t.start..t.end]
                    } else {
                        ""
                    };
                    !(w == "in" || w == "instanceof")
                }
                _ => true,
            };
        }
        match b.first().copied().unwrap_or(0) {
            b'`' | b',' | b':' | b';' | b'*' | b'%' | b'>' | b'<' | b'=' | b'[' | b'(' | b'?'
            | b'^' | b'|' | b'&' | b'/' => false,
            b'.' => b.get(1).is_some_and(u8::is_ascii_digit),
            b'+' => b.get(1) == Some(&b'+'),
            b'-' => b.get(1) == Some(&b'-'),
            b'!' => b.get(1) != Some(&b'='),
            b'i' => {
                let w = if t.k == L::Name {
                    &self.src[t.start..t.end]
                } else {
                    ""
                };
                !(w == "in" || w == "instanceof")
            }
            _ => true,
        }
    }

    /// A statement's end: `;`, or an automatic semicolon (no node).
    fn semicolon(&mut self, items: &mut Vec<Item>) {
        self.semicolon_after(items, false);
    }

    /// A statement's end, after an expression (`after_expression`: where
    /// `||` could still follow). tree-sitter's scanner then gives up at a
    /// comment left on the line, and puts the automatic semicolon after
    /// it, so the statement holds the comment, unless a `}` follows;
    /// after anything else (an import's source) it goes before.
    fn semicolon_after(&mut self, items: &mut Vec<Item>, after_expression: bool) {
        if self.at(";") {
            items.push(self.punct());
            return;
        }
        if !self.asi_after(after_expression) {
            items.push(self.missing(";", false));
            return;
        }
        let t = self.tk(0);
        // tree-sitter-typescript's (older) scanner gives up at any comment
        // left on the line, so its statements always hold it.
        if !self.ts() && (!t.nl || !after_expression || t.k == L::Op("}")) {
            return;
        }
        let from = self.prev_end();
        let mut end = None;
        for e in &self.extras {
            if e.start >= from
                && e.end <= t.start
                && !self.src[from..e.start].contains(['\n', '\r', '\u{2028}', '\u{2029}'])
            {
                end = Some(end.map_or(e.end, |x: usize| x.max(e.end)));
            }
        }
        if let Some(end) = end {
            items.push(Item::Tok(Tok {
                kind: "",
                named: false,
                start: end,
                end,
                field: None,
                missing: false,
                extra: false,
                hidden: true,
            }));
        }
    }

    /// The rest of the statement as an `ERROR` node; at least one token.
    fn error_statement(&mut self) -> Item {
        self.errors = true;
        let mut items = Vec::new();
        let mut depth = 0usize;
        while !self.eof() {
            if depth == 0 && self.at("}") && !items.is_empty() {
                break;
            }
            if self.at("(") || self.at("[") || self.at("{") {
                depth += 1;
            } else if (self.at(")") || self.at("]") || self.at("}")) && depth > 0 {
                depth -= 1;
            }
            let semi = depth == 0 && self.at(";");
            let nl = !items.is_empty() && self.tk(0).nl && depth == 0;
            if nl {
                break;
            }
            let t = self.advance();
            items.push(Self::tok(t, "ERROR", false));
            if semi {
                break;
            }
        }
        if items.is_empty() && !self.eof() {
            let t = self.advance();
            items.push(Self::tok(t, "ERROR", false));
        }
        node("ERROR", items)
    }

    // ---- scanning ahead --------------------------------------------------

    /// Past the bracket at token `n` and its match.
    fn scan_balanced(&mut self, n: usize) -> Option<usize> {
        let mut depth = 0usize;
        let mut k = n;
        loop {
            match self.tk(k).k {
                L::Op("(" | "[" | "{" | "${") => depth += 1,
                L::Op(")" | "]" | "}") => {
                    depth = depth.checked_sub(1)?;
                    if depth == 0 {
                        return Some(k + 1);
                    }
                }
                L::Eof => return None,
                _ => {}
            }
            k += 1;
            if k > 100_000 {
                return None;
            }
        }
    }

    /// At a statement's `{`: whether it can only be an object (a block
    /// cannot start `"key":`, or hold `label: value, key:`), as
    /// tree-sitter reads a file that is one object.
    fn object_statement_at(&mut self) -> bool {
        // `{ a = 1 } = o`: a destructuring assignment.
        // TypeScript: `{ … } as T` / `satisfies T`, which a block cannot be.
        if let Some(k) = self.scan_balanced(0)
            && (self.is(k, "=") || (self.ts() && (self.is(k, "as") || self.is(k, "satisfies"))))
        {
            return true;
        }
        match self.tk(1).k {
            L::Quote | L::Number => {
                // `{"a": 1}`: skip the string to its `:`.
                let mut n = 2;
                while !matches!(self.tk(n).k, L::Quote | L::Eof) {
                    n += 1;
                }
                self.tk(1).k == L::Number && self.is(2, ":")
                    || (self.tk(1).k == L::Quote && self.is(n + 1, ":"))
            }
            L::Name if self.is(2, ":") => {
                let mut n = 3;
                let mut depth = 0usize;
                loop {
                    match self.tk(n).k {
                        L::Op("(" | "[" | "{" | "${") => depth += 1,
                        L::Op(")" | "]" | "}") if depth > 0 => depth -= 1,
                        L::Op(",") if depth == 0 => {
                            return self.tk(n + 1).k == L::Name && self.is(n + 2, ":")
                                || matches!(self.tk(n + 1).k, L::Quote);
                        }
                        L::Op(";" | "}") | L::Eof if depth == 0 => return false,
                        _ => {}
                    }
                    n += 1;
                    if n > 10_000 {
                        return false;
                    }
                }
            }
            _ => false,
        }
    }

    /// Whether an arrow function starts at token `n`.
    fn arrow_at(&mut self, n: usize) -> bool {
        let mut n = n;
        if self.is(n, "async")
            && !self.tk(n + 1).nl
            && (self.is_ident(n + 1) || self.is(n + 1, "("))
        {
            n += 1;
        }
        if self.is_ident(n) {
            return self.is(n + 1, "=>");
        }
        if self.is(n, "(") {
            return self.scan_balanced(n).is_some_and(|k| self.is(k, "=>"));
        }
        false
    }

    // ---- the program -------------------------------------------------------

    pub fn program(&mut self) -> Item {
        let mut items = Vec::new();
        if self.peek() == L::HashBang {
            items.push(self.leaf("hash_bang_line"));
        }
        while !self.eof() {
            let before = self.i;
            if self.at("}") {
                items.push(self.error_statement());
                continue;
            }
            items.push(self.statement());
            if self.i == before {
                items.push(self.error_statement());
            }
        }
        node("program", items)
    }

    // ---- statements --------------------------------------------------------

    fn statement(&mut self) -> Item {
        if self.at("@") {
            return self.decorated();
        }
        if self.ts() && self.peek() == L::Name {
            if let Some(d) = self.ts_declaration() {
                return d;
            }
            if self.at("namespace")
                && !self.tk(1).nl
                && (self.is_ident(1) || self.tk(1).k == L::Quote)
            {
                let mut items = vec![self.module_declaration("internal_module")];
                self.semicolon(&mut items);
                return node("expression_statement", items);
            }
            if self.at("import") && self.is_ident(1) && self.is(2, "=") && !self.is(3, "require") {
                let mut items = vec![self.punct(), self.binding_identifier(), self.punct()];
                items.push(self.nested_identifier());
                self.semicolon(&mut items);
                return node("import_alias", items);
            }
        }
        if self.peek() == L::Name {
            match self.word() {
                "import" if !self.is(1, "(") && !self.is(1, ".") => return self.import_statement(),
                "export" => return self.export_statement(Vec::new()),
                "var" => return self.variable_declaration(),
                "let" if self.is_ident(1) || self.is(1, "[") || self.is(1, "{") => {
                    return self.lexical_declaration();
                }
                "const" => return self.lexical_declaration(),
                // `using` is JavaScript's (tree-sitter-typescript's older
                // base grammar has none).
                "using"
                    if !self.ts()
                        && self.is_ident(1)
                        && !self.tk(1).nl
                        && !self.is(1, "in")
                        && !self.is(1, "of") =>
                {
                    return self.using_declaration();
                }
                "await" if !self.ts() && self.is(1, "using") && self.is_ident(2) => {
                    return self.using_declaration();
                }
                "function" => return self.function(true),
                "async" if self.is(1, "function") && !self.tk(1).nl => return self.function(true),
                "class" => return self.class(true, Vec::new()),
                "if" => return self.if_statement(),
                "switch" => return self.switch_statement(),
                "for" => return self.for_statement(),
                "while" => {
                    let items = vec![
                        self.punct(),
                        self.parenthesized().with("condition"),
                        self.statement().with("body"),
                    ];
                    return node("while_statement", items);
                }
                "do" => {
                    let mut items = vec![
                        self.punct(),
                        self.statement().with("body"),
                        self.expect("while"),
                        self.parenthesized().with("condition"),
                    ];
                    if self.at(";") {
                        items.push(self.punct());
                    }
                    return node("do_statement", items);
                }
                "try" => return self.try_statement(),
                "with" => {
                    let items = vec![
                        self.punct(),
                        self.parenthesized().with("object"),
                        self.statement().with("body"),
                    ];
                    return node("with_statement", items);
                }
                w @ ("break" | "continue") => {
                    let kind = if w == "break" {
                        "break_statement"
                    } else {
                        "continue_statement"
                    };
                    let mut items = vec![self.punct()];
                    if self.is_ident(0) && !self.tk(0).nl {
                        items.push(self.leaf("statement_identifier").with("label"));
                    }
                    self.semicolon(&mut items);
                    return node(kind, items);
                }
                "debugger" => {
                    let mut items = vec![self.punct()];
                    self.semicolon(&mut items);
                    return node("debugger_statement", items);
                }
                "return" => {
                    let mut items = vec![self.punct()];
                    let value = !self.at(";") && !self.asi_after(false);
                    if value {
                        items.push(self.expressions());
                    }
                    self.semicolon_after(&mut items, value);
                    return node("return_statement", items);
                }
                "throw" => {
                    let mut items = vec![self.punct(), self.expressions()];
                    self.semicolon_after(&mut items, true);
                    return node("throw_statement", items);
                }
                _ => {}
            }
            if (self.is_ident(0) || RESERVED_IDENTIFIERS.contains(&self.word())) && self.is(1, ":")
            {
                let items = vec![
                    self.leaf("statement_identifier").with("label"),
                    self.punct(),
                    self.statement().with("body"),
                ];
                return node("labeled_statement", items);
            }
        }
        if self.at("{") && !self.object_statement_at() {
            return self.statement_block();
        }
        if self.at(";") {
            return node("empty_statement", vec![self.punct()]);
        }
        let mut items = vec![self.expressions()];
        self.semicolon_after(&mut items, true);
        node("expression_statement", items)
    }

    fn statement_block(&mut self) -> Item {
        // A `for` head's `in` rule stops at a function body inside it.
        let saved = self.no_in;
        self.no_in = false;
        let mut items = vec![self.expect("{")];
        self.statements(&mut items, None);
        items.push(self.expect("}"));
        self.no_in = saved;
        // tree-sitter-typescript's block ends with an optional automatic
        // semicolon, which its scanner puts after a comment left on the
        // line: the block holds the comment.
        if self.ts() {
            self.trailing_comment(&mut items);
        }
        node("statement_block", items)
    }

    /// A zero-width mark after the comments left on the line, when an
    /// automatic semicolon could follow them.
    fn trailing_comment(&mut self, items: &mut Vec<Item>) {
        let t = self.tk(0);
        if !t.nl && t.k != L::Eof {
            return;
        }
        if !self.asi_after(false) {
            return;
        }
        let from = self.prev_end();
        let mut end = None;
        for e in &self.extras {
            if e.start >= from
                && e.end <= t.start
                && !self.src[from..e.start].contains(['\n', '\r', '\u{2028}', '\u{2029}'])
            {
                end = Some(end.map_or(e.end, |x: usize| x.max(e.end)));
            }
        }
        if let Some(end) = end {
            items.push(Item::Tok(Tok {
                kind: "",
                named: false,
                start: end,
                end,
                field: None,
                missing: false,
                extra: false,
                hidden: true,
            }));
        }
    }

    /// Statements up to a `}` (or, `case`, a `case` / `default` label),
    /// each with `field` if given.
    fn statements(&mut self, items: &mut Vec<Item>, field: Option<&'static str>) {
        while !self.at("}") && !self.eof() {
            if field.is_some() && (self.at("case") || (self.at("default") && self.is(1, ":"))) {
                break;
            }
            let before = self.i;
            let s = self.statement();
            items.push(match field {
                Some(f) => s.with(f),
                None => s,
            });
            if self.i == before {
                items.push(self.error_statement());
            }
        }
    }

    fn variable_declaration(&mut self) -> Item {
        let mut items = vec![self.punct()];
        let value = self.declarators(&mut items);
        self.semicolon_after(&mut items, value);
        node("variable_declaration", items)
    }

    fn lexical_declaration(&mut self) -> Item {
        let mut items = vec![self.punct().with("kind")];
        let value = self.declarators(&mut items);
        self.semicolon_after(&mut items, value);
        node("lexical_declaration", items)
    }

    fn using_declaration(&mut self) -> Item {
        let mut items = Vec::new();
        if self.at("await") {
            items.push(self.punct().with("kind"));
        }
        items.push(self.punct().with("kind"));
        let value = self.declarators(&mut items);
        self.semicolon_after(&mut items, value);
        node("using_declaration", items)
    }

    /// `: Type` after a name, in a JavaScript file that is TypeScript
    /// (the `js` backend reads `.ts` too): an `ERROR` up to what may follow
    /// the annotation, so the rest of the statement still reads.
    fn skip_type_annotation(&mut self, items: &mut Vec<Item>) {
        if !self.at(":") {
            return;
        }
        self.errors = true;
        let mut bad = Vec::new();
        let mut depth = 0usize;
        while !self.eof() {
            if depth == 0 && matches!(self.peek(), L::Op("=" | "," | ";" | ")" | "}" | "=>")) {
                break;
            }
            match self.peek() {
                L::Op("(" | "[" | "{" | "<") => depth += 1,
                L::Op(")" | "]" | "}" | ">") => depth = depth.saturating_sub(1),
                _ => {}
            }
            let t = self.advance();
            bad.push(Self::tok(t, "ERROR", false));
        }
        items.push(node("ERROR", bad));
    }

    /// Declarators into `items`: whether the last has a value.
    fn declarators(&mut self, items: &mut Vec<Item>) -> bool {
        loop {
            let mut d = vec![self.binding().with("name")];
            if self.ts() {
                if self.at("!") {
                    d.push(self.punct());
                }
                if self.at(":") {
                    d.push(self.type_annotation().with("type"));
                }
            } else {
                self.skip_type_annotation(&mut d);
            }
            let value = self.at("=");
            if value {
                d.push(self.punct());
                d.push(self.expression().with("value"));
            }
            items.push(node("variable_declarator", d));
            if !self.at(",") {
                return value;
            }
            items.push(self.punct());
        }
    }

    fn if_statement(&mut self) -> Item {
        let mut items = vec![
            self.punct(),
            self.parenthesized().with("condition"),
            self.statement().with("consequence"),
        ];
        if self.at("else") {
            let e = vec![self.punct(), self.statement()];
            items.push(node("else_clause", e).with("alternative"));
        }
        node("if_statement", items)
    }

    fn switch_statement(&mut self) -> Item {
        let kw = self.punct();
        let value = self.parenthesized().with("value");
        let mut body = vec![self.expect("{")];
        while !self.at("}") && !self.eof() {
            let before = self.i;
            if self.at("case") {
                let mut c = vec![
                    self.punct(),
                    self.expressions().with("value"),
                    self.expect(":"),
                ];
                self.statements(&mut c, Some("body"));
                body.push(node("switch_case", c));
            } else if self.at("default") {
                let mut c = vec![self.punct(), self.expect(":")];
                self.statements(&mut c, Some("body"));
                body.push(node("switch_default", c));
            } else {
                body.push(self.error_statement());
            }
            if self.i == before {
                break;
            }
        }
        body.push(self.expect("}"));
        node(
            "switch_statement",
            vec![kw, value, node("switch_body", body).with("body")],
        )
    }

    fn try_statement(&mut self) -> Item {
        let mut items = vec![self.punct(), self.statement_block().with("body")];
        if self.at("catch") {
            let mut c = vec![self.punct()];
            if self.at("(") {
                c.push(self.punct());
                c.push(self.binding().with("parameter"));
                if self.ts() && self.at(":") {
                    c.push(self.type_annotation().with("type"));
                }
                c.push(self.expect(")"));
            }
            c.push(self.statement_block().with("body"));
            items.push(node("catch_clause", c).with("handler"));
        }
        if self.at("finally") {
            let f = vec![self.punct(), self.statement_block().with("body")];
            items.push(node("finally_clause", f).with("finalizer"));
        }
        node("try_statement", items)
    }

    fn for_statement(&mut self) -> Item {
        let mut items = vec![self.punct()];
        let is_await = self.at("await");
        if is_await {
            items.push(self.punct());
        }
        items.push(self.expect("("));
        // `for (const x of xs)`, `for (x in o)`: a for-in head.
        let kw = self.word();
        let declares = matches!(kw, "var" | "let" | "const")
            && (self.is_ident(1) || self.is(1, "[") || self.is(1, "{"));
        let using = !self.ts()
            && ((kw == "using" && self.is_ident(1)) || (kw == "await" && self.is(1, "using")));
        if declares || using {
            // Look past the binding for `in` / `of`.
            let save = self.i;
            let mut head = Vec::new();
            if self.at("await") {
                head.push(self.punct().with("kind"));
            }
            head.push(self.punct().with("kind"));
            let left = self.binding().with("left");
            if self.at("in") || self.at("of") {
                items.extend(head);
                items.push(left);
                return self.for_in_rest(items);
            }
            if kw == "var" && self.at("=") {
                // `for (var x = 1 in o)`, old style.
                let eq = self.punct();
                let saved = self.no_in;
                self.no_in = true;
                let v = self.expression().with("value");
                self.no_in = saved;
                if self.at("in") {
                    items.extend(head);
                    items.push(left);
                    items.push(eq);
                    items.push(v);
                    return self.for_in_rest(items);
                }
            }
            self.i = save;
            let saved = self.no_in;
            self.no_in = true;
            let decl = if kw == "var" {
                self.variable_declaration()
            } else if using {
                self.using_declaration()
            } else {
                self.lexical_declaration()
            };
            self.no_in = saved;
            items.push(decl.with("initializer"));
        } else if self.at(";") {
            items.push(node("empty_statement", vec![self.punct()]).with("initializer"));
        } else {
            let saved = self.no_in;
            self.no_in = true;
            let e = self.expressions();
            self.no_in = saved;
            if self.at("in") || self.at("of") {
                items.push(to_pattern(e).with("left"));
                return self.for_in_rest(items);
            }
            items.push(e.with("initializer"));
            items.push(self.expect(";"));
        }
        if self.at(";") {
            items.push(node("empty_statement", vec![self.punct()]).with("condition"));
        } else {
            items.push(self.expressions().with("condition"));
            items.push(self.expect(";").with("condition"));
        }
        if !self.at(")") {
            items.push(self.expressions().with("increment"));
        }
        items.push(self.expect(")"));
        items.push(self.statement().with("body"));
        node("for_statement", items)
    }

    fn for_in_rest(&mut self, mut items: Vec<Item>) -> Item {
        items.push(self.punct().with("operator"));
        items.push(self.expressions().with("right"));
        items.push(self.expect(")"));
        items.push(self.statement().with("body"));
        node("for_in_statement", items)
    }

    // ---- modules -----------------------------------------------------------

    fn import_statement(&mut self) -> Item {
        let mut items = vec![self.punct()];
        if self.ts() && (self.at("type") || self.at("typeof")) && self.import_modifier_at(1) {
            items.push(self.punct());
        }
        if self.ts() && self.is_ident(0) && self.is(1, "=") && self.is(2, "require") {
            let mut c = vec![self.binding_identifier(), self.punct(), self.punct()];
            c.push(self.expect("("));
            c.push(self.string().with("source"));
            c.push(self.expect(")"));
            items.push(node("import_require_clause", c));
            self.semicolon(&mut items);
            return node("import_statement", items);
        }
        if self.peek() == L::Quote {
            items.push(self.string().with("source"));
        } else {
            let mut clause = Vec::new();
            if self.at("*") {
                clause.push(self.namespace_import());
            } else if self.at("{") {
                clause.push(self.named_imports());
            } else {
                clause.push(self.binding_identifier());
                if self.at(",") {
                    clause.push(self.punct());
                    clause.push(if self.at("*") {
                        self.namespace_import()
                    } else {
                        self.named_imports()
                    });
                }
            }
            items.push(node("import_clause", clause));
            items.push(self.expect("from"));
            items.push(self.string().with("source"));
        }
        if self.at("with") || self.at("assert") {
            let a = vec![self.punct(), self.object()];
            items.push(node("import_attribute", a));
        }
        self.semicolon(&mut items);
        node("import_statement", items)
    }

    /// After `import type`: whether `type` modifies the import (rather
    /// than being the default import's name).
    fn import_modifier_at(&mut self, n: usize) -> bool {
        if self.is(n, "{") || self.is(n, "*") {
            return true;
        }
        if self.is(n, "from") {
            return self.is(n + 1, "from");
        }
        self.is_ident(n) && !self.is(n, "=")
    }

    fn namespace_import(&mut self) -> Item {
        let items = vec![self.punct(), self.expect("as"), self.binding_identifier()];
        node("namespace_import", items)
    }

    fn named_imports(&mut self) -> Item {
        let mut items = vec![self.expect("{")];
        while !self.at("}") && !self.eof() {
            let before = self.i;
            let mut spec = Vec::new();
            self.specifier_modifier(&mut spec);
            let name = self.module_export_name();
            if self.at("as") {
                let a = self.punct();
                let alias = self.binding_identifier().with("alias");
                spec.extend([name.with("name"), a, alias]);
            } else {
                spec.push(
                    name.renamed_if("property_identifier", "identifier")
                        .with("name"),
                );
            }
            items.push(node("import_specifier", spec));
            if self.i == before || !self.at(",") {
                break;
            }
            items.push(self.punct());
        }
        items.push(self.expect("}"));
        node("named_imports", items)
    }

    /// TypeScript's `type` / `typeof` before an import or export
    /// specifier's name.
    fn specifier_modifier(&mut self, spec: &mut Vec<Item>) {
        if self.ts()
            && (self.at("type") || self.at("typeof"))
            && matches!(self.tk(1).k, L::Name | L::Quote)
            && !self.is(1, "as")
        {
            spec.push(self.punct());
        }
    }

    /// An identifier, a string, or `default`, naming an export
    /// (TypeScript's older base grammar reads `default` as an identifier).
    fn module_export_name(&mut self) -> Item {
        if self.peek() == L::Quote {
            return self.string();
        }
        if self.at("default") && !self.ts() {
            return self.punct();
        }
        if self.peek() == L::Name {
            return self.leaf("identifier");
        }
        self.missing("identifier", true)
    }

    fn export_statement(&mut self, mut items: Vec<Item>) -> Item {
        items.push(self.punct());
        if self.ts() {
            if self.at("=") {
                items.push(self.punct());
                items.push(self.expression());
                self.semicolon_after(&mut items, true);
                return node("export_statement", items);
            }
            if self.at("as") && self.is(1, "namespace") {
                items.push(self.punct());
                items.push(self.punct());
                items.push(self.binding_identifier());
                self.semicolon(&mut items);
                return node("export_statement", items);
            }
            if self.at("type") && self.is(1, "{") {
                items.push(self.punct());
            }
            if self.at("namespace") && (self.is_ident(1) || self.tk(1).k == L::Quote) {
                items.push(
                    self.module_declaration("internal_module")
                        .with("declaration"),
                );
                return node("export_statement", items);
            }
            if self.at("import") && self.is_ident(1) && self.is(2, "=") {
                items.push(self.statement().with("declaration"));
                return node("export_statement", items);
            }
        }
        if self.at("*") {
            let star = self.punct();
            if self.at("as") {
                let a = self.punct();
                let name = self.module_export_name();
                items.push(node("namespace_export", vec![star, a, name]));
            } else {
                items.push(star);
            }
            items.push(self.expect("from"));
            items.push(self.string().with("source"));
            self.semicolon(&mut items);
            return node("export_statement", items);
        }
        if self.at("{") {
            let mut clause = vec![self.punct()];
            while !self.at("}") && !self.eof() {
                let before = self.i;
                let mut s = Vec::new();
                self.specifier_modifier(&mut s);
                s.push(self.module_export_name().with("name"));
                if self.at("as") {
                    s.push(self.punct());
                    s.push(self.module_export_name().with("alias"));
                }
                clause.push(node("export_specifier", s));
                if self.i == before || !self.at(",") {
                    break;
                }
                clause.push(self.punct());
            }
            clause.push(self.expect("}"));
            items.push(node("export_clause", clause));
            if self.at("from") {
                items.push(self.punct());
                items.push(self.string().with("source"));
            }
            self.semicolon(&mut items);
            return node("export_statement", items);
        }
        if self.at("default") {
            items.push(self.punct());
            let named_fn = (self.at("function")
                && (self.is_ident(1) || (self.is(1, "*") && self.is_ident(2))))
                || (self.at("async")
                    && self.is(1, "function")
                    && (self.is_ident(2) || self.is(2, "*")));
            let named_class = self.at("class")
                && self.is_ident(1)
                && !self.is(1, "extends")
                && !self.is(1, "implements");
            let ts_declaration = self.ts()
                && ((self.at("interface") && self.is_ident(1))
                    || (self.at("abstract") && self.is(1, "class")));
            if named_fn || named_class || ts_declaration {
                items.push(self.statement().with("declaration"));
                return node("export_statement", items);
            }
            items.push(self.expression().with("value"));
            self.semicolon_after(&mut items, true);
            return node("export_statement", items);
        }
        if self.at("@") {
            let mut decorators = Vec::new();
            while self.at("@") {
                decorators.push(self.decorator().with("decorator"));
            }
            items.push(self.class(true, decorators).with("declaration"));
            return node("export_statement", items);
        }
        items.push(self.statement().with("declaration"));
        node("export_statement", items)
    }

    /// Decorators, then the class (or `export`) they decorate.
    fn decorated(&mut self) -> Item {
        let mut decorators = Vec::new();
        while self.at("@") {
            decorators.push(self.decorator().with("decorator"));
        }
        if self.at("export") {
            return self.export_statement(decorators);
        }
        self.class(true, decorators)
    }

    fn decorator(&mut self) -> Item {
        let at = self.punct();
        let mut e = self.binding_identifier();
        while self.at(".") && self.peek_name(1) {
            let dot = self.punct();
            let p = self.leaf("property_identifier").with("property");
            e = node("member_expression", vec![e.with("object"), dot, p]);
        }
        if self.at("(") {
            let args = self.arguments().with("arguments");
            e = node("call_expression", vec![e.with("function"), args]);
        }
        node("decorator", vec![at, e])
    }

    fn peek_name(&mut self, n: usize) -> bool {
        self.tk(n).k == L::Name
    }

    /// `a`, or `a.b.c`: a `nested_identifier` whose inner links are
    /// `member_expression`s (tree-sitter's alias).
    fn nested_identifier(&mut self) -> Item {
        let mut e = self.binding_identifier();
        while self.at(".") && self.peek_name(1) {
            let dot = self.punct();
            let p = self.leaf("property_identifier").with("property");
            e = node("member_expression", vec![e.with("object"), dot, p]);
        }
        e.renamed_if("member_expression", "nested_identifier")
    }

    // ---- functions and classes ---------------------------------------------

    /// `function` (async, generator), as a declaration or an expression.
    fn function(&mut self, declaration: bool) -> Item {
        let mut items = Vec::new();
        if self.at("async") {
            items.push(self.punct());
        }
        items.push(self.punct());
        let generator = self.at("*");
        if generator {
            items.push(self.punct());
        }
        if self.is_ident(0) {
            items.push(self.binding_identifier().with("name"));
        } else if declaration {
            items.push(self.missing("identifier", true).with("name"));
        }
        if self.ts() {
            self.call_signature(&mut items);
            // An overload's signature: no body.
            if declaration && !self.at("{") {
                self.semicolon(&mut items);
                return node("function_signature", items);
            }
        } else {
            items.push(self.formal_parameters().with("parameters"));
        }
        items.push(self.statement_block().with("body"));
        let kind = match (declaration, generator) {
            (true, false) => "function_declaration",
            (true, true) => "generator_function_declaration",
            (false, false) => "function_expression",
            (false, true) => "generator_function",
        };
        node(kind, items)
    }

    fn formal_parameters(&mut self) -> Item {
        let mut items = vec![self.expect("(")];
        while !self.at(")") && !self.eof() {
            let before = self.i;
            let p = if self.ts() {
                self.ts_parameter()
            } else {
                self.parameter()
            };
            items.push(p);
            if self.i == before || !self.at(",") {
                break;
            }
            items.push(self.punct());
        }
        items.push(self.expect(")"));
        node("formal_parameters", items)
    }

    /// A formal parameter: a pattern, a rest pattern, or one with a
    /// default.
    fn parameter(&mut self) -> Item {
        if self.at("...") {
            let dots = self.punct();
            let p = self.binding();
            return node("rest_pattern", vec![dots, p]);
        }
        let p = self.binding();
        if self.at(":") {
            let mut items = vec![p];
            self.skip_type_annotation(&mut items);
            return node("ERROR", items);
        }
        if self.at("=") {
            let eq = self.punct();
            let v = self.expression().with("right");
            return node("assignment_pattern", vec![p.with("left"), eq, v]);
        }
        p
    }

    fn class(&mut self, declaration: bool, mut items: Vec<Item>) -> Item {
        if self.ts() {
            let kind = if declaration {
                "class_declaration"
            } else {
                "class"
            };
            return self.class_ts(declaration, items, kind);
        }
        items.push(self.punct());
        let named = self.is_ident(0) && !self.at("extends");
        if named {
            items.push(self.binding_identifier().with("name"));
        } else if declaration {
            items.push(self.missing("identifier", true).with("name"));
        }
        if self.at("extends") {
            let h = vec![self.punct(), self.postfix()];
            items.push(node("class_heritage", h));
        }
        items.push(self.class_body().with("body"));
        node(
            if declaration {
                "class_declaration"
            } else {
                "class"
            },
            items,
        )
    }

    fn class_body(&mut self) -> Item {
        let mut items = vec![self.expect("{")];
        while !self.at("}") && !self.eof() {
            let before = self.i;
            if self.at(";") {
                items.push(self.punct());
                continue;
            }
            if self.at("static") && self.is(1, "{") {
                let s = vec![self.punct(), self.statement_block().with("body")];
                items.push(node("class_static_block", s).with("member"));
                continue;
            }
            let member = self.class_member();
            let is_method = member.kind() == "method_definition";
            let valued = matches!(&member, Item::Node(n) if n.items.last().is_some_and(|i| i.field() == Some("value")));
            items.push(member.with("member"));
            if is_method {
                if self.at(";") {
                    items.push(self.punct());
                }
            } else {
                self.semicolon_after(&mut items, valued);
            }
            if self.i == before {
                items.push(self.error_statement());
            }
        }
        items.push(self.expect("}"));
        node("class_body", items)
    }

    fn class_member(&mut self) -> Item {
        let mut items = Vec::new();
        while self.at("@") {
            items.push(self.decorator().with("decorator"));
        }
        if self.at("static") && self.is_modifier_before_name(1) {
            items.push(self.punct());
        }
        self.method_modifiers(&mut items);
        let name = self.property_name();
        if self.at("(") {
            items.push(name.with("name"));
            items.push(self.formal_parameters().with("parameters"));
            items.push(self.statement_block().with("body"));
            return node("method_definition", items);
        }
        items.push(name.with("property"));
        if self.at("=") {
            items.push(self.punct());
            items.push(self.expression().with("value"));
        }
        node("field_definition", items)
    }

    /// Whether the word before token `n` is a modifier (the member's name
    /// follows), not the member's name itself.
    fn is_modifier_before_name(&mut self, n: usize) -> bool {
        matches!(
            self.tk(n).k,
            L::Name | L::Private | L::Number | L::Quote | L::Op("[" | "*")
        )
    }

    /// `async`, `get` / `set`, `*` before a method's name.
    fn method_modifiers(&mut self, items: &mut Vec<Item>) {
        if self.at("async") && self.is_modifier_before_name(1) && !self.tk(1).nl {
            items.push(self.punct());
        }
        let accessor = (self.at("get") || self.at("set"))
            && self.is_modifier_before_name(1)
            && !self.is(1, "*");
        if accessor || self.at("*") {
            items.push(self.punct());
        }
    }

    /// A property's name: an identifier (any word), a private name, a
    /// string, a number, or `[computed]`.
    fn property_name(&mut self) -> Item {
        match self.peek() {
            L::Name => self.leaf("property_identifier"),
            L::Private => self.leaf("private_property_identifier"),
            L::Quote => self.string(),
            L::Number => self.leaf("number"),
            L::Op("[") => {
                let items = vec![self.punct(), self.expression(), self.expect("]")];
                node("computed_property_name", items)
            }
            _ => self.missing("property_identifier", true),
        }
    }

    // ---- patterns ----------------------------------------------------------

    /// A binding pattern: a name, or an object or array to destructure.
    fn binding(&mut self) -> Item {
        match self.peek() {
            L::Op("[") => self.array_pattern(),
            L::Op("{") => self.object_pattern(),
            _ => self.binding_identifier(),
        }
    }

    fn array_pattern(&mut self) -> Item {
        let mut items = vec![self.punct()];
        while !self.at("]") && !self.eof() {
            if self.at(",") {
                items.push(self.punct());
                continue;
            }
            let before = self.i;
            items.push(self.parameter());
            if self.i == before {
                break;
            }
            if !self.at(",") {
                break;
            }
            items.push(self.punct());
        }
        items.push(self.expect("]"));
        node("array_pattern", items)
    }

    fn object_pattern(&mut self) -> Item {
        let mut items = vec![self.punct()];
        while !self.at("}") && !self.eof() {
            if self.at(",") {
                items.push(self.punct());
                continue;
            }
            let before = self.i;
            let member = if self.at("...") {
                let dots = self.punct();
                let p = self.binding();
                node("rest_pattern", vec![dots, p])
            } else if self.is(1, ":") || self.at("[") || matches!(self.peek(), L::Quote | L::Number)
            {
                let key = self.property_name().with("key");
                let colon = self.expect(":");
                let value = self.parameter().with("value");
                node("pair_pattern", vec![key, colon, value])
            } else {
                let name = self.leaf("shorthand_property_identifier_pattern");
                if self.at("=") {
                    let eq = self.punct();
                    let v = self.expression().with("right");
                    node("object_assignment_pattern", vec![name.with("left"), eq, v])
                } else {
                    name
                }
            };
            items.push(member);
            if self.i == before || !self.at(",") {
                break;
            }
            items.push(self.punct());
        }
        items.push(self.expect("}"));
        node("object_pattern", items)
    }

    // ---- expressions -------------------------------------------------------

    /// An expression, or several as a `sequence_expression`.
    fn expressions(&mut self) -> Item {
        let first = self.expression();
        if !self.at(",") {
            return first;
        }
        let mut items = vec![first];
        while self.at(",") {
            items.push(self.punct());
            items.push(self.expression());
        }
        node("sequence_expression", items)
    }

    fn expression(&mut self) -> Item {
        let arrow = if self.ts() {
            self.ts_arrow_at(0)
        } else {
            self.arrow_at(0)
        };
        if arrow {
            return self.arrow();
        }
        if self.at("yield") && !self.is(1, "=") && !self.is(1, ")") {
            return self.yield_expression();
        }
        if self.ts() && self.at("using") && self.is_ident(1) && self.is(2, "=") {
            let using = self.punct();
            let left = self.binding_identifier().with("left");
            let eq = self.punct();
            let right = self.expression().with("right");
            return node("assignment_expression", vec![using, left, eq, right]);
        }
        let left = self.ternary();
        if self.at("=") {
            let eq = self.punct();
            let right = self.expression().with("right");
            return node(
                "assignment_expression",
                vec![to_pattern(left).with("left"), eq, right],
            );
        }
        if let L::Op(o) = self.peek()
            && AUGMENTED.contains(&o)
        {
            let op = self.punct().with("operator");
            let right = self.expression().with("right");
            return node(
                "augmented_assignment_expression",
                vec![left.with("left"), op, right],
            );
        }
        left
    }

    fn yield_expression(&mut self) -> Item {
        let mut items = vec![self.punct()];
        if self.at("*") {
            items.push(self.punct());
            items.push(self.expression());
        } else if !self.asi_after(false)
            && !matches!(self.peek(), L::Op(")" | "]" | "}" | "," | ";" | ":"))
            && !self.tk(0).nl
        {
            items.push(self.expression());
        }
        node("yield_expression", items)
    }

    fn arrow(&mut self) -> Item {
        let mut items = Vec::new();
        if self.at("async") && !self.is(1, "=>") {
            items.push(self.punct());
        }
        if self.at("(") || (self.ts() && self.at("<")) {
            if self.ts() {
                self.call_signature(&mut items);
            } else {
                items.push(self.formal_parameters().with("parameters"));
            }
        } else {
            items.push(self.leaf("identifier").with("parameter"));
        }
        items.push(self.expect("=>"));
        let body = if self.at("{") {
            self.statement_block()
        } else {
            self.expression()
        };
        items.push(body.with("body"));
        node("arrow_function", items)
    }

    fn ternary(&mut self) -> Item {
        let cond = self.binary(1);
        if !self.at("?") {
            return cond;
        }
        let q = self.punct();
        let saved = self.no_in;
        self.no_in = false;
        let then = self.expression().with("consequence");
        self.no_in = saved;
        let colon = self.expect(":");
        let alt = self.expression().with("alternative");
        node(
            "ternary_expression",
            vec![cond.with("condition"), q, then, colon, alt],
        )
    }

    /// The binary operator here and its precedence.
    fn binop(&mut self) -> Option<(u8, bool)> {
        let t = self.tk(0);
        if t.k == L::Regex {
            // A `/` the lookahead took for a regex: a division here.
            self.relex(Mode::Code { regex: false });
        }
        let t = self.tk(0);
        let prec = match t.k {
            L::Op(o) => match o {
                "??" => 1,
                "||" => 2,
                "&&" => 3,
                "|" => 4,
                "^" => 5,
                "&" => 6,
                "==" | "===" | "!=" | "!==" => 7,
                "<" | "<=" | ">" | ">=" => 8,
                "<<" | ">>" | ">>>" => 9,
                "+" | "-" => 10,
                "*" | "/" | "%" => 11,
                "**" => return Some((12, true)),
                _ => return None,
            },
            L::Name => match self.text(t) {
                "instanceof" => 8,
                "in" if !self.no_in => 8,
                "as" | "satisfies" if self.dialect != Dialect::Js && !t.nl => 8,
                _ => return None,
            },
            _ => return None,
        };
        Some((prec, false))
    }

    fn binary(&mut self, min: u8) -> Item {
        let mut left = if self.peek() == L::Private && self.is(1, "in") {
            self.leaf("private_property_identifier")
        } else {
            self.unary()
        };
        while let Some((prec, right_assoc)) = self.binop() {
            if prec < min {
                break;
            }
            if self.ts() && (self.at("as") || self.at("satisfies")) {
                let kind = if self.at("as") {
                    "as_expression"
                } else {
                    "satisfies_expression"
                };
                let kw = self.punct();
                let t = if kind == "as_expression" && self.at("const") {
                    self.punct()
                } else {
                    self.ty()
                };
                left = node(kind, vec![left, kw, t]);
                continue;
            }
            let op = self.punct().with("operator");
            let right = self.binary(if right_assoc { prec } else { prec + 1 });
            left = node(
                "binary_expression",
                vec![left.with("left"), op, right.with("right")],
            );
        }
        left
    }

    fn unary(&mut self) -> Item {
        let w = self.word();
        if matches!(self.peek(), L::Op("!" | "~" | "-" | "+"))
            || (matches!(w, "typeof" | "void" | "delete"))
        {
            let op = self.punct().with("operator");
            let arg = self.unary().with("argument");
            return node("unary_expression", vec![op, arg]);
        }
        if w == "await" && self.await_operand_at(1) {
            let kw = self.punct();
            // TypeScript's base grammar has `using` only as an
            // assignment's prefix: `await using x = f()`.
            let arg = if self.ts() && self.at("using") && self.is_ident(1) && self.is(2, "=") {
                let using = self.punct();
                let left = self.binding_identifier().with("left");
                let eq = self.punct();
                let right = self.expression().with("right");
                node("assignment_expression", vec![using, left, eq, right])
            } else {
                self.unary()
            };
            return node("await_expression", vec![kw, arg]);
        }
        if self.at("++") || self.at("--") {
            let op = self.punct().with("operator");
            let arg = self.unary().with("argument");
            return node("update_expression", vec![op, arg]);
        }
        if self.dialect == Dialect::Ts && self.at("<") {
            let args = self.type_arguments();
            let e = self.unary();
            return node("type_assertion", vec![args, e]);
        }
        let mut e = self.postfix();
        while (self.at("++") || self.at("--")) && !self.tk(0).nl {
            let op = self.punct().with("operator");
            e = node("update_expression", vec![e.with("argument"), op]);
        }
        e
    }

    /// After `await`: whether an operand follows (else `await` is a name).
    fn await_operand_at(&mut self, n: usize) -> bool {
        let t = self.tk(n);
        match t.k {
            L::Name => !matches!(self.text(t), "in" | "instanceof" | "of"),
            L::Number | L::Quote | L::Backtick | L::Regex | L::Private => true,
            L::Op(o) => matches!(
                o,
                "(" | "[" | "{" | "!" | "~" | "-" | "+" | "++" | "--" | "/" | "<"
            ),
            _ => false,
        }
    }

    /// A primary expression and its member accesses, calls, subscripts
    /// and template calls.
    fn postfix(&mut self) -> Item {
        let mut e = self.primary();
        loop {
            e = match self.peek() {
                L::Op(".") => {
                    let dot = self.punct();
                    let p = self.member_property();
                    node("member_expression", vec![e.with("object"), dot, p])
                }
                L::Op("?.") if self.ts() && (self.is(1, "(") || self.is(1, "<")) => {
                    // TypeScript's optional call: a bare `?.`.
                    let q = self.punct();
                    let mut items = vec![e.with("function"), q];
                    if self.at("<") {
                        items.push(self.type_arguments().with("type_arguments"));
                    }
                    items.push(self.arguments().with("arguments"));
                    node("call_expression", items)
                }
                L::Op("!") if self.ts() && !self.tk(0).nl => {
                    let bang = self.punct();
                    node("non_null_expression", vec![e, bang])
                }
                L::Op("<") if self.ts() => match self.scan_type_arguments(0) {
                    Some(k) if self.is(k, "(") => {
                        let args = self.type_arguments().with("type_arguments");
                        let call = self.arguments().with("arguments");
                        node("call_expression", vec![e.with("function"), args, call])
                    }
                    Some(k) if self.instantiation_follows(k) => {
                        let args = self.type_arguments().with("type_arguments");
                        node("instantiation_expression", vec![e, args])
                    }
                    _ => return e,
                },
                L::Op("?.") => {
                    // tree-sitter-typescript's older base grammar wraps the
                    // token; tree-sitter-javascript 0.25's is a leaf.
                    let q = if self.ts() {
                        node("optional_chain", vec![self.punct()])
                    } else {
                        self.leaf("optional_chain")
                    }
                    .with("optional_chain");
                    if self.at("(") {
                        let args = self.arguments().with("arguments");
                        node("call_expression", vec![e.with("function"), q, args])
                    } else if self.at("[") {
                        let open = self.punct();
                        let index = self.expressions().with("index");
                        let close = self.expect("]");
                        node(
                            "subscript_expression",
                            vec![e.with("object"), q, open, index, close],
                        )
                    } else {
                        let p = self.member_property();
                        node("member_expression", vec![e.with("object"), q, p])
                    }
                }
                L::Op("[") => {
                    let open = self.punct();
                    let saved = self.no_in;
                    self.no_in = false;
                    let index = self.expressions().with("index");
                    self.no_in = saved;
                    let close = self.expect("]");
                    node(
                        "subscript_expression",
                        vec![e.with("object"), open, index, close],
                    )
                }
                L::Op("(") => {
                    let args = self.arguments().with("arguments");
                    node("call_expression", vec![e.with("function"), args])
                }
                L::Backtick => {
                    let t = self.template().with("arguments");
                    node("call_expression", vec![e.with("function"), t])
                }
                _ => return e,
            };
        }
    }

    /// After type arguments that end before token `n`: whether they make an
    /// instantiation expression (`f<T>;`), not a comparison (`a < b > c`).
    fn instantiation_follows(&mut self, n: usize) -> bool {
        let t = self.tk(n);
        t.nl || matches!(
            t.k,
            L::Eof | L::Op(")" | "]" | "}" | "," | ";" | "." | "?." | ":" | "=")
        )
    }

    fn member_property(&mut self) -> Item {
        match self.peek() {
            L::Private => self.leaf("private_property_identifier").with("property"),
            L::Name => self.leaf("property_identifier").with("property"),
            _ => self.missing("property_identifier", true).with("property"),
        }
    }

    fn arguments(&mut self) -> Item {
        let mut items = vec![self.punct()];
        let saved = self.no_in;
        self.no_in = false;
        while !self.at(")") && !self.eof() {
            if self.at(",") {
                items.push(self.punct());
                continue;
            }
            let before = self.i;
            items.push(self.element());
            if self.i == before {
                break;
            }
            if !self.at(",") {
                break;
            }
            items.push(self.punct());
        }
        self.no_in = saved;
        items.push(self.expect(")"));
        node("arguments", items)
    }

    /// An element of an array or argument list: an expression or a spread.
    fn element(&mut self) -> Item {
        if self.at("...") {
            let dots = self.punct();
            let e = self.expression();
            return node("spread_element", vec![dots, e]);
        }
        self.expression()
    }

    fn primary(&mut self) -> Item {
        if matches!(self.peek(), L::Op("/" | "/=")) {
            self.relex(Mode::Code { regex: true });
        }
        match self.peek() {
            L::Number => self.leaf("number"),
            L::Quote => self.string(),
            L::Backtick => self.template(),
            L::Regex => self.regex(),
            L::Private => self.leaf("private_property_identifier"),
            L::Op("(") => self.parenthesized(),
            L::Op("[") => self.array(),
            L::Op("{") => self.object(),
            L::Op("<") if self.jsx() => self.jsx_element(),
            L::Op("@") => {
                let mut decorators = Vec::new();
                while self.at("@") {
                    decorators.push(self.decorator().with("decorator"));
                }
                self.class(false, decorators)
            }
            L::Name => match self.word() {
                "this" => self.leaf("this"),
                "super" => self.leaf("super"),
                "true" => self.leaf("true"),
                "false" => self.leaf("false"),
                "null" => self.leaf("null"),
                "undefined" => self.leaf("undefined"),
                "function" => self.function(false),
                "async" if self.is(1, "function") && !self.tk(1).nl => self.function(false),
                "class" => self.class(false, Vec::new()),
                "new" => self.new_expression(),
                "import" => {
                    if self.is(1, ".") {
                        let items = vec![self.punct(), self.punct(), self.leaf_word()];
                        return node("meta_property", items);
                    }
                    // `import(…)`: a node over the keyword.
                    node("import", vec![self.punct()])
                }
                w if KEYWORDS.contains(&w) => self.missing("identifier", true),
                _ => self.identifier(),
            },
            _ => self.missing("identifier", true),
        }
    }

    /// `target` / `meta` after `new.` / `import.`.
    fn leaf_word(&mut self) -> Item {
        if self.peek() == L::Name {
            self.punct()
        } else {
            self.missing("meta", false)
        }
    }

    fn new_expression(&mut self) -> Item {
        let kw = self.punct();
        if self.at(".") {
            let items = vec![kw, self.punct(), self.leaf_word()];
            return node("meta_property", items);
        }
        let constructor = if self.at("new") {
            self.new_expression()
        } else {
            // A primary with member accesses (no calls: `(` is the new's).
            let mut e = self.primary();
            loop {
                e = match self.peek() {
                    L::Op(".") => {
                        let dot = self.punct();
                        let p = self.member_property();
                        node("member_expression", vec![e.with("object"), dot, p])
                    }
                    L::Op("[") => {
                        let open = self.punct();
                        let index = self.expressions().with("index");
                        let close = self.expect("]");
                        node(
                            "subscript_expression",
                            vec![e.with("object"), open, index, close],
                        )
                    }
                    L::Backtick => {
                        let t = self.template().with("arguments");
                        node("call_expression", vec![e.with("function"), t])
                    }
                    _ => break e,
                };
            }
        };
        let mut items = vec![kw, constructor.with("constructor")];
        if self.ts() && self.at("<") && self.scan_type_arguments(0).is_some() {
            items.push(self.type_arguments().with("type_arguments"));
        }
        if self.at("(") {
            items.push(self.arguments().with("arguments"));
        }
        node("new_expression", items)
    }

    fn parenthesized(&mut self) -> Item {
        let open = self.expect("(");
        let saved = self.no_in;
        self.no_in = false;
        let e = self.expressions();
        self.no_in = saved;
        let close = self.expect(")");
        node("parenthesized_expression", vec![open, e, close])
    }

    fn array(&mut self) -> Item {
        let mut items = vec![self.punct()];
        let saved = self.no_in;
        self.no_in = false;
        while !self.at("]") && !self.eof() {
            if self.at(",") {
                items.push(self.punct());
                continue;
            }
            let before = self.i;
            items.push(self.element());
            if self.i == before || !self.at(",") {
                break;
            }
            items.push(self.punct());
        }
        self.no_in = saved;
        items.push(self.expect("]"));
        node("array", items)
    }

    fn object(&mut self) -> Item {
        let mut items = vec![self.expect("{")];
        let saved = self.no_in;
        self.no_in = false;
        while !self.at("}") && !self.eof() {
            if self.at(",") {
                items.push(self.punct());
                continue;
            }
            let before = self.i;
            items.push(self.object_member());
            if self.i == before || !self.at(",") {
                break;
            }
            items.push(self.punct());
        }
        self.no_in = saved;
        items.push(self.expect("}"));
        node("object", items)
    }

    fn object_member(&mut self) -> Item {
        if self.at("...") {
            let dots = self.punct();
            let e = self.expression();
            return node("spread_element", vec![dots, e]);
        }
        // A shorthand: a name and nothing more (or, in a pattern to be,
        // a default).
        if self.peek() == L::Name && matches!(self.tk(1).k, L::Op("," | "}" | "=")) {
            let name = self.leaf("shorthand_property_identifier");
            if self.at("=") {
                let eq = self.punct();
                let v = self.expression().with("right");
                return node(
                    "object_assignment_pattern",
                    vec![
                        name.renamed("shorthand_property_identifier_pattern")
                            .with("left"),
                        eq,
                        v,
                    ],
                );
            }
            return name;
        }
        let mut items = Vec::new();
        self.method_modifiers(&mut items);
        let key = self.property_name();
        if self.at("(") || (self.ts() && self.at("<")) {
            items.push(key.with("name"));
            if self.ts() {
                self.call_signature(&mut items);
            } else {
                items.push(self.formal_parameters().with("parameters"));
            }
            items.push(self.statement_block().with("body"));
            return node("method_definition", items);
        }
        if !items.is_empty() {
            // `get: 1`: the modifier was the key.
            self.errors = true;
        }
        let colon = self.expect(":");
        let value = self.expression().with("value");
        node("pair", vec![key.with("key"), colon, value])
    }

    fn string(&mut self) -> Item {
        let open = self.advance();
        let quote = &self.src[open.start..open.end];
        let qkind = if quote == "'" { "'" } else { "\"" };
        let mut items = vec![Self::tok(open, qkind, false)];
        loop {
            match self.peek() {
                L::Frag => items.push(self.leaf("string_fragment")),
                L::Escape => items.push(self.leaf("escape_sequence")),
                L::CharRef => items.push(self.leaf("html_character_reference")),
                L::Quote => {
                    let t = self.advance();
                    if t.end > t.start {
                        items.push(Self::tok(t, qkind, false));
                    } else {
                        items.push(self.missing(qkind, false));
                    }
                    break;
                }
                _ => {
                    items.push(self.missing(qkind, false));
                    break;
                }
            }
        }
        node("string", items)
    }

    fn template(&mut self) -> Item {
        let mut items = vec![self.leaf_as("`")];
        loop {
            match self.peek() {
                L::Frag => items.push(self.leaf("string_fragment")),
                L::Escape => items.push(self.leaf("escape_sequence")),
                L::Op("${") => {
                    let mut s = vec![self.punct()];
                    let saved = self.no_in;
                    self.no_in = false;
                    s.push(self.expressions());
                    self.no_in = saved;
                    s.push(self.expect("}"));
                    items.push(node("template_substitution", s));
                }
                L::Backtick => {
                    let t = self.advance();
                    if t.end > t.start {
                        items.push(Self::tok(t, "`", false));
                    } else {
                        items.push(self.missing("`", false));
                    }
                    break;
                }
                _ => {
                    items.push(self.missing("`", false));
                    break;
                }
            }
        }
        node("template_string", items)
    }

    fn leaf_as(&mut self, kind: &'static str) -> Item {
        let t = self.advance();
        Self::tok(t, kind, false)
    }

    fn regex(&mut self) -> Item {
        let t = self.advance();
        let mut items = vec![Item::Tok(Tok {
            kind: "/",
            named: false,
            start: t.start,
            end: t.start + 1,
            field: None,
            missing: false,
            extra: false,
            hidden: false,
        })];
        items.push(Item::Tok(Tok {
            kind: "regex_pattern",
            named: true,
            start: t.start + 1,
            end: t.aux,
            field: Some("pattern"),
            missing: false,
            extra: false,
            hidden: false,
        }));
        items.push(Item::Tok(Tok {
            kind: "/",
            named: false,
            start: t.aux,
            end: t.aux + 1,
            field: None,
            missing: false,
            extra: false,
            hidden: false,
        }));
        if t.end > t.aux + 1 {
            items.push(Item::Tok(Tok {
                kind: "regex_flags",
                named: true,
                start: t.aux + 1,
                end: t.end,
                field: Some("flags"),
                missing: false,
                extra: false,
                hidden: false,
            }));
        }
        node("regex", items)
    }

    // ---- JSX -----------------------------------------------------------------

    /// A JSX element at `<`.
    fn jsx_element(&mut self) -> Item {
        let open = self.punct();
        self.relex(Mode::JsxTag);
        if self.at(">") {
            // A fragment: `<>…</>`.
            let close = self.punct();
            let opening = node("jsx_opening_element", vec![open, close]);
            return self.jsx_children(opening);
        }
        let mut items = vec![open, self.jsx_element_name().with("name")];
        // TSX: `<Select<Option> …>`.
        self.relex(Mode::JsxTag);
        if self.ts() && self.at("<") {
            items.push(self.type_arguments().with("type_arguments"));
        }
        loop {
            self.relex(Mode::JsxTag);
            if self.at("/>") {
                items.push(self.punct());
                return node("jsx_self_closing_element", items);
            }
            if self.at(">") {
                items.push(self.punct());
                let opening = node("jsx_opening_element", items);
                return self.jsx_children(opening);
            }
            if self.at("{") {
                items.push(self.jsx_expression().with("attribute"));
                continue;
            }
            if self.peek() == L::Name {
                items.push(self.jsx_attribute().with("attribute"));
                continue;
            }
            items.push(self.missing(">", false));
            return node("jsx_opening_element", items);
        }
    }

    fn jsx_children(&mut self, opening: Item) -> Item {
        let mut items = vec![opening.with("open_tag")];
        loop {
            self.relex(Mode::JsxText);
            match self.peek() {
                L::JsxText => items.push(self.leaf("jsx_text")),
                L::CharRef => items.push(self.leaf("html_character_reference")),
                L::Op("{") => items.push(self.jsx_expression()),
                L::Op("</") => {
                    let mut c = vec![self.punct()];
                    self.relex(Mode::JsxTag);
                    if !self.at(">") {
                        c.push(self.jsx_element_name().with("name"));
                        self.relex(Mode::JsxTag);
                    }
                    c.push(self.expect(">"));
                    items.push(node("jsx_closing_element", c).with("close_tag"));
                    return node("jsx_element", items);
                }
                L::Op("<") => items.push(self.jsx_element()),
                _ => {
                    items.push(self.missing("</", false));
                    return node("jsx_element", items);
                }
            }
        }
    }

    /// A tag's name: `div`, `a.b.c`, `svg:rect`.
    fn jsx_element_name(&mut self) -> Item {
        let first = self.jsx_identifier();
        self.relex(Mode::JsxTag);
        if self.at(":") {
            let colon = self.punct();
            self.relex(Mode::JsxTag);
            let second = self.jsx_identifier();
            return node("jsx_namespace_name", vec![first, colon, second]);
        }
        let mut e = first;
        while self.at(".") {
            let dot = self.punct();
            self.relex(Mode::JsxTag);
            let p = if self.peek() == L::Name {
                self.leaf("property_identifier")
            } else {
                self.missing("property_identifier", true)
            };
            e = node(
                "member_expression",
                vec![e.with("object"), dot, p.with("property")],
            );
            self.relex(Mode::JsxTag);
        }
        e
    }

    fn jsx_identifier(&mut self) -> Item {
        if self.peek() == L::Name {
            self.leaf("identifier")
        } else {
            self.missing("identifier", true)
        }
    }

    fn jsx_attribute(&mut self) -> Item {
        let first = self.leaf("property_identifier");
        self.relex(Mode::JsxTag);
        let name = if self.at(":") {
            let colon = self.punct();
            self.relex(Mode::JsxTag);
            let second = self.jsx_identifier();
            node(
                "jsx_namespace_name",
                vec![first.renamed("identifier"), colon, second],
            )
        } else {
            first
        };
        let mut items = vec![name];
        self.relex(Mode::JsxTag);
        if self.at("=") {
            items.push(self.punct());
            self.relex(Mode::JsxTag);
            items.push(match self.peek() {
                L::Quote => self.string(),
                L::Op("{") => self.jsx_expression(),
                L::Op("<") => self.jsx_element(),
                _ => self.missing("string", true),
            });
        }
        node("jsx_attribute", items)
    }

    /// `{expression}` in JSX (empty, or a spread, too).
    fn jsx_expression(&mut self) -> Item {
        let mut items = vec![self.punct()];
        // The braces' contents are code.
        self.relex(Mode::Code { regex: true });
        if !self.at("}") {
            if self.at("...") {
                let dots = self.punct();
                let e = self.expression();
                items.push(node("spread_element", vec![dots, e]));
            } else {
                items.push(self.expressions());
            }
        }
        items.push(self.expect("}"));
        node("jsx_expression", items)
    }
}

impl Item {
    /// `self` renamed to `to` if its kind is `from`.
    fn renamed_if(self, from: &'static str, to: &'static str) -> Self {
        if self.kind() == from {
            self.renamed(to)
        } else {
            self
        }
    }
}

/// An expression read where a pattern belongs (an assignment's left side,
/// a `for … in`'s), as that pattern: objects and arrays destructure.
fn to_pattern(item: Item) -> Item {
    let kind = match item.kind() {
        "object" => "object_pattern",
        "array" => "array_pattern",
        _ => return item,
    };
    let Item::Node(n) = item else {
        return item;
    };
    let items = n
        .items
        .into_iter()
        .map(|i| element_pattern(i, kind == "object_pattern"))
        .collect();
    Item::Node(Node {
        kind,
        field: n.field,
        items,
    })
}

/// One element of an object or array being made a pattern.
fn element_pattern(item: Item, in_object: bool) -> Item {
    if !item.is_named() {
        return item;
    }
    let field = item.field();
    let out = match item.kind() {
        "shorthand_property_identifier" => item.renamed("shorthand_property_identifier_pattern"),
        "spread_element" => match item {
            Item::Node(n) => {
                let items = n.items.into_iter().map(to_pattern).collect();
                node("rest_pattern", items)
            }
            other => other,
        },
        "pair" if in_object => match item {
            Item::Node(n) => {
                let items = n
                    .items
                    .into_iter()
                    .map(|i| {
                        if i.field() == Some("value") {
                            element_pattern(i, false).with("value")
                        } else {
                            i
                        }
                    })
                    .collect();
                node("pair_pattern", items)
            }
            other => other,
        },
        "assignment_expression" => match item {
            Item::Node(n) => {
                let items = n
                    .items
                    .into_iter()
                    .map(|i| {
                        if i.field() == Some("left") {
                            to_pattern(i)
                        } else {
                            i
                        }
                    })
                    .collect();
                node("assignment_pattern", items)
            }
            other => other,
        },
        _ => to_pattern(item),
    };
    match field {
        Some(f) => out.with(f),
        None => out,
    }
}
