//! Python's tokens: names, numbers, operators, strings in their parts
//! (prefix and quote, text, escapes, f-string interpolations lexed as
//! code), and the layout tokens of the offside rule.
//!
//! Layout follows tree-sitter-python's scanner, since where its tokens fall
//! decides where nodes start and end:
//!
//! - A statement's `Newline` is zero-width, at the end of the last token or
//!   comment on its line. Newlines inside brackets are whitespace.
//! - An `Indent` is taken only after a line ending in `:`; deeper
//!   indentation anywhere else is whitespace, as it is to tree-sitter.
//! - A `Dedent` falls after the comments that follow a block while the
//!   first of them is indented at least as far as the block: those
//!   comments are the block's, the rest the enclosing one's.

/// What a token is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum L {
    /// An identifier or a keyword: the parser reads keywords by their text,
    /// since most of Python's newer ones are soft.
    Name,
    Op(&'static str),
    Int,
    Float,
    /// A string's prefix and opening quote(s).
    StrStart,
    /// A run of a string's text with no escape in it.
    StrText,
    /// `\n`, `\x41`, `\N{DASH}`, …
    Escape,
    /// A backslash that starts no escape.
    NotEscape,
    /// `{{` or `}}` in an f-string.
    EscInterp,
    StrEnd,
    /// `!r` in an f-string interpolation.
    Conversion,
    /// The text of a format specifier.
    FmtText,
    Newline,
    Indent,
    Dedent,
    /// A character no token starts with.
    Bad,
    Eof,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Lx {
    pub k: L,
    pub start: usize,
    pub end: usize,
}

/// A comment or a line continuation: a token the tree holds wherever it
/// falls, placed after parsing.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Extra {
    pub kind: &'static str,
    pub start: usize,
    pub end: usize,
}

const OPS: &[&str] = &[
    "**=", "//=", ">>=", "<<=", "...", "!=", "<>", "->", ":=", "**", "//", "<<", ">>", "<=", ">=",
    "==", "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=", "@=", "+", "-", "*", "/", "%", "@", "&",
    "|", "^", "~", "<", ">", "(", ")", "[", "]", "{", "}", ",", ":", ".", ";", "=", "!",
];

const AUGMENTED_OPS: &[&str] = &[
    "+=", "-=", "*=", "/=", "@=", "//=", "%=", "**=", ">>=", "<<=", "&=", "^=", "|=",
];

pub(crate) struct Lexed {
    pub tokens: Vec<Lx>,
    pub extras: Vec<Extra>,
    /// A string left open, or a character no token starts with.
    pub errors: bool,
}

pub(crate) fn lex(src: &str) -> Lexed {
    let mut lx = Lexer {
        src,
        b: src.as_bytes(),
        at: 0,
        out: Vec::new(),
        extras: Vec::new(),
        indents: vec![0],
        depth: 0,
        errors: false,
        line_has_tokens: false,
        last_end: 0,
    };
    if src.starts_with('\u{feff}') {
        lx.at = 3;
    }
    lx.run();
    let eof = src.len();
    lx.out.push(Lx {
        k: L::Eof,
        start: eof,
        end: eof,
    });
    Lexed {
        tokens: lx.out,
        extras: lx.extras,
        errors: lx.errors,
    }
}

struct Lexer<'s> {
    src: &'s str,
    b: &'s [u8],
    at: usize,
    out: Vec<Lx>,
    extras: Vec<Extra>,
    indents: Vec<usize>,
    /// Open brackets, inside which a newline is whitespace.
    depth: usize,
    errors: bool,
    /// Whether the logical line being read has a token yet.
    line_has_tokens: bool,
    /// The end of the line's last token or comment.
    last_end: usize,
}

