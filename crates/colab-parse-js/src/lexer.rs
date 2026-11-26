//! JavaScript's tokens, lexed on demand: what a `/` is (a regex or a
//! division) and what follows a JSX tag (text) depend on where the parser
//! is, so the parser pulls tokens one at a time, in a [`Mode`], and can lex
//! a token again in another mode from the [`State`] before it.
//!
//! Template literals are the lexer's own: a backtick pushes a template
//! frame, `${` a substitution frame (code, whose `}` at depth 0 returns to
//! the template). Comments are extras, recorded with their positions;
//! every token records whether a line ended before it, for automatic
//! semicolons.

/// What a token is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum L {
    Name,
    /// `#name`
    Private,
    Op(&'static str),
    Number,
    /// A quote opening or closing a string (`"` or `'`).
    Quote,
    /// A string's or template's text between escapes.
    Frag,
    Escape,
    /// A template's opening or closing backtick.
    Backtick,
    /// `/pattern/flags`; `Lx::aux` is the closing slash.
    Regex,
    /// JSX text between tags.
    JsxText,
    /// `&amp;` and the like, in JSX.
    CharRef,
    HashBang,
    Bad,
    Eof,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Lx {
    pub k: L,
    pub start: usize,
    pub end: usize,
    /// A line ended between the previous token and this one.
    pub nl: bool,
    pub aux: usize,
}

/// How to lex the next token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    /// Code; `regex` where an operand is expected (a `/` starts a regex).
    Code { regex: bool },
    /// JSX between tags: text, character references, `<`, `{`.
    JsxText,
    /// Inside a JSX tag: names with dashes, strings without escapes.
    JsxTag,
}

/// A lexer frame: inside a template, or a template's substitution (with
/// the braces open in it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Frame {
    Template,
    /// A string's text, in `quote`: the next token is a fragment, an
    /// escape or the closing quote. `jsx` strings have no escapes.
    Str {
        quote: u8,
        jsx: bool,
    },
    Subst(usize),
}

/// Where the lexer is: its position and frames.
#[derive(Clone, Debug, Default)]
pub(crate) struct State {
    pub pos: usize,
    pub frames: Vec<Frame>,
}

/// A comment, placed after parsing.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Extra {
    pub kind: &'static str,
    pub start: usize,
    pub end: usize,
}

const OPS: &[&str] = &[
    ">>>=", "...", "===", "!==", "**=", "<<=", ">>=", ">>>", "&&=", "||=", "??=", "=>", "==", "!=",
    "<=", ">=", "&&", "||", "??", "?.", "++", "--", "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=",
    "**", "<<", ">>", "+", "-", "*", "/", "%", "&", "|", "^", "~", "!", "<", ">", "=", "?", ":",
    ";", ",", ".", "@", "(", ")", "[", "]", "{", "}",
];

pub(crate) struct Lexer<'s> {
    pub src: &'s str,
    b: &'s [u8],
}

impl<'s> Lexer<'s> {
    pub fn new(src: &'s str) -> Self {
        Self {
            src,
            b: src.as_bytes(),
        }
    }

    fn byte(&self, at: usize) -> u8 {
        self.b.get(at).copied().unwrap_or(0)
    }

