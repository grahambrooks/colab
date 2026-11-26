//! PHP's tokens, lexed up front.
//!
//! Strings are cut where tree-sitter-php's external scanner cuts them, since
//! that decides the tree's nodes: a double-quoted string, a heredoc's line or
//! a shell command is `Content` runs, `Escape`s and the tokens of its
//! interpolations (`$a`, `$a->b`, `$a[0]`, `{…}` and `${…}`, whose code is
//! lexed as code until the brace that closes it). A heredoc's lines never
//! hold their newline; a nowdoc's `NowdocString` after the first holds the
//! newlines before it. Comments are extras, kept apart.

/// A token's kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum L {
    Name,
    Int,
    Float,
    Op(&'static str),
    /// `<?php`, `<?=`, `<?`.
    Tag,
    /// `?>`.
    EndTag,
    /// A string's opening quote (`'`, `"`, `` ` ``), with its `b` prefix if
    /// any.
    StrOpen(&'static str),
    StrClose(&'static str),
    Content,
    Escape,
    NowdocString,
    /// A heredoc's or nowdoc's word after `<<<`.
    HeredocStart,
    HeredocEnd,
    /// The newline that opens a heredoc's or nowdoc's body.
    BodyNl,
    /// A character nothing reads.
    Error,
    Eof,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Lx {
    pub k: L,
    pub start: usize,
    pub end: usize,
}

pub(crate) struct Lexed {
    pub tokens: Vec<Lx>,
    /// Comments, in order.
    pub extras: Vec<(usize, usize)>,
    pub errors: bool,
}

/// What ends an interpolated run.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Dq,
    Shell,
    Heredoc,
}

const OPS: &[&str] = &[
    "<=>", "**=", "...", "<<=", ">>=", "===", "!==", "??=", "?->", "->", "=>", "::", "++", "--",
    "**", "??", "==", "!=", "<>", "<=", ">=", "<<", ">>", "&&", "||", "|>", "+=", "-=", "*=", "/=",
    "%=", ".=", "&=", "|=", "^=", "!", "%", "&", "(", ")", "*", "+", ",", "-", ".", "/", ":", ";",
    "<", "=", ">", "?", "@", "[", "\\", "]", "^", "{", "|", "}", "~", "$",
];

struct Lexer<'s> {
    src: &'s [u8],
    text: &'s str,
    at: usize,
    tokens: Vec<Lx>,
    extras: Vec<(usize, usize)>,
    errors: bool,
}

pub(crate) fn lex(text: &str) -> Lexed {
    let mut lx = Lexer {
        src: text.as_bytes(),
        text,
        at: 0,
        tokens: Vec::new(),
        extras: Vec::new(),
        errors: false,
    };
    lx.code(false);
    Lexed {
        tokens: lx.tokens,
        extras: lx.extras,
        errors: lx.errors,
    }
}

fn name_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b >= 0x80
}

fn name_char(b: u8) -> bool {
    name_start(b) || b.is_ascii_digit()
}

