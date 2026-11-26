//! C and C++ tokens: words (identifiers and keywords alike; the parser
//! reads keywords by their text), numbers, strings, characters and raw
//! strings in their parts, operators, and the preprocessor's lines.
//! `>` is always one token, so `>>` can close two template-argument
//! lists; the expression parser joins adjacent ones (`>>`, `>=`, `>>=`).
//! `[` and `]` are single too, joined into `[[` / `]]` where an attribute
//! is. A number's sign is the parser's to join (`-1` is one literal at an
//! expression's start). Comments are extras, placed after parsing.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum L {
    Name,
    Op(&'static str),
    Number,
    /// `literal_suffix`: a word touching a number, character or string.
    Suffix,
    /// A string's opening quote with its prefix (`"`, `L"`, `u8"`, …).
    StrOpen(&'static str),
    StrContent,
    Escape,
    /// A string's or character's closing quote.
    Close(&'static str),
    /// A character's opening quote with its prefix (`'`, `L'`, …).
    CharOpen(&'static str),
    /// `character`: one character of a character literal.
    Character,
    /// A raw string's opening (`R"`, `u8R"`, …).
    RawOpen(&'static str),
    RawDelimiter,
    RawContent,
    /// `system_lib_string` (`<stdio.h>` after `#include`).
    SysLib,
    /// `#include`, `#define`, `#if`, `#ifdef`, … (the text may hold
    /// spaces after the `#`).
    Directive(&'static str),
    /// `preproc_directive`: any other directive's word (`#pragma`).
    OtherDirective,
    PreprocArg,
    /// The hidden newline ending `#include`, `#define` and other lines.
    LineEnd,
    /// The visible `"\n"` ending `#if` and `#elif` conditions.
    Newline,
    Bad,
    Eof,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Lx {
    pub k: L,
    pub start: usize,
    pub end: usize,
}

/// A comment, placed after parsing.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Extra {
    pub start: usize,
    pub end: usize,
}

const OPS: &[&str] = &[
    "<<=", "<=>", "->*", "...", "::", "->", ".*", "++", "--", "&&", "||", "==", "!=", "<=", "<<",
    "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=", "+", "-", "*", "/", "%", "&", "|", "^", "!",
    "~", "<", ">", "=", "?", ":", ";", ",", ".", "(", ")", "[", "]", "{", "}",
];

const DIRECTIVES: &[(&str, &str)] = &[
    ("include", "#include"),
    ("define", "#define"),
    ("if", "#if"),
    ("ifdef", "#ifdef"),
    ("ifndef", "#ifndef"),
    ("elif", "#elif"),
    ("elifdef", "#elifdef"),
    ("elifndef", "#elifndef"),
    ("else", "#else"),
    ("endif", "#endif"),
];

pub(crate) struct Lexed {
    pub tokens: Vec<Lx>,
    pub extras: Vec<Extra>,
    pub errors: bool,
}

pub(crate) fn lex(src: &str, c: bool) -> Lexed {
    let mut lx = Lexer {
        src,
        b: src.as_bytes(),
        at: 0,
        out: Vec::new(),
        extras: Vec::new(),
        errors: false,
        line: Line::Code,
        c,
    };
    if src.starts_with('\u{feff}') {
        lx.at = 3;
    }
    lx.run();
    lx.out.push(Lx {
        k: L::Eof,
        start: src.len(),
        end: src.len(),
    });
    Lexed {
        tokens: lx.out,
        extras: lx.extras,
        errors: lx.errors,
    }
}

/// What a newline means on the current line.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Line {
    Code,
    /// `#if` / `#elif`: the newline is a visible token.
    Condition,
    /// `#include` and the like: the newline is a hidden one.
    Directive,
    /// After `#include`: `<…>` is a system library string.
    Include,
}

struct Lexer<'s> {
    src: &'s str,
    b: &'s [u8],
    at: usize,
    out: Vec<Lx>,
    extras: Vec<Extra>,
    errors: bool,
    line: Line,
    /// Lexing C: no raw strings or literal suffixes, C's number suffixes.
    c: bool,
}

fn is_ident_start(c: char) -> bool {
    c == '_' || c == '$' || c.is_alphabetic()
}

fn is_ident_continue(c: char) -> bool {
    c == '_' || c == '$' || c.is_alphanumeric()
}

impl Lexer<'_> {
    fn byte(&self, i: usize) -> u8 {
        self.b.get(i).copied().unwrap_or(0)
    }

    fn char_at(&self, i: usize) -> Option<char> {
        self.src.get(i..).and_then(|s| s.chars().next())
    }

    fn push(&mut self, k: L, start: usize, end: usize) {
        if k == L::Bad {
            self.errors = true;
        }
        self.out.push(Lx { k, start, end });
    }

    fn run(&mut self) {
        while self.at < self.b.len() {
            self.token();
        }
    }

    /// Whether only spaces and tabs come before `at` on its line.
    fn line_start(&self, at: usize) -> bool {
        let first = if self.src.starts_with('\u{feff}') {
            3
        } else {
            0
        };
        let mut i = at;
        while i > first {
            match self.b[i - 1] {
                b' ' | b'\t' | b'\x0c' | b'\x0b' => i -= 1,
                b'\n' => return true,
                _ => return false,
            }
        }
        true
    }

    /// A line continuation (`\` and a newline) at `i`: its end.
    fn continuation(&self, i: usize) -> Option<usize> {
        if self.byte(i) != b'\\' {
            return None;
        }
        match self.byte(i + 1) {
            b'\n' => Some(i + 2),
            b'\r' if self.byte(i + 2) == b'\n' => Some(i + 3),
            _ => None,
        }
    }

    fn token(&mut self) {
        let c = self.byte(self.at);
        let start = self.at;
        if let Some(end) = self.continuation(start) {
            self.at = end;
            return;
        }
        match c {
            b'\n' => {
                match self.line {
                    // tree-sitter-c's token is `\n+`.
                    Line::Condition if self.c => {
                        let mut end = start + 1;
                        while self.byte(end) == b'\n' {
                            end += 1;
                        }
                        self.push(L::Newline, start, end);
                        self.at = end - 1;
                    }
                    Line::Condition => self.push(L::Newline, start, start + 1),
                    Line::Directive | Line::Include => self.push(L::LineEnd, start, start + 1),
                    Line::Code => {}
                }
                self.line = Line::Code;
                self.at += 1;
            }
            b'\r'
                if self.byte(start + 1) == b'\n'
                    && matches!(self.line, Line::Directive | Line::Include) =>
            {
                self.push(L::LineEnd, start, start + 2);
                self.line = Line::Code;
                self.at += 2;
            }
            b' ' | b'\t' | b'\r' | b'\x0b' | b'\x0c' => self.at += 1,
            b'/' if self.byte(start + 1) == b'/' => {
                // To the end of the line; a backslash continues it.
                let mut i = start + 2;
                while i < self.b.len() && self.b[i] != b'\n' {
                    if self.b[i] == b'\\' {
                        i += 1;
                        if self.byte(i) == b'\r' {
                            i += 1;
                        }
                    }
                    i += 1;
                }
                let end = i.min(self.b.len());
                self.extras.push(Extra { start, end });
                self.at = end;
            }
            b'/' if self.byte(start + 1) == b'*' => {
                let end = self.src[start + 2..]
                    .find("*/")
                    .map_or(self.b.len(), |i| start + 2 + i + 2);
                if !self.src[start..end].ends_with("*/") {
                    self.errors = true;
                }
                self.extras.push(Extra { start, end });
                self.at = end;
            }
            b'#' if self.line == Line::Code && self.line_start(start) => self.directive(),
            b'<' if self.line == Line::Include => {
                let mut i = start + 1;
                while i < self.b.len() && self.b[i] != b'>' && self.b[i] != b'\n' {
                    i += 1;
                }
                if self.byte(i) == b'>' {
                    self.push(L::SysLib, start, i + 1);
                    self.at = i + 1;
                } else {
                    self.push(L::Bad, start, i);
                    self.at = i;
                }
            }
            b'"' => self.string(start, start, "\""),
            b'\'' => self.char_literal(start, start, "'"),
            b'0'..=b'9' => self.number(start),
            b'.' if self.byte(start + 1).is_ascii_digit() => self.number(start),
            _ => {
                let ch = self.char_at(start).unwrap_or('\0');
                if is_ident_start(ch) || (ch == '\\' && matches!(self.byte(start + 1), b'u' | b'U'))
                {
                    self.word(start);
                    return;
                }
                if ch.is_whitespace() {
                    self.at += ch.len_utf8();
                    return;
                }
                if let Some(op) = OPS.iter().find(|o| self.src[start..].starts_with(**o)) {
                    self.at = start + op.len();
                    self.push(L::Op(op), start, self.at);
                    return;
                }
                self.at = start + ch.len_utf8();
                self.push(L::Bad, start, self.at);
            }
        }
    }

    fn word(&mut self, start: usize) {
        let mut i = start;
        let mut first = true;
        while let Some(c) = self.char_at(i) {
            if c == '\\' && matches!(self.byte(i + 1), b'u' | b'U') {
                let n = if self.byte(i + 1) == b'u' { 4 } else { 8 };
                if self.b.len() >= i + 2 + n
                    && self.b[i + 2..i + 2 + n].iter().all(u8::is_ascii_hexdigit)
                {
                    i += 2 + n;
                    first = false;
                    continue;
                }
                break;
            }
            let ok = if first {
                is_ident_start(c)
            } else {
                is_ident_continue(c)
            };
            if !ok {
                break;
            }
            i += c.len_utf8();
            first = false;
        }
        if first {
            self.at = start + 1;
            self.push(L::Bad, start, self.at);
            return;
        }
        // A string's or character's prefix.
        let w = &self.src[start..i];
        match (w, self.byte(i)) {
            ("L" | "u" | "U" | "u8", b'"') => {
                let open = match w {
                    "L" => "L\"",
                    "u" => "u\"",
                    "U" => "U\"",
                    _ => "u8\"",
                };
                return self.string(start, i, open);
            }
            ("L" | "u" | "U" | "u8", b'\'') => {
                let open = match w {
                    "L" => "L'",
                    "u" => "u'",
                    "U" => "U'",
                    _ => "u8'",
                };
                return self.char_literal(start, i, open);
            }
            ("R" | "LR" | "uR" | "UR" | "u8R", b'"') if !self.c => {
                let open = match w {
                    "R" => "R\"",
                    "LR" => "LR\"",
                    "uR" => "uR\"",
                    "UR" => "UR\"",
                    _ => "u8R\"",
                };
                return self.raw_string(start, i, open);
            }
            _ => {}
        }
        self.at = i;
        self.push(L::Name, start, i);
    }

    fn number(&mut self, start: usize) {
        let b = self.b;
        let at = |i: usize| b.get(i).copied().unwrap_or(0);
        let mut i = start;
        let digits = |mut i: usize, hex: bool| {
            loop {
                let c = at(i);
                let ok = if hex {
                    c.is_ascii_hexdigit()
                } else {
                    c.is_ascii_digit()
                };
                // A digit, or a `'` separator before one.
                let d = at(i + 1);
                let separator = c == b'\''
                    && if hex {
                        d.is_ascii_hexdigit()
                    } else {
                        d.is_ascii_digit()
                    };
                if !ok && !separator {
                    return i;
                }
                i += 1;
            }
        };
        let hex = at(i) == b'0' && matches!(at(i + 1), b'x' | b'X');
        let bin = at(i) == b'0' && matches!(at(i + 1), b'b' | b'B');
        let mut float = false;
        if hex || bin {
            i = digits(i + 2, hex);
        } else if at(i) != b'.' {
            i = digits(i, false);
        }
        if at(i) == b'.' && !(bin) {
            float = true;
            i = digits(i + 1, hex);
        }
        let exp = if hex {
            matches!(at(i), b'p' | b'P')
        } else {
            matches!(at(i), b'e' | b'E')
        };
        if exp {
            let mut k = i + 1;
            if matches!(at(k), b'+' | b'-') {
                k += 1;
            }
            if at(k).is_ascii_digit() {
                float = true;
                i = digits(k, false);
            }
        }
        // The suffixes tree-sitter-cpp's number takes; C's takes any run of
        // `uUlLwWfFbBdD`.
        let rest = &self.src[i..];
        let suffix = if self.c {
            rest.bytes()
                .take_while(|b| b"uUlLwWfFbBdD".contains(b))
                .count()
        } else if float || exp {
            [
                "f128", "F128", "f16", "F16", "f32", "F32", "f64", "F64", "bf16", "BF16", "f", "F",
                "l", "L",
            ]
            .iter()
            .find(|s| rest.starts_with(**s))
            .map_or(0, |s| s.len())
        } else {
            [
                "llu", "llU", "LLu", "LLU", "ull", "uLL", "Ull", "ULL", "ll", "LL", "ul", "uL",
                "Ul", "UL", "uz", "uZ", "Uz", "UZ", "lu", "lU", "Lu", "LU", "zu", "zU", "Zu", "ZU",
                "u", "U", "l", "L", "z", "Z",
            ]
            .iter()
            .find(|s| rest.starts_with(**s))
            .map_or(0, |s| s.len())
        };
        i += suffix;
        self.at = i;
        self.push(L::Number, start, i);
        self.literal_suffix();
    }

    /// A word touching the literal just lexed: `literal_suffix`.
    fn literal_suffix(&mut self) {
        if self.c {
            return;
        }
        let i = self.at;
        if self
            .char_at(i)
            .is_some_and(|c| c == '_' || c.is_ascii_alphabetic())
        {
            let mut k = i;
            while self
                .char_at(k)
                .is_some_and(|c| c == '_' || c.is_ascii_alphanumeric())
            {
                k += 1;
            }
            self.push(L::Suffix, i, k);
            self.at = k;
        }
    }

    /// An escape sequence at `at` (a `\`): its end.
    fn escape_end(&self, at: usize) -> usize {
        let hex = |from: usize, max: usize| {
            from + self.b[from.min(self.b.len())..]
                .iter()
                .take(max)
                .take_while(|c| c.is_ascii_hexdigit())
                .count()
        };
        match self.byte(at + 1) {
            b'x' => hex(at + 2, 4).max(at + 2),
            b'u' if self.b.len() >= at + 6 => hex(at + 2, 4),
            b'U' if self.b.len() >= at + 10 => hex(at + 2, 8),
            b'0'..=b'9' => {
                let n = self.b[at + 1..]
                    .iter()
                    .take(3)
                    .take_while(|c| c.is_ascii_digit())
                    .count();
                at + 1 + if n >= 2 { n } else { 1 }
            }
            0 => at + 1,
            _ => at + 1 + self.char_at(at + 1).map_or(1, char::len_utf8),
        }
    }

    fn string(&mut self, start: usize, quote: usize, open: &'static str) {
        self.push(L::StrOpen(open), start, quote + 1);
        let mut i = quote + 1;
        loop {
            match self.byte(i) {
                0 if i >= self.b.len() => {
                    self.errors = true;
                    break;
                }
                b'"' => {
                    self.push(L::Close("\""), i, i + 1);
                    i += 1;
                    break;
                }
                b'\\' if self.continuation(i).is_some() => {
                    i = self.continuation(i).unwrap_or(i + 1);
                }
                b'\\' => {
                    let e = self.escape_end(i);
                    self.push(L::Escape, i, e);
                    i = e;
                }
                b'\n' => {
                    self.errors = true;
                    break;
                }
                _ => {
                    let mut k = i;
                    while k < self.b.len() && !matches!(self.b[k], b'"' | b'\\' | b'\n') {
                        k += 1;
                    }
                    self.push(L::StrContent, i, k);
                    i = k;
                }
            }
        }
        self.at = i;
        self.literal_suffix();
    }

    fn char_literal(&mut self, start: usize, quote: usize, open: &'static str) {
        self.push(L::CharOpen(open), start, quote + 1);
        let mut i = quote + 1;
        loop {
            match self.byte(i) {
                b'\'' => {
                    self.push(L::Close("'"), i, i + 1);
                    i += 1;
                    break;
                }
                b'\n' | 0 => {
                    self.errors = true;
                    break;
                }
                b'\\' => {
                    let e = self.escape_end(i);
                    self.push(L::Escape, i, e);
                    i = e;
                }
                _ => {
                    let n = self.char_at(i).map_or(1, char::len_utf8);
                    self.push(L::Character, i, i + n);
                    i += n;
                }
            }
        }
        self.at = i;
        self.literal_suffix();
    }

    fn raw_string(&mut self, start: usize, quote: usize, open: &'static str) {
        self.push(L::RawOpen(open), start, quote + 1);
        let mut i = quote + 1;
        let delim_start = i;
        while i < self.b.len() && !matches!(self.b[i], b'(' | b'"' | b'\n' | b' ' | b')' | b'\\') {
            i += 1;
        }
        if self.byte(i) != b'(' {
            self.errors = true;
            self.at = i;
            return;
        }
        let delim = &self.src[delim_start..i];
        if !delim.is_empty() {
            self.push(L::RawDelimiter, delim_start, i);
        }
        self.push(L::Op("("), i, i + 1);
        let content = i + 1;
        let close = format!("){delim}\"");
        let Some(end) = self.src[content..].find(&close).map(|k| content + k) else {
            self.push(L::RawContent, content, self.b.len());
            self.errors = true;
            self.at = self.b.len();
            return;
        };
        self.push(L::RawContent, content, end);
        self.push(L::Op(")"), end, end + 1);
        if !delim.is_empty() {
            self.push(L::RawDelimiter, end + 1, end + 1 + delim.len());
        }
        let q = end + 1 + delim.len();
        self.push(L::Close("\""), q, q + 1);
        self.at = q + 1;
        self.literal_suffix();
    }

    /// A directive: `#`, spaces, its name, and how its line is read.
    fn directive(&mut self) {
        let start = self.at;
        let mut i = start + 1;
        while matches!(self.byte(i), b' ' | b'\t') {
            i += 1;
        }
        let name_start = i;
        while self.byte(i).is_ascii_alphanumeric() || self.byte(i) == b'_' {
            i += 1;
        }
        let name = &self.src[name_start..i];
        self.at = i;
        match DIRECTIVES.iter().find(|(n, _)| *n == name) {
            Some(&(_, kind)) => {
                self.push(L::Directive(kind), start, i);
                match kind {
                    "#include" => self.line = Line::Include,
                    "#define" => self.define(),
                    "#if" | "#elif" => self.line = Line::Condition,
                    _ => {}
                }
            }
            None if name.is_empty() => {
                self.push(L::Bad, start, i.max(start + 1));
                self.at = i.max(start + 1);
            }
            None => {
                self.push(L::OtherDirective, start, i);
                self.line = Line::Directive;
                self.preproc_arg();
            }
        }
    }

    /// `#define NAME`, its parameters if a `(` touches the name, then the
    /// rest of the line as its value.
    fn define(&mut self) {
        self.line = Line::Directive;
        let mut i = self.at;
        while matches!(self.byte(i), b' ' | b'\t') || self.continuation(i).is_some() {
            i = self.continuation(i).unwrap_or(i + 1);
        }
        self.at = i;
        if !self.char_at(i).is_some_and(is_ident_start) {
            return;
        }
        self.word(i);
        if self.byte(self.at) == b'(' {
            // The parameters: words, `...` and commas, to `)`.
            loop {
                let save = self.at;
                if matches!(self.byte(save), b'\n' | 0) {
                    return;
                }
                let before = self.out.len();
                self.token();
                if self.out.len() > before && self.out[before].k == L::Op(")") {
                    break;
                }
                if self.at == save {
                    self.at += 1;
                }
            }
        }
        self.preproc_arg();
    }

    /// The rest of a directive's line as `preproc_arg` (after spaces and
    /// continuations, and comments that start it).
    fn preproc_arg(&mut self) {
        loop {
            let mut i = self.at;
            while matches!(self.byte(i), b' ' | b'\t' | b'\x0b' | b'\x0c')
                || self.continuation(i).is_some()
            {
                i = self.continuation(i).unwrap_or(i + 1);
            }
            self.at = i;
            match self.byte(i) {
                b'\n' | 0 => return,
                b'\r' if self.byte(i + 1) == b'\n' => return,
                b'/' if matches!(self.byte(i + 1), b'/' | b'*') => {
                    // A comment wins over the argument it would start.
                    self.token();
                    continue;
                }
                _ => {}
            }
            let start = i;
            while i < self.b.len() {
                if let Some(e) = self.continuation(i) {
                    i = e;
                    continue;
                }
                match self.b[i] {
                    b'\n' => break,
                    b'/' if self.byte(i + 1) == b'*' => break,
                    b'/' => i += 2,
                    _ => i += self.char_at(i).map_or(1, char::len_utf8),
                }
            }
            // A `\r` before the newline is the argument's (`[^/\n]`).
            let end = i.min(self.b.len());
            self.push(L::PreprocArg, start, end);
            self.at = end;
            return;
        }
    }
}