    /// The token at `state` in `mode`, moving `state` past it; comments
    /// before it go to `extras`.
    pub fn next(&mut self, state: &mut State, mode: Mode, extras: &mut Vec<Extra>) -> Lx {
        // Inside a string or a template, the text is the token.
        match state.frames.last().copied() {
            Some(Frame::Str { quote, jsx }) => return self.string_part(state, quote, jsx),
            Some(Frame::Template) => return self.template_part(state),
            _ => {}
        }
        if mode == Mode::JsxText {
            return self.jsx_text(state);
        }
        let nl = self.skip(state, extras);
        let start = state.pos;
        if start >= self.b.len() {
            return self.tok(state, L::Eof, start, start, nl);
        }
        if start == 0 && self.src.starts_with("#!") {
            let end = self.src.find('\n').unwrap_or(self.b.len());
            return self.tok(state, L::HashBang, 0, end, nl);
        }
        let c = self.byte(start);
        if mode == Mode::JsxTag {
            return self.jsx_tag(state, nl);
        }
        // A substitution's braces.
        if let Some(Frame::Subst(depth)) = state.frames.last().copied() {
            if c == b'}' {
                if depth == 0 {
                    state.frames.pop();
                    return self.tok(state, L::Op("}"), start, start + 1, nl);
                }
                *state.frames.last_mut().unwrap_or_else(|| unreachable!()) =
                    Frame::Subst(depth - 1);
            } else if c == b'{' {
                *state.frames.last_mut().unwrap_or_else(|| unreachable!()) =
                    Frame::Subst(depth + 1);
            }
        }
        match c {
            b'"' | b'\'' => {
                state.frames.push(Frame::Str {
                    quote: c,
                    jsx: false,
                });
                return self.tok(state, L::Quote, start, start + 1, nl);
            }
            b'`' => {
                state.frames.push(Frame::Template);
                return self.tok(state, L::Backtick, start, start + 1, nl);
            }
            b'#' if is_ident_start(self.src, start + 1) => {
                let end = self.ident_end(start + 1);
                return self.tok(state, L::Private, start, end, nl);
            }
            _ => {}
        }
        if c.is_ascii_digit() || (c == b'.' && self.byte(start + 1).is_ascii_digit()) {
            let end = self.number_end(start);
            return self.tok(state, L::Number, start, end, nl);
        }
        if is_ident_start(self.src, start) {
            let end = self.ident_end(start);
            return self.tok(state, L::Name, start, end, nl);
        }
        if c == b'/'
            && matches!(mode, Mode::Code { regex: true })
            && let Some((close, end)) = self.regex_end(start)
        {
            let mut t = self.tok(state, L::Regex, start, end, nl);
            t.aux = close;
            return t;
        }
        for op in OPS {
            if self.src[start..].starts_with(op) {
                // `?.5` is `?` then `.5`.
                if *op == "?." && self.byte(start + 2).is_ascii_digit() {
                    continue;
                }
                return self.tok(state, L::Op(op), start, start + op.len(), nl);
            }
        }
        let end = start + char_len(self.src, start);
        self.tok(state, L::Bad, start, end, nl)
    }

    fn tok(&self, state: &mut State, k: L, start: usize, end: usize, nl: bool) -> Lx {
        state.pos = end;
        Lx {
            k,
            start,
            end,
            nl,
            aux: 0,
        }
    }

    /// Past whitespace and comments; whether a line ended among them.
    fn skip(&mut self, state: &mut State, extras: &mut Vec<Extra>) -> bool {
        let mut nl = false;
        loop {
            let at = state.pos;
            let c = self.byte(at);
            match c {
                b'\n' | b'\r' => {
                    nl = true;
                    state.pos += 1;
                }
                b' ' | b'\t' | 0x0b | 0x0c => state.pos += 1,
                b'/' if self.byte(at + 1) == b'/' => {
                    let end = self.line_end(at);
                    extras.push(Extra {
                        kind: "comment",
                        start: at,
                        end,
                    });
                    state.pos = end;
                }
                b'/' if self.byte(at + 1) == b'*' => {
                    let end = match self.src[at + 2..].find("*/") {
                        Some(i) => at + 2 + i + 2,
                        None => self.b.len(),
                    };
                    if self.src[at..end].contains(['\n', '\r', '\u{2028}', '\u{2029}']) {
                        nl = true;
                    }
                    extras.push(Extra {
                        kind: "comment",
                        start: at,
                        end,
                    });
                    state.pos = end;
                }
                _ if c >= 0x80 => {
                    let ch = self.src[at..].chars().next().unwrap_or('\0');
                    if matches!(ch, '\u{2028}' | '\u{2029}') {
                        nl = true;
                        state.pos += ch.len_utf8();
                    } else if ch.is_whitespace()
                        || matches!(ch, '\u{feff}' | '\u{2060}' | '\u{200b}')
                    {
                        state.pos += ch.len_utf8();
                    } else {
                        return nl;
                    }
                }
                _ => return nl,
            }
        }
    }