impl Lexer<'_> {
    fn byte(&self, at: usize) -> u8 {
        self.b.get(at).copied().unwrap_or(0)
    }

    fn push(&mut self, k: L, start: usize, end: usize) {
        self.out.push(Lx { k, start, end });
        if !matches!(k, L::Newline | L::Indent | L::Dedent) {
            self.line_has_tokens = true;
            self.last_end = end;
        }
    }

    fn run(&mut self) {
        loop {
            self.skip(self.depth > 0);
            if self.at >= self.b.len() {
                if self.line_has_tokens {
                    self.end_line();
                } else {
                    self.dedent_all(self.at);
                }
                return;
            }
            if self.byte(self.at) == b'\n' {
                if self.line_has_tokens {
                    self.end_line();
                } else {
                    self.at += 1;
                }
                continue;
            }
            self.token();
        }
    }

    /// Past spaces, tabs, form feeds, comments and line continuations; past
    /// newlines too when `newlines`.
    fn skip(&mut self, newlines: bool) {
        loop {
            match self.byte(self.at) {
                b' ' | b'\t' | b'\x0c' | b'\r' => self.at += 1,
                b'\n' if newlines => self.at += 1,
                b'#' => {
                    // A comment runs to the newline, a `\r` before it included.
                    let end = self.src[self.at..]
                        .find('\n')
                        .map_or(self.b.len(), |i| self.at + i);
                    self.extra("comment", self.at, end);
                    self.at = end;
                }
                b'\\' => {
                    let mut j = self.at + 1;
                    if self.byte(j) == b'\r' {
                        j += 1;
                    }
                    if self.byte(j) == b'\n' {
                        // Before a string, tree-sitter's scanner passes
                        // over a continuation as whitespace: no token.
                        if !self.string_follows(j + 1) {
                            self.extra("line_continuation", self.at, j + 1);
                        }
                        self.at = j + 1;
                    } else if j >= self.b.len() {
                        self.extra("line_continuation", self.at, j);
                        self.at = j;
                    } else {
                        return;
                    }
                }
                0xc2 if self.src[self.at..].starts_with('\u{a0}') => self.at += 2,
                0xe2 if self.src[self.at..].starts_with('\u{2060}')
                    || self.src[self.at..].starts_with('\u{200b}') =>
                {
                    self.at += 3;
                }
                0xef if self.src[self.at..].starts_with('\u{feff}') => self.at += 3,
                _ => return,
            }
        }
    }

    /// Whether a string starts at `at`, after whitespace, blank lines and
    /// more continuations (a comment stops it).
    fn string_follows(&self, mut at: usize) -> bool {
        loop {
            match self.byte(at) {
                b' ' | b'\t' | b'\x0c' | b'\r' | b'\n' => at += 1,
                b'\\' if matches!(self.byte(at + 1), b'\n' | b'\r') => at += 2,
                _ => break,
            }
        }
        let mut j = at;
        while j < at + 3 && b"rRbBuUfFtT".contains(&self.byte(j)) {
            j += 1;
        }
        matches!(self.byte(j), b'"' | b'\'')
    }

    fn extra(&mut self, kind: &'static str, start: usize, end: usize) {
        self.extras.push(Extra { kind, start, end });
        if self.line_has_tokens && kind == "comment" {
            self.last_end = end;
        }
    }

    /// At the end of a line with tokens: its `Newline`, then the comment
    /// and blank lines up to the next line of code, and the `Indent` or
    /// `Dedent`s that line's indentation calls for.
    fn end_line(&mut self) {
        if self.needs_operand() {
            // tree-sitter's scanner gives a newline no token where no
            // statement can end, so the line goes on (invalid Python,
            // read as tree-sitter reads it).
            self.at += 1;
            return;
        }
        let p0 = self.last_end;
        let ends_block_header = matches!(self.out.last(), Some(Lx { k: L::Op(":"), .. }));
        self.push(L::Newline, p0, p0);
        self.line_has_tokens = false;
        // Each comment line before the next code line: its indentation and
        // its end.
        let mut comments: Vec<(usize, usize)> = Vec::new();
        let mut p = self.at;
        let next_indent = loop {
            if p >= self.b.len() {
                break 0;
            }
            if self.byte(p) == b'\n' {
                p += 1;
            }
            let mut indent = 0;
            loop {
                match self.byte(p) {
                    b' ' => indent += 1,
                    b'\t' => indent += 8,
                    b'\r' | b'\x0c' => indent = 0,
                    _ => break,
                }
                p += 1;
            }
            match self.byte(p) {
                _ if p >= self.b.len() => break 0,
                b'\n' => {}
                b'#' => {
                    let end = self.src[p..].find('\n').map_or(self.b.len(), |i| p + i);
                    self.extra("comment", p, end);
                    comments.push((indent, end));
                    p = end;
                }
                b'\\' if matches!(self.byte(p + 1), b'\n' | b'\r') => {
                    // A continuation on a line of its own joins it to the next.
                    let mut j = p + 1;
                    if self.byte(j) == b'\r' {
                        j += 1;
                    }
                    if !self.string_follows(j + 1) {
                        self.extra("line_continuation", p, j + 1);
                    }
                    p = j;
                }
                _ => break indent,
            }
        };
        self.at = p;
        let top = self.indents.last().copied().unwrap_or(0);
        if next_indent > top {
            if ends_block_header {
                self.indents.push(next_indent);
                self.out.push(Lx {
                    k: L::Indent,
                    start: p0,
                    end: p0,
                });
            }
            return;
        }
        let mut k = 0;
        while self.indents.len() > 1 && next_indent < *self.indents.last().unwrap_or(&0) {
            let cur = *self.indents.last().unwrap_or(&0);
            while k < comments.len() && comments[k].0 >= cur {
                k += 1;
            }
            let at = if k == 0 { p0 } else { comments[k - 1].1 };
            self.indents.pop();
            self.out.push(Lx {
                k: L::Dedent,
                start: at,
                end: at,
            });
        }
    }

    /// Whether the line's last token is an operator still waiting for its
    /// right operand.
    fn needs_operand(&self) -> bool {
        if self.at >= self.b.len() {
            return false;
        }
        let n = self.out.len();
        let Some(last) = self.out.last() else {
            return false;
        };
        match last.k {
            L::Op(o) => {
                matches!(
                    o,
                    "+" | "-"
                        | "/"
                        | "%"
                        | "@"
                        | "&"
                        | "|"
                        | "^"
                        | "~"
                        | "**"
                        | "//"
                        | "<<"
                        | ">>"
                        | "<"
                        | ">"
                        | "=="
                        | "!="
                        | "<="
                        | ">="
                        | "<>"
                        | "="
                        | ":="
                        | "->"
                ) || AUGMENTED_OPS.contains(&o)
                    || (o == "*"
                        && n >= 2
                        && !matches!(
                            &self.src[self.out[n - 2].start..self.out[n - 2].end],
                            "import" | "(" | ","
                        ))
            }
            L::Name => matches!(
                &self.src[last.start..last.end],
                "and" | "or" | "not" | "in" | "is" | "if" | "else"
            ),
            _ => false,
        }
    }

    fn dedent_all(&mut self, at: usize) {
        while self.indents.len() > 1 {
            self.indents.pop();
            self.out.push(Lx {
                k: L::Dedent,
                start: at,
                end: at,
            });
        }
    }

    /// One token at `self.at`, which is not whitespace.
    fn token(&mut self) {
        let start = self.at;
        let c = self.byte(start);
        if c == b'"' || c == b'\'' {
            self.string(start);
            return;
        }
        if c.is_ascii_digit() || (c == b'.' && self.byte(start + 1).is_ascii_digit()) {
            self.number();
            return;
        }
        if is_name_start(self.src, start) {
            let mut j = start;
            while j < self.b.len() && is_name_char(self.src, j) {
                j += char_len(self.src, j);
            }
            let word = &self.src[start..j];
            if matches!(self.byte(j), b'"' | b'\'')
                && word.len() <= 3
                && word.bytes().all(|b| b"rRbBuUfFtT".contains(&b))
            {
                self.string(start);
                return;
            }
            self.at = j;
            self.push(L::Name, start, j);
            return;
        }
        for op in OPS {
            if self.src[start..].starts_with(op) {
                match *op {
                    "(" | "[" | "{" => self.depth += 1,
                    ")" | "]" | "}" => self.depth = self.depth.saturating_sub(1),
                    _ => {}
                }
                self.at = start + op.len();
                self.push(L::Op(op), start, self.at);
                return;
            }
        }
        self.errors = true;
        self.at = start + char_len(self.src, start);
        self.push(L::Bad, start, self.at);
    }

    fn number(&mut self) {
        let start = self.at;
        let digits = |lx: &mut Self, ok: fn(u8) -> bool| {
            let from = lx.at;
            while ok(lx.byte(lx.at)) || (lx.byte(lx.at) == b'_' && ok(lx.byte(lx.at + 1))) {
                lx.at += 1;
            }
            lx.at > from
        };
        if self.byte(start) == b'0' && matches!(self.byte(start + 1) | 0x20, b'x' | b'o' | b'b') {
            let base = self.byte(start + 1) | 0x20;
            self.at += 2;
            if self.byte(self.at) == b'_' {
                self.at += 1;
            }
            match base {
                b'x' => digits(self, |c| c.is_ascii_hexdigit()),
                b'o' => digits(self, |c| (b'0'..=b'7').contains(&c)),
                _ => digits(self, |c| c == b'0' || c == b'1'),
            };
            if matches!(self.byte(self.at), b'l' | b'L') {
                self.at += 1;
            }
            self.push(L::Int, start, self.at);
            return;
        }
        let mut float = false;
        digits(self, |c| c.is_ascii_digit());
        if self.byte(self.at) == b'.' {
            self.at += 1;
            float = true;
            digits(self, |c| c.is_ascii_digit());
        }
        if matches!(self.byte(self.at), b'e' | b'E') {
            let mut j = self.at + 1;
            if matches!(self.byte(j), b'+' | b'-') {
                j += 1;
            }
            if self.byte(j).is_ascii_digit() {
                self.at = j;
                digits(self, |c| c.is_ascii_digit());
                float = true;
            }
        }
        // An imaginary suffix, or Python 2's long one on an integer.
        if matches!(self.byte(self.at), b'j' | b'J')
            || (!float && matches!(self.byte(self.at), b'l' | b'L'))
        {
            self.at += 1;
        }
        let k = if float { L::Float } else { L::Int };
        self.push(k, start, self.at);
    }

    /// A string at `start` (its prefix, if any, first).
    fn string(&mut self, start: usize) {
        let (mut raw, mut bytes, mut format) = (false, false, false);
        let mut i = start;
        while let Some(c) = self.b.get(i).filter(|c| c.is_ascii_alphabetic()) {
            match c | 0x20 {
                b'r' => raw = true,
                b'b' => bytes = true,
                b'f' | b't' => format = true,
                _ => {}
            }
            i += 1;
        }
        let q = self.byte(i);
        let triple = self.byte(i + 1) == q && self.byte(i + 2) == q;
        i += if triple { 3 } else { 1 };
        self.push(L::StrStart, start, i);
        self.at = i;
        self.string_body(q, triple, raw, bytes, format);
    }

    /// A string's body, in the pieces tree-sitter's scanner makes of it.
    /// `run` is where the scanner's current piece began and `has_content`
    /// its flag: a raw string's backslash pairs set no content, so a piece
    /// of nothing else before the closing quote is that quote's (the
    /// scanner's `string_end` takes it).
    fn string_body(&mut self, q: u8, triple: bool, raw: bool, bytes: bool, format: bool) {
        let mut run = self.at;
        let mut has_content = false;
        loop {
            let at = self.at;
            if at >= self.b.len() {
                self.text(run, at);
                self.errors = true;
                return;
            }
            let c = self.byte(at);
            if c == q {
                if !triple || (self.byte(at + 1) == q && self.byte(at + 2) == q) {
                    let end = at + if triple { 3 } else { 1 };
                    if at > run && !has_content {
                        self.push(L::StrEnd, run, end);
                    } else {
                        self.text(run, at);
                        self.push(L::StrEnd, at, end);
                    }
                    self.at = end;
                    return;
                }
                // One or two quotes inside a triple-quoted string end a
                // piece of its text.
                self.at = at + if self.byte(at + 1) == q { 2 } else { 1 };
                self.text(run, self.at);
                run = self.at;
                has_content = false;
                continue;
            }
            if c == b'\n' && !triple {
                self.text(run, at);
                self.errors = true;
                return;
            }
            if c == b'\\' {
                if raw {
                    self.at += 1;
                    if self.byte(self.at) == q || self.byte(self.at) == b'\\' {
                        self.at += 1;
                    }
                    if self.byte(self.at) == b'\r' {
                        self.at += 1;
                    }
                    if self.byte(self.at) == b'\n' {
                        self.at += 1;
                    }
                    continue;
                }
                if bytes && matches!(self.byte(at + 1), b'N' | b'u' | b'U') {
                    self.at += 2;
                    has_content = true;
                    continue;
                }
                self.text(run, at);
                match escape_len(self.b, at) {
                    Some(n) => self.push(L::Escape, at, at + n),
                    None => self.push(L::NotEscape, at, at + 1),
                }
                self.at = self.out.last().map_or(at + 1, |t| t.end);
                run = self.at;
                has_content = false;
                continue;
            }
            if format && (c == b'{' || c == b'}') {
                if at > run && !has_content {
                    // A raw string's backslashes and nothing else before a
                    // brace: tree-sitter's scanner gives up on the piece,
                    // and its lexer reads the backslashes as escapes.
                    let mut k = run;
                    while k < at {
                        match escape_len(self.b, k).filter(|n| k + n <= at) {
                            Some(n) => {
                                self.push(L::Escape, k, k + n);
                                k += n;
                            }
                            None => {
                                self.push(L::NotEscape, k, k + 1);
                                k += 1;
                            }
                        }
                    }
                    run = at;
                }
                self.text(run, at);
                if self.byte(at + 1) == c {
                    self.push(L::EscInterp, at, at + 2);
                    self.at = at + 2;
                } else if c == b'}' {
                    self.errors = true;
                    self.push(L::Bad, at, at + 1);
                    self.at = at + 1;
                } else {
                    self.interpolation();
                }
                run = self.at;
                has_content = false;
                continue;
            }
            self.at += char_len(self.src, at);
            has_content = true;
        }
    }

    fn text(&mut self, from: usize, to: usize) {
        if to > from {
            self.push(L::StrText, from, to);
        }
    }

    /// An f-string interpolation at its `{`: code up to its `}`, with a
    /// conversion and a format specifier where they are.
    fn interpolation(&mut self) {
        let outer = self.depth;
        self.push(L::Op("{"), self.at, self.at + 1);
        self.at += 1;
        self.depth = 0;
        // Lambdas whose `:` is still to come: theirs, not a format
        // specifier's (as tree-sitter reads `f"{lambda x: x}"`).
        let mut lambdas = 0usize;
        loop {
            self.skip(true);
            if self.at >= self.b.len() {
                self.errors = true;
                break;
            }
            let c = self.byte(self.at);
            if self.depth == 0 {
                if c == b'}' {
                    self.push(L::Op("}"), self.at, self.at + 1);
                    self.at += 1;
                    break;
                }
                if c == b':' && lambdas > 0 && self.byte(self.at + 1) != b'=' {
                    lambdas -= 1;
                    self.push(L::Op(":"), self.at, self.at + 1);
                    self.at += 1;
                    continue;
                }
                if self.src[self.at..].starts_with("lambda") && !is_name_char(self.src, self.at + 6)
                {
                    lambdas += 1;
                }
                if c == b':' {
                    self.push(L::Op(":"), self.at, self.at + 1);
                    self.at += 1;
                    self.format_spec();
                    break;
                }
                if c == b'!' && self.byte(self.at + 1).is_ascii_lowercase() {
                    self.push(L::Conversion, self.at, self.at + 2);
                    self.at += 2;
                    continue;
                }
            }
            self.token();
        }
        self.depth = outer;
    }

    /// A format specifier after its `:`, up to and including the
    /// interpolation's `}`.
    fn format_spec(&mut self) {
        loop {
            let run = self.at;
            while self.at < self.b.len() && !matches!(self.byte(self.at), b'{' | b'}' | b'\n') {
                self.at += char_len(self.src, self.at);
            }
            if self.at > run {
                self.push(L::FmtText, run, self.at);
            }
            match self.byte(self.at) {
                // Newlines are whitespace here, as everywhere in tree-sitter.
                b'\n' => self.at += 1,
                b'{' => self.interpolation(),
                b'}' => {
                    self.push(L::Op("}"), self.at, self.at + 1);
                    self.at += 1;
                    return;
                }
                _ => {
                    self.errors = true;
                    return;
                }
            }
        }
    }
}

