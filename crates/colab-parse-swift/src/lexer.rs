//! Swift's tokens, lexed up front.
//!
//! Each token records whether a line ends between it and the token or
//! comment before it (`nl`), which tree-sitter-swift's scanner reads to
//! insert a statement's semicolon. Operators are Swift's: a run of operator
//! characters is one operator (a `.` only in a run that starts with one),
//! a fixed one of the grammar's or a custom one (the parser splits a `>`
//! off one where type arguments close). Strings are cut into text,
//! escapes and interpolations (`\(…)`, `\#(…)` in a raw string), whose code
//! is lexed as code to its closing parenthesis. Comments are extras, kept
//! apart; block comments nest.

/// A token's kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum L {
    /// A name or keyword (`` `quoted` ``, `$0` and `$name` too).
    Word,
    Int,
    Hex,
    Oct,
    Bin,
    Real,
    /// One of the grammar's operators or punctuation.
    Op(&'static str),
    /// Any other run of operator characters.
    Custom,
    /// `#if`, `#elseif`, `#else`, `#endif`.
    Directive(&'static str),
    /// A string's opening (`"` or `"""`).
    StrOpen(&'static str),
    StrClose(&'static str),
    /// A string's text.
    Text,
    /// `\n` and the other one-character escapes.
    Escape,
    /// `\u{…}`.
    Unicode,
    /// `\(`.
    InterpOpen,
    /// A raw string's text before an interpolation (from `#"`).
    RawPart,
    /// A raw string's last text, to its closing `"#`.
    RawEnd,
    /// `\#(`.
    RawInterp,
    /// `#!…` at the start.
    Shebang,
    /// A `#warning` / `#error` / `#sourceLocation` line after the `#`.
    DiagTail,
    Error,
    Eof,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Lx {
    pub k: L,
    pub start: usize,
    pub end: usize,
    /// A line ends between this token and what is before it.
    pub nl: bool,
}

pub(crate) struct Lexed {
    pub tokens: Vec<Lx>,
    /// Comments, in order: (start, end, block).
    pub extras: Vec<(usize, usize, bool)>,
    pub errors: bool,
}

/// The grammar's own operators; any other run is a custom one.
const FIXED: &[&str] = &[
    "!", "!=", "!==", "%", "%=", "&", "&&", "*", "*=", "+", "++", "+=", "-", "--", "-=", "->", ".",
    "...", "..<", "/", "/=", "<", "<<", "<=", "=", "==", "===", ">", ">=", ">>", "?", "??", "^",
    "|", "||", "~",
];

/// An operator run's kind: a fixed operator's, else custom.
pub(crate) fn fixed(run: &str) -> L {
    FIXED
        .iter()
        .find(|f| **f == run)
        .map_or(L::Custom, |f| L::Op(f))
}

const PUNCT: &[&str] = &["(", ")", "[", "]", "{", "}", ",", ":", ";", "@", "\\"];

fn op_char(b: u8) -> bool {
    matches!(
        b,
        b'/' | b'='
            | b'-'
            | b'+'
            | b'!'
            | b'*'
            | b'%'
            | b'<'
            | b'>'
            | b'&'
            | b'|'
            | b'^'
            | b'?'
            | b'~'
    )
}

fn ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

fn ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

struct Lexer<'s> {
    src: &'s [u8],
    text: &'s str,
    at: usize,
    last: usize,
    tokens: Vec<Lx>,
    extras: Vec<(usize, usize, bool)>,
    errors: bool,
}

pub(crate) fn lex(text: &str) -> Lexed {
    let mut lx = Lexer {
        src: text.as_bytes(),
        text,
        at: 0,
        last: 0,
        tokens: Vec::new(),
        extras: Vec::new(),
        errors: false,
    };
    if text.starts_with("#!") {
        let end = text.find('\n').unwrap_or(text.len());
        lx.push(L::Shebang, 0, end);
        lx.at = end;
    }
    lx.code(false);
    Lexed {
        tokens: lx.tokens,
        extras: lx.extras,
        errors: lx.errors,
    }
}

impl Lexer<'_> {
    fn byte(&self, i: usize) -> u8 {
        self.src.get(i).copied().unwrap_or(0)
    }

    fn eof_at(&self, i: usize) -> bool {
        i >= self.src.len()
    }

    fn char_at(&self, i: usize) -> Option<char> {
        self.text.get(i..).and_then(|s| s.chars().next())
    }

    fn char_len(&self, i: usize) -> usize {
        self.char_at(i).map_or(1, char::len_utf8)
    }

    fn push(&mut self, k: L, start: usize, end: usize) {
        let nl = self.text[self.last.min(start)..start].contains('\n');
        self.tokens.push(Lx { k, start, end, nl });
        self.last = end;
    }

    /// Code tokens to the end, or (`paren`) to the `)` closing an
    /// interpolation, which is pushed.
    fn code(&mut self, paren: bool) {
        let mut depth = 0usize;
        loop {
            self.skip_trivia();
            if self.eof_at(self.at) {
                if paren {
                    self.errors = true;
                }
                return;
            }
            let b = self.byte(self.at);
            if paren && b == b'(' {
                depth += 1;
            }
            if paren && b == b')' {
                if depth == 0 {
                    let at = self.at;
                    self.push(L::Op(")"), at, at + 1);
                    self.at += 1;
                    return;
                }
                depth -= 1;
            }
            self.token();
        }
    }

    fn skip_trivia(&mut self) {
        loop {
            match self.char_at(self.at) {
                Some(c) if c.is_whitespace() => {
                    self.at += c.len_utf8();
                    continue;
                }
                _ => {}
            }
            let (b, next) = (self.byte(self.at), self.byte(self.at + 1));
            if b == b'/' && next == b'/' {
                let start = self.at;
                let end = self.text[start..]
                    .find('\n')
                    .map_or(self.src.len(), |p| start + p);
                self.extras.push((start, end, false));
                self.last = end;
                self.at = end;
                continue;
            }
            if b == b'/' && next == b'*' {
                let start = self.at;
                let i = self.block_comment_end(start);
                self.extras.push((start, i, true));
                self.last = i;
                self.at = i;
                continue;
            }
            return;
        }
    }

    /// At `start` in a string: the end of a block comment there, if one
    /// closes (in a `"` string, on its line).
    fn string_comment_end(&self, start: usize, multi: bool) -> Option<usize> {
        if !self.text[start..].starts_with("/*") {
            return None;
        }
        let mut i = start + 2;
        let mut depth = 1usize;
        while depth > 0 {
            if self.eof_at(i) || (!multi && self.byte(i) == b'\n') {
                return None;
            }
            if self.text[i..].starts_with("*/") {
                depth -= 1;
                i += 2;
            } else if self.text[i..].starts_with("/*") {
                depth += 1;
                i += 2;
            } else {
                i += self.char_len(i);
            }
        }
        Some(i)
    }

    /// The end of the block comment at `start` (they nest).
    fn block_comment_end(&mut self, start: usize) -> usize {
        let mut i = start + 2;
        let mut depth = 1usize;
        while depth > 0 {
            if self.eof_at(i) {
                self.errors = true;
                break;
            }
            if self.byte(i) == b'*' && self.byte(i + 1) == b'/' {
                depth -= 1;
                i += 2;
            } else if self.byte(i) == b'/' && self.byte(i + 1) == b'*' {
                depth += 1;
                i += 2;
            } else {
                i += self.char_len(i);
            }
        }
        i
    }

    fn name_end(&self, mut i: usize) -> usize {
        while let Some(c) = self.char_at(i) {
            if !ident_char(c) {
                break;
            }
            i += c.len_utf8();
        }
        i
    }

    fn token(&mut self) {
        let start = self.at;
        let b = self.byte(start);
        let c = self.char_at(start).unwrap_or('\0');
        if b == b'"' {
            return self.string(start);
        }
        if b == b'#' {
            // Raw strings: `#"…"#`, `##"""…"""##`.
            let mut k = start;
            while self.byte(k) == b'#' {
                k += 1;
            }
            if self.byte(k) == b'"' {
                return self.raw_string(start, k - start);
            }
            for d in ["#elseif", "#endif", "#else", "#if"] {
                if self.text[start..].starts_with(d)
                    && !self.char_at(start + d.len()).is_some_and(ident_char)
                {
                    self.push(L::Directive(d), start, start + d.len());
                    self.at = start + d.len();
                    return;
                }
            }
            self.push(L::Op("#"), start, start + 1);
            self.at = start + 1;
            for d in ["warning", "error", "sourceLocation"] {
                if self.text[start + 1..].starts_with(d) {
                    let end = self.text[start..]
                        .find(['\n', '\r'])
                        .map_or(self.src.len(), |p| start + p);
                    self.push(L::DiagTail, start + 1, end);
                    self.at = end;
                }
            }
            return;
        }
        if b.is_ascii_digit() {
            return self.number();
        }
        if b == b'`' {
            let end = self.text[start + 1..]
                .find(['`', '\n', '\r'])
                .map_or(self.src.len(), |p| start + 1 + p);
            let end = if self.byte(end) == b'`' { end + 1 } else { end };
            self.push(L::Word, start, end);
            self.at = end;
            return;
        }
        if b == b'$' {
            let end = self.name_end(start + 1);
            self.push(L::Word, start, end.max(start + 1));
            self.at = end.max(start + 1);
            return;
        }
        if ident_start(c) {
            let end = self.name_end(start);
            self.push(L::Word, start, end);
            self.at = end;
            return;
        }
        // An operator: a run of operator characters (dots only in a run
        // that starts with one), not into a comment.
        if op_char(b) || b == b'.' {
            let dot = b == b'.';
            let mut i = start;
            while !self.eof_at(i) {
                let x = self.byte(i);
                if i > start && x == b'/' && matches!(self.byte(i + 1), b'/' | b'*') {
                    break;
                }
                if !(op_char(x) || (dot && x == b'.')) {
                    break;
                }
                i += 1;
            }
            let kind = fixed(&self.text[start..i]);
            self.push(kind, start, i);
            self.at = i;
            return;
        }
        let rest = &self.text[start..];
        if let Some(p) = PUNCT.iter().find(|p| rest.starts_with(**p)) {
            self.push(L::Op(p), start, start + p.len());
            self.at = start + p.len();
            return;
        }
        let n = self.char_len(start);
        self.errors = true;
        self.push(L::Error, start, start + n);
        self.at = start + n;
    }

    fn digits(&self, mut i: usize, ok: impl Fn(u8) -> bool) -> usize {
        while ok(self.byte(i)) || (self.byte(i) == b'_' && i > 0) {
            if self.byte(i) == b'_' && !(ok(self.byte(i + 1)) || self.byte(i + 1) == b'_') {
                break;
            }
            i += 1;
        }
        i
    }

    fn number(&mut self) {
        let start = self.at;
        let dec = |b: u8| b.is_ascii_digit();
        if self.byte(start) == b'0' {
            type Digit = fn(u8) -> bool;
            let radix: Option<(L, Digit)> = match self.byte(start + 1) {
                b'x' | b'X' => Some((L::Hex, |b| b.is_ascii_hexdigit())),
                b'o' | b'O' => Some((L::Oct, |b| (b'0'..=b'7').contains(&b))),
                b'b' | b'B' => Some((L::Bin, |b| b == b'0' || b == b'1')),
                _ => None,
            };
            if let Some((kind, ok)) = radix {
                let mut i = self.digits(start + 2, ok);
                let mut k = kind;
                if kind == L::Hex {
                    // A hex float: `0x1.8p3`.
                    let mut j = i;
                    if self.byte(j) == b'.' && self.byte(j + 1).is_ascii_hexdigit() {
                        j = self.digits(j + 1, ok);
                    }
                    if matches!(self.byte(j), b'p' | b'P') {
                        let mut e = j + 1;
                        if matches!(self.byte(e), b'+' | b'-') {
                            e += 1;
                        }
                        if self.byte(e).is_ascii_digit() {
                            i = self.digits(e, dec);
                            k = L::Real;
                        }
                    }
                }
                self.push(k, start, i);
                self.at = i;
                return;
            }
        }
        let mut i = self.digits(start, dec);
        let mut real = false;
        if self.byte(i) == b'.' && self.byte(i + 1).is_ascii_digit() {
            real = true;
            i = self.digits(i + 1, dec);
        }
        if matches!(self.byte(i), b'e' | b'E') {
            let mut k = i + 1;
            if matches!(self.byte(k), b'+' | b'-') {
                k += 1;
            }
            if self.byte(k).is_ascii_digit() {
                real = true;
                i = self.digits(k, dec);
            }
        }
        self.push(if real { L::Real } else { L::Int }, start, i);
        self.at = i;
    }

    /// A `"…"` or `"""…"""` string at `start`.
    fn string(&mut self, start: usize) {
        let multi = self.text[start..].starts_with("\"\"\"");
        let q = if multi { "\"\"\"" } else { "\"" };
        self.push(L::StrOpen(q), start, start + q.len());
        self.at = start + q.len();
        loop {
            let at = self.at;
            // At a text token's start (after the opening, an escape, an
            // interpolation, blank text or a comment), where the scanner
            // looks for a comment first.
            let first = self.last == at
                && self.tokens.last().is_none_or(|t| {
                    t.k != L::Text
                        || self.text[t.start..t.end]
                            .bytes()
                            .all(|b| b.is_ascii_whitespace())
                });
            if self.eof_at(at) {
                self.errors = true;
                return;
            }
            if self.text[at..].starts_with(q) {
                self.push(L::StrClose(q), at, at + q.len());
                self.at = at + q.len();
                return;
            }
            match self.byte(at) {
                b'"' => {
                    // A quote in a `"""` string; a `"` string ended above.
                    self.push(L::Op("\""), at, at + 1);
                    self.at = at + 1;
                }
                b'\\' => {
                    let n = self.byte(at + 1);
                    if n == b'(' {
                        self.push(L::InterpOpen, at, at + 2);
                        self.at = at + 2;
                        self.code(true);
                    } else if n == b'u' && self.byte(at + 2) == b'{' {
                        let end = self.text[at..].find('}').map_or(at + 2, |p| at + p + 1);
                        self.push(L::Unicode, at, end);
                        self.at = end;
                    } else if matches!(n, b'0' | b'\\' | b't' | b'n' | b'r' | b'"' | b'\'' | b'\n')
                    {
                        self.push(L::Escape, at, at + 2);
                        self.at = at + 2;
                    } else {
                        self.errors = true;
                        let end = at
                            + 1
                            + if self.eof_at(at + 1) {
                                0
                            } else {
                                self.char_len(at + 1)
                            };
                        self.push(L::Escape, at, end);
                        self.at = end;
                    }
                }
                // tree-sitter-swift's scanner reads a block comment at the
                // start of a string's text (after its blank space).
                b'/' if first && self.string_comment_end(at, multi).is_some() => {
                    let end = self.string_comment_end(at, multi).unwrap_or(at);
                    self.extras.push((at, end, true));
                    self.at = end;
                }
                _ => {
                    let mut i = at;
                    while !self.eof_at(i) && !matches!(self.byte(i), b'"' | b'\\') {
                        if first
                            && self.byte(i) == b'/'
                            && self.text[at..i].bytes().all(|b| b.is_ascii_whitespace())
                            && self.string_comment_end(i, multi).is_some()
                        {
                            break;
                        }
                        if !multi && self.byte(i) == b'\n' {
                            break;
                        }
                        i += self.char_len(i);
                    }
                    if i == at {
                        // A line end in a `"` string: unterminated.
                        self.errors = true;
                        return;
                    }
                    self.push(L::Text, at, i);
                    self.at = i;
                }
            }
        }
    }

    /// A raw string at `start` with `hashes` `#`s: one token to its end, or
    /// parts around its `\#(…)` interpolations.
    fn raw_string(&mut self, start: usize, hashes: usize) {
        let pounds = "#".repeat(hashes);
        let open = start + hashes;
        let multi = self.text[open..].starts_with("\"\"\"");
        let close = format!("{}{pounds}", if multi { "\"\"\"" } else { "\"" });
        let interp = format!("\\{pounds}(");
        let mut part_start = start;
        let mut i = open + if multi { 3 } else { 1 };
        loop {
            if self.eof_at(i) {
                self.errors = true;
                self.push(L::RawEnd, part_start, self.src.len());
                self.at = self.src.len();
                return;
            }
            if self.text[i..].starts_with(&close) {
                let end = i + close.len();
                self.push(L::RawEnd, part_start, end);
                self.at = end;
                return;
            }
            if self.text[i..].starts_with(&interp) {
                self.push(L::RawPart, part_start, i);
                self.push(L::RawInterp, i, i + interp.len());
                self.at = i + interp.len();
                self.code(true);
                part_start = self.at;
                i = self.at;
                continue;
            }
            i += self.char_len(i);
        }
    }
}