    fn line_end(&self, at: usize) -> usize {
        let rest = &self.src[at..];
        let mut end = rest.len();
        for (i, ch) in rest.char_indices() {
            if matches!(ch, '\n' | '\r' | '\u{2028}' | '\u{2029}') {
                end = i;
                break;
            }
        }
        at + end
    }

    fn ident_end(&self, mut at: usize) -> usize {
        loop {
            if self.byte(at) == b'\\' && self.byte(at + 1) == b'u' {
                at = escape_end(self.b, at);
                continue;
            }
            if at < self.b.len() && is_ident_char(self.src, at) {
                at += char_len(self.src, at);
            } else {
                return at;
            }
        }
    }

    fn number_end(&self, start: usize) -> usize {
        let b = self.b;
        let mut j = start;
        let digits = |j: &mut usize, ok: fn(u8) -> bool| {
            while *j < b.len() && (ok(b[*j]) || (b[*j] == b'_' && ok(*b.get(*j + 1).unwrap_or(&0))))
            {
                *j += 1;
            }
        };
        if b[j] == b'0' && matches!(self.byte(j + 1) | 0x20, b'x' | b'b' | b'o') {
            let base = self.byte(j + 1) | 0x20;
            j += 2;
            match base {
                b'x' => digits(&mut j, |c| c.is_ascii_hexdigit()),
                b'b' => digits(&mut j, |c| c == b'0' || c == b'1'),
                _ => digits(&mut j, |c| (b'0'..=b'7').contains(&c)),
            }
            if self.byte(j) == b'n' {
                j += 1;
            }
            return j;
        }
        digits(&mut j, |c| c.is_ascii_digit());
        let mut int = true;
        if self.byte(j) == b'.' {
            j += 1;
            digits(&mut j, |c| c.is_ascii_digit());
            int = false;
        }
        if matches!(self.byte(j), b'e' | b'E') {
            let mut k = j + 1;
            if matches!(self.byte(k), b'+' | b'-') {
                k += 1;
            }
            if self.byte(k).is_ascii_digit() {
                j = k;
                digits(&mut j, |c| c.is_ascii_digit());
                int = false;
            }
        }
        if int && self.byte(j) == b'n' {
            j += 1;
        }
        j
    }

    /// A regex at `start` (a `/`): its closing slash and end, or `None`
    /// when the line ends first.
    fn regex_end(&self, start: usize) -> Option<(usize, usize)> {
        let mut j = start + 1;
        let mut class = false;
        let first = j;
        loop {
            match self.byte(j) {
                b'\n' | b'\r' | 0 => return None,
                b'\\' => {
                    j += 1;
                    if matches!(self.byte(j), b'\n' | b'\r' | 0) {
                        return None;
                    }
                    j += char_len(self.src, j);
                }
                b'[' => {
                    class = true;
                    j += 1;
                }
                b']' => {
                    class = false;
                    j += 1;
                }
                b'/' if !class => break,
                _ => j += char_len(self.src, j),
            }
        }
        if j == first {
            return None;
        }
        let close = j;
        j += 1;
        while self.byte(j).is_ascii_lowercase() {
            j += 1;
        }
        Some((close, j))
    }

    /// A string's text at `state`: a fragment, an escape, or the closing
    /// quote (which pops the frame).
    fn string_part(&mut self, state: &mut State, quote: u8, jsx: bool) -> Lx {
        let at = state.pos;
        let c = self.byte(at);
        if c == quote {
            state.frames.pop();
            return self.tok(state, L::Quote, at, at + 1, false);
        }
        if at >= self.b.len() || (!jsx && matches!(c, b'\n' | b'\r')) {
            // Unclosed: the string ends here.
            state.frames.pop();
            return self.tok(state, L::Quote, at, at, false);
        }
        if jsx
            && c == b'&'
            && let Some(end) = char_ref_end(self.src, at)
        {
            return self.tok(state, L::CharRef, at, end, false);
        }
        if !jsx && c == b'\\' {
            let end = escape_end(self.b, at);
            return self.tok(state, L::Escape, at, end, false);
        }
        let mut j = at;
        while j < self.b.len() {
            let c = self.byte(j);
            if c == quote
                || (!jsx && matches!(c, b'\\' | b'\n' | b'\r'))
                || (jsx && c == b'&' && char_ref_end(self.src, j).is_some())
            {
                break;
            }
            j += char_len(self.src, j);
        }
        self.tok(state, L::Frag, at, j, false)
    }