impl Lexer<'_> {
    fn byte(&self, i: usize) -> u8 {
        self.src.get(i).copied().unwrap_or(0)
    }

    fn eof_at(&self, i: usize) -> bool {
        i >= self.src.len()
    }

    fn push(&mut self, k: L, start: usize, end: usize) {
        self.tokens.push(Lx { k, start, end });
    }

    /// The length of the whitespace character at `i` (PHP's extras: ASCII
    /// space and U+00A0, U+200B, U+2060, U+FEFF), or 0.
    fn space_at(&self, i: usize) -> usize {
        match self.byte(i) {
            b' ' | b'\t' | b'\n' | b'\r' | 0x0B | 0x0C => 1,
            0xC2 if self.byte(i + 1) == 0xA0 => 2,
            0xE2 if self.byte(i + 1) == 0x80 && self.byte(i + 2) == 0x8B => 3,
            0xE2 if self.byte(i + 1) == 0x81 && self.byte(i + 2) == 0xA0 => 3,
            0xEF if self.byte(i + 1) == 0xBB && self.byte(i + 2) == 0xBF => 3,
            _ => 0,
        }
    }

    /// The length of the (UTF-8) character at `i`.
    fn char_len(&self, i: usize) -> usize {
        self.text[i..].chars().next().map_or(1, char::len_utf8)
    }

    fn name_end(&self, mut i: usize) -> usize {
        while !self.eof_at(i) && name_char(self.byte(i)) && self.space_at(i) == 0 {
            i += self.char_len(i);
        }
        i
    }

    /// Code tokens to the end, or (`brace`) to the `}` that closes an
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
            let start = self.at;
            let b = self.byte(start);
            if brace && b == b'{' {
                depth += 1;
            }
            if brace && b == b'}' {
                if depth == 0 {
                    self.push(L::Op("}"), start, start + 1);
                    self.at += 1;
                    return;
                }
                depth -= 1;
            }
            self.token();
        }
    }

    /// Whitespace and comments.
    fn skip_trivia(&mut self) {
        loop {
            let n = self.space_at(self.at);
            if n > 0 {
                self.at += n;
                continue;
            }
            let b = self.byte(self.at);
            let next = self.byte(self.at + 1);
            if (b == b'/' && next == b'/') || (b == b'#' && next != b'[') {
                let start = self.at;
                let i = self.line_comment_end(start);
                self.extras.push((start, i));
                self.at = i;
                continue;
            }
            if b == b'/' && next == b'*' {
                let start = self.at;
                let end = self.text[start + 2..].find("*/").map_or_else(
                    || {
                        self.errors = true;
                        self.src.len()
                    },
                    |p| start + 2 + p + 2,
                );
                self.extras.push((start, end));
                self.at = end;
                continue;
            }
            return;
        }
    }

    /// The end of the `//` or `#` comment at `start`: before a line end or
    /// `?>`, a `?` taking the character after it (`??` is two), a `?` just
    /// before the line end taking the line end too.
    fn line_comment_end(&self, start: usize) -> usize {
        let mut i = start + if self.byte(start) == b'/' { 2 } else { 1 };
        loop {
            match self.byte(i) {
                _ if self.eof_at(i) => return i,
                b'\n' | b'\r' => return i,
                b'?' => match self.byte(i + 1) {
                    b'>' => return i,
                    b'\n' => return i + 2,
                    b'\r' => return i + if self.byte(i + 2) == b'\n' { 3 } else { 2 },
                    _ if self.eof_at(i + 1) => return i + 1,
                    _ => i += 1 + self.char_len(i + 1),
                },
                _ => i += self.char_len(i),
            }
        }
    }

    /// Whether only whitespace and comments lie in `from..to`; the
    /// comments, if so.
    fn comments_only(&self, from: usize, to: usize) -> Option<Vec<(usize, usize)>> {
        let mut out = Vec::new();
        let mut i = from;
        while i < to {
            let n = self.space_at(i);
            if n > 0 {
                i += n;
                continue;
            }
            let (b, next) = (self.byte(i), self.byte(i + 1));
            let end = if (b == b'/' && next == b'/') || (b == b'#' && next != b'[') {
                self.line_comment_end(i)
            } else if b == b'/' && next == b'*' {
                i + 2 + self.text[i + 2..].find("*/")? + 2
            } else {
                return None;
            };
            if end > to {
                return None;
            }
            out.push((i, end));
            i = end;
        }
        Some(out)
    }

    fn token(&mut self) {
        let start = self.at;
        let b = self.byte(start);
        let next = self.byte(start + 1);
        if b == b'<' && next == b'?' {
            let rest = &self.text[start + 2..];
            let len = if rest.len() >= 3 && rest[..3].eq_ignore_ascii_case("php") {
                5
            } else if rest.starts_with('=') {
                3
            } else {
                2
            };
            self.push(L::Tag, start, start + len);
            self.at = start + len;
            return;
        }
        if b == b'?' && next == b'>' {
            self.push(L::EndTag, start, start + 2);
            self.at = start + 2;
            return;
        }
        if b == b'#' && next == b'[' {
            self.push(L::Op("#["), start, start + 2);
            self.at = start + 2;
            return;
        }
        if (b == b'b' || b == b'B') && (next == b'\'' || next == b'"') {
            self.at = start + 1;
            return self.string(start);
        }
        if b == b'\'' || b == b'"' || b == b'`' {
            return self.string(start);
        }
        if b == b'<' && next == b'<' && self.byte(start + 2) == b'<' && self.heredoc() {
            return;
        }
        if b.is_ascii_digit() || (b == b'.' && next.is_ascii_digit()) {
            return self.number();
        }
        if name_start(b) && self.space_at(start) == 0 {
            let end = self.name_end(start);
            self.push(L::Name, start, end);
            self.at = end;
            return;
        }
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
        while ok(self.byte(i)) || (self.byte(i) == b'_' && ok(self.byte(i + 1))) {
            i += 1;
        }
        i
    }

    fn number(&mut self) {
        let start = self.at;
        let b1 = self.byte(start + 1);
        if self.byte(start) == b'0' {
            let radix: Option<fn(u8) -> bool> = match b1 {
                b'x' | b'X' if self.byte(start + 2).is_ascii_hexdigit() => {
                    Some(|b| b.is_ascii_hexdigit())
                }
                b'b' | b'B' if matches!(self.byte(start + 2), b'0' | b'1') => {
                    Some(|b| b == b'0' || b == b'1')
                }
                b'o' | b'O' => Some(|b| (b'0'..=b'7').contains(&b)),
                _ => None,
            };
            if let Some(ok) = radix {
                let end = self.digits(start + 2, ok);
                self.push(L::Int, start, end);
                self.at = end;
                return;
            }
        }
        let digit = |b: u8| b.is_ascii_digit();
        let mut i = self.digits(start, digit);
        let mut float = false;
        if self.byte(i) == b'.' && self.byte(i + 1) != b'.' {
            float = true;
            i = self.digits(i + 1, digit);
        }
        if matches!(self.byte(i), b'e' | b'E') {
            let mut k = i + 1;
            if matches!(self.byte(k), b'+' | b'-') {
                k += 1;
            }
            if self.byte(k).is_ascii_digit() {
                float = true;
                i = self.digits(k, digit);
            }
        }
        self.push(if float { L::Float } else { L::Int }, start, i);
        self.at = i;
    }

    /// A quoted string from `start` (its `b` prefix, if any, already
    /// passed).
    fn string(&mut self, start: usize) {
        let q = self.byte(self.at);
        let quote = match q {
            b'\'' => "'",
            b'"' => "\"",
            _ => "`",
        };
        self.at += 1;
        self.push(L::StrOpen(quote), start, self.at);
        match q {
            b'\'' => self.single(),
            b'"' => self.interp(Mode::Dq, None),
            _ => self.interp(Mode::Shell, None),
        }
        if self.byte(self.at) == q && !self.eof_at(self.at) {
            self.push(L::StrClose(quote), self.at, self.at + 1);
            self.at += 1;
        } else {
            self.errors = true;
        }
    }

    /// A single-quoted string's contents: `\\` and `\'` are escapes.
    fn single(&mut self) {
        loop {
            let start = self.at;
            if self.eof_at(start) || self.byte(start) == b'\'' {
                return;
            }
            if self.byte(start) == b'\\' && matches!(self.byte(start + 1), b'\\' | b'\'') {
                self.push(L::Escape, start, start + 2);
                self.at = start + 2;
                continue;
            }
            let mut i = start;
            while !self.eof_at(i)
                && self.byte(i) != b'\''
                && !(self.byte(i) == b'\\' && matches!(self.byte(i + 1), b'\\' | b'\''))
            {
                i += self.char_len(i);
            }
            self.push(L::Content, start, i);
            self.at = i;
        }
    }

    /// The scanner's `is_escapable_sequence` for the `\` at `i - 1`:
    /// whether it stops there, and where it looked to.
    fn escapable(&self, i: usize) -> (bool, usize) {
        match self.byte(i) {
            b'n' | b'r' | b't' | b'v' | b'e' | b'f' | b'\\' | b'$' | b'"' | b'u' => (true, i),
            b'x' => (self.byte(i + 1).is_ascii_hexdigit(), i + 1),
            b'0'..=b'7' => (true, i),
            _ => (false, i),
        }
    }

    /// Where a run of content from `start` ends, scanning from `from`, as
    /// tree-sitter-php's `scan_encapsed_part_string`; `start` when there is
    /// none.
    fn content_end(&self, start: usize, from: usize, mut after_var: bool, mode: Mode) -> usize {
        let mut i = from;
        loop {
            let mark = i;
            if self.eof_at(i) {
                return mark;
            }
            match self.byte(i) {
                b'"' if mode == Mode::Dq => return mark,
                b'`' if mode == Mode::Shell => return mark,
                b'\n' | b'\r' if mode == Mode::Heredoc => return mark,
                b'\\' => {
                    i += 1;
                    let c = self.byte(i);
                    if c == b'{' {
                        i += 1;
                    } else if mode == Mode::Shell && c == b'`' {
                        return mark;
                    } else if mode == Mode::Heredoc && c == b'\\' {
                        i += 1;
                    } else {
                        let (stop, to) = self.escapable(i);
                        if stop {
                            return mark;
                        }
                        i = to;
                    }
                }
                b'$' => {
                    i += 1;
                    let c = self.byte(i);
                    if (name_char(c) && !c.is_ascii_digit()) || c == b'{' {
                        return mark;
                    }
                }
                b'-' if after_var => {
                    i += 1;
                    if self.byte(i) == b'>' {
                        i += 1;
                        if name_char(self.byte(i)) {
                            return mark;
                        }
                    }
                }
                b'[' if after_var => return mark,
                b'{' => {
                    i += 1;
                    if self.byte(i) == b'$' {
                        return mark;
                    }
                }
                _ => i += self.char_len(i),
            }
            after_var = false;
            let _ = start;
        }
    }

    /// An interpolated run (a string's, a shell command's, or one heredoc
    /// line's, `line_from` past the line's indentation), to its closing
    /// quote or the line's end.
    fn interp(&mut self, mode: Mode, mut line_from: Option<usize>) {
        let mut after_var = false;
        loop {
            let start = self.at;
            if self.eof_at(start) {
                self.errors = true;
                return;
            }
            let from = line_from.take().unwrap_or(start);
            let end = self.content_end(start, from, after_var, mode);
            if end > start {
                self.push(L::Content, start, end);
                self.at = end;
                after_var = false;
                continue;
            }
            after_var = false;
            match self.byte(start) {
                b'"' if mode == Mode::Dq => return,
                b'`' if mode == Mode::Shell => return,
                b'\n' | b'\r' if mode == Mode::Heredoc => return,
                b'\\' => self.escape(),
                b'$' if self.byte(start + 1) == b'{' => {
                    self.push(L::Op("$"), start, start + 1);
                    self.push(L::Op("{"), start + 1, start + 2);
                    self.at = start + 2;
                    self.code(true);
                }
                b'$' => {
                    self.push(L::Op("$"), start, start + 1);
                    let end = self.name_end(start + 1);
                    self.push(L::Name, start + 1, end);
                    self.at = end;
                    after_var = !self.simple_part();
                }
                b'{' => {
                    self.push(L::Op("{"), start, start + 1);
                    self.at = start + 1;
                    self.code(true);
                }
                _ => {
                    let n = self.char_len(start);
                    self.errors = true;
                    self.push(L::Content, start, start + n);
                    self.at = start + n;
                }
            }
        }
    }

    /// After a variable in a string: `->name` or `[…]`, if there; whether
    /// one was.
    fn simple_part(&mut self) -> bool {
        let at = self.at;
        if self.byte(at) == b'-' && self.byte(at + 1) == b'>' && name_start(self.byte(at + 2)) {
            self.push(L::Op("->"), at, at + 2);
            let end = self.name_end(at + 2);
            self.push(L::Name, at + 2, end);
            self.at = end;
            return true;
        }
        if self.byte(at) != b'[' {
            return false;
        }
        self.push(L::Op("["), at, at + 1);
        let mut i = at + 1;
        let b = self.byte(i);
        if b == b'-' && self.byte(i + 1).is_ascii_digit() {
            self.push(L::Op("-"), i, i + 1);
            i += 1;
        }
        let b2 = self.byte(i);
        if b2.is_ascii_digit() {
            let end = self.digits(i, |c| c.is_ascii_alphanumeric());
            self.push(L::Int, i, end);
            i = end;
        } else if b2 == b'$' && name_start(self.byte(i + 1)) {
            self.push(L::Op("$"), i, i + 1);
            let end = self.name_end(i + 1);
            self.push(L::Name, i + 1, end);
            i = end;
        } else if name_start(b2) {
            let end = self.name_end(i);
            self.push(L::Name, i, end);
            i = end;
        }
        if self.byte(i) == b']' {
            self.push(L::Op("]"), i, i + 1);
            i += 1;
        } else {
            self.errors = true;
        }
        self.at = i;
        true
    }

    /// An escape sequence at `\`, or `\u` as content.
    fn escape(&mut self) {
        let start = self.at;
        let i = start + 1;
        let end = match self.byte(i) {
            b'n' | b'r' | b't' | b'v' | b'e' | b'f' | b'\\' | b'$' | b'"' | b'`' => Some(i + 1),
            b'0'..=b'7' => {
                let mut k = i;
                while k < i + 3 && (b'0'..=b'7').contains(&self.byte(k)) {
                    k += 1;
                }
                Some(k)
            }
            b'x' if self.byte(i + 1).is_ascii_hexdigit() => {
                let mut k = i + 1;
                while k < i + 3 && self.byte(k).is_ascii_hexdigit() {
                    k += 1;
                }
                Some(k)
            }
            b'u' if self.byte(i + 1) == b'{' && self.byte(i + 2).is_ascii_hexdigit() => {
                let mut k = i + 2;
                while self.byte(k).is_ascii_hexdigit() {
                    k += 1;
                }
                (self.byte(k) == b'}').then_some(k + 1)
            }
            _ => None,
        };
        match end {
            Some(end) => {
                self.push(L::Escape, start, end);
                self.at = end;
            }
            None => {
                // `\u` alone is content of its own; anything else a lone
                // backslash.
                let end = if self.byte(i) == b'u' { i + 1 } else { i };
                self.push(L::Content, start, end);
                self.at = end;
            }
        }
    }

    /// `<<<` and a heredoc or nowdoc, if one is there.
    fn heredoc(&mut self) -> bool {
        let start = self.at;
        let mut i = start + 3;
        while matches!(self.byte(i), b' ' | b'\t') {
            i += 1;
        }
        let quote = self.byte(i);
        let q_at = i;
        if quote == b'"' || quote == b'\'' {
            i += 1;
        }
        if !name_start(self.byte(i)) {
            return false;
        }
        let word_start = i;
        let word_end = self.name_end(i);
        let word = self.text[word_start..word_end].to_string();
        self.push(L::Op("<<<"), start, start + 3);
        // The word's token starts after `<<<` or its quote, the spaces
        // before the word included.
        let mut token_start = start + 3;
        if quote == b'"' || quote == b'\'' {
            let q = if quote == b'"' { "\"" } else { "'" };
            self.push(L::Op(q), q_at, q_at + 1);
            token_start = q_at + 1;
        }
        self.push(L::HeredocStart, token_start, word_end);
        i = word_end;
        if (quote == b'"' || quote == b'\'') && self.byte(i) == quote {
            let q = if quote == b'"' { "\"" } else { "'" };
            self.push(L::Op(q), i, i + 1);
            i += 1;
        }
        self.at = i;
        let nl = self.newline_len(i);
        if nl == 0 {
            self.errors = true;
            return true;
        }
        self.push(L::BodyNl, i, i + nl);
        self.at = i + nl;
        if quote == b'\'' {
            self.nowdoc_lines(word.as_bytes());
        } else {
            self.heredoc_lines(word.as_bytes());
        }
        true
    }

    fn newline_len(&self, i: usize) -> usize {
        match self.byte(i) {
            b'\r' if self.byte(i + 1) == b'\n' => 2,
            b'\n' | b'\r' => 1,
            _ => 0,
        }
    }

    /// Where `tag` ends if it is at `i`, a whole word.
    fn tag_at(&self, i: usize, tag: &[u8]) -> Option<usize> {
        let end = i + tag.len();
        (self.src.get(i..end) == Some(tag) && !name_char(self.byte(end))).then_some(end)
    }

    fn heredoc_lines(&mut self, tag: &[u8]) {
        // Each line's start and its first token's index.
        let mut lines: Vec<(usize, usize)> = Vec::new();
        loop {
            let line = self.at;
            if self.eof_at(line) {
                self.errors = true;
                return;
            }
            let mut i = line;
            while matches!(self.byte(i), b' ' | b'\t' | 0x0B | 0x0C) {
                i += 1;
            }
            if let Some(end) = self.tag_at(i, tag) {
                self.trailing_comments(&lines, line);
                self.push(L::HeredocEnd, i, end);
                self.at = end;
                return;
            }
            lines.push((line, self.tokens.len()));
            // A partly matched tag is content, as the scanner consumed it.
            let mut from = i;
            while from < i + tag.len() && self.byte(from) == tag[from - i] {
                from += 1;
            }
            let from = (from > line).then_some(from);
            self.interp(Mode::Heredoc, from);
            let nl = self.newline_len(self.at);
            if nl == 0 {
                return;
            }
            self.at += nl;
        }
    }

    /// A heredoc's last comment-like line (one `//` or `#` comment, or one
    /// `/* */` over lines), when nothing but whitespace follows it before
    /// the end (at `end_line`) and a line comes before it, is a comment to
    /// tree-sitter, not the body's content: its parse that ends the body
    /// before it wins.
    fn trailing_comments(&mut self, lines: &[(usize, usize)], end_line: usize) {
        let Some((&(_, first_token), comments)) = lines.iter().skip(1).rev().find_map(|l| {
            self.comments_only(l.0, end_line)
                .filter(|c| c.len() == 1)
                .map(|c| (l, c))
        }) else {
            return;
        };
        self.tokens.truncate(first_token);
        self.extras.extend(comments);
        self.extras.sort_unstable();
    }

    fn nowdoc_lines(&mut self, tag: &[u8]) {
        // Blank lines before the first string are no part of it.
        while matches!(self.byte(self.at), b'\n' | b'\r') {
            self.at += 1;
        }
        loop {
            let start = self.at;
            let mut i = start;
            while self.space_at(i) > 0 {
                i += self.space_at(i);
            }
            if let Some(end) = self.tag_at(i, tag) {
                self.push(L::HeredocEnd, i, end);
                self.at = end;
                return;
            }
            if self.eof_at(i) {
                self.errors = true;
                self.at = i;
                return;
            }
            while !self.eof_at(i) && !matches!(self.byte(i), b'\n' | b'\r') {
                i += self.char_len(i);
            }
            self.push(L::NowdocString, start, i);
            self.at = i;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<(L, &str)> {
        lex(src)
            .tokens
            .iter()
            .map(|t| (t.k, &src[t.start..t.end]))
            .collect()
    }

    #[test]
    fn an_interpolated_string_is_cut_as_the_scanner_cuts_it() {
        let src = r#""a $b c {$d} $e->f $g[0] \n""#;
        let k = kinds(src);
        let texts: Vec<&str> = k.iter().map(|t| t.1).collect();
        assert_eq!(
            texts,
            [
                "\"", "a ", "$", "b", " c ", "{", "$", "d", "}", " ", "$", "e", "->", "f", " ",
                "$", "g", "[", "0", "]", " ", "\\n", "\""
            ]
        );
    }

    #[test]
    fn heredoc_lines_and_nowdoc_strings() {
        let src = "<<<EOT\n  a $x\n  EOT;\n<<<'N'\n x\n\n y\nN;";
        let k = kinds(src);
        assert!(k.contains(&(L::HeredocEnd, "EOT")));
        assert!(k.contains(&(L::NowdocString, " x")));
        assert!(k.contains(&(L::NowdocString, "\n\n y")));
    }
}