/// The length of the escape sequence at `at` (a backslash), if it is one.
fn escape_len(b: &[u8], at: usize) -> Option<usize> {
    let hex = |from: usize, n: usize| {
        b.get(from..from + n)
            .is_some_and(|s| s.iter().all(u8::is_ascii_hexdigit))
    };
    match *b.get(at + 1)? {
        b'u' if hex(at + 2, 4) => Some(6),
        b'U' if hex(at + 2, 8) => Some(10),
        b'x' if hex(at + 2, 2) => Some(4),
        c if c.is_ascii_digit() => {
            let n = b[at + 1..]
                .iter()
                .take(3)
                .take_while(|c| c.is_ascii_digit())
                .count();
            Some(1 + n)
        }
        b'\r' if b.get(at + 2) == Some(&b'\n') => Some(3),
        b'\n' => Some(2),
        b'\'' | b'"' | b'a' | b'b' | b'f' | b'r' | b'n' | b't' | b'v' | b'\\' => Some(2),
        b'N' if b.get(at + 2) == Some(&b'{') => {
            let close = b[at + 3..].iter().position(|c| *c == b'}')?;
            (close > 0).then_some(4 + close)
        }
        _ => None,
    }
}

fn char_len(src: &str, at: usize) -> usize {
    src[at..].chars().next().map_or(1, char::len_utf8)
}

fn is_name_start(src: &str, at: usize) -> bool {
    src[at..]
        .chars()
        .next()
        .is_some_and(|c| c == '_' || c.is_alphabetic() || (!c.is_ascii() && is_xid_extra(c)))
}

fn is_name_char(src: &str, at: usize) -> bool {
    src[at..]
        .chars()
        .next()
        .is_some_and(|c| c == '_' || c.is_alphanumeric() || (!c.is_ascii() && is_xid_extra(c)))
}

/// Non-ASCII characters that continue a name though not alphanumeric:
/// combining marks and the like. Whitespace and punctuation are not.
fn is_xid_extra(c: char) -> bool {
    !c.is_whitespace()
        && !matches!(c, '\u{a0}' | '\u{2060}' | '\u{200b}' | '\u{feff}')
        && !c.is_ascii_punctuation()
        && (c as u32) >= 0x300
        && !('\u{2000}'..='\u{2bff}').contains(&c)
        && !('\u{3000}'..='\u{303f}').contains(&c)
        && !('\u{ff00}'..='\u{ffef}').contains(&c)
}