    /// A template's text at `state`: a fragment, an escape, `${`, or the
    /// closing backtick.
    fn template_part(&mut self, state: &mut State) -> Lx {
        let at = state.pos;
        match self.byte(at) {
            b'`' => {
                state.frames.pop();
                return self.tok(state, L::Backtick, at, at + 1, false);
            }
            b'$' if self.byte(at + 1) == b'{' => {
                state.frames.push(Frame::Subst(0));
                return self.tok(state, L::Op("${"), at, at + 2, false);
            }
            b'\\' => {
                let end = escape_end(self.b, at);
                return self.tok(state, L::Escape, at, end, false);
            }
            _ if at >= self.b.len() => {
                state.frames.pop();
                return self.tok(state, L::Backtick, at, at, false);
            }
            _ => {}
        }
        let mut j = at;
        while j < self.b.len() {
            match self.byte(j) {
                b'`' | b'\\' => break,
                b'$' if self.byte(j + 1) == b'{' => break,
                _ => j += char_len(self.src, j),
            }
        }
        self.tok(state, L::Frag, at, j, false)
    }

    /// JSX between tags: its text (tree-sitter's `jsx_text`: text with
    /// something besides a newline's indentation in it), a character
    /// reference, or the tag or brace that ends it.
    fn jsx_text(&mut self, state: &mut State) -> Lx {
        let at = state.pos;
        let mut j = at;
        let mut saw_text = false;
        let mut at_newline = false;
        while j < self.b.len() && !matches!(self.byte(j), b'<' | b'>' | b'{' | b'}' | b'&') {
            let ch = self.src[j..].chars().next().unwrap_or(' ');
            if ch == '\n' {
                at_newline = true;
            } else {
                at_newline &= ch.is_whitespace();
                if !at_newline {
                    saw_text = true;
                }
            }
            j += ch.len_utf8();
        }
        if saw_text {
            return self.tok(state, L::JsxText, at, j, false);
        }
        // Only layout: whitespace before the next token.
        state.pos = j;
        let c = self.byte(j);
        if j >= self.b.len() {
            return self.tok(state, L::Eof, j, j, false);
        }
        if c == b'&' {
            if let Some(end) = char_ref_end(self.src, j) {
                return self.tok(state, L::CharRef, j, end, false);
            }
            let end = j + 1;
            return self.tok(state, L::Bad, j, end, false);
        }
        if c == b'<' && self.byte(j + 1) == b'/' {
            return self.tok(state, L::Op("</"), j, j + 2, false);
        }
        let op = match c {
            b'<' => "<",
            b'>' => ">",
            b'{' => "{",
            _ => "}",
        };
        self.tok(state, L::Op(op), j, j + 1, false)
    }

