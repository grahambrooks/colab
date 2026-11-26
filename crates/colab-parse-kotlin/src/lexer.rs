//! Kotlin's tokens, lexed up front.
//!
//! Each token records whether a line ends between it and the token or
//! comment before it (`nl`), which is what tree-sitter-kotlin's scanner
//! reads to insert a statement's semicolon. Strings are cut as
//! tree-sitter-kotlin cuts them: in a `"…"` string a `$` is content of its
//! own (the grammar's content token is longer than any name after it) and
//! only `${…}` interpolates; in a `"""…"""` string the scanner's content
//! stops before `$` and a letter, or `${`, or three quotes. An
//! interpolation's code is lexed as code to its closing brace. Comments
//! are extras, kept apart; block comments nest.

/// A token's kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum L {
    /// A name or keyword (a `` `quoted` `` name too).
    Word,
    /// `name@`.
    Label,
    Number,
    Float,
    Op(&'static str),
    /// `'c'`: the quotes and what is between, one token; the parser splits
    /// it.
    Char,
    /// A string's opening (`"` or `"""`).
    StrOpen(&'static str),
    /// A string's closing (`"`, `"""` or `""""`).
    StrClose(&'static str),
    Content,
    Escape,
    /// `${`.
    InterpOpen,
    /// `$` before a name in a `"""` string.
    Dollar,
    Shebang,
    /// A character nothing reads.
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

const OPS: &[&str] = &[
    "!==", "===", "..<", "!!", "!=", "==", "&&", "||", "++", "--", "+=", "-=", "*=", "/=", "%=",
    "->", "..", "::", "?:", "<=", ">=", "!", "%", "&", "(", ")", "*", "+", ",", "-", ".", "/", ":",
    ";", "<", "=", ">", "?", "@", "[", "]", "{", "}",
];

struct Lexer<'s> {
    src: &'s [u8],
    text: &'s str,
    at: usize,
    /// Where the last token or comment ended.
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

fn ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

fn ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
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

    /// Code tokens to the end, or (`brace`) to the `}` closing an
    /// interpolation, which is pushed.
    fn code(&mut self, brace: bool) {
        let mut depth = 0usize;
        loop {
            self.skip_trivia();
            if self.eof_at(self.at) {
                if brace {
                    self.errors = true;
                }
                return;
            }
            let b = self.byte(self.at);
            if brace && b == b'{' {
                depth += 1;
            }
            if brace && b == b'}' {
                if depth == 0 {
                    let at = self.at;
                    self.push(L::Op("}"), at, at + 1);
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
                    .find(['\n', '\r'])
                    .map_or(self.src.len(), |p| start + p);
                self.extras.push((start, end, false));
                self.last = end;
                self.at = end;
                continue;
            }
            if b == b'/' && next == b'*' {
                let start = self.at;
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
                self.extras.push((start, i, true));
                self.last = i;
                self.at = i;
                continue;
            }
            return;
        }
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
            return self.string();
        }
        if b == b'\'' {
            return self.char_literal();
        }
        if b.is_ascii_digit() || (b == b'.' && self.byte(start + 1).is_ascii_digit()) {
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
        if ident_start(c) {
            let end = self.name_end(start);
            // `name@` is a label (`this@`, `return@` among them).
            if self.byte(end) == b'@' && self.text[start..end].is_ascii() {
                self.push(L::Label, start, end + 1);
                self.at = end + 1;
                return;
            }
            // `as?`.
            if &self.text[start..end] == "as" && self.byte(end) == b'?' {
                self.push(L::Op("as?"), start, end + 1);
                self.at = end + 1;
                return;
            }
            self.push(L::Word, start, end);
            self.at = end;
            return;
        }
        if b == b'!' {
            // `!is` and `!in`, not before more of a name.
            for w in ["is", "in"] {
                if self.text[start + 1..].starts_with(w)
                    && !self.char_at(start + 3).is_some_and(ident_char)
                {
                    let op = if w == "is" { "!is" } else { "!in" };
                    self.push(L::Op(op), start, start + 3);
                    self.at = start + 3;
                    return;
                }
            }
        }
        // `?.` is two tokens here; the parser joins them after an
        // expression (tree-sitter's scanner token), not in a receiver type
        // (`fun T?.f()`).
        let rest = &self.text[start..];
        if let Some(op) = OPS.iter().find(|o| rest.starts_with(**o)) {
            self.push(L::Op(op), start, start + op.len());
            self.at = start + op.len();
            return;
        }
        let n = self.char_len(start);
        self.errors = true;
        self.push(L::Error, start, start + n);
        self.at = start + n;
    }

    fn digits(&self, mut i: usize, ok: impl Fn(u8) -> bool) -> usize {
        while ok(self.byte(i)) || (self.byte(i) == b'_' && i > 0 && ok(self.byte(i - 1))) {
            i += 1;
        }
        // A trailing `_` is no part of the number.
        while i > 0 && self.byte(i - 1) == b'_' {
            i -= 1;
        }
        i
    }

    fn number(&mut self) {
        let start = self.at;
        let dec = |b: u8| b.is_ascii_digit();
        if self.byte(start) == b'0' && matches!(self.byte(start + 1), b'x' | b'X' | b'b' | b'B') {
            let hex = matches!(self.byte(start + 1), b'x' | b'X');
            let ok = |b: u8| {
                if hex {
                    b.is_ascii_hexdigit()
                } else {
                    b == b'0' || b == b'1'
                }
            };
            let mut i = self.digits(start + 2, ok);
            i = self.int_suffix(i);
            self.push(L::Number, start, i);
            self.at = i;
            return;
        }
        let mut i = self.digits(start, dec);
        let mut float = false;
        if self.byte(i) == b'.' && self.byte(i + 1).is_ascii_digit() {
            float = true;
            i = self.digits(i + 1, dec);
        }
        if matches!(self.byte(i), b'e' | b'E') {
            let mut k = i + 1;
            if matches!(self.byte(k), b'+' | b'-') {
                k += 1;
            }
            if self.byte(k).is_ascii_digit() {
                float = true;
                i = self.digits(k, dec);
            }
        }
        if matches!(self.byte(i), b'f' | b'F') {
            float = true;
            i += 1;
        }
        if !float {
            i = self.int_suffix(i);
        }
        self.push(if float { L::Float } else { L::Number }, start, i);
        self.at = i;
    }

    fn int_suffix(&self, i: usize) -> usize {
        match self.byte(i) {
            b'l' | b'L' => i + 1,
            b'u' | b'U' if matches!(self.byte(i + 1), b'l' | b'L') => i + 2,
            b'u' | b'U' => i + 1,
            _ => i,
        }
    }

    fn char_literal(&mut self) {
        let start = self.at;
        let mut i = start + 1;
        if self.byte(i) == b'\\' {
            i += 1;
            if self.byte(i) == b'u' && (1..5).all(|k| self.byte(i + k).is_ascii_hexdigit()) {
                i += 5;
            } else {
                i += self.char_len(i);
            }
        } else if self.byte(i) != b'\'' {
            i += self.char_len(i);
        }
        if self.byte(i) == b'\'' {
            i += 1;
        } else {
            self.errors = true;
        }
        self.push(L::Char, start, i);
        self.at = i;
    }

    fn string(&mut self) {
        let start = self.at;
        if self.text[start..].starts_with("\"\"\"") {
            self.push(L::StrOpen("\"\"\""), start, start + 3);
            self.at = start + 3;
            return self.multiline();
        }
        self.push(L::StrOpen("\""), start, start + 1);
        self.at = start + 1;
        loop {
            let at = self.at;
            if self.eof_at(at) {
                self.errors = true;
                return;
            }
            match self.byte(at) {
                b'"' => {
                    self.push(L::StrClose("\""), at, at + 1);
                    self.at = at + 1;
                    return;
                }
                b'\\' => {
                    let n = self.byte(at + 1);
                    // `A` is content `\u` and more content: the content
                    // token's precedence beats the longer escape.
                    let (k, end) =
                        if matches!(n, b'b' | b'n' | b'r' | b't' | b'\'' | b'"' | b'\\' | b'$') {
                            (L::Escape, at + 2)
                        } else if self.eof_at(at + 1) {
                            (L::Content, at + 1)
                        } else {
                            (L::Content, at + 1 + self.char_len(at + 1))
                        };
                    self.push(k, at, end);
                    self.at = end;
                }
                b'$' if self.byte(at + 1) == b'{' => {
                    self.push(L::InterpOpen, at, at + 2);
                    self.at = at + 2;
                    self.code(true);
                }
                b'$' => {
                    self.push(L::Content, at, at + 1);
                    self.at = at + 1;
                }
                _ => {
                    let mut i = at;
                    while !self.eof_at(i) && !matches!(self.byte(i), b'"' | b'\\' | b'$') {
                        i += self.char_len(i);
                    }
                    self.push(L::Content, at, i);
                    self.at = i;
                }
            }
        }
    }

    fn multiline(&mut self) {
        loop {
            let start = self.at;
            let mut i = start;
            loop {
                if self.eof_at(i) {
                    break;
                }
                match self.byte(i) {
                    b'$' if self.char_at(i + 1).is_some_and(char::is_alphabetic)
                        || self.byte(i + 1) == b'{' =>
                    {
                        break;
                    }
                    b'"' if self.text[i..].starts_with("\"\"\"") => break,
                    _ => i += self.char_len(i),
                }
            }
            if i > start {
                self.push(L::Content, start, i);
                self.at = i;
            }
            if self.eof_at(i) {
                self.errors = true;
                return;
            }
            if self.byte(i) == b'"' {
                let n = if self.text[i..].starts_with("\"\"\"\"") {
                    4
                } else {
                    3
                };
                let q = if n == 4 { "\"\"\"\"" } else { "\"\"\"" };
                self.push(L::StrClose(q), i, i + n);
                self.at = i + n;
                return;
            }
            if self.byte(i + 1) == b'{' {
                self.push(L::InterpOpen, i, i + 2);
                self.at = i + 2;
                self.code(true);
            } else {
                self.push(L::Dollar, i, i + 1);
                let end = self.name_end(i + 1);
                self.push(L::Word, i + 1, end);
                self.at = end;
            }
        }
    }
}
