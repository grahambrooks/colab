//! Java's tokens: words (identifiers and keywords alike; the parser reads
//! keywords by their text, since many of Java's are contextual), numbers
//! by their literal kind, character literals, strings and text blocks in
//! their parts, and operators. `>` is always one token, so `>>` can close
//! two type-argument lists; the expression parser joins adjacent ones into
//! `>>`, `>>>`, `>>=` and `>>>=`. Comments are extras, placed after
//! parsing.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum L {
    Name,
    Op(&'static str),
    /// A number, by its literal kind (`decimal_integer_literal`, …).
    Num(&'static str),
    Char,
    /// A string's opening `"` or a text block's `"""`.
    StrOpen,
    StrClose,
    /// `string_fragment` or `multiline_string_fragment`.
    Frag(&'static str),
    Escape,
    /// A text block escape tree-sitter-java keeps as no node (`\` and a
    /// character that is not one of `bfnrts'"\`): text inside the string.
    HiddenEscape,
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
    pub kind: &'static str,
    pub start: usize,
    pub end: usize,
}

const OPS: &[&str] = &[
    ">>>=", "<<=", "...", "->", "::", "++", "--", "&&", "||", "==", "!=", "<=", ">=", "+=", "-=",
    "*=", "/=", "&=", "|=", "^=", "%=", "<<", "\\{", "+", "-", "*", "/", "%", "&", "|", "^", "~",
    "!", "<", ">", "=", "?", ":", ";", ",", ".", "@", "(", ")", "[", "]", "{", "}",
];

pub(crate) struct Lexed {
    pub tokens: Vec<Lx>,
    pub extras: Vec<Extra>,
    pub errors: bool,
}

pub(crate) fn lex(src: &str) -> Lexed {
    let mut lx = Lexer {
        src,
        b: src.as_bytes(),
        at: 0,
        out: Vec::new(),
        extras: Vec::new(),
        errors: false,
    };
    if src.starts_with('\u{feff}') {
        lx.at = 3;
    }
    lx.run(false);
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

struct Lexer<'s> {
    src: &'s str,
    b: &'s [u8],
    at: usize,
    out: Vec<Lx>,
    extras: Vec<Extra>,
    errors: bool,
}

impl Lexer<'_> {
    fn byte(&self, at: usize) -> u8 {
        self.b.get(at).copied().unwrap_or(0)
    }

    fn push(&mut self, k: L, start: usize, end: usize) {
        self.out.push(Lx { k, start, end });
        self.at = end;
    }

    /// Tokens to the end, or (`interpolation`) to the `}` that closes a
    /// string template's `\{`.
    fn run(&mut self, interpolation: bool) {
        let mut depth = 0usize;
        loop {
            self.skip();
            if self.at >= self.b.len() {
                if interpolation {
                    self.errors = true;
                }
                return;
            }
            if interpolation && self.byte(self.at) == b'}' && depth == 0 {
                self.push(L::Op("}"), self.at, self.at + 1);
                return;
            }
            match self.byte(self.at) {
                b'{' => depth += 1,
                b'}' => depth = depth.saturating_sub(1),
                _ => {}
            }
            self.token();
        }
    }

    fn skip(&mut self) {
        loop {
            let c = self.byte(self.at);
            if c.is_ascii_whitespace() {
                self.at += 1;
            } else if c == b'/' && self.byte(self.at + 1) == b'/' {
                let end = self.src[self.at..]
                    .find('\n')
                    .map_or(self.b.len(), |i| self.at + i);
                self.extra("line_comment", self.at, end);
            } else if c == b'/' && self.byte(self.at + 1) == b'*' {
                let end = match self.src[self.at + 2..].find("*/") {
                    Some(i) => self.at + 2 + i + 2,
                    None => {
                        self.errors = true;
                        self.b.len()
                    }
                };
                self.extra("block_comment", self.at, end);
            } else if c >= 0x80 && self.src[self.at..].starts_with(char::is_whitespace) {
                self.at += char_len(self.src, self.at);
            } else {
                return;
            }
        }
    }

    fn extra(&mut self, kind: &'static str, start: usize, end: usize) {
        self.extras.push(Extra { kind, start, end });
        self.at = end;
    }

    fn token(&mut self) {
        let start = self.at;
        let c = self.byte(start);
        if c == b'"' {
            if self.src[start..].starts_with("\"\"\"") {
                self.text_block(start);
            } else {
                self.string(start);
            }
            return;
        }
        if c == b'\'' {
            self.character(start);
            return;
        }
        if c.is_ascii_digit() || (c == b'.' && self.byte(start + 1).is_ascii_digit()) {
            self.number(start);
            return;
        }
        if is_name_start(self.src, start) {
            let mut j = start;
            while j < self.b.len() && is_name_char(self.src, j) {
                j += char_len(self.src, j);
            }
            // `non-sealed` is one modifier.
            if &self.src[start..j] == "non" && self.src[j..].starts_with("-sealed") {
                let k = j + 7;
                if !(k < self.b.len() && is_name_char(self.src, k)) {
                    j = k;
                }
            }
            self.push(L::Name, start, j);
            return;
        }
        // `>>` is not in the table: `>` stays single, so `>>` may close two
        // lists of type arguments. (`>=` and `>>>=` are kept whole: no type
        // list closes on them.)
        for op in OPS {
            if self.src[start..].starts_with(op) {
                self.push(L::Op(op), start, start + op.len());
                return;
            }
        }
        self.errors = true;
        let end = start + char_len(self.src, start);
        self.push(L::Bad, start, end);
    }

    fn number(&mut self, start: usize) {
        let b = self.b;
        let mut j = start;
        let digits = |j: &mut usize, ok: fn(u8) -> bool| {
            let from = *j;
            while *j < b.len() && (ok(b[*j]) || (b[*j] == b'_' && *j > from)) {
                *j += 1;
            }
            // A number does not end in `_`.
            while *j > from && b[*j - 1] == b'_' {
                *j -= 1;
            }
            *j > from
        };
        let lower = |at: usize| b.get(at).map_or(0, u8::to_ascii_lowercase);
        if b[j] == b'0' && matches!(lower(j + 1), b'x' | b'b' | b'o') {
            let base = lower(j + 1);
            j += 2;
            if base == b'x' {
                digits(&mut j, |c| c.is_ascii_hexdigit());
                let mut float = false;
                if self.byte(j) == b'.' {
                    j += 1;
                    digits(&mut j, |c| c.is_ascii_hexdigit());
                    float = true;
                }
                if lower(j) == b'p' {
                    j += 1;
                    if matches!(self.byte(j), b'+' | b'-') {
                        j += 1;
                    }
                    digits(&mut j, |c| c.is_ascii_digit());
                    float = true;
                    if matches!(lower(j), b'f' | b'd') {
                        j += 1;
                    }
                }
                if !float && lower(j) == b'l' {
                    j += 1;
                }
                let kind = if float {
                    "hex_floating_point_literal"
                } else {
                    "hex_integer_literal"
                };
                self.push(L::Num(kind), start, j);
                return;
            }
            let ok: fn(u8) -> bool = if base == b'b' {
                |c| c == b'0' || c == b'1'
            } else {
                |c| (b'0'..=b'7').contains(&c)
            };
            digits(&mut j, ok);
            if lower(j) == b'l' {
                j += 1;
            }
            let kind = if base == b'b' {
                "binary_integer_literal"
            } else {
                "octal_integer_literal"
            };
            self.push(L::Num(kind), start, j);
            return;
        }
        let mut float = false;
        digits(&mut j, |c| c.is_ascii_digit());
        let int_end = j;
        // A decimal point: `1.5`, `1.`, `1.e5`, `1.f`; not `1..` or a
        // name after the dot.
        if self.byte(j) == b'.' {
            let next = self.byte(j + 1);
            let after = self.byte(j + 2);
            let exponent = matches!(next, b'e' | b'E')
                && (after.is_ascii_digit() || matches!(after, b'+' | b'-'));
            let suffix = matches!(next.to_ascii_lowercase(), b'f' | b'd')
                && !(after.is_ascii_alphanumeric() || after == b'_');
            let exponent_or_suffix = exponent || suffix;
            if next.is_ascii_digit()
                || exponent_or_suffix
                || !(next == b'.' || next.is_ascii_alphabetic() || next == b'_' || next == b'$')
            {
                j += 1;
                digits(&mut j, |c| c.is_ascii_digit());
                float = true;
            }
        }
        if lower(j) == b'e' {
            let mut k = j + 1;
            if matches!(self.byte(k), b'+' | b'-') {
                k += 1;
            }
            if self.byte(k).is_ascii_digit() {
                j = k;
                digits(&mut j, |c| c.is_ascii_digit());
                float = true;
            }
        }
        if matches!(lower(j), b'f' | b'd') {
            j += 1;
            float = true;
        } else if !float && lower(j) == b'l' {
            j += 1;
        }
        let kind = if float {
            "decimal_floating_point_literal"
        } else if b[start] == b'0' && int_end > start + 1 {
            "octal_integer_literal"
        } else {
            "decimal_integer_literal"
        };
        self.push(L::Num(kind), start, j);
    }

    fn character(&mut self, start: usize) {
        let mut j = start + 1;
        loop {
            match self.byte(j) {
                b'\\' => j += 2,
                b'\'' => {
                    j += 1;
                    break;
                }
                b'\n' | 0 => {
                    self.errors = true;
                    break;
                }
                _ => j += char_len(self.src, j),
            }
        }
        let j = j.min(self.b.len());
        self.push(L::Char, start, j);
    }

    fn string(&mut self, start: usize) {
        self.push(L::StrOpen, start, start + 1);
        let mut run = self.at;
        loop {
            let at = self.at;
            match self.byte(at) {
                b'"' => {
                    self.frag("string_fragment", run, at);
                    self.push(L::StrClose, at, at + 1);
                    return;
                }
                b'\\' if self.byte(at + 1) == b'{' => {
                    self.frag("string_fragment", run, at);
                    self.push(L::Op("\\{"), at, at + 2);
                    self.run(true);
                    run = self.at;
                }
                b'\\' => {
                    self.frag("string_fragment", run, at);
                    let n = escape_len(self.b, at);
                    self.push(L::Escape, at, at + n);
                    run = self.at;
                }
                b'\n' | b'\r' => {
                    self.frag("string_fragment", run, at);
                    self.errors = true;
                    return;
                }
                _ if at >= self.b.len() => {
                    self.frag("string_fragment", run, at);
                    self.errors = true;
                    return;
                }
                _ => self.at += char_len(self.src, at),
            }
        }
    }

    fn frag(&mut self, kind: &'static str, from: usize, to: usize) {
        if to > from {
            self.push(L::Frag(kind), from, to);
        }
    }

    /// A text block: fragments break at a backslash and at a `"`, which
    /// starts a fragment of its own (tree-sitter's two fragment tokens).
    fn text_block(&mut self, start: usize) {
        self.push(L::StrOpen, start, start + 3);
        loop {
            let at = self.at;
            if at >= self.b.len() {
                self.errors = true;
                return;
            }
            match self.byte(at) {
                b'"' if self.src[at..].starts_with("\"\"\"") => {
                    self.push(L::StrClose, at, at + 3);
                    return;
                }
                b'"' => {
                    let mut j = at + 1;
                    loop {
                        match self.byte(j) {
                            b'"' | 0 => break,
                            b'\\' if self.byte(j + 1) == b'"' => j += 2,
                            b'\\' => break,
                            _ => j += char_len(self.src, j),
                        }
                    }
                    self.push(L::Frag("multiline_string_fragment"), at, j);
                }
                b'\\' if self.byte(at + 1) == b'{' => {
                    self.push(L::Op("\\{"), at, at + 2);
                    self.run(true);
                }
                b'\\' => {
                    let c = self.byte(at + 1);
                    let n = escape_len(self.b, at);
                    if b"bfnrts'\"\\".contains(&c) || n > 2 {
                        self.push(L::Escape, at, at + n);
                    } else {
                        let end = (at + 1 + char_len(self.src, at + 1)).min(self.b.len());
                        self.push(L::HiddenEscape, at, end);
                    }
                }
                _ => {
                    let mut j = at;
                    while j < self.b.len() && !matches!(self.byte(j), b'"' | b'\\') {
                        j += char_len(self.src, j);
                    }
                    self.push(L::Frag("multiline_string_fragment"), at, j);
                }
            }
        }
    }
}

/// The length of the escape sequence at `at` (a backslash).
fn escape_len(b: &[u8], at: usize) -> usize {
    let hex = |from: usize, n: usize| {
        b.get(from..from + n)
            .is_some_and(|s| s.iter().all(u8::is_ascii_hexdigit))
    };
    match b.get(at + 1).copied().unwrap_or(0) {
        b'u' if b.get(at + 2) == Some(&b'{') => {
            let n = b[at + 3..]
                .iter()
                .take_while(|c| c.is_ascii_hexdigit())
                .count();
            if b.get(at + 3 + n) == Some(&b'}') && n > 0 {
                n + 4
            } else {
                2
            }
        }
        b'u' => {
            // `\uuuu0041` is one escape too.
            let mut k = at + 1;
            while b.get(k) == Some(&b'u') {
                k += 1;
            }
            if hex(k, 4) { k + 4 - at } else { 2 }
        }
        b'x' if hex(at + 2, 2) => 4,
        b'0'..=b'7' => {
            let n = b[at + 1..]
                .iter()
                .take(3)
                .take_while(|c| (b'0'..=b'7').contains(c))
                .count();
            1 + n
        }
        0 => 1,
        c if c >= 0x80 => {
            let s = std::str::from_utf8(&b[at + 1..]).ok();
            1 + s.and_then(|s| s.chars().next()).map_or(1, char::len_utf8)
        }
        _ => 2,
    }
}

fn char_len(src: &str, at: usize) -> usize {
    src[at..].chars().next().map_or(1, char::len_utf8)
}

fn is_name_start(src: &str, at: usize) -> bool {
    src[at..]
        .chars()
        .next()
        .is_some_and(|c| c == '_' || c == '$' || c.is_alphabetic())
}

fn is_name_char(src: &str, at: usize) -> bool {
    src[at..].chars().next().is_some_and(|c| {
        c == '_'
            || c == '$'
            || c.is_alphanumeric()
            || (!c.is_ascii() && !c.is_whitespace() && (c as u32) >= 0x300 && c != '\u{feff}')
    })
}