    /// A token inside a JSX tag: a name (dashes allowed), a string, or
    /// punctuation (`</`, `/>`).
    fn jsx_tag(&mut self, state: &mut State, nl: bool) -> Lx {
        let at = state.pos;
        let c = self.byte(at);
        if c == b'"' || c == b'\'' {
            state.frames.push(Frame::Str {
                quote: c,
                jsx: true,
            });
            return self.tok(state, L::Quote, at, at + 1, nl);
        }
        if is_ident_start(self.src, at) {
            let mut j = self.ident_end(at);
            while self.byte(j) == b'-' {
                j += 1;
                while j < self.b.len()
                    && (self.byte(j).is_ascii_alphanumeric()
                        || matches!(self.byte(j), b'_' | b'$' | b'-'))
                {
                    j += 1;
                }
            }
            return self.tok(state, L::Name, at, j, nl);
        }
        if c == b'<' && self.byte(at + 1) == b'/' {
            return self.tok(state, L::Op("</"), at, at + 2, nl);
        }
        if c == b'/' && self.byte(at + 1) == b'>' {
            return self.tok(state, L::Op("/>"), at, at + 2, nl);
        }
        let op = match c {
            b'<' => "<",
            b'>' => ">",
            b'{' => "{",
            b'}' => "}",
            b'=' => "=",
            b'.' => ".",
            b':' => ":",
            _ => {
                let end = at + char_len(self.src, at);
                return self.tok(state, L::Bad, at, end, nl);
            }
        };
        self.tok(state, L::Op(op), at, at + 1, nl)
    }
}

/// The end of the escape sequence at `at` (a backslash).
fn escape_end(b: &[u8], at: usize) -> usize {
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
            if b.get(at + 3 + n) == Some(&b'}') {
                at + 4 + n
            } else {
                at + 2
            }
        }
        b'u' if hex(at + 2, 4) => at + 6,
        b'x' if hex(at + 2, 2) => at + 4,
        b'0'..=b'7' => {
            let n = b[at + 1..]
                .iter()
                .take(3)
                .take_while(|c| (b'0'..=b'7').contains(c))
                .count();
            at + 1 + n
        }
        b'\r' if b.get(at + 2) == Some(&b'\n') => at + 3,
        0 => at + 1,
        c if c >= 0x80 => {
            let s = std::str::from_utf8(&b[at + 1..]).ok();
            at + 1 + s.and_then(|s| s.chars().next()).map_or(1, char::len_utf8)
        }
        _ => at + 2,
    }
}

/// `&amp;`, `&#38;`, `&#x26;` at `at`: its end.
fn char_ref_end(src: &str, at: usize) -> Option<usize> {
    let b = src.as_bytes();
    let mut j = at + 1;
    if b.get(j) == Some(&b'#') {
        j += 1;
        let hex = matches!(b.get(j), Some(b'x' | b'X'));
        if hex {
            j += 1;
        }
        let from = j;
        while b.get(j).is_some_and(|c| {
            if hex {
                c.is_ascii_hexdigit()
            } else {
                c.is_ascii_digit()
            }
        }) {
            j += 1;
        }
        let n = j - from;
        if n == 0 || n > if hex { 6 } else { 5 } {
            return None;
        }
    } else {
        let from = j;
        while b.get(j).is_some_and(u8::is_ascii_alphabetic) {
            j += 1;
        }
        if j == from || j - from > 30 {
            return None;
        }
    }
    (b.get(j) == Some(&b';')).then_some(j + 1)
}

fn char_len(src: &str, at: usize) -> usize {
    src[at..].chars().next().map_or(1, char::len_utf8)
}

/// Characters no identifier holds (tree-sitter-javascript's class).
fn excluded(c: char) -> bool {
    c.is_control()
        || c.is_whitespace()
        || matches!(
            c,
            ':' | ';'
                | '`'
                | '"'
                | '\''
                | '@'
                | '#'
                | '.'
                | ','
                | '|'
                | '^'
                | '&'
                | '<'
                | '='
                | '>'
                | '+'
                | '-'
                | '*'
                | '/'
                | '\\'
                | '%'
                | '?'
                | '!'
                | '~'
                | '('
                | ')'
                | '['
                | ']'
                | '{'
                | '}'
                | '\u{feff}'
                | '\u{2060}'
                | '\u{200b}'
                | '\u{2028}'
                | '\u{2029}'
        )
}

pub(crate) fn is_ident_start(src: &str, at: usize) -> bool {
    if src[at..].starts_with("\\u") {
        return true;
    }
    src[at..]
        .chars()
        .next()
        .is_some_and(|c| !excluded(c) && !c.is_ascii_digit())
}

fn is_ident_char(src: &str, at: usize) -> bool {
    src[at..].chars().next().is_some_and(|c| !excluded(c))
}
